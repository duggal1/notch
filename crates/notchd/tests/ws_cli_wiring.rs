//! Integration proof that `notchd serve` actually starts the authenticated
//! WebSocket endpoint when the operator opts in with `NOTCH_WS=1`.
//! No mock: spawns the real binary with a temp state dir and a real NDJSON socket.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("notch-ws-cli-{tag}-{}", std::process::id()));
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

fn spawn_daemon(root: &Path, ws: Option<u16>) -> Child {
    let mut c = Command::new(env!("CARGO_BIN_EXE_notchd"));
    c.arg("serve")
        .env("NOTCH_STATE_DIR", root.join("state"))
        .env("NOTCH_SOCK", root.join("state/notch.sock"))
        .env("NOTCH_HERDR_CMD", "/bin/false") // herdr absence must be a warning, not fatal
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match ws {
        Some(port) => {
            c.env("NOTCH_WS", "1")
                .env("NOTCH_WS_PORT", port.to_string());
        }
        None => {
            c.env_remove("NOTCH_WS").env_remove("NOTCH_WS_PORT");
        }
    }
    c.spawn().unwrap()
}

fn until<F: Fn() -> bool>(limit: Duration, f: F) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < limit {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    f()
}

fn ndjson_ready(root: &Path) -> bool {
    UnixStream::connect(root.join("state/notch.sock")).is_ok()
}

/// A raw unauthenticated WebSocket upgrade. An opted-in server rejects it with
/// HTTP 403 (token check); a machine with no WS server refuses the TCP connect.
fn raw_upgrade_status(port: u16) -> Option<u16> {
    let mut s = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(500),
    )
    .ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1:PLACEHOLDER\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").ok()?;
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).ok()?;
    let head = String::from_utf8_lossy(&buf[..n]);
    head.split_whitespace().nth(1)?.parse().ok()
}

#[test]
fn serve_starts_authenticated_ws_when_opted_in() {
    let root = scratch("on");
    let port = free_port();
    let mut child = spawn_daemon(&root, Some(port));
    assert!(
        until(Duration::from_secs(5), || ndjson_ready(&root)),
        "daemon did not start"
    );
    let status = until(Duration::from_secs(5), || {
        raw_upgrade_status(port).is_some()
    })
    .then(|| raw_upgrade_status(port).unwrap());
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        matches!(status, Some(401) | Some(403)),
        "NOTCH_WS=1 must start the auth-guarded WS listener; got {status:?}"
    );
}

#[test]
fn serve_has_no_ws_listener_by_default() {
    let root = scratch("off");
    let port = free_port();
    let mut child = spawn_daemon(&root, None);
    assert!(
        until(Duration::from_secs(5), || ndjson_ready(&root)),
        "daemon did not start"
    );
    let open = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(200),
    )
    .is_ok();
    let _ = child.kill();
    let _ = child.wait();
    assert!(!open, "WS must stay unwired unless NOTCH_WS=1");
}
