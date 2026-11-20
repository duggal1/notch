//! A9 correctness gates for the real loopback WebSocket endpoint.
//!
//! Real TCP sockets, real `Daemon`, real `ws::serve_with_mode`. No fake CLI,
//! no injected runners, no sleeps used as synchronization. The only wait is a
//! bounded read deadline on the socket under test.
//!
//! Latency numbers are NOT here (timing tests are flaky as gates); they live
//! as raw tables in $PROOF/a9.md.

use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::ws::{self, OutFrame};
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

static N: AtomicU64 = AtomicU64::new(0);

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-wsreal-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(p.join("tmp")).unwrap();
    p
}

struct Srv {
    port: u16,
    token: String,
    root: PathBuf,
    daemon: Arc<Daemon>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Srv {
    fn start(tag: &str, browser_mode: bool) -> Self {
        let root = root(tag);
        let config = Config {
            state_dir: root.join("state"),
            tmp_dir: root.join("tmp"),
            stuck_after_secs: 3600,
            poll_interval_ms: 3_600_000, // no poll loop in these tests
            opencode_decider: false,
        };
        let token = ws::load_or_create_token(&config.state_dir).unwrap();
        let daemon =
            Arc::new(Daemon::open(config, Herdr::from_env(), Arc::new(SystemClock)).unwrap());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let d = Arc::clone(&daemon);
        let s = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            let _ =
                ws::serve_with_mode(listener, d, s, Arc::new(AtomicUsize::new(0)), browser_mode);
        });
        Self {
            port,
            token,
            root,
            daemon,
            shutdown,
            handle: Some(handle),
        }
    }
}

