//! Engine wiring against a real (temporary) notchd state dir.
//!   daemon open (create state dir, load state) → reconcile → poll thread +
//!   flusher → subscriber receives a snapshot push.
//!
//! Exercises the REAL `notchd` library (not a mock). Read-only against herdr
//! (`herdr agent list`); never spawns, splits, closes or focuses panes.
//!
//! If herdr is unavailable/hangs, notchd still pushes snapshots (with a
//! warnings entry), so the assertions hold either way.

use std::sync::Arc;
use std::time::Duration;

use notchd::{Config, Daemon, Herdr, SystemClock};

#[test]
fn reconcile_poll_flusher_and_subscriber_push_snapshot() {
    let dir = std::env::temp_dir().join(format!("notch-test-state-{}", std::process::id()));
    std::env::set_var("NOTCH_STATE_DIR", &dir);
    std::env::remove_var("NOTCH_HERDR_CMD");
    std::env::set_var("NOTCH_POLL_INTERVAL_MS", "250"); // keep the test fast

    let config = Config::from_env();
    let herdr = Herdr::from_env();
    let daemon = Daemon::open(config, herdr, Arc::new(SystemClock)).expect("open daemon");
    daemon.reconcile();
    let daemon = Arc::new(daemon);

    let rx = notch::engine::spawn_daemon_threads(Arc::clone(&daemon));

    let line = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|e| panic!("no snapshot push within 15s: {e:?}"));

    let v: serde_json::Value = serde_json::from_str(&line).expect("snapshot line is JSON");
    assert_eq!(v["event"], "snapshot");
    let snap = &v["snapshot"];
    assert_eq!(snap["version"], 1);
    assert_eq!(snap["orchestrator"]["id"], "ORCH");
    assert!(snap["sessions"].is_array() && snap["supervisors"].is_array());
    assert!(snap["warnings"].is_array());

    // The wire keys are camelCase — the same contract the UI parses.
    for key in [
        "generatedAt",
        "paneId",
        "startedAt",
        "lastAction",
        "lastRevision",
        "latencyMs",
        "failureReason",
    ] {
        if let Some(first) = snap["sessions"].as_array().and_then(|s| s.first()) {
            assert!(first.get(key).is_some(), "missing {key}");
        }
    }
    for key in ["generatedAt", "eps", "conflicts", "attribution", "warnings"] {
        assert!(snap.get(key).is_some(), "missing top-level {key}");
    }

    // Direct build (what the `snapshot` command returns) must also work.
    let direct = serde_json::to_value(daemon.snapshot()).expect("snapshot serializes");
    assert_eq!(direct["version"], 1);
    assert_eq!(direct["orchestrator"]["id"], "ORCH");

    std::env::remove_var("NOTCH_STATE_DIR");
    std::env::remove_var("NOTCH_POLL_INTERVAL_MS");
    let _ = std::fs::remove_dir_all(&dir);
}
