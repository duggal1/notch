//! Half-write then close from the client mid-frame. The server must not
//! hang, must not error-log-storm, and clients behind the flood must
//! reconnect fine.
//! Naming: socket_halfwrite_<case>.

#[path = "fixtures/socket/harness.rs"]
mod harness;

use harness::*;
use serde_json::json;
use std::io::Write;
use std::time::Duration;

/// Client writes HALF a frame, then vanishes (both halves of the socket
/// closed). Server must not hang; fresh clients must reconnect fine.
#[test]
fn socket_halfwrite_close_mid_frame() {
    let d = TestDaemon::start("half-close");
    let _ = d.run_plan();

    // Round 1: bare half-frame "{"cmd":"p" (no newline), then full close.
    {
        let stream = std::os::unix::net::UnixStream::connect(&d.sock).unwrap();
        let mut w = stream.try_clone().unwrap();
        w.write_all(b"{\"cmd\":\"p").unwrap();
        w.flush().unwrap();
        // Full close without newline.
        drop(w);
        drop(stream);
    }

    // Round 2: complete frames followed by a partial frame, then close.
    {
        let stream = std::os::unix::net::UnixStream::connect(&d.sock).unwrap();
        let mut w = stream.try_clone().unwrap();
        w.write_all(b"{\"cmd\":\"ping\"}\n{\"cmd\":\"sna").unwrap();
        w.flush().unwrap();
        drop(w);
        drop(stream);
    }

    // Round 3: half-frame of a bulk-looking write.
    {
        let stream = std::os::unix::net::UnixStream::connect(&d.sock).unwrap();
        let mut w = stream.try_clone().unwrap();
        w.write_all(&[b'{'; 4096]).unwrap();
        w.flush().unwrap();
        drop(w);
        drop(stream);
    }

    // Clients behind the same flood reconnect fine, immediately after.
    for i in 0..20 {
        let mut c = TestConn::connect_with_timeouts(
            &d.sock,
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let _hello = c.hello();
        let ack = c.rpc(&json!({"cmd":"ping"}));
        assert_eq!(ack["ok"], true, "reconnect #{i} failed");
    }

    // A subscriber behind the same flood still receives pushes.
    let mut sub = TestConn::connect(&d.sock);
    let _hello = sub.hello();
    let (_, _initial) = sub.subscribe();
    {
        let mut c = TestConn::connect(&d.sock);
        let _hello = c.hello();
        let ack = c.rpc(&json!({"cmd":"kill","id":"S1-OC01","reason":"halfwrite-after"}));
        assert_eq!(ack["ok"], true);
    }
    let hit = sub.wait_for_line(deadline_in(2000), |v| {
        v["event"] == "snapshot"
            && v["snapshot"]["sessions"]
                .as_array()
                .map(|ss| ss.iter().any(|s| s["failureReason"] == "halfwrite-after"))
                .unwrap_or(false)
    });
    assert!(
        hit.is_some(),
        "subscriber behind the half-write flood never saw the push"
    );

    // No error-log storm: the daemon logs nothing about parse failures or
    // half-frames (it never logged them to begin with) — assert that no such
    // lines exist, and stderr is not full of repeated complaints.
    let log = d.events_log().to_ascii_lowercase();
    for needle in [
        "invalid_json",
        "parse error",
        "half-frame",
        "half frame",
        "malformed",
    ] {
        assert!(
            !log.contains(needle),
            "events.jsonl contains {needle:?} — error-log storm after half-writes:\n{log}"
        );
    }
    let stderr = d.stderr_log();
    let repeat_count = stderr
        .lines()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("error") || l.contains("panic")
        })
        .count();
    eprintln!("socket_halfwrite_close_mid_frame: error-ish stderr lines after 3 half-writes: {repeat_count}");
    assert!(
        repeat_count < 4,
        "stderr shows repeated errors after half-writes:\n{stderr}"
    );
}

/// Client writes half a frame, then shuts down its WRITE side only and keeps
/// the socket open for reading. At EOF the server processes the partial line:
/// ACTUAL behavior (documented) — the partial frame is parsed and rejected
/// with an `invalid_json` ack (or treated as end-of-stream), and the server
/// closes its side afterwards. It must not hang.
#[test]
fn socket_halfwrite_shutdown_write_side_only() {
    let d = TestDaemon::start("half-shutdown");
    let stream = std::os::unix::net::UnixStream::connect(&d.sock).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut w = stream.try_clone().unwrap();
    w.write_all(b"{\"cmd\":\"ping\"}\n{\"cmd\":\"p").unwrap();
    w.flush().unwrap();
    w.shutdown(std::net::Shutdown::Write).unwrap();
    drop(w);

    // The server sends hello first; consume it.
    let mut reader = std::io::BufReader::new(stream);
    let mut l0 = String::new();
    std::io::BufRead::read_line(&mut reader, &mut l0).expect("read hello");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(l0.trim_end()).unwrap()["event"],
        "hello"
    );

    // First frame line: the complete ping frame acks ok.
    let mut l1 = String::new();
    std::io::BufRead::read_line(&mut reader, &mut l1).expect("read ping ack");
    let v1: serde_json::Value = serde_json::from_str(l1.trim_end()).unwrap();
    assert_eq!(v1["cmd"], "ping");
    assert_eq!(v1["ok"], true);

    // Second line: the EOF-terminated partial frame — behavior: either an
    // invalid_json ack, or a clean server-side close (EOF). Both are
    // acceptable; a hang is not.
    let mut l2 = String::new();
    match std::io::BufRead::read_line(&mut reader, &mut l2) {
        Ok(0) => {
            eprintln!("socket_halfwrite_shutdown_write_side_only: server closed at EOF after partial frame (no ack)");
        }
        Ok(_) => {
            let v2: serde_json::Value = serde_json::from_str(l2.trim_end())
                .unwrap_or_else(|e| panic!("partial-frame reply not JSON: {e}: {l2:?}"));
            eprintln!(
                "socket_halfwrite_shutdown_write_side_only: partial frame got reply: {}",
                l2.trim_end()
            );
            assert_eq!(v2["event"], "ack");
            assert_eq!(v2["ok"], false);
        }
        Err(e) => {
            panic!("read error after half-write (server hung or broke): {e}");
        }
    }

    // The daemon is unaffected for everyone else.
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    assert_eq!(c.rpc(&json!({"cmd":"ping"}))["ok"], true);
}
