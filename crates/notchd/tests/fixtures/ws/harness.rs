//! Shared harness for the real-WebSocket tests. Included via `#[path]`.
//! NOT a cargo test target itself (lives under fixtures/).
//!
//! Every test binds an ephemeral loopback port via `NOTCH_WS_PORT=0`, opens a
//! real `Daemon` over a fresh temp state dir with an in-process fake herdr
//! runner, and serves `notchd::ws::serve` on a real TCP listener. Clients are
//! real tungstenite WebSockets — real bytes, no mocks inside the server.
#![allow(dead_code)]

use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::ws;
use serde_json::{json, Value};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Error as WsError, Message, WebSocket};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-ws-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// In-process fake herdr. Answers exactly the commands `submit_task`/`message`
/// need; never touches a real pane.
fn fake_herdr() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("agent", "list") => Ok(json!({
                "id": "cli:agent:list",
                "result": {"type": "agent_list", "agents": []}
            })
            .to_string()),
            ("pane", "list") => Ok(json!({
                "id": "cli:pane:list",
                "result": {"type": "pane_list", "panes": []}
            })
            .to_string()),
            ("pane", "split") => Ok(json!({
                "id": "cli:pane:split",
                "result": {"type": "pane_info", "pane": {"pane_id": "w90:p7", "tab_id": "w1:t1", "workspace_id": "w1"}}
            })
            .to_string()),
            ("agent", "start") => Ok(json!({
                "id": "cli:agent:start",
                "result": {"type": "agent_started", "agent": {"pane_id": "w90:p7", "agent_status": "working", "revision": 1}}
            })
            .to_string()),
            ("agent", "prompt") => Ok(json!({
                "id": "cli:agent:prompt",
                "result": {"type": "agent_prompted", "agent": {"pane_id": "w90:p7", "agent_status": "working", "revision": 2}}
            })
            .to_string()),
            ("agent", "read") => Ok("last line of pane output\n".to_string()),
            ("agent", "wait") | ("agent", "focus") | ("pane", "close") => {
                Ok(json!({"id": "cli:ok", "result": {"type": "ok"}}).to_string())
            }
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected fake herdr call: {}", args.join(" ")),
            }),
        }
    })
}

/// One running WS endpoint over a real daemon.
pub struct WsDaemon {
    pub root: PathBuf,
    pub port: u16,
    pub daemon: Arc<Daemon>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl WsDaemon {
    pub fn start(tag: &str) -> Self {
        // Ephemeral port selected through the documented env var.
        std::env::set_var("NOTCH_WS_PORT", "0");
        let port = ws::port_from_env();
        assert_eq!(port, 0, "NOTCH_WS_PORT=0 must request an ephemeral port");
        let listener = ws::bind(port).unwrap_or_else(|e| panic!("bind loopback: {e}"));
        let port = listener.local_addr().unwrap().port();

        let root = tmp_root(tag);
        let config = Config {
            state_dir: root.join("state"),
            tmp_dir: root.join("tmp"),
            stuck_after_secs: 900,
            poll_interval_ms: 3_600_000, // no poll loop in these tests
            opencode_decider: false,
        };
        std::fs::create_dir_all(&config.tmp_dir).unwrap();
        let daemon = Arc::new(
            Daemon::open(config, fake_herdr(), Arc::new(SystemClock))
                .unwrap_or_else(|e| panic!("open daemon: {e}")),
        );
        let shutdown = Arc::new(AtomicBool::new(false));
        let d = Arc::clone(&daemon);
        let s = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || {
            let _ = ws::serve(listener, d, s);
        });

        let server = WsDaemon {
            root,
            port,
            daemon,
            shutdown,
            handle: Some(handle),
        };
        // Readiness: a real client must get a real hello.
        let mut probe = server.connect();
        let hello = probe.read_value_within(Duration::from_secs(5));
        assert_eq!(hello.as_ref().and_then(|v| v["event"].as_str()), Some("hello"));
        server
    }

    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    pub fn connect(&self) -> WsClient {
        let url = self.url();
        let (mut ws, _) = connect(&url)
            .unwrap_or_else(|e| panic!("connect {url}: {e}"));
        if let MaybeTlsStream::Plain(stream) = ws.get_mut() {
            stream.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        }
        WsClient { ws }
    }
}

impl Drop for WsDaemon {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A real tungstenite client with deadline-based frame reads.
pub struct WsClient {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl WsClient {
    pub fn send(&mut self, value: &Value) {
        self.ws
            .send(Message::Text(value.to_string().into()))
            .unwrap_or_else(|e| panic!("send frame: {e}"));
    }

    pub fn send_text(&mut self, text: &str) {
        self.ws
            .send(Message::Text(text.into()))
            .unwrap_or_else(|e| panic!("send raw text: {e}"));
    }

    pub fn send_binary(&mut self, bytes: Vec<u8>) {
        self.ws
            .send(Message::Binary(bytes.into()))
            .unwrap_or_else(|e| panic!("send binary: {e}"));
    }

    pub fn close(&mut self) {
        let _ = self.ws.close(None);
    }

    /// Read the next JSON frame or `None` on deadline/EOF.
    pub fn read_value_within(&mut self, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return None;
            }
            match self.ws.read() {
                Ok(Message::Text(text)) => {
                    return Some(
                        serde_json::from_str(text.as_str())
                            .unwrap_or_else(|e| panic!("bad JSON frame {text:?}: {e}")),
                    )
                }
                Ok(Message::Ping(_)) => {
                    let _ = self.ws.flush();
                }
                Ok(Message::Pong(_) | Message::Binary(_) | Message::Frame(_)) => {}
                Ok(Message::Close(_)) => return None,
                Err(WsError::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) =>
                {
                    continue
                }
                Err(_) => return None,
            }
        }
    }

    pub fn read_value(&mut self) -> Value {
        self.read_value_within(Duration::from_secs(5))
            .unwrap_or_else(|| panic!("no frame within 5 s"))
    }

    /// Read frames until one satisfies `pred`, or the deadline passes.
    pub fn wait_for<F>(&mut self, timeout: Duration, mut pred: F) -> Option<Value>
    where
        F: FnMut(&Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let v = self.read_value_within(remaining)?;
            if pred(&v) {
                return Some(v);
            }
        }
    }

    pub fn wait_for_event(&mut self, event: &str, timeout: Duration) -> Option<Value> {
        self.wait_for(timeout, |v| v["event"].as_str() == Some(event))
    }

    /// One-request-one-frame RPC (for commands with a single reply).
    pub fn rpc(&mut self, value: &Value) -> Value {
        self.send(value);
        self.read_value()
    }

    /// Like `rpc`, but skips pushed event frames until the matching ack. Use
    /// this when the connection already has pending snapshot/delta pushes.
    pub fn rpc_ack(&mut self, cmd: &str, value: &Value) -> Value {
        self.send(value);
        self.wait_for(Duration::from_secs(5), |v| {
            v["event"] == "ack" && v["cmd"] == cmd
        })
        .unwrap_or_else(|| panic!("no ack for {cmd}"))
    }
}

pub fn deadline(ms: u64) -> Duration {
    Duration::from_millis(ms)
}
