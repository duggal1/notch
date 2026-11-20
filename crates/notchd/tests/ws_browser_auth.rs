//! Browser-compatible WS handshake (opt-in) — RED tests. A real webview can
//! send Origin but cannot set Authorization; the opt-in accepts a 64-hex query
//! token with an exact Host. Default strict mode must be byte-for-byte the
//! same auth policy as before.

use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::ws;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tungstenite::client::ClientRequestBuilder;
use tungstenite::connect;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn fake_herdr() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        match (
            args.get(1).map(String::as_str),
            args.get(2).map(String::as_str),
        ) {
            (Some("agent"), Some("list")) | (Some("pane"), Some("list")) => {
                Ok(json!({"result":{"agents":[],"panes":[]}}).to_string())
            }
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: "unexpected fake herdr call".into(),
            }),
        }
    })
}

struct Server {
    root: PathBuf,
    port: u16,
    token: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Server {
    fn start(tag: &str, browser_mode: bool) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "notchd-ws-browser-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            state_dir: root.join("state"),
            tmp_dir: root.join("tmp"),
            stuck_after_secs: 900,
            poll_interval_ms: 3_600_000,
            opencode_decider: false,
        };
        std::fs::create_dir_all(&config.tmp_dir).unwrap();
        let token = ws::load_or_create_token(&config.state_dir).unwrap();
        let listener = ws::bind(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let daemon = Arc::new(Daemon::open(config, fake_herdr(), Arc::new(SystemClock)).unwrap());
        let shutdown = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || {
            let _ = ws::serve_with_mode(
                listener,
                daemon,
                s,
                Arc::new(AtomicUsize::new(0)),
                browser_mode,
            );
        });
        let server = Self {
            root,
            port,
            token,
            shutdown,
            handle: Some(handle),
        };
        // Strict-mode readiness probe uses the existing Bearer path.
        assert!(
            server
                .raw_status("", None, Some(&format!("Bearer {}", server.token)))
                .is_some()
                || browser_mode,
            "server did not come up"
        );
        server
    }

    fn url(&self, query: &str) -> String {
        format!("ws://127.0.0.1:{}/?{query}", self.port)
    }

    /// Raw HTTP upgrade status. `host` overrides Host for DNS-rebinding tests;
    /// `bearer` exercises the legacy header path.
    fn raw_status(&self, host: &str, origin: Option<&str>, bearer: Option<&str>) -> Option<u16> {
        let addr: std::net::SocketAddr = format!("127.0.0.1:{}", self.port).parse().unwrap();
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
        s.set_read_timeout(Some(Duration::from_millis(600))).ok()?;
        let h = if host.is_empty() {
            format!("127.0.0.1:{}", self.port)
        } else {
            host.to_string()
        };
        let origin_line = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        let auth_line = bearer
            .map(|b| format!("Authorization: {b}\r\n"))
            .unwrap_or_default();
        let req = format!(
            "GET /?token={} HTTP/1.1\r\nHost: {h}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{origin_line}{auth_line}\r\n",
            self.token
        );
        s.write_all(req.as_bytes()).ok()?;
        let mut buf = [0u8; 256];
        let n = s.read(&mut buf).ok()?;
        let head = String::from_utf8_lossy(&buf[..n]);
        head.split_whitespace().nth(1)?.parse().ok()
    }

    fn raw_path_status(&self, path: &str, host: &str, origin: Option<&str>) -> Option<u16> {
        let addr: std::net::SocketAddr = format!("127.0.0.1:{}", self.port).parse().unwrap();
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
        s.set_read_timeout(Some(Duration::from_millis(600))).ok()?;
        let h = if host.is_empty() {
            format!("127.0.0.1:{}", self.port)
        } else {
            host.to_string()
        };
        let origin_line = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {h}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{origin_line}\r\n"
        );
        s.write_all(req.as_bytes()).ok()?;
        let mut buf = [0u8; 256];
        let n = s.read(&mut buf).ok()?;
        String::from_utf8_lossy(&buf[..n])
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
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

