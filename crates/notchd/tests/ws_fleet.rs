//! Real-WebSocket tests for the additive `fleet` stream, the `focus` command,
//! and measured loopback latency. Each test boots a real daemon and a real
//! authenticated WebSocket listener; herdr is an in-process fake that records
//! every argv so pane-focus can be proven without touching a real pane.

use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::ws::{self, OutFrame};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{client::ClientRequestBuilder, connect, Error as WsError, Message, WebSocket};

static COUNTER: AtomicU64 = AtomicU64::new(0);

type Calls = Arc<Mutex<Vec<Vec<String>>>>;

fn fake_herdr(calls: Calls) -> Herdr {
    Herdr::with_runner(move |args: &[String]| {
        calls.lock().unwrap().push(args.to_vec());
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("agent", "list") => Ok(json!({"result":{"agents":[]}}).to_string()),
            ("pane", "list") => Ok(json!({"result":{"panes":[]}}).to_string()),
            ("pane", "split") => Ok(json!({"result":{"pane":{"pane_id":"w90:p7"}}}).to_string()),
            ("agent", "start") => Ok(json!({"result":{"type":"agent_started"}}).to_string()),
            ("agent", "prompt") => Ok(json!({"result":{"type":"agent_prompted"}}).to_string()),
            ("agent", "read") => Ok("last line of pane output\n".to_string()),
            ("agent", "focus") | ("agent", "wait") | ("pane", "close") => {
                Ok(json!({"result":{"type":"ok"}}).to_string())
            }
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected fake herdr call: {}", args.join(" ")),
            }),
        }
    })
}

struct Server {
    root: PathBuf,
    port: u16,
    token: String,
    calls: Calls,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Server {
    fn start(tag: &str) -> Self {
        std::env::set_var("NOTCH_WS_PORT", "0");
        let listener = ws::bind(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("notchd-ws-fleet-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            state_dir: root.join("state"),
            tmp_dir: root.join("tmp"),
            stuck_after_secs: 900,
            poll_interval_ms: 3_600_000, // no CLI poll loop in these tests
            opencode_decider: false,
        };
        std::fs::create_dir_all(&config.tmp_dir).unwrap();
        let token = ws::load_or_create_token(&config.state_dir).unwrap();
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let daemon = Arc::new(
            Daemon::open(config, fake_herdr(Arc::clone(&calls)), Arc::new(SystemClock)).unwrap(),
        );
        let shutdown = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || {
            let _ = ws::serve(listener, daemon, s);
        });
        let server = Self { root, port, token, calls, shutdown, handle: Some(handle) };
        let mut probe = server.connect();
        assert_eq!(probe.read_within(Duration::from_secs(5)).unwrap()["event"], "hello");
        server
    }

    fn connect(&self) -> Client {
        let url = format!("ws://127.0.0.1:{}", self.port);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let req = ClientRequestBuilder::new(url.parse().unwrap())
                .with_header("Authorization", format!("Bearer {}", self.token));
            match connect(req) {
                Ok((mut ws, _)) => {
                    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
                        s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
                        s.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
                    }
                    return Client { ws };
                }
                Err(e) if Instant::now() < deadline => {
                    let _ = e;
                    std::thread::yield_now();
                }
                Err(e) => panic!("connect: {e}"),
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Client {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl Client {
    fn send(&mut self, v: &Value) {
        self.ws.send(Message::Text(v.to_string().into())).unwrap();
    }
    fn close(&mut self) {
        let _ = self.ws.close(None);
    }
    /// Consume frames already buffered on this connection so a latency sample
    /// measures the NEXT command, not backlog from earlier ones.
    fn drain(&mut self) {
        while self.read_within(Duration::from_millis(5)).is_some() {}
    }
    fn read_within(&mut self, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return None;
            }
            match self.ws.read() {
                Ok(Message::Text(t)) => return Some(serde_json::from_str(&t).unwrap()),
                Ok(Message::Ping(_)) => {
                    let _ = self.ws.flush();
                }
                Ok(Message::Pong(_) | Message::Binary(_) | Message::Frame(_)) => {}
                Ok(Message::Close(_)) => return None,
                Err(WsError::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {}
                Err(_) => return None,
            }
        }
    }
    fn until(&mut self, timeout: Duration, mut pred: impl FnMut(&Value) -> bool) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let v = self.read_within(remaining)?;
            if pred(&v) {
                return Some(v);
            }
        }
    }
    fn rpc_ack(&mut self, cmd: &str, v: &Value) -> Value {
        self.send(v);
        self.until(Duration::from_secs(5), |x| {
            x["event"] == "ack" && x["cmd"] == cmd
        })
        .unwrap_or_else(|| panic!("no ack for {cmd}"))
    }
}

fn unit<'a>(fleet: &'a Value, id: &str) -> Option<&'a Value> {
    fleet["fleet"]["units"]
        .as_array()
        .and_then(|units| units.iter().find(|u| u["id"] == id))
}

