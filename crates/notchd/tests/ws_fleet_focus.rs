//! T4 — the WS `focus` command focuses ONLY the target session's herdr pane.
//!
//! A call-recording fake herdr proves that `focus` invokes exactly
//! `agent focus <target-pane>` and never closes/splits/prompts another pane
//! or opens a window. Real daemon, real WS bytes, real herdr argv seam.

use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
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
    let p = std::env::temp_dir().join(format!("fleetprobe-focus-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("tmp")).unwrap();
    p
}

/// Fake herdr that records every invocation as a space-joined argv string.
struct RecordingHerdr {
    calls: Mutex<Vec<Vec<String>>>,
}

impl RecordingHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }
    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| fake.run(args))
    }
    fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        self.calls.lock().unwrap().push(args.to_vec());
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("agent", "list") | ("pane", "list") => Ok(json!({
                "id":"fh","result":{"agents":[],"panes":[]}
            }).to_string()),
            ("pane", "split") => Ok(json!({
                "id":"fh","result":{"type":"pane_info","pane":{"pane_id":"w77:p3","workspace_id":"w77"}}
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
                        serde_json::from_str(&t).unwrap_or_else(|e| panic!("bad JSON {t}: {e}")),
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
    fn ack(&mut self, cmd: &str, value: &Value) -> Value {
        self.ws
            .send(Message::Text(value.to_string().into()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let v = self
                .read(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|| panic!("no ack for {cmd}"));
            if v["event"] == "ack" && v["cmd"] == cmd {
                return v;
            }
        }
    }
}

#[test]
fn ws_focus_targets_only_the_requested_pane() {
    let root = tmp_root("main");
    std::env::set_var("NOTCH_WS_PORT", "0");
    let listener = ws::bind(0).unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = ws::load_or_create_token(&root.join("state")).unwrap();

    let herdr = RecordingHerdr::new();
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

    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let a = daemon.submit_task(
        "focus target".to_string(),
        Some(work.to_string_lossy().to_string()),
        None,
        None,
        None,
        None,
    );
    assert!(a.ok, "seed failed: {:?}", a.error);
    let sid = a.session_id.clone().unwrap();

    // Wait for the real pane id to be published by the async spawn worker.
    let deadline = Instant::now() + Duration::from_secs(5);
    let pane = loop {
        let s = daemon.snapshot();
        if let Some(x) = s.sessions.iter().find(|x| x.id == sid) {
            if !x.pane_id.is_empty() {
                break x.pane_id.clone();
            }
        }
        assert!(Instant::now() < deadline, "pane id never published");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(pane, "w77:p3");

    let shutdown = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let d = Arc::clone(&daemon);
    let s = Arc::clone(&shutdown);
    let ac = Arc::clone(&active);
    let handle = std::thread::spawn(move || {
        let _ = ws::serve_with_active(listener, d, s, ac);
    });

    let mut c = Client::connect(&format!("ws://127.0.0.1:{port}"), &token);
    let hello = c.read(Duration::from_secs(5)).expect("no hello");
    assert_eq!(hello["event"], "hello");

    // Reset the recorder so only calls caused by the focus request are seen.
    herdr.clear();
    let ack = c.ack("focus", &json!({"cmd":"focus","id":sid}));
    assert_eq!(ack["ok"], true, "focus ack: {ack}");
    assert_eq!(ack["id"], sid);

    let calls = herdr.calls();
    let focus_calls: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| {
            c.get(1).map(String::as_str) == Some("agent")
                && c.get(2).map(String::as_str) == Some("focus")
        })
        .collect();
    assert_eq!(
        focus_calls.len(),
        1,
        "exactly one focus call, got {calls:?}"
    );
    // herdr agent targets are the lowercased notch session id (types.rs
    // `agent_target`); panes are never referenced by name. The single focus
    // call must carry exactly that target and nothing else.
    let expected_target = sid.to_lowercase();
    assert_eq!(
        focus_calls[0].as_slice(),
        &["<fake>", "agent", "focus", expected_target.as_str()],
        "focus must target only the session's herdr agent with argv `agent focus <target>`"
    );
    assert!(!pane.is_empty(), "the session must have a real pane id");

    // No pane was closed, split, prompted, or otherwise touched by focus.
    for c in &calls {
        let pair = (
            c.get(1).map(String::as_str).unwrap_or(""),
            c.get(2).map(String::as_str).unwrap_or(""),
        );
        assert!(
            !matches!(
                pair,
                ("pane", "close") | ("pane", "split") | ("agent", "prompt") | ("agent", "start")
            ),
            "focus touched another pane/window: {c:?}"
        );
    }

    shutdown.store(true, Ordering::SeqCst);
    let _ = handle.join();
    let _ = std::fs::remove_dir_all(&root);
}
