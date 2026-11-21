//! Real-WebSocket concurrency, backpressure and lifecycle tests.

#[path = "fixtures/ws/harness.rs"]
mod harness;

use harness::*;
use notchd::ws::{enqueue_bounded, OutFrame, MAX_CLIENTS};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::Message;

#[test]
fn ws_four_clients_concurrently_each_get_their_own_deltas() {
    let server = Arc::new(WsDaemon::start("four"));
    let work = server.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let cwd = work.to_string_lossy().to_string();

    // Seed four distinct sessions sequentially. Concurrent `submit_task`
    // calls race the daemon's file-backed registry (a known daemon issue,
    // out of this agent's scope), so seeding is done before the client fan-out.
    let mut sessions = Vec::new();
    for i in 0..4 {
        let mut seed = server.connect();
        let _ = seed.read_value();
        let a = seed.rpc_ack("submit_task", &json!({
            "cmd":"submit_task","task":format!("client {i} task"),"cwd":cwd
        }));
        assert_eq!(a["ok"], true, "{a}");
        sessions.push(a["sessionId"].as_str().unwrap().to_string());
    }
    sessions.sort();
    sessions.dedup();
    assert_eq!(sessions.len(), 4, "seed must produce four ids: {sessions:?}");

    // Four real clients connect and act at the same time. A barrier lines up
    // a single bus message that must be fanned out to all four.
    let barrier = Arc::new(Barrier::new(5));
    let mut handles = Vec::new();
    for (i, sid) in sessions.iter().cloned().enumerate() {
        let s = Arc::clone(&server);
        let b = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let mut c = s.connect();
            let hello = c.read_value();
            assert_eq!(hello["event"], "hello");
            let pong = c.rpc_ack("ping", &json!({"cmd":"ping"}));
            assert_eq!(pong["ok"], true);
            let rep = c.rpc_ack("reprompt", &json!({"cmd":"reprompt","id":sid,"text":"continue"}));
            assert_eq!(rep["ok"], true, "{rep}");
            b.wait();
            let msg = c.wait_for(Duration::from_secs(3), |v| {
                v["event"] == "bus_message" && v["busMessage"]["to"] == sid
            });
            assert!(msg.is_some(), "client {i} missed the fanned-out delta");
        }));
    }
    barrier.wait();
    std::thread::sleep(Duration::from_millis(50));
    {
        let mut trigger = server.connect();
        let _ = trigger.read_value();
        for sid in &sessions {
            let a = trigger.rpc_ack("message", &json!({
                "cmd":"message","from":sessions[0],"to":sid,"type":"task","body":"fan out to all"
            }));
            assert_eq!(a["ok"], true, "{a}");
        }
    }
    for h in handles {
        h.join().expect("client thread panicked");
    }
}

#[test]
fn ws_backpressure_drops_oldest_snapshot_and_counts_lag() {
    let mut q: VecDeque<OutFrame> = VecDeque::new();
    let mut lag = 0u64;
    for i in 0..4 {
        enqueue_bounded(&mut q, 3, OutFrame::Snapshot(json!({"i": i})), &mut lag);
    }
    assert_eq!(q.len(), 3, "queue stays bounded");
    assert_eq!(lag, 1, "one snapshot was dropped");
    assert!(matches!(&q[0], OutFrame::Snapshot(v) if v["i"] == 1), "oldest snapshot dropped");

    // With no snapshots left, the oldest EVENT is dropped instead.
    for i in 0..3 {
        enqueue_bounded(&mut q, 3, OutFrame::Event(json!({"e": i})), &mut lag);
    }
    assert_eq!(q.len(), 3);
    assert_eq!(lag, 4);
    assert!(q.iter().all(|f| matches!(f, OutFrame::Event(_))));
    assert!(matches!(&q[0], OutFrame::Event(v) if v["e"] == 0), "oldest event dropped last");
}