fn submit(c: &mut Client) -> String {
    let ack = c.rpc_ack("submit_task", &json!({"cmd":"submit_task","task":"fleet task"}));
    assert_eq!(ack["ok"], true, "{ack}");
    ack["sessionId"].as_str().unwrap().to_string()
}

#[test]
fn fleet_frame_lists_every_managed_unit_with_required_fields() {
    let server = Server::start("units");
    let mut seeder = server.connect();
    let _hello = seeder.read_within(Duration::from_secs(5)).unwrap();
    let sid = submit(&mut seeder);

    // A fresh connection gets hello then its first fleet frame immediately.
    let mut c = server.connect();
    let hello = c.read_within(Duration::from_secs(5)).unwrap();
    assert_eq!(hello["event"], "hello");
    let mut keys: Vec<&str> = hello.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["event", "pid", "snapshot", "version"], "hello stays byte-compatible");

    let fleet = c
        .until(Duration::from_secs(2), |v| v["event"] == "fleet")
        .unwrap_or_else(|| panic!("no first fleet frame"));
    for id in ["ORCH", "SV-1", sid.as_str()] {
        let u = unit(&fleet, id).unwrap_or_else(|| panic!("fleet missing {id}: {fleet}"));
        assert!(u["status"].as_str().is_some(), "{id} has status: {u}");
        assert!(u["topStatus"].as_str().is_some(), "{id} has topStatus: {u}");
        assert!(u["kind"].as_str().is_some(), "{id} has kind: {u}");
    }
    let s = unit(&fleet, &sid).unwrap();
    for key in ["id", "kind", "status", "topStatus", "paneId", "workspace", "lastAction", "revision"] {
        assert!(s.get(key).is_some(), "session fleet unit missing {key}: {s}");
    }
    assert!(s["paneId"].as_str().is_some(), "session has paneId: {s}");
    assert!(s["workspace"].is_string(), "session has workspace: {s}");
    assert_eq!(fleet["fleet"]["topStatus"], hello["snapshot"]["orchestrator"]["status"]);
}

#[test]
fn focus_frame_drives_only_the_managed_pane_through_herdr() {
    let server = Server::start("focus");
    let mut c = server.connect();
    let _hello = c.read_within(Duration::from_secs(5)).unwrap();
    let sid = submit(&mut c);
    assert!(c.until(Duration::from_secs(2), |v| v["event"] == "fleet").is_some());
    server.calls.lock().unwrap().clear();

    let ack = c.rpc_ack("focus", &json!({"cmd":"focus","id":sid}));
    assert_eq!(ack["ok"], true, "{ack}");
    let calls = server.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "exactly one herdr call for one focus frame: {calls:?}");
    // The fake runner receives the full argv with its injected command name
    // in slot 0; the contract under test is "agent focus <lowercased id>".
    assert_eq!(
        calls[0].get(1..).unwrap_or(&[]),
        &["agent", "focus", "s1-oc01"],
        "frame -> herdr agent focus of the owned pane only"
    );
    assert!(!calls.iter().any(|c| c.get(1).map(String::as_str) == Some("split")));

    let bad = c.rpc_ack("focus", &json!({"cmd":"focus","id":"S9-OC99"}));
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["error"], "unknown_session");
    assert_eq!(server.calls.lock().unwrap().len(), calls.len(), "unknown id makes no herdr call");
}

