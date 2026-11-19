//! A2's adversarial review of A3's watchdog (cross-review ring A1→A2→A3).
//!
//! Hypothesis: the stall guard resets every session's `revision_changed_at_ms`
//! (new grace window) but does NOT re-arm the watchdog's per-episode action
//! sets. A session that was already escalated Stuck before a long stall will
//! therefore go silent on the NEXT genuine stuck episode — the escalation is
//! swallowed by `take_stuck_action` returning false forever.
//!
//! kill_on_stuck is OFF so the session remains non-terminal and can exhibit a
//! second stuck episode; this isolates the escalation bug from pane closing.
//!
//! Run: cargo test -p notchd --test a2_review_a3

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a2rev-a3-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn config(root: &Path) -> Config {
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 2,      // stuck threshold 4 s
        poll_interval_ms: 1000,   // stall gap > 3 s
        opencode_decider: false,
    }
}

/// One stable `working` pane, revision never changes, no agent_session value
/// (so no opencode busy second opinion can rescue it).
fn frozen_working_herdr() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("pane", "split") => Ok(json!({"result":{"pane":{"pane_id":"wA:p1"}}}).to_string()),
            ("agent", "list") => Ok(json!({"result":{"agents":[{
                "agent":"opencode", "agent_status":"working",
                "pane_id":"wA:p1", "revision":7,
                "terminal_title":"OC | frozen", "focused":false, "name":"frozen"
            }]}}).to_string()),
            ("agent", "read") => Ok("no progress\n".to_string()),
            _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
        }
    })
}

fn stuck_escalations(daemon: &Daemon) -> usize {
    let snap: Value = serde_json::to_value(daemon.snapshot()).unwrap();
    snap["escalations"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["kind"] == json!("stuck"))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_long_stall_must_rearm_the_stuck_escalation() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root();
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = Daemon::open(config(&r), frozen_working_herdr(), clock.clone()).unwrap();
    // No opencode busy signal; no decider judgment.
    daemon.set_opencode(Opencode::with_runner(|_| Err(HerdrError::Io("off".into()))));
    daemon.set_kill_on_stuck(false); // keep the session alive for episode #2
    let ack = daemon.submit_task(
        "frozen session".to_string(),
        Some(r.to_string_lossy().into_owned()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok, "submit failed: {:?}", ack.error);

    // Baseline: wait for the async spawn outcome to attach the pane (bounded
    // wall-clock wait for the worker; the fake clock does NOT advance here).
    let wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        daemon.poll_once();
        let snap: Value = serde_json::to_value(daemon.snapshot()).unwrap();
        if snap["sessions"].as_array().unwrap().iter().any(|s| s["paneId"] == json!("wA:p1")) {
            break;
        }
        assert!(std::time::Instant::now() < wait_deadline, "async spawn never attached pane");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(stuck_escalations(&daemon), 0, "no stuck verdict at T0");

    // Episode 1: 6 s of silence in 1 s ticks reaches the Stuck threshold
    // (2×2 s) without tripping the stall guard (> 3 s gap).
    for _ in 0..6 {
        clock.advance_ms(1_000);
        daemon.poll_once();
    }
    let first = stuck_escalations(&daemon);
    assert!(first >= 1, "episode 1 should escalate stuck; snapshot={}", serde_json::to_value(daemon.snapshot()).unwrap());

    // The daemon stalls for 10 s (> 3× poll interval): grace, no kills.
    clock.advance_ms(10_000);
    daemon.poll_once();

    // Episode 2: another 6 s of genuine silence in 1 s ticks. This is a NEW
    // episode; the watchdog must emit a second `stuck` escalation.
    for _i in 0..6 {
        clock.advance_ms(1_000);
        daemon.poll_once();
        println!("post-stall tick {_i}: stuck_escalations={}", stuck_escalations(&daemon));
    }
    let second = stuck_escalations(&daemon);
    assert!(
        second > first,
        "stall re-armed the grace window but the watchdog action set was not re-armed: \
         stuck escalations stayed at {first} (expected > {first}); snapshot={}",
        serde_json::to_value(daemon.snapshot()).unwrap()
    );
}
