//! T5 — when the primary idle event source (opencode /api/session) is killed,
//! the daemon's polling fallback (herdr status / quiet heuristic) keeps real
//! snapshots flowing on the WS and the stream never wedges. Restarting the
//! primary switches the signal back. The exact `idle_signal` value fired is
//! read back from events.jsonl and reported.

use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;
use notchd::ws;
use serde_json::{json, Value};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tungstenite::client::ClientRequestBuilder;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Error as WsError, Message, WebSocket};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("fleetprobe-fallback-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("tmp")).unwrap();
    p
}

struct ToggleHerdr {
    status: Mutex<String>,
    revision: AtomicU64,
}

impl ToggleHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self { status: Mutex::new("working".to_string()), revision: AtomicU64::new(1) })
    }
    fn set(&self, s: &str) {
        *self.status.lock().unwrap() = s.to_string();
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
    fn herdr(self: &Arc<Self>) -> Herdr {
        let f = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| f.run(args))
    }
    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        let status = self.status.lock().unwrap().clone();
        let rev = self.revision.load(Ordering::SeqCst);
        match (sub, sub2) {
            ("agent", "list") | ("pane", "list") => Ok(json!({
                "id":"fh","result":{
                    "agents":[{
                        "pane_id":"w55:p2","agent_status":status,"revision":rev,
                        "agent_session":{"agent":"opencode","value":"w55:p2"},"focused":false
                    }],
                    "panes":[]
                }
            }).to_string()),
            ("pane", "split") => Ok(json!({
                "id":"fh","result":{"type":"pane_info","pane":{"pane_id":"w55:p2","workspace_id":"w55"}}
            }).to_string()),
            ("agent", "start") | ("agent", "prompt") | ("agent", "focus")
            | ("agent", "wait") | ("pane", "close") => {
                Ok(json!({"id":"fh","result":{"type":"ok"}}).to_string())
            }
            ("agent", "read") => Ok("pane line\n".to_string()),
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected herdr call: {}", args.join(" ")),
            }),
        }
    }
}

/// Toggleable primary idle signal. `api_up=false` simulates a killed source.
struct ToggleOc {
    api_up: AtomicBool,
    idle: AtomicBool,
}

impl ToggleOc {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            api_up: AtomicBool::new(true),
            idle: AtomicBool::new(false),
        })
    }
    fn opencode(self: &Arc<Self>) -> Opencode {
        let f = Arc::clone(self);
        Opencode::with_runner(move |args: &[String]| f.run(args))
    }
    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        let rest = &args[1..];
        match rest.join(" ").as_str() {
            "api GET /api/session" => {
                if !self.api_up.load(Ordering::SeqCst) {
                    return Err(HerdrError::Io("primary event source down".to_string()));
                }
                let time = if self.idle.load(Ordering::SeqCst) {
                    json!({"created":1000,"updated":2000,"busy":1000,"idle":2000})
                } else {
                    json!({"created":1000,"updated":2000,"busy":3000})
                };
                Ok(json!({"cursor":null,"data":[{
                    "id":"w55:p2","model":{"id":"m"},"time":time,
                    "location":{"directory":"/tmp"}
                }]}).to_string())
            }
            s if s.starts_with("run --model") => Ok("CONTINUE: keep working".to_string()),
            "models" => Ok("opencode/hy4-flash-free\n".to_string()),
            other => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected opencode call: {other}"),
            }),
        }
    }
}

struct Client {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl Client {
    fn connect(url: &str, token: &str) -> Self {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let req = ClientRequestBuilder::new(url.parse().unwrap())
                .with_header("Authorization", format!("Bearer {token}"));
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
                Err(e) => panic!("connect: {e}"),
            }
        }
    }
    /// Wait for any snapshot or fleet frame (proof the stream is flowing).
    fn wait_any_frame(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return false; }
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        if v["event"] == "snapshot" || v["event"] == "fleet" {
                            return true;
                        }
                    }
                }
                Ok(Message::Ping(_)) => { let _ = self.ws.flush(); }
                Ok(_) => {}
                Err(WsError::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {}
                Err(_) => return false,
            }
        }
    }
    fn ping_ok(&mut self) -> bool {
        if self.ws.send(Message::Text(json!({"cmd":"ping"}).to_string().into())).is_err() {
            return false;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Message::Text(t)) = self.ws.read() {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    if v["event"] == "ack" && v["cmd"] == "ping" { return v["ok"] == true; }
                }
            }
        }
        false
    }
}

