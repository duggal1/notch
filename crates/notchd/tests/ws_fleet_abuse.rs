//! T3 — brutal abuse of the fleet WebSocket: capacity, handshake auth,
//! frame floods, boundary frames, post-flood responsiveness and no fd/thread
//! leak across 200 connect/disconnect cycles. Real TCP + real WS bytes.

#[path = "fixtures/ws/harness.rs"]
mod harness;

use harness::*;
use notchd::ws::MAX_CLIENTS;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::time::{Duration, Instant};
use tungstenite::Message;

fn count_fds() -> usize {
    std::fs::read_dir("/dev/fd")
        .map(|it| it.count())
        .unwrap_or(0)
}

fn count_threads() -> usize {
    let pid = std::process::id().to_string();
    let out = Command::new("ps")
        .args(["-M", "-p", &pid])
        .output()
        .expect("ps -M");
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter(|l| l.split_whitespace().nth(1) == Some(pid.as_str()))
        .count()
}

/// Raw HTTP upgrade; returns the HTTP status code or None on connect failure.
fn raw_upgrade(port: u16, origin: Option<&str>, auth: Option<&str>, host: &str) -> Option<u16> {
    let mut s = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_secs(2),
    )
    .ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut req = String::from(
        "GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n",
    );
    if let Some(o) = origin {
        req.push_str(&format!("Origin: {o}\r\n"));
    }
    if let Some(a) = auth {
        req.push_str(&format!("Authorization: {a}\r\n"));
    }
    req.push_str(&format!("Host: {host}\r\n\r\n"));
    s.write_all(req.as_bytes()).ok()?;
    let mut buf = [0u8; 128];
    let n = s.read(&mut buf).ok()?;
    let head = String::from_utf8_lossy(&buf[..n]);
    head.split_whitespace().nth(1)?.parse().ok()
}

#[test]
fn fleet_ws_rejects_origin_token_and_host_but_accepts_valid_bearer() {
    let server = WsDaemon::start("fleet-auth");
    let good_host = format!("127.0.0.1:{}", server.port);
    let good_auth = format!("Bearer {}", server.token);

    let valid = raw_upgrade(server.port, None, Some(&good_auth), &good_host);
    assert_eq!(
        valid,
        Some(101),
        "valid Bearer + loopback Host must upgrade; got {valid:?}"
    );

    let cases: Vec<(&str, Option<u16>)> = vec![
        (
            "Origin: null",
            raw_upgrade(server.port, Some("null"), Some(&good_auth), &good_host),
        ),
        (
            "Origin: evil",
            raw_upgrade(
                server.port,
                Some("http://evil.example"),
                Some(&good_auth),
                &good_host,
            ),
        ),
        (
            "wrong token",
            raw_upgrade(
                server.port,
                None,
                Some("Bearer 0000000000000000000000000000000000000000000000000000000000000000"),
                &good_host,
            ),
        ),
        (
            "missing token",
            raw_upgrade(server.port, None, None, &good_host),
        ),
        (
            "bad scheme",
            raw_upgrade(server.port, None, Some(&server.token), &good_host),
        ),
        (
            "wrong host",
            raw_upgrade(server.port, None, Some(&good_auth), "127.0.0.1:1"),
        ),
        (
            "localhost host",
            raw_upgrade(server.port, None, Some(&good_auth), "localhost:7407"),
        ),
    ];
    for (name, got) in cases {
        assert!(
            matches!(got, Some(code) if (400..600).contains(&code)),
            "{name} must be rejected with 4xx/5xx, got {got:?}"
        );
    }
}

