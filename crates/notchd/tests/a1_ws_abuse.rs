//! A1 / T8: live WebSocket abuse matrix against a real listener + daemon.
//!
//! RED FIRST: written before the auth implementation; now verifies real
//! handshakes and frame flooding. Run: cargo test -p notchd --test a1_ws_abuse

#[path = "fixtures/ws/harness.rs"]
mod harness;

use harness::*;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Raw HTTP upgrade so every header is under test control (the tungstenite
/// client rewrites `Host` from the URI, which would mask a Host-check bug).
fn handshake_status(
    server: &WsDaemon,
    origin: Option<&str>,
    token: &str,
    host: &str,
) -> Option<u16> {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let mut req = format!(
        "GET / HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
         Authorization: Bearer {token}\r\n"
    );
    if let Some(o) = origin {
        req.push_str(&format!("Origin: {o}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut buf = [0u8; 128];
    let n = stream.read(&mut buf).ok()?;
    let line = String::from_utf8_lossy(&buf[..n]);
    line.split_whitespace().nth(1)?.parse::<u16>().ok()
}

#[test]
fn t8_live_handshake_rejects_origin_wrong_token_and_host() {
    let server = WsDaemon::start("a1-abuse-auth");
    let good_host = format!("127.0.0.1:{}", server.port);
    // Control: valid bearer + bound loopback Host + no Origin is admitted.
    assert_eq!(
        handshake_status(&server, None, &server.token, &good_host),
        Some(101),
        "valid handshake must be upgraded"
    );

    for origin in ["null", "https://evil.example", "http://127.0.0.1:1"] {
        assert_ne!(
            handshake_status(&server, Some(origin), &server.token, &good_host),
            Some(101),
            "Origin {origin:?} must be rejected at the handshake"
        );
    }
    assert_ne!(
        handshake_status(&server, None, &"0".repeat(64), &good_host),
        Some(101),
        "wrong token must be rejected"
    );
    assert_ne!(
        handshake_status(&server, None, "", &good_host),
        Some(101),
        "empty bearer must be rejected"
    );
    assert_ne!(
        handshake_status(&server, None, &server.token, "localhost:7407"),
        Some(101),
        "non-loopback Host must be rejected"
    );
    assert_ne!(
        handshake_status(&server, None, &server.token, "127.0.0.1:1"),
        Some(101),
        "wrong loopback port must be rejected"
    );

    // The daemon is still responsive for an authenticated client.
    let mut c = server.connect();
    assert_eq!(c.read_value()["event"], "hello");
    let pong = c.rpc_ack("ping", &json!({"cmd": "ping"}));
    assert_eq!(pong["ok"], true);
}

#[test]
fn t8_100k_frames_and_256kb_boundary_leave_daemon_responsive() {
    let server = WsDaemon::start("a1-abuse-flood");
    let mut c = server.connect();
    assert_eq!(c.read_value()["event"], "hello");

    // 100,000 random-ish command frames in batches, draining acks so a slow
    // reader can never be mistaken for a server stall.
    let t0 = Instant::now();
    let mut delivered = 0usize;
    for base in (0..100_000u64).step_by(1000) {
        for i in base..base + 1000 {
            c.send(&json!({"cmd": "ping", "n": i, "pad": i.wrapping_mul(2654435761)}));
        }
        delivered += c.drain_frames(1000, Duration::from_secs(10));
    }
    let flood_ms = t0.elapsed().as_millis();
    assert_eq!(delivered, 100_000, "every frame must get a reply, got {delivered} in {flood_ms}ms");

    // Exactly at the 256 KiB frame cap: server answers (invalid_json ack).
    let at_cap = "x".repeat(256 * 1024);
    c.send_text(&at_cap);
    let ack = c.read_value_within(Duration::from_secs(5));
    assert!(ack.is_some(), "frame at the cap must be processed: {ack:?}");
    assert_eq!(ack.unwrap()["error"], "invalid_json");

    // One byte over the cap: dropped by the server's WebSocketConfig.
    c.send_text(&"x".repeat(256 * 1024 + 1));
    assert!(
        c.read_value_within(Duration::from_secs(3)).is_none(),
        "oversize frame must not produce an ack"
    );
    drop(c);

    // Fresh authenticated client proves the daemon never wedged.
    let mut fresh = server.connect();
    let hello = fresh.read_value_within(Duration::from_secs(5));
    assert_eq!(hello.as_ref().and_then(|v| v["event"].as_str()), Some("hello"));
    let pong = fresh.rpc_ack("ping", &json!({"cmd": "ping"}));
    assert_eq!(pong["ok"], true);

    eprintln!("T8: 100000 frames in {flood_ms}ms; 256KiB boundary accepted; +1 rejected");
}