#[test]
fn killed_primary_signal_falls_back_without_wedging_ws() {
    let root = tmp_root("main");
    std::env::set_var("NOTCH_WS_PORT", "0");
    let listener = ws::bind(0).unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = ws::load_or_create_token(&root.join("state")).unwrap();
    let events_path = root.join("state/events.jsonl");

    let herdr = ToggleHerdr::new();
    let oc = ToggleOc::new();
    let config = Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 900,
        poll_interval_ms: 3_600_000,
        opencode_decider: false,
    };
    let daemon = Arc::new(
        Daemon::open(config, herdr.herdr(), Arc::new(SystemClock))
            .unwrap_or_else(|e| panic!("open daemon: {e}")),
    );
    daemon.set_opencode(oc.opencode());

    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let a = daemon.submit_task(
        "fallback seed".to_string(),
        Some(work.to_string_lossy().to_string()),
        None, None, None, None,
    );
    assert!(a.ok, "seed failed: {:?}", a.error);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = daemon.snapshot();
        if s.sessions.iter().any(|x| x.id == "S1-OC01" && !x.pane_id.is_empty()) { break; }
        assert!(Instant::now() < deadline, "pane id never published");
        std::thread::sleep(Duration::from_millis(10));
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let d = Arc::clone(&daemon);
    let s = Arc::clone(&shutdown);
    let ac = Arc::clone(&active);
    let handle = std::thread::spawn(move || {
        let _ = ws::serve_with_active(listener, d, s, ac);
    });

    let mut client = Client::connect(&format!("ws://127.0.0.1:{port}"), &token);
    let _hello = client.wait_any_frame(Duration::from_secs(5));
    // Drain any seed frames so subsequent waits measure fresh polls.
    while client.wait_any_frame(Duration::from_millis(80)) {}

    // Phase A — primary alive and reporting idle -> signal "primary".
    herdr.set("idle");
    oc.idle.store(true, Ordering::SeqCst);
    daemon.poll_once();
    assert!(client.wait_any_frame(Duration::from_secs(3)), "stream wedged in phase A");

    // Back to working; keep polling so snapshots keep flowing.
    herdr.set("working");
    oc.idle.store(false, Ordering::SeqCst);
    for _ in 0..6 {
        daemon.poll_once();
        assert!(client.wait_any_frame(Duration::from_secs(3)), "stream wedged while healthy");
    }

    // Phase B — KILL the primary source; herdr still says idle -> fallback.
    oc.api_up.store(false, Ordering::SeqCst);
    herdr.set("idle");
    daemon.poll_once();
    assert!(client.wait_any_frame(Duration::from_secs(3)), "stream wedged during outage");

    // Several more polls with the source down: snapshots must keep flowing.
    herdr.set("working");
    for _ in 0..6 {
        daemon.poll_once();
        assert!(client.wait_any_frame(Duration::from_secs(3)), "stream wedged during outage polls");
    }

    // Phase C — RESTART the primary -> signal "primary" again.
    oc.api_up.store(true, Ordering::SeqCst);
    herdr.set("idle");
    oc.idle.store(true, Ordering::SeqCst);
    daemon.poll_once();
    assert!(client.wait_any_frame(Duration::from_secs(3)), "stream wedged after restart");
    herdr.set("working");
    oc.idle.store(false, Ordering::SeqCst);

    assert!(client.ping_ok(), "WS wedged across the whole outage/restart cycle");

    // Read the exact signals that fired from the real event log.
    let log = std::fs::read_to_string(&events_path).unwrap_or_default();
    let signals: Vec<String> = log
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["kind"] == "idle_signal")
        .filter_map(|v| v["detail"].as_str().map(str::to_string))
        .collect();
    eprintln!("FLEET FALLBACK idle signals fired: {signals:?}");
    assert!(signals.iter().any(|s| s == "primary"), "primary signal never fired: {signals:?}");
    assert!(
        signals.iter().any(|s| s == "herdr" || s == "quiet"),
        "fallback signal never fired during the outage: {signals:?}"
    );
    // In order: at least one primary before the outage, a fallback during it.
    let first_primary = signals.iter().position(|s| s == "primary").unwrap();
    let first_fallback = signals.iter().position(|s| s == "herdr" || s == "quiet").unwrap();
    assert!(first_fallback > first_primary, "fallback must fire after the primary: {signals:?}");

    shutdown.store(true, Ordering::SeqCst);
    let _ = handle.join();
    let _ = std::fs::remove_dir_all(&root);
}