impl Drop for Srv {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(30));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Send a raw HTTP upgrade and return only the status line. `host` is explicit
/// so the pinned-Host check can be attacked without URL tricks.
fn handshake(port: u16, path: &str, host: &str, extra: &[&str]) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n"
    );
    for h in extra {
        req.push_str(h);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = [0u8; 1024];
    let n = s.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

#[test]
fn strict_mode_rejects_every_bypass_and_accepts_only_bearer() {
    let srv = Srv::start("strict", false);
    let host = format!("127.0.0.1:{}", srv.port);
    let bearer = format!("Authorization: Bearer {}", srv.token);
    let good = handshake(srv.port, "/", &host, &[&bearer]);
    assert!(good.contains("101"), "valid Bearer must upgrade: {good}");

    let cases: Vec<(&str, Vec<String>)> = vec![
        ("no token", vec![]),
        ("wrong token", vec!["Authorization: Bearer 0000000000000000000000000000000000000000000000000000000000000000".into()]),
        ("foreign origin", vec![bearer.clone(), "Origin: https://evil.example".into()]),
        ("bad host", vec![bearer.clone()]),
        ("bad scheme token", vec!["Authorization: Basic dXNlcjpwYXNz".into()]),
    ];
    for (name, extra) in cases {
        let refs: Vec<&str> = extra.iter().map(String::as_str).collect();
        let h = if name == "bad host" {
            handshake(srv.port, "/", "evil.example:7407", &refs)
        } else {
            handshake(srv.port, "/", &host, &refs)
        };
        assert!(h.contains("401"), "{name} must be 401: {h}");
    }
}

#[test]
fn browser_mode_requires_query_token_and_exact_host() {
    let srv = Srv::start("browser", true);
    let host = format!("127.0.0.1:{}", srv.port);
    let good = handshake(
        srv.port,
        &format!("/?token={}", srv.token),
        &host,
        &["Origin: https://ui.local"],
    );
    assert!(
        good.contains("101"),
        "valid query token + Origin must upgrade: {good}"
    );

    let no_token = handshake(srv.port, "/", &host, &["Origin: https://ui.local"]);
    assert!(
        no_token.contains("401"),
        "missing query token must be 401: {no_token}"
    );

    let wrong = handshake(
        srv.port,
        "/?token=0000000000000000000000000000000000000000000000000000000000000000",
        &host,
        &[],
    );
    assert!(
        wrong.contains("401"),
        "wrong query token must be 401: {wrong}"
    );

    let bad_host = handshake(
        srv.port,
        &format!("/?token={}", srv.token),
        "evil.example:7407",
        &[],
    );
    assert!(
        bad_host.contains("401"),
        "bad Host must be 401 even with token: {bad_host}"
    );
}

#[test]
fn ws_fleet_frame_uses_single_rust_derivation() {
    let srv = Srv::start("fleet", false);
    let snap = serde_json::to_value(srv.daemon.snapshot()).unwrap();
    let frame = ws::fleet_frame(&srv.daemon, &snap).expect("ledger must be readable");
    let ledger = notchd::ledger::Ledger::open(&srv.daemon.config.state_dir).unwrap();
    let expected =
        notchd::snapshot::fleet_view(&snap, ledger.last_seq(), ledger.current_epoch("ORCH"));
    assert_eq!(frame["event"], "fleet");
    assert_eq!(
        frame["fleet"], expected,
        "WS frame must be A4's fleet_view verbatim"
    );
    assert_eq!(frame["fleet"]["ledger"]["seq"], ledger.last_seq());
}

#[test]
fn token_file_is_0600_in_0700_dir() {
    let r = root("perm");
    let dir = r.join("state");
    let token = ws::load_or_create_token(&dir).unwrap();
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    let dmode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    let fmode = fs::metadata(dir.join("ws.token"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dmode, 0o700, "state dir must be 0700");
    assert_eq!(fmode, 0o600, "token file must be 0600");
    let _ = fs::remove_dir_all(&r);
}

#[test]
fn outbox_is_bounded_and_drops_snapshots_first() {
    let mut q: VecDeque<OutFrame> = VecDeque::new();
    let mut lag = 0u64;
    for i in 0..8 {
        ws::enqueue_bounded(
            &mut q,
            8,
            OutFrame::Snapshot(serde_json::json!({"i": i})),
            &mut lag,
        );
    }
    // 9th push overflows: the OLDEST SNAPSHOT (i=0) must be the victim.
    ws::enqueue_bounded(
        &mut q,
        8,
        OutFrame::Event(serde_json::json!({"i": 99})),
        &mut lag,
    );
    assert_eq!(lag, 1, "one drop counted");
    match &q[0] {
        OutFrame::Snapshot(v) => assert_eq!(v["i"], 1, "oldest snapshot dropped, not the event"),
        OutFrame::Event(_) => panic!("an event must not be dropped while a snapshot is queued"),
    }
    for i in 8..1_000 {
        ws::enqueue_bounded(
            &mut q,
            8,
            OutFrame::Event(serde_json::json!({"i": i})),
            &mut lag,
        );
        assert!(q.len() <= 8, "queue must never exceed cap: {}", q.len());
    }
    assert_eq!(q.len(), 8);
    assert!(lag >= 993, "every overflow increments lag: {lag}");
}

#[test]
fn oversized_frame_is_closed_without_affecting_new_clients() {
    let srv = Srv::start("frame", false);
    let host = format!("127.0.0.1:{}", srv.port);
    let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let req = format!(
        "GET / HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
         Authorization: Bearer {}\r\n\r\n",
        srv.token
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = [0u8; 4096];
    // Drain 101 + hello + any fleet frame; end on the bounded read deadline.
    while s.read(&mut buf).unwrap_or(0) > 0 {}

    // One raw masked text frame of 256 KiB + 1 byte.
    let n = 256 * 1024 + 1;
    let mask = [1u8, 2, 3, 4];
    let mut frame = Vec::with_capacity(n + 14);
    frame.push(0x81);
    frame.push(0x80 | 127);
    frame.extend_from_slice(&(n as u64).to_be_bytes());
    frame.extend_from_slice(&mask);
    for i in 0..n {
        frame.push(b'x' ^ mask[i % 4]);
    }
    s.write_all(&frame).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut reply = [0u8; 8];
    let got = s.read(&mut reply).unwrap_or(0);
    assert!(
        got > 0 && reply[0] == 0x88,
        "oversized frame must get a WS close, got {reply:?}"
    );

    let host = format!("127.0.0.1:{}", srv.port);
    let fresh = handshake(
        srv.port,
        "/",
        &host,
        &[&format!("Authorization: Bearer {}", srv.token)],
    );
    assert!(
        fresh.contains("101"),
        "a new client must still connect: {fresh}"
    );
}
