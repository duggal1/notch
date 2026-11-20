//! T1 — the fleet WebSocket actually carries live fleet state.
//!
//! No mocks in the server: a REAL `notchd serve` is spawned with NOTCH_WS=1 on
//! an ephemeral loopback port, a token in a neutral-prefix temp state dir, and
//! a fake-herdr shim (scripted process) so four sessions can be seeded without
//! touching a real pane. A real tungstenite client connects with the token,
//! reads the `hello` frame, then waits for the `fleet` frame and asserts it
//! lists the orchestrator, both supervisors and all four sessions with real
//! ids and pane/workspace fields.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tungstenite::client::ClientRequestBuilder;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Error as WsError, Message, WebSocket};

fn scratch(tag: &str) -> PathBuf {
    // Neutral prefix (NOT `notch-`): the daemon's temp sweep must never own it.
    let root = std::env::temp_dir().join(format!("fleetprobe-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("state")).unwrap();
    root
}

fn free_port() -> u16 {
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

/// A scripted fake herdr. Emits valid CLI envelopes, a distinct pane id per
/// split, and records `agent focus` calls to a log the tests can read.
fn write_fake_herdr(root: &Path) -> PathBuf {
    let script = root.join("fake-herdr.py");
    let counter = root.join("pane-counter");
    let focus_log = root.join("focus.log");
    let body = format!(
        r#"#!/usr/bin/env python3
import sys, json
a = sys.argv[1:]
sub = a[0] if len(a) > 0 else ""
sub2 = a[1] if len(a) > 1 else ""
counter = {counter:?}
focus_log = {focus_log:?}
def out(obj): print(json.dumps(obj))
def nextn():
    try: n = int(open(counter).read().strip() or "0")
    except Exception: n = 0
    n += 1
    open(counter, "w").write(str(n))
    return n
if sub == "pane" and sub2 == "list":
    out({{"id":"fh","result":{{"type":"pane_list","panes":[]}}}})
elif sub == "agent" and sub2 == "list":
    out({{"id":"fh","result":{{"type":"agent_list","agents":[]}}}})
elif sub == "pane" and sub2 == "split":
    n = nextn()
    out({{"id":"fh","result":{{"type":"pane_info","pane":{{"pane_id":f"w9{{n}}:p1","tab_id":f"w9{{n}}:t1","workspace_id":f"w9{{n}}"}}}}}})
elif sub == "agent" and sub2 == "start":
    out({{"id":"fh","result":{{"type":"agent_started"}}}})
elif sub == "agent" and sub2 == "prompt":
    out({{"id":"fh","result":{{"type":"agent_prompted"}}}})
elif sub == "agent" and sub2 == "focus":
    open(focus_log, "a").write(" ".join(a) + "\n")
    out({{"id":"fh","result":{{"type":"ok"}}}})
elif sub == "agent" and sub2 == "read":
    print("fake pane line")
else:
    out({{"id":"fh","result":{{"type":"ok"}}}})
"#,
        counter = counter.to_string_lossy(),
        focus_log = focus_log.to_string_lossy(),
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

struct RealDaemon {
    root: PathBuf,
    port: u16,
    child: Child,
}

impl RealDaemon {
    fn start(tag: &str) -> Self {
        let root = scratch(tag);
        let port = free_port();
        let herdr = write_fake_herdr(&root);
        let sock = root.join("state/notch.sock");
        let stdout = std::fs::File::create(root.join("stdout.log")).unwrap();
        let stderr = std::fs::File::create(root.join("stderr.log")).unwrap();

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_notchd"));
        cmd.arg("serve")
            .env("NOTCH_STATE_DIR", root.join("state"))
            .env("NOTCH_SOCK", &sock)
            .env("NOTCH_HERDR_CMD", &herdr)
            .env("NOTCH_WS", "1")
            .env("NOTCH_WS_PORT", port.to_string())
            .env("NOTCH_POLL_INTERVAL_MS", "3600000") // no poll: keep seeded state stable
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let child = cmd.spawn().unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&sock).is_err() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            UnixStream::connect(&sock).is_ok(),
            "notchd serve NDJSON socket never became ready: {:?}",
            std::fs::read_to_string(root.join("stderr.log")).unwrap_or_default()
        );
        Self { root, port, child }
    }

    fn token(&self) -> String {
        let path = self.root.join("state/ws.token");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let t = text.trim();
                if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return t.to_string();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("WS token file missing or malformed at {}", path.display());
    }

    fn cwd(&self) -> String {
        let p = self.root.join("work");
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().to_string()
    }
}

impl Drop for RealDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ---------- NDJSON socket client (seed + wait for panes) ----------

struct Ndjson {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Ndjson {
    fn connect(sock: &Path) -> Self {
        let stream = UnixStream::connect(sock).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        Self {
            reader,
            writer: stream,
        }
    }

    fn next(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "NDJSON server closed unexpectedly");
        serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("bad NDJSON {line:?}: {e}"))
    }

    fn cmd(&mut self, cmd: &str, extra: Value) -> Value {
        let mut req = extra;
        req["cmd"] = json!(cmd);
        writeln!(self.writer, "{req}").unwrap();
        self.writer.flush().unwrap();
        loop {
            let v = self.next();
            if v["event"] == "ack" && v["cmd"] == cmd {
                return v;
            }
        }
    }

    fn raw_cmd(&mut self, cmd: &str) -> Value {
        writeln!(self.writer, "{{\"cmd\":\"{cmd}\"}}").unwrap();
        self.writer.flush().unwrap();
        self.next()
    }
}

/// Seed SV-1/SV-2 with 2 sessions each via the real NDJSON socket and wait
/// until the fast-ack spawn workers have published every real pane id.
fn seed_fleet(d: &RealDaemon) -> [String; 4] {
    let sock = d.root.join("state/notch.sock");
    let mut c = Ndjson::connect(&sock);
    let _hello = c.next();
    assert_eq!(_hello["event"], "hello");

    let cwd = d.cwd();
    let specs = [
        (json!({"task":"fleet task one","cwd":cwd}), None),
        (json!({"task":"fleet task two","cwd":cwd}), Some("SV-1")),
        (json!({"task":"fleet task three","cwd":cwd}), None),
        (json!({"task":"fleet task four","cwd":cwd}), Some("SV-2")),
    ];
    let mut ids = Vec::new();
    for (mut req, sup) in specs {
        if let Some(sup) = sup {
            req["supervisorId"] = json!(sup);
        }
        let ack = c.cmd("submit_task", req);
        assert_eq!(ack["ok"], true, "submit_task ack failed: {ack}");
        ids.push(ack["sessionId"].as_str().unwrap().to_string());
    }
    assert_eq!(ids, ["S1-OC01", "S1-OC02", "S2-OC01", "S2-OC02"]);

    // Event-driven wait for the real pane ids (fast-ack publishes them shortly
    // after the ack; no fixed sleep used as synchronization).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snap = c.raw_cmd("snapshot");
        let sessions = snap["snapshot"]["sessions"].as_array().unwrap();
        let ready = sessions.len() == 4
            && sessions
                .iter()
                .all(|s| s["paneId"].as_str().map(|p| !p.is_empty()).unwrap_or(false));
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "pane ids never published: {snap}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    [
        ids[0].clone(),
        ids[1].clone(),
        ids[2].clone(),
        ids[3].clone(),
    ]
}

// ---------- real WS client ----------

struct Client {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl Client {
    fn connect(d: &RealDaemon) -> Self {
        let url = format!("ws://127.0.0.1:{}", d.port);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let req = ClientRequestBuilder::new(url.parse().unwrap())
                .with_header("Authorization", format!("Bearer {}", d.token()));
            match connect(req) {
                Ok((mut ws, _)) => {
                    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
                        s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
                    }
                    return Self { ws };
                }
                Err(e) if Instant::now() < deadline => {
                    let _ = e;
                    std::thread::yield_now();
                }
                Err(e) => panic!("WS connect {url}: {e}"),
            }
        }
    }

    fn read(&mut self, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    return Some(
                        serde_json::from_str(&t).unwrap_or_else(|e| panic!("bad WS JSON {t}: {e}")),
                    )
                }
                Ok(Message::Ping(_)) => {
                    let _ = self.ws.flush();
                }
                Ok(_) => {}
                Err(WsError::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(_) => return None,
            }
        }
    }

    fn wait_for(&mut self, event: &str, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let v = self.read(remaining)?;
            if v["event"].as_str() == Some(event) {
                return Some(v);
            }
        }
    }
}

