//! A4 REAL regression test for C3 / B4 / lie #9.
//!
//! The fleet surface is derived ONCE, from a §1.3 snapshot plus the REAL ledger
//! seq/epoch. `topStatus` must follow `snapshot::rollup` precedence
//! (working > failed), and the ledger numbers must be the ones passed in from
//! the ledger — never array lengths or a hash of `generatedAt`.

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::snapshot::fleet_view;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const T0: u64 = 1_700_000_000_000;
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notch-a4-fleet-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fake_herdr() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        Ok(json!({"result": {"type": "ok", "args": args}}).to_string())
    })
}

fn real_daemon(dir: &PathBuf) -> Daemon {
    Daemon::open(
        Config {
            state_dir: dir.join("state"),
            tmp_dir: dir.join("tmp"),
            stuck_after_secs: 600,
            poll_interval_ms: 2000,
            opencode_decider: false,
        },
        fake_herdr(),
        Arc::new(FakeClock::new(T0)),
    )
    .unwrap()
}

fn snap_with(statuses: &[&str]) -> serde_json::Value {
    let sessions: Vec<_> = statuses
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "id": format!("S{i}-OC01"),
                "sup": "SV-1",
                "status": s,
                "title": format!("session {i}"),
                "cwd": "/tmp/wt",
                "paneId": "w1:p1",
                "lastAction": null,
                "lastRevision": 0,
                "failureReason": null,
            })
        })
        .collect();
    json!({
        "generatedAt": 1763520000000u64,
        "version": 1,
        "orchestrator": { "id": "ORCH", "status": statuses.first().copied().unwrap_or("idle") },
        "supervisors": [{ "id": "SV-1", "status": "working", "title": "SV-1", "paused": false }],
        "sessions": sessions,
        "decisions": [{ "action": "reprompt" }],
        "busMessages": [{ "from": "a", "to": "b" }],
        "escalations": [],
    })
}

#[test]
fn top_status_uses_rollup_precedence_working_beats_failed() {
    let snap = snap_with(&["failed", "working"]);
    let out = fleet_view(&snap, 42, 7);
    assert_eq!(out["topStatus"], "working", "rollup: working > failed");
    // Every unit carries the same global truth.
    for u in out["units"].as_array().unwrap() {
        assert_eq!(u["topStatus"], "working");
    }
}

#[test]
fn top_status_full_precedence_matrix() {
    assert_eq!(fleet_view(&snap_with(&[]), 0, 0)["topStatus"], "idle");
    assert_eq!(fleet_view(&snap_with(&["failed"]), 0, 0)["topStatus"], "failed");
    assert_eq!(fleet_view(&snap_with(&["done", "done"]), 0, 0)["topStatus"], "done");
    assert_eq!(fleet_view(&snap_with(&["idle", "done"]), 0, 0)["topStatus"], "idle");
    assert_eq!(fleet_view(&snap_with(&["working", "done"]), 0, 0)["topStatus"], "working");
}

#[test]
fn ledger_seq_epoch_fence_are_real_ledger_values() {
    let snap = snap_with(&["working"]);
    let out = fleet_view(&snap, 42, 7);
    // 2 array-derived counts exist in the snapshot (1 decision + 1 message = 2),
    // but the reported seq must be the real ledger seq (42).
    assert_eq!(out["ledger"]["seq"], 42);
    assert_eq!(out["ledger"]["epoch"], 7);
    assert_eq!(
        out["ledger"]["fence"],
        format!("{:016x}{:016x}", 7u64, 42u64),
        "fence is a pure function of the real epoch+seq"
    );
}

#[test]
fn fence_moves_with_real_ledger_not_with_generated_at() {
    let base = snap_with(&["working"]);
    let a = fleet_view(&base, 41, 7);
    let b = fleet_view(&base, 42, 7);
    assert_ne!(a["ledger"]["fence"], b["ledger"]["fence"], "seq moved -> fence moved");
    assert_ne!(a["ledger"]["seq"], b["ledger"]["seq"]);

    let mut newer = base.clone();
    newer["generatedAt"] = json!(1763520000001u64);
    let c = fleet_view(&newer, 41, 7);
    assert_eq!(a["ledger"]["fence"], c["ledger"]["fence"], "generatedAt must not change fence");
    assert_eq!(a["topStatus"], c["topStatus"]);
}

#[test]
fn real_daemon_mixed_fleet_top_status_is_one_truth() {
    let dir = root();
    let d = real_daemon(&dir);
    let cwd = dir.to_string_lossy().to_string();
    let a = d.submit_task("a".into(), Some(cwd.clone()), None, None, None, None);
    let b = d.submit_task("b".into(), Some(cwd), None, None, None, None);
    let failed = b.session_id.unwrap();
    let _working = a.session_id.unwrap();
    d.kill(&failed, Some("test".into())).unwrap();

    let snap = d.snapshot();
    let value = serde_json::to_value(&snap).unwrap();
    let fleet = fleet_view(&value, 0, 0);

    // rollup(working > failed): one working, one failed -> working.
    assert_eq!(fleet["topStatus"], "working");
    // The Tauri event and the WS frame both render fleet_view; the snapshot's
    // orchestrator status is computed by the SAME snapshot::rollup.
    assert_eq!(fleet["topStatus"], value["orchestrator"]["status"]);
    // Real ledger values are reachable from the daemon's state dir.
    let ledger = notchd::ledger::Ledger::open(&d.config.state_dir).unwrap();
    let fleet_real = fleet_view(&value, ledger.last_seq(), ledger.current_epoch("ORCH"));
    assert_eq!(fleet_real["ledger"]["seq"], ledger.last_seq());
}

#[test]
fn fleet_units_include_orchestrator_supervisors_and_sessions() {
    let out = fleet_view(&snap_with(&["working", "failed"]), 1, 1);
    let units = out["units"].as_array().unwrap();
    assert_eq!(units.len(), 4, "ORCH + 1 supervisor + 2 sessions");
    assert_eq!(units[0]["id"], "ORCH");
    assert_eq!(units[0]["kind"], "orchestrator");
    for u in units {
        assert!(u["topStatus"].as_str().is_some());
        assert!(u["kind"].as_str().is_some());
        assert!(u["status"].as_str().is_some());
    }
}
