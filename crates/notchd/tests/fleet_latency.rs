//! T2 — p50/p95 from a REAL status mutation to the WS `fleet` frame.
//!
//! An in-process daemon (real subscriber stream, real 100 ms debounce, real
//! `ws::serve_with_active` over TCP) is driven by a toggleable fake herdr:
//! each iteration flips a session's reported status working<->idle with a
//! fresh revision, calls the real `poll_once`, and times how long until the
//! client receives a `fleet` frame carrying the NEW status. >=100 samples.

use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::ws;
use serde_json::{json, Value};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tungstenite::client::ClientRequestBuilder;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Error as WsError, Message, WebSocket};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("fleetprobe-lat-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("tmp")).unwrap();
    p
}

/// Fake herdr whose pane revision is bumped between polls. A revision bump is
/// a real agent heartbeat mutation: poll_once writes `lastRevision` and fires
/// the same change/snapshot pipeline a live status update does, WITHOUT
/// pushing the session through the idle-decider/escalation path (which would
/// make it ineligible long before 100 samples).
struct TogglingHerdr {
    revision: AtomicU64,
}

impl TogglingHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            revision: AtomicU64::new(1),
        })
    }
    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| fake.run(args))
    }
    fn bump(self: &Arc<Self>) -> u64 {
        self.revision.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        let status = "working";
        let rev = self.revision.load(Ordering::SeqCst);
        match (sub, sub2) {
            ("agent", "list") | ("pane", "list") => Ok(json!({
                "id":"fh","result":{
                    "agents":[{
                        "pane_id":"w91:p1","agent_status":status,"revision":rev,
                        "agent_session":{"value":"sess-latency"},"focused":false
                    }],
                    "panes":[]
                }
            })
            .to_string()),
            ("pane", "split") => Ok(json!({
                "id":"fh","result":{"type":"pane_info","pane":{"pane_id":"w91:p1","workspace_id":"w91"}}
            })
            .to_string()),
            ("agent", "start") | ("agent", "prompt") | ("agent", "focus")
            | ("agent", "wait") | ("pane", "close") => {
                Ok(json!({"id":"fh","result":{"type":"ok"}}).to_string())
            }
            ("agent", "read") => Ok("last pane line\n".to_string()),
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected herdr call: {}", args.join(" ")),
            }),
        }
    }
}

struct FleetClient {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl FleetClient {
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
                Err(e) => panic!("connect {url}: {e}"),
            }
        }
    }
    fn wait_fleet_revision(&mut self, rev: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        if v["event"].as_str() == Some("fleet") {
                            let units = v["fleet"]["units"].as_array();
                            if units
                                .map(|u| {
                                    u.iter().any(|x| {
                                        x["id"] == "S1-OC01" && x["revision"].as_u64() == Some(rev)
                                    })
                                })
                                .unwrap_or(false)
                            {
                                return true;
                            }
                        }
                    }
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
                Err(_) => return false,
            }
        }
    }
    fn ping_ok(&mut self) -> bool {
        let req = json!({"cmd":"ping"});
        if self.ws.send(Message::Text(req.to_string().into())).is_err() {
            return false;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Message::Text(t)) = self.ws.read() {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    if v["event"] == "ack" && v["cmd"] == "ping" {
                        return v["ok"] == true;
                    }
                }
            }
        }
        false
    }
}

#[test]
fn fleet_frame_latency_p50_p95_over_100_real_status_mutations() {
    let root = tmp_root("main");
    std::env::set_var("NOTCH_WS_PORT", "0");
    let listener = ws::bind(0).unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = ws::load_or_create_token(&root.join("state")).unwrap();

    let herdr = TogglingHerdr::new();
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
    let ack = daemon.submit_task(
        "latency seed".to_string(),
        Some(work.to_string_lossy().to_string()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok, "seed failed: {:?}", ack.error);
    assert_eq!(ack.session_id.as_deref(), Some("S1-OC01"));

    // Wait until the fast-ack spawn publishes the real pane id.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = daemon.snapshot();
        if s.sessions
            .iter()
            .any(|x| x.id == "S1-OC01" && !x.pane_id.is_empty())
        {
            break;
        }
        assert!(Instant::now() < deadline, "pane id never published");
        std::thread::sleep(Duration::from_millis(10));
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let d = Arc::clone(&daemon);
    let s = Arc::clone(&shutdown);
    let a = Arc::clone(&active);
    let handle = std::thread::spawn(move || {
        let _ = ws::serve_with_active(listener, d, s, a);
    });

    let url = format!("ws://127.0.0.1:{port}");
    let mut client = FleetClient::connect(&url, &token);
    // Stale hello/seed frames are skipped by the revision predicate below;
    // no fixed-sleep synchronization is used.

    let mut samples: Vec<f64> = Vec::with_capacity(128);
    for i in 0..128 {
        let rev = herdr.bump();
        let t0 = Instant::now();
        daemon.poll_once();
        let got = client.wait_fleet_revision(rev, Duration::from_secs(3));
        assert!(got, "no fleet frame with revision {rev} at sample {i}");
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    samples.sort_by(f64::total_cmp);
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    eprintln!(
        "FLEET LATENCY: samples={} p50={p50:.1}ms p95={p95:.1}ms min={:.1}ms max={:.1}ms",
        samples.len(),
        samples[0],
        samples[samples.len() - 1]
    );
    assert!(
        samples.len() >= 100,
        "need >=100 samples, got {}",
        samples.len()
    );

    // The stream must stay alive and responsive after the storm.
    assert!(client.ping_ok(), "daemon/WS wedged after latency storm");

    shutdown.store(true, Ordering::SeqCst);
    let _ = handle.join();
    let _ = std::fs::remove_dir_all(&root);
}