#[test]
fn ws_slow_reader_does_not_block_other_clients_or_daemon() {
    let server = WsDaemon::start("slow");
    let work = server.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let cwd = work.to_string_lossy().to_string();
    let mut seed = server.connect();
    let _hello = seed.read_value();
    let mut ids = Vec::new();
    for i in 0..2 {
        let a = seed.rpc_ack("submit_task", &json!({"cmd":"submit_task","task":format!("seed {i}"),"cwd":cwd}));
        assert_eq!(a["ok"], true);
        ids.push(a["sessionId"].as_str().unwrap().to_string());
    }
    // Let the seed snapshots drain before the burst.
    let _ = seed.wait_for(Duration::from_millis(600), |v| v["event"] == "snapshot");

    // This client reads its hello then goes quiet — the slow reader.
    let mut slow = server.connect();
    let _hello = slow.read_value();
    let mut fast = server.connect();
    let _hello = fast.read_value();

    let mut last_id = String::new();
    for i in 0..12 {
        let a = fast.rpc_ack("message", &json!({
            "cmd":"message","from":ids[0],"to":ids[1],"type":"task","body":format!("burst {i}")
        }));
        assert_eq!(a["ok"], true, "{a}");
        last_id = a["messageId"].as_str().unwrap().to_string();
    }

    // The responsive client still receives its real delta promptly, despite
    // the silent client sharing the same daemon and event pump.
    let frame = fast.wait_for(Duration::from_millis(1000), |v| {
        v["event"] == "bus_message" && v["busMessage"]["id"] == last_id
    });
    assert!(frame.is_some(), "fast client starved by slow reader");

    let t0 = Instant::now();
    let pong = fast.rpc_ack("ping", &json!({"cmd":"ping"}));
    assert_eq!(pong["ok"], true);
    assert!(t0.elapsed() < Duration::from_millis(200), "daemon blocked by slow reader");

    // When the slow client finally reads, it gets a bounded number of frames
    // (never an unbounded backlog) and can still talk to the daemon.
    let mut received = 0usize;
    while slow.read_value_within(Duration::from_millis(60)).is_some() {
        received += 1;
        if received > 256 {
            break;
        }
    }
    assert!(received <= 256, "outbox must be bounded, got {received} frames");
    let pong = slow.rpc_ack("ping", &json!({"cmd":"ping"}));
    assert_eq!(pong["ok"], true, "slow reader connection stayed alive");
}

#[test]
fn ws_disconnect_and_reconnect_gets_a_fresh_hello() {
    let server = WsDaemon::start("reconnect");
    {
        let mut c = server.connect();
        let hello = c.read_value();
        assert_eq!(hello["event"], "hello");
        let pong = c.rpc_ack("ping", &json!({"cmd":"ping"}));
        assert_eq!(pong["ok"], true);
        c.close();
    }
    std::thread::sleep(Duration::from_millis(80));
    let mut c = server.connect();
    let hello = c.read_value_within(Duration::from_secs(2));
    assert_eq!(hello.as_ref().and_then(|v| v["event"].as_str()), Some("hello"));
    let pong = c.rpc_ack("ping", &json!({"cmd":"ping"}));
    assert_eq!(pong["ok"], true, "reconnected client works");
}

#[test]
fn ws_client_count_is_bounded() {
    let server = WsDaemon::start("cap");
    // Deterministic readiness: wait on the server's admitted-client counter,
    // not on a fixed sleep. The harness readiness probe has disconnected by
    // the time start() returns, but the server may not have reaped it yet.
    assert!(
        server.wait_for_active(0, Duration::from_secs(5)),
        "readiness probe slot was never released"
    );
    let mut clients = Vec::new();
    for n in 1..=MAX_CLIENTS {
        let mut c = server.connect();
        let hello = c.read_value_within(Duration::from_secs(5));
        assert_eq!(hello.as_ref().and_then(|v| v["event"].as_str()), Some("hello"));
        clients.push(c);
        assert!(
            server.wait_for_active(n, Duration::from_secs(5)),
            "admitted count did not reach {n}"
        );
    }
    // Every admitted client still works.
    let pong = clients[0].rpc_ack("ping", &json!({"cmd":"ping"}));
    assert_eq!(pong["ok"], true);

    // The 17th client is authenticated so it reaches the capacity check (an
    // unauthenticated one would be rejected at handshake for a different
    // reason). It must get WS Close 1012 (Restart) and never a hello.
    match server.connect_raw_with_auth() {
        Err(e) => panic!("17th client should complete the upgrade, got {e}"),
        Ok(mut ws) => match ws.read() {
            Ok(Message::Close(Some(frame))) => {
                assert_eq!(frame.code, CloseCode::Restart, "capacity close must be 1012");
            }
            other => panic!("17th client must be closed with 1012, got {other:?}"),
        },
    }
    assert_eq!(server.active(), MAX_CLIENTS, "rejected client must not consume a slot");

    // Freeing a slot allows a new client in — again wait on the counter.
    clients[0].close();
    assert!(
        server.wait_for_active(MAX_CLIENTS - 1, Duration::from_secs(5)),
        "disconnected slot was never released"
    );
    let mut c = server.connect();
    assert_eq!(
        c.read_value_within(Duration::from_secs(5))
            .as_ref()
            .and_then(|v| v["event"].as_str()),
        Some("hello")
    );
}