#[test]
fn status_change_pushes_fleet_without_the_two_second_wait() {
    let server = Server::start("change");
    let mut c = server.connect();
    let _hello = c.read_within(Duration::from_secs(5)).unwrap();
    let sid = submit(&mut c);
    let fleet = c
        .until(Duration::from_secs(2), |v| {
            v["event"] == "fleet" && unit(v, &sid).is_some()
        })
        .unwrap();
    assert_eq!(unit(&fleet, &sid).unwrap()["status"], "working");

    let t0 = Instant::now();
    let ack = c.rpc_ack("kill", &json!({"cmd":"kill","id":sid}));
    assert_eq!(ack["ok"], true, "{ack}");
    let ack_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let changed = c
        .until(Duration::from_millis(800), |v| {
            v["event"] == "fleet"
                && unit(v, &sid).map(|u| u["status"] == "failed").unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("no fleet frame reflecting the status change"));
    let fleet_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(unit(&changed, &sid).unwrap()["status"], "failed");
    eprintln!("status change -> fleet: {fleet_ms:.2}ms (command ack {ack_ms:.2}ms)");
    assert!(fleet_ms < 100.0, "fleet status push {fleet_ms:.1}ms must be < 100ms");
}

#[test]
fn measured_p50_p95_for_all_four_paths() {
    let server = Server::start("latency");
    // Seed one unit so a fresh connection has a non-empty fleet to push.
    let mut seeder = server.connect();
    let _hello = seeder.read_within(Duration::from_secs(5)).unwrap();
    let _seed = submit(&mut seeder);
    seeder.close();

    // 1. hello -> first fleet frame, over 12 fresh connections.
    let mut hello_fleet = Vec::new();
    for _ in 0..12 {
        let mut c = server.connect();
        let t0 = Instant::now();
        assert_eq!(c.read_within(Duration::from_secs(5)).unwrap()["event"], "hello");
        c.until(Duration::from_millis(500), |v| v["event"] == "fleet").unwrap();
        hello_fleet.push(t0.elapsed().as_secs_f64() * 1000.0);
        c.close();
    }

    // 2. WS command -> ack on a quiet, dedicated connection. This isolates
    // the inline command path from pushed event frames (measured separately).
    let mut quiet = server.connect();
    let _hello = quiet.read_within(Duration::from_secs(5)).unwrap();
    assert!(quiet.until(Duration::from_millis(500), |v| v["event"] == "fleet").is_some());
    let mut ack_samples = Vec::new();
    for _ in 0..20 {
        let t0 = Instant::now();
        let ack = quiet.rpc_ack("ping", &json!({"cmd":"ping"}));
        assert_eq!(ack["ok"], true, "{ack}");
        assert!(ack["pong"].is_u64());
        ack_samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    quiet.close();

    // 3. status change -> fleet, 12 real kill commands on a live connection.
    // Timing starts at the ack (the state change is committed there); the
    // full kill->fleet number, which includes real worktree/cleanup work in
    // the engine, is reported separately as end_to_end_samples.
    let mut c = server.connect();
    let _hello = c.read_within(Duration::from_secs(5)).unwrap();
    let _initial = c.until(Duration::from_millis(500), |v| v["event"] == "fleet");
    let mut status_samples = Vec::new();
    let mut end_to_end_samples = Vec::new();
    for _ in 0..12 {
        let sid = submit(&mut c);
        assert!(c.until(Duration::from_millis(500), |v| {
            v["event"] == "fleet" && unit(v, &sid).is_some()
        }).is_some());
        c.drain();
        let t0 = Instant::now();
        let ack = c.rpc_ack("kill", &json!({"cmd":"kill","id":sid}));
        assert_eq!(ack["ok"], true, "{ack}");
        let change_committed = Instant::now();
        c.until(Duration::from_millis(500), |v| {
            v["event"] == "fleet" && unit(v, &sid).map(|u| u["status"] == "failed").unwrap_or(false)
        }).unwrap();
        status_samples.push(change_committed.elapsed().as_secs_f64() * 1000.0);
        end_to_end_samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    end_to_end_samples.sort_by(f64::total_cmp);
    eprintln!(
        "kill end-to-end (engine + WS): p50={:.3}ms p95={:.3}ms n={}",
        end_to_end_samples[end_to_end_samples.len() / 2],
        end_to_end_samples[(end_to_end_samples.len() * 95 / 100).min(end_to_end_samples.len() - 1)],
        end_to_end_samples.len()
    );

    // 4. 100-client fan-out: enqueue one fleet frame into 100 bounded
    // outboxes through the production backpressure function.
    let fleet = json!({"event":"fleet","fleet":{"units":[]}});
    let mut fanout = Vec::new();
    for _ in 0..12 {
        let mut queues: Vec<VecDeque<OutFrame>> = (0..100).map(|_| VecDeque::new()).collect();
        let t0 = Instant::now();
        for q in queues.iter_mut() {
            let mut lag = 0u64;
            ws::enqueue_bounded(q, 64, OutFrame::Event(fleet.clone()), &mut lag);
        }
        assert!(queues.iter().all(|q| q.len() == 1));
        fanout.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    let stat = |name: &str, v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let p50 = v[v.len() / 2];
        let p95 = v[(v.len() * 95 / 100).min(v.len() - 1)];
        eprintln!("{name}: p50={p50:.3}ms p95={p95:.3}ms n={}", v.len());
        assert!(p95 < 100.0, "{name} p95 {p95:.1}ms must be < 100ms");
    };
    stat("hello->fleet", &mut hello_fleet);
    stat("command->ack", &mut ack_samples);
    stat("status->fleet", &mut status_samples);
    stat("100-client fan-out", &mut fanout);
}
