//! Slow-reader fuzz cases: a subscriber that connects and never reads must
//! not wedge the daemon; after it closes, the server must prune it.
//! Naming: socket_slowreader_<case>.

#[path = "fixtures/socket/harness.rs"]
mod harness;

use harness::*;
use serde_json::json;
use std::io::Write;
use std::time::Duration;

/// A subscriber that connects and NEVER reads anything after subscribe.
/// Other clients must keep getting served — also while a change storm
/// overflows that subscriber's OS receive buffer.
#[test]
fn socket_slowreader_others_proceed() {
    let d = TestDaemon::start("slow-proceed");
    let _ = d.run_plan();

    // Slow client: connect, hello, subscribe — then never read again.
    let slow_stream = std::os::unix::net::UnixStream::connect(&d.sock).unwrap();
    let mut slow_writer = slow_stream.try_clone().unwrap();
    slow_writer.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
    slow_writer.flush().unwrap();
    // Keep the slow socket open without ever draining it.

    // Meanwhile a fast client pings and snapshots fine.
    let mut fast = TestConn::connect(&d.sock);
    let _hello = fast.hello();
    let ack = fast.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
    let snap = fast.rpc(&json!({"cmd":"snapshot"}));
    assert_eq!(snap["event"], "snapshot");

    // Change storm: 60 kills of the same already-failed session. Every kill
    // notifies a snapshot push (~1.3 KB for 4 sessions); ~80 KB total, which
    // overflows a typical Unix-socket receive buffer the slow client never
    // drains. The writer thread for the slow peer may then block inside the
    // kernel, but the daemon's accept/fan-out paths and every other client
    // must proceed.
    {
        let mut c = TestConn::connect(&d.sock);
        let _hello = c.hello();
        let ack = c.rpc(&json!({"cmd":"kill","id":"S1-OC01","reason":"pre-fail"}));
        assert_eq!(ack["ok"], true);
    }
    let mut storm = TestConn::connect(&d.sock);
    let _hello = storm.hello();
    for i in 0..60 {
        let ack = storm.rpc(&json!({"cmd":"kill","id":"S1-OC01","reason": format!("storm-{i}")}));
        assert_eq!(ack["ok"], true, "kill {i} failed");
    }

    // Clients behind the same daemon still answer quickly, mid-storm.
    let mut behind =
        TestConn::connect_with_timeouts(&d.sock, Duration::from_secs(3), Duration::from_secs(5));
    let _hello = behind.hello();
    let start = std::time::Instant::now();
    let ack = behind.rpc(&json!({"cmd":"ping"}));
    let ping_latency = start.elapsed();
    eprintln!("socket_slowreader_others_proceed: ping behind a full slow-subscriber buffer in {ping_latency:?}");
    assert_eq!(ack["ok"], true);
    assert!(
        ping_latency < Duration::from_secs(3),
        "ping behind the slow reader took {ping_latency:?} — daemon is wedged"
    );
    let status = behind.rpc(&json!({"cmd":"status"}));
    assert_eq!(status["event"], "status");

    // Daemon-level errors must not pile up while this is going on.
    let log = d.events_log().to_ascii_lowercase();
    for needle in [
        "subscriber",
        "epipe",
        "broken pipe",
        "slow consumer",
        "backpressure",
    ] {
        assert!(
            !log.contains(needle),
            "events.jsonl contains {needle:?} — the daemon is logging subscriber trouble:\n{log}"
        );
    }
    drop(slow_writer);
    drop(slow_stream);
}

/// After the slow client closes, the server must prune it: pushes stop
/// piling onto it, flushes keep working for everyone else, and nothing
/// appears in the warnings/events log about it.
///
/// Note: the daemon exposes no subscriber-count getter, so pruning is
/// verified through its observable effects — the unregister happens
/// synchronously in the connection's read loop on EOF, and any send that
/// still fails is dropped by the flusher's retain(). We assert: a fresh
/// subscriber receives post-close kill pushes promptly, the slow connection
/// path produces zero error entries, and the snapshot warnings array stays
/// empty.
#[test]
fn socket_slowreader_pruned_after_close() {
    let d = TestDaemon::start("slow-prune");
    let _ = d.run_plan();

    // Slow client subscribes, then closes without reading.
    {
        let stream = std::os::unix::net::UnixStream::connect(&d.sock).unwrap();
        let mut w = stream.try_clone().unwrap();
        w.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        w.flush().unwrap();
        // Give the server a moment to process the subscribe.
        std::thread::sleep(Duration::from_millis(200));
        // Drop WITHOUT reading: the server's read side sees EOF.
        drop(w);
        drop(stream);
    }
    std::thread::sleep(Duration::from_millis(200));

    // A fresh subscriber must receive pushes after the close.
    let mut fresh = TestConn::connect(&d.sock);
    let _hello = fresh.hello();
    let (_, _initial) = fresh.subscribe();
    // Cause a change; the fresh subscriber must see it.
    {
        let mut c = TestConn::connect(&d.sock);
        let _hello = c.hello();
        let ack = c.rpc(&json!({"cmd":"kill","id":"S1-OC01","reason":"after-close"}));
        assert_eq!(ack["ok"], true);
    }
    let hit = fresh.wait_for_line(deadline_in(2000), |v| {
        v["event"] == "snapshot"
            && v["snapshot"]["sessions"]
                .as_array()
                .map(|ss| ss.iter().any(|s| s["failureReason"] == "after-close"))
                .unwrap_or(false)
    });
    assert!(
        hit.is_some(),
        "fresh subscriber never saw the post-close kill push"
    );

    // Nothing about the dead subscriber may surface in warnings or events.
    let warnings = d.warnings();
    assert!(
        warnings.iter().all(|w| {
            let l = w.to_ascii_lowercase();
            !(l.contains("subscriber") || l.contains("epipe") || l.contains("broken pipe"))
        }),
        "warnings mention the dead subscriber: {warnings:?}"
    );
    let log = d.events_log().to_ascii_lowercase();
    for needle in ["subscriber", "epipe", "broken pipe", "slow consumer"] {
        assert!(
            !log.contains(needle),
            "events.jsonl contains {needle:?} after the slow client closed:\n{log}"
        );
    }
    // The daemon is still healthy.
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    assert_eq!(c.rpc(&json!({"cmd":"ping"}))["ok"], true);
}