#[test]
fn real_serve_hello_then_fleet_frame_carries_live_fleet() {
    let d = RealDaemon::start("t1");
    let ids = seed_fleet(&d);

    let mut c = Client::connect(&d);
    let hello = c.read(Duration::from_secs(5)).expect("no hello frame");
    assert_eq!(hello["event"], "hello", "hello frame: {hello}");
    assert_eq!(hello["version"], 1);
    assert!(hello["pid"].as_u64().is_some());

    // The fleet frame is additive: hello stays intact, then fleet arrives.
    let frame = c
        .wait_for("fleet", Duration::from_secs(5))
        .unwrap_or_else(|| panic!("no `fleet` frame after hello (hello={hello})"));
    let fleet = &frame["fleet"];

    // The fleet is a flat `units` array (one entry per managed unit), keyed by
    // the real ids; `topStatus` is the orchestrator roll-up.
    let units = fleet["units"]
        .as_array()
        .unwrap_or_else(|| panic!("no fleet.units in frame: {fleet}"));
    let unit = |id: &str| -> Option<&Value> { units.iter().find(|u| u["id"].as_str() == Some(id)) };

    let orch = unit("ORCH").unwrap_or_else(|| panic!("fleet missing ORCH: {fleet}"));
    assert!(
        orch["status"].as_str().is_some(),
        "orchestrator status: {orch}"
    );
    assert!(
        orch["topStatus"].as_str().is_some(),
        "orchestrator topStatus: {orch}"
    );
    assert!(orch["kind"].as_str().is_some(), "orchestrator kind: {orch}");
    assert_eq!(
        fleet["topStatus"], hello["snapshot"]["orchestrator"]["status"],
        "fleet roll-up must match the hello snapshot"
    );

    let sup_ids: Vec<&str> = units
        .iter()
        .filter(|u| {
            u["id"]
                .as_str()
                .map(|id| id.starts_with("SV-"))
                .unwrap_or(false)
        })
        .filter_map(|u| u["id"].as_str())
        .collect();
    assert_eq!(sup_ids, ["SV-1", "SV-2"], "fleet supervisors: {fleet}");
    for sid in ["SV-1", "SV-2"] {
        let s = unit(sid).unwrap();
        assert!(s["status"].as_str().is_some(), "supervisor status: {s}");
        assert!(
            s["topStatus"].as_str().is_some(),
            "supervisor topStatus: {s}"
        );
        assert!(s["kind"].as_str().is_some(), "supervisor kind: {s}");
    }

    // Sessions: real, ordered ids plus the required pane/workspace fields.
    let session_units: Vec<&Value> = units
        .iter()
        .filter(|u| {
            u["id"]
                .as_str()
                .map(|id| id.contains("-OC"))
                .unwrap_or(false)
        })
        .collect();
    let got: Vec<&str> = session_units
        .iter()
        .map(|s| s["id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        got, ids,
        "fleet session ids must be real, ordered ids: {fleet}"
    );

    for s in &session_units {
        assert!(s["status"].as_str().is_some(), "session status: {s}");
        assert!(s["topStatus"].as_str().is_some(), "session topStatus: {s}");
        assert!(s["kind"].as_str().is_some(), "session kind: {s}");
        let pane = s["paneId"].as_str().unwrap_or("");
        assert!(
            !pane.is_empty() && pane.starts_with('w'),
            "session paneId must be a real pane: {s}"
        );
        let workspace = s["workspace"].as_str().unwrap_or("");
        assert!(
            !workspace.is_empty(),
            "session workspace must be present: {s}"
        );
        // lastAction / revision are best-effort projections of lastAction and
        // lastRevision; they are legitimately null until a poll has sampled a
        // pane tail/heartbeat. When present they must have the documented type.
        assert!(
            s["lastAction"].is_string() || s["lastAction"].is_null(),
            "session lastAction must be a string or null: {s}"
        );
        assert!(
            s["revision"].as_u64().is_some() || s["revision"].is_null(),
            "session revision must be an integer or null: {s}"
        );
    }
}