/// Real browser-like upgrade: Origin header + query token, no Authorization.
fn browser_connect(url: &str) -> Option<Value> {
    let req = ClientRequestBuilder::new(url.parse().unwrap())
        .with_header("Origin", "https://demo.example");
    let (mut ws, _) = connect(req)
        .map_err(|e| {
            eprintln!("browser_connect failed for {url}: {e}");
            e
        })
        .ok()?;
    if let tungstenite::stream::MaybeTlsStream::Plain(stream) = ws.get_mut() {
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .ok()?;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        match ws.read() {
            Ok(tungstenite::Message::Text(t)) => return Some(serde_json::from_str(&t).unwrap()),
            Ok(tungstenite::Message::Ping(_)) => {
                let _ = ws.flush();
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(_) => return None,
        }
    }
}

#[test]
fn browser_mode_accepts_origin_with_query_token_and_hello() {
    let server = Server::start("accept", true);
    let hello = browser_connect(&server.url(&format!("token={}", server.token)))
        .expect("browser-mode handshake with Origin + query token must succeed");
    assert_eq!(hello["event"], "hello");
    assert_eq!(hello["version"], 1);
    assert!(hello["snapshot"].is_object());
}

#[test]
fn browser_mode_rejects_hostile_no_token_wrong_token_and_wrong_host() {
    let server = Server::start("reject", true);
    // A hostile page sends Origin but has no token.
    let hostile = server.raw_path_status("/", "", Some("https://evil.example"));
    assert!(
        matches!(hostile, Some(401 | 403)),
        "no-token Origin must be rejected, got {hostile:?}"
    );
    // A guessed token must not work.
    let wrong = server.raw_path_status(
        "/?token=0000000000000000000000000000000000000000000000000000000000000000",
        "",
        Some("https://evil.example"),
    );
    assert!(
        matches!(wrong, Some(401 | 403)),
        "wrong token must be rejected, got {wrong:?}"
    );
    // Correct token with wrong Host (DNS rebinding) must not work.
    let rebound = server.raw_path_status(
        &format!("/?token={}", server.token),
        "evil.example:7407",
        Some("https://evil.example"),
    );
    assert!(
        matches!(rebound, Some(401 | 403)),
        "wrong Host must be rejected, got {rebound:?}"
    );
}

#[test]
fn default_strict_mode_rejects_browser_style_and_keeps_bearer() {
    let server = Server::start("strict", false);
    let browser = server.raw_path_status(
        &format!("/?token={}", server.token),
        "",
        Some("https://demo.example"),
    );
    assert!(
        matches!(browser, Some(401 | 403)),
        "strict mode rejects any Origin, got {browser:?}"
    );
    let legacy = server.raw_status("", None, Some(&format!("Bearer {}", server.token)));
    assert_eq!(legacy, Some(101), "strict Bearer handshake unchanged");
}

#[test]
fn token_is_unguessable_and_not_reusable_across_daemons() {
    let a = Server::start("token-a", true);
    let b = Server::start("token-b", true);
    assert_eq!(a.token.len(), 64);
    assert!(a.token.bytes().all(|x| x.is_ascii_hexdigit()));
    assert_ne!(
        a.token, b.token,
        "independent daemons must not share a token"
    );
    let reused = b.raw_path_status(&format!("/?token={}", a.token), "", None);
    assert!(
        matches!(reused, Some(401 | 403)),
        "another daemon's token must not authenticate, got {reused:?}"
    );
}

#[test]
fn browser_handshake_latency_p50_p95_under_100ms() {
    let server = Server::start("latency", true);
    let url = server.url(&format!("token={}", server.token));
    let mut samples = Vec::new();
    for _ in 0..20 {
        let t0 = Instant::now();
        let hello = browser_connect(&url).expect("handshake must succeed");
        assert_eq!(hello["event"], "hello");
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    eprintln!(
        "browser handshake: p50={p50:.3}ms p95={p95:.3}ms n={}",
        samples.len()
    );
    assert!(
        p95 < 100.0,
        "browser handshake p95 {p95:.1}ms must be < 100ms"
    );
}

#[test]
fn env_flag_defaults_off_and_opt_in_is_explicit() {
    // Process-global env: serialize with this test's own mutex; no other
    // test in this crate reads NOTCH_WS_BROWSER.
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _g = ENV_LOCK.lock().unwrap();
    std::env::remove_var("NOTCH_WS_BROWSER");
    assert!(!ws::browser_ws_enabled(), "strict mode is the default");
    std::env::set_var("NOTCH_WS_BROWSER", "1");
    assert!(ws::browser_ws_enabled(), "NOTCH_WS_BROWSER=1 opts in");
    std::env::set_var("NOTCH_WS_BROWSER", "true");
    assert!(!ws::browser_ws_enabled(), "only the exact value 1 opts in");
    std::env::remove_var("NOTCH_WS_BROWSER");
}