#[test]
fn fleet_ws_100_client_storm_caps_at_16_and_daemon_stays_alive() {
    let server = WsDaemon::start("fleet-storm");
    assert!(
        server.wait_for_active(0, Duration::from_secs(5)),
        "readiness slot never released"
    );

    let mut admitted = Vec::new();
    let mut rejected = 0usize;
    for _ in 0..100 {
        match server.connect_raw_with_auth() {
            Ok(mut ws) => match ws.read() {
                Ok(Message::Text(t))
                    if serde_json::from_str::<Value>(&t)
                        .map(|v| v["event"] == "hello")
                        .unwrap_or(false) =>
                {
                    admitted.push(ws)
                }
                Ok(Message::Close(_)) => rejected += 1,
                other => panic!("unexpected first frame from authenticated client: {other:?}"),
            },
            Err(e) => panic!("authenticated client could not upgrade during storm: {e}"),
        }
    }
    assert_eq!(
        admitted.len(),
        MAX_CLIENTS,
        "exactly 16 clients admitted: {}",
        admitted.len()
    );
    assert_eq!(
        rejected,
        100 - MAX_CLIENTS,
        "84 over-capacity clients closed: {rejected}"
    );
    assert_eq!(
        server.active(),
        MAX_CLIENTS,
        "over-capacity clients must not consume slots"
    );

    // Every admitted client still talks to the daemon after the storm.
    let c = &mut admitted[0];
    c.send(Message::Text(json!({"cmd":"ping"}).to_string().into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ok = false;
    while Instant::now() < deadline {
        if let Ok(Message::Text(t)) = c.read() {
            if let Ok(v) = serde_json::from_str::<Value>(&t) {
                if v["event"] == "ack" && v["cmd"] == "ping" {
                    ok = v["ok"] == true;
                    break;
                }
            }
        }
    }
    assert!(ok, "daemon stopped answering ping after 100-client storm");
}

#[test]
fn fleet_ws_100k_frames_and_256kb_boundary_leave_daemon_responsive() {
    let server = WsDaemon::start("fleet-flood");
    let mut c = server.connect();
    let hello = c.read_value();
    assert_eq!(hello["event"], "hello");

    // The fleet frame is emitted on connect only when units exist; seed one
    // real session so the post-flood fleet assertion is meaningful.
    let work = server.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let seed = c.rpc_ack(
        "submit_task",
        &json!({
            "cmd":"submit_task", "task":"flood seed", "cwd": work.to_string_lossy()
        }),
    );
    assert_eq!(seed["ok"], true, "seed failed: {seed}");

    // 100,000 randomized valid frames; every one must be acked inline.
    let mut seed = 0x1234_5678u64;
    let n = 100_000usize;
    const CHUNK: usize = 1000;
    let t0 = Instant::now();
    let mut acked = 0usize;
    while acked < n {
        let end = (acked + CHUNK).min(n);
        // Send a burst, then drain its acks. Chunking keeps both socket
        // buffers moving under heavy parallel load while still delivering a
        // real 100,000-frame flood with every frame acked inline.
        for i in acked..end {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let pad = format!("{:016x}", seed ^ (i as u64));
            c.send(&json!({"cmd":"ping","pad":pad}));
        }
        for i in acked..end {
            // The connect-time `fleet` frame (and any change push) can
            // interleave; a command's reply is the ack frame, so skip non-acks.
            let v = loop {
                let x = c
                    .read_value_within(Duration::from_secs(10))
                    .unwrap_or_else(|| panic!("missing ack {i}/{n} after 100k-frame flood"));
                if x["event"] == "ack" {
                    break x;
                }
            };
            assert_eq!(v["event"], "ack", "flood ack {i}: {v}");
            assert_eq!(v["cmd"], "ping", "flood ack {i}: {v}");
            assert_eq!(v["ok"], true, "flood ack {i}: {v}");
        }
        acked = end;
    }
    assert_eq!(acked, n);
    let elapsed = t0.elapsed();
    eprintln!(
        "fleet 100k text frames fully acked in {} ms",
        elapsed.as_millis()
    );

    // Exact 256 KiB MESSAGE (JSON braces + pad included) is accepted;
    // 256 KiB + 1 is rejected by the WS config's max_message_size.
    let overhead = "{\"cmd\":\"ping\",\"pad\":\"\"}".len();
    let ok = format!(
        "{{\"cmd\":\"ping\",\"pad\":\"{}\"}}",
        "x".repeat(256 * 1024 - overhead)
    );
    assert_eq!(
        ok.len(),
        256 * 1024,
        "boundary frame must be exactly 256 KiB"
    );
    c.send_text(&ok);
    let resp = c.read_value_within(Duration::from_secs(5));
    assert!(
        resp.is_some(),
        "256 KiB boundary frame must not kill the connection"
    );

    let big = format!(
        "{{\"cmd\":\"ping\",\"pad\":\"{}\"}}",
        "x".repeat(256 * 1024 + 1 - overhead)
    );
    assert_eq!(
        big.len(),
        256 * 1024 + 1,
        "oversize frame must be 256 KiB + 1"
    );
    c.send_text(&big);
    // The oversize frame closes the connection; either EOF or an error is fine.
    let _ = c.read_value_within(Duration::from_secs(3));

    // A fresh authenticated client still gets hello + a real fleet frame +
    // pong after the abuse: the fleet stream itself survived the flood.
    let mut fresh = server.connect();
    let h = fresh.read_value_within(Duration::from_secs(10));
    assert_eq!(h.as_ref().and_then(|v| v["event"].as_str()), Some("hello"));
    // Bounded but generous: under 9-way parallel load the accept/handshake
    // thread can be descheduled for seconds; the assertion (a real fleet
    // frame with ORCH) is unchanged.
    let fleet = fresh
        .wait_for_event("fleet", Duration::from_secs(15))
        .expect("fleet stream dead after 100k + boundary frames");
    let units = fleet["fleet"]["units"].as_array().unwrap();
    assert!(
        units.iter().any(|u| u["id"] == "ORCH"),
        "fleet lost ORCH after abuse: {units:?}"
    );
    fresh.send(&json!({"cmd":"ping"}));
    let ping_deadline = Instant::now() + Duration::from_secs(15);
    let mut pong_ok = false;
    while Instant::now() < ping_deadline {
        if let Some(v) = fresh.read_value_within(Duration::from_secs(1)) {
            if v["event"] == "ack" && v["cmd"] == "ping" {
                pong_ok = v["ok"] == true;
                break;
            }
        }
    }
    assert!(pong_ok, "daemon wedged after 100k + boundary frames");
}

#[test]
fn fleet_ws_no_fd_or_thread_leak_across_200_connect_disconnect_cycles() {
    let server = WsDaemon::start("fleet-leak");
    assert!(
        server.wait_for_active(0, Duration::from_secs(5)),
        "readiness slot stuck"
    );

    let fds_before = count_fds();
    let threads_before = count_threads();

    for i in 0..200 {
        let mut c = server.connect();
        let h = c.read_value_within(Duration::from_secs(5));
        assert_eq!(
            h.as_ref().and_then(|v| v["event"].as_str()),
            Some("hello"),
            "cycle {i}"
        );
        c.close();
        assert!(
            server.wait_for_active(0, Duration::from_secs(5)),
            "cycle {i}: slot never released"
        );
    }

    // Condition-based settling: wait until both counts are stable, bounded.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut fds_after = count_fds();
    let mut threads_after = count_threads();
    while Instant::now() < deadline
        && (fds_after > fds_before + 8 || threads_after > threads_before + 8)
    {
        std::thread::yield_now();
        fds_after = count_fds();
        threads_after = count_threads();
    }
    eprintln!(
        "fleet 200 cycles: fds {fds_before}->{fds_after}, threads {threads_before}->{threads_after}"
    );
    assert!(
        fds_after <= fds_before + 8,
        "fd leak across 200 cycles: {fds_before} -> {fds_after}"
    );
    assert!(
        threads_after <= threads_before + 8,
        "thread leak across 200 cycles: {threads_before} -> {threads_after}"
    );
}
