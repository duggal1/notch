//! Concurrency fuzz/flood cases for the NDJSON socket daemon.
//! Naming: socket_concurrency_<case>.

#[path = "fixtures/socket/harness.rs"]
mod harness;

use harness::*;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn socket_concurrency_200_ping() {
    let d = TestDaemon::start("conc-200");
    let ok_count = Arc::new(AtomicU64::new(0));
    let refusal_total = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let start = Instant::now();
    let mut handles = Vec::with_capacity(200);
    for _ in 0..200 {
        let sock = d.sock.clone();
        let ok_count = Arc::clone(&ok_count);
        let refusal_total = Arc::clone(&refusal_total);
        let errors = Arc::clone(&errors);
        handles.push(std::thread::spawn(move || {
            match std::panic::catch_unwind(|| {
                // The daemon's listener backlog (std default 128) can refuse
                // the excess of an instantaneous >128-connect burst;
                // well-behaved clients behind a flood retry immediately
                // (same reconnect story as the half-write flood, §6).
                let (mut c, refusals) =
                    TestConn::connect_retry(&sock, std::time::Duration::from_secs(5));
                let _hello = c.hello();
                let ack = c.rpc(&json!({"cmd":"ping"}));
                (
                    ack["ok"].as_bool() == Some(true),
                    ack["pong"].as_u64().is_some(),
                    refusals,
                )
            }) {
                Ok((ok, pong, refusals)) => {
                    refusal_total.fetch_add(refusals, Ordering::SeqCst);
                    if ok && pong {
                        ok_count.fetch_add(1, Ordering::SeqCst);
                    } else {
                        errors
                            .lock()
                            .unwrap()
                            .push("ack not ok/pong missing".to_string());
                    }
                }
                Err(_) => errors.lock().unwrap().push("thread panicked".to_string()),
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let elapsed = start.elapsed();
    let ok = ok_count.load(Ordering::SeqCst);
    let refusals = refusal_total.load(Ordering::SeqCst);
    let errs = errors.lock().unwrap();
    eprintln!("socket_concurrency_200_ping: {ok}/200 acks in {elapsed:?}; total connect refusals absorbed by retry: {refusals}; errors: {errs:?}");
    assert!(errs.is_empty(), "errors from concurrent clients: {errs:?}");
    assert_eq!(
        ok, 200,
        "every one of the 200 simultaneous clients must get an ok ping ack"
    );
}

#[test]
fn socket_concurrency_50_snapshot() {
    let d = TestDaemon::start("conc-50snap");
    let _ = d.run_plan();
    let responses = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let start = Instant::now();
    let mut handles = Vec::with_capacity(50);
    for _ in 0..50 {
        let sock = d.sock.clone();
        let responses = Arc::clone(&responses);
        handles.push(std::thread::spawn(move || {
            let mut c = TestConn::connect(&sock);
            let _hello = c.hello();
            let v = c.rpc(&json!({"cmd":"snapshot"}));
            responses.lock().unwrap().push(v);
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let elapsed = start.elapsed();
    let responses = responses.lock().unwrap();
    assert_eq!(responses.len(), 50);
    // The ONLY tolerated differences between simultaneous snapshots are
    // generatedAt (ms resolution) and eps (rolling 10 s events/sec window).
    // Everything else — sessions, supervisors, conflicts, attribution,
    // decisions, escalations, busMessages, warnings — must be byte-equal.
    let strip = |v: &serde_json::Value| -> serde_json::Value {
        let mut snap = v["snapshot"].clone();
        if let Some(obj) = snap.as_object_mut() {
            obj.remove("generatedAt");
            obj.remove("eps");
        }
        snap
    };
    let reference = strip(&responses[0]);
    for (i, r) in responses.iter().enumerate() {
        assert_eq!(
            strip(r),
            reference,
            "snapshot #{i} differs beyond generatedAt/eps noise"
        );
    }
    let eps: Vec<f64> = responses
        .iter()
        .map(|r| r["snapshot"]["eps"].as_f64().unwrap_or(0.0))
        .collect();
    let max = eps.iter().cloned().fold(f64::MIN, f64::max);
    let min = eps.iter().cloned().fold(f64::MAX, f64::min);
    let spread = max - min;
    eprintln!("socket_concurrency_50_snapshot: 50/50 snapshots in {elapsed:?}; eps min={min:?} max={max:?} spread={spread:?}");
    assert!(spread <= 1.0, "eps spread {spread} exceeds tolerance");
}

/// 20 subscribers watch; one client kills an already-failed session.
/// The killer gets an ok ack; every subscriber receives a snapshot containing
/// the kill (polled, ~200 ms-ish).
#[test]
fn socket_concurrency_kill_with_20_subscribers() {
    let d = TestDaemon::start("conc-kill20");
    let ack = d.run_plan();
    assert_eq!(ack["ok"], true, "run_plan failed: {ack}");

    // Pre-fail S1-OC01 so this is a kill of an ALREADY FAILED session.
    {
        let mut c = TestConn::connect(&d.sock);
        let _hello = c.hello();
        let ack = c.rpc(&json!({"cmd":"kill","id":"S1-OC01","reason":"pre-fail"}));
        assert_eq!(ack["ok"], true);
    }
    // Let the debounced pre-fail push drain before subscribers connect.
    std::thread::sleep(Duration::from_millis(400));

    // Connect 20 subscribers; each gets hello + subscribe ack + initial snapshot.
    let mut subscribers = Vec::with_capacity(20);
    for _ in 0..20 {
        let mut c = TestConn::connect(&d.sock);
        let _hello = c.hello();
        let (_, snap) = c.subscribe();
        assert_eq!(snap["snapshot"]["sessions"].as_array().unwrap().len(), 4);
        subscribers.push(c);
    }

    // One client kills the already-failed session; distinct reason to track it.
    let killer = std::thread::spawn({
        let sock = d.sock.clone();
        move || {
            let mut c = TestConn::connect(&sock);
            let _hello = c.hello();
            c.rpc(&json!({"cmd":"kill","id":"S1-OC01","reason":"kill-storm"}))
        }
    });
    let ack = killer.join().expect("killer thread");
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "kill");
    assert_eq!(
        ack["ok"], true,
        "kill of already-failed session must ack ok"
    );
    assert_eq!(ack["reason"], "kill-storm");

    // Every subscriber must receive a snapshot containing the kill.
    // Poll each subscriber with a 2 s deadline (the debounce is 100 ms and the
    // flusher ticks every 25 ms, so ~150 ms is typical).
    let mut handles = Vec::with_capacity(20);
    for (i, mut c) in subscribers.into_iter().enumerate() {
        handles.push(std::thread::spawn(move || {
            let deadline = deadline_in(2000);
            let hit = c.wait_for_line(deadline, |v| {
                v["event"] == "snapshot"
                    && v["snapshot"]["sessions"]
                        .as_array()
                        .map(|ss| {
                            ss.iter().any(|s| {
                                s["id"] == "S1-OC01"
                                    && s["status"] == "failed"
                                    && s["failureReason"] == "kill-storm"
                            })
                        })
                        .unwrap_or(false)
            });
            (i, hit)
        }));
    }
    let mut max_latency = Duration::ZERO;
    let mut received = 0usize;
    for h in handles {
        match h.join().expect("subscriber thread") {
            (i, Some((_, elapsed))) => {
                eprintln!("socket_concurrency_kill_with_20_subscribers: subscriber {i} saw kill-storm in {elapsed:?}");
                max_latency = max_latency.max(elapsed);
                received += 1;
            }
            (i, None) => panic!(
                "subscriber {i} never received a snapshot containing the kill (2 s deadline)"
            ),
        }
    }
    eprintln!("socket_concurrency_kill_with_20_subscribers: 20/20 subscribers saw the kill; worst latency {max_latency:?}");
    assert_eq!(received, 20);
}
