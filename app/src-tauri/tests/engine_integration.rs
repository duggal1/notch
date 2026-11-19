//! Engine against a real (temporary) state dir. Exercises:
//!   create state dir → start engine (re-adopt, poll thread) → receive the
//!   initial snapshot over the channel → empty tree → registry instantiated.
//!
//! Read-only against herdr (`herdr agent list`); never spawns/closes panes.
//!
//! NOTE: this test depends on herdr being callable. If herdr is missing or
//! hangs, the engine still builds a snapshot (with `warnings`), so the
//! assertions hold either way.

use std::time::Duration;

#[test]
fn engine_starts_and_speaks_first_snapshot() {
    let dir = std::env::temp_dir().join(format!("notch-test-state-{}", std::process::id()));
    std::env::set_var("NOTCH_STATE_DIR", &dir);
    std::env::remove_var("NOTCH_HERDR"); // default binary; read-only usage

    let (engine, rx) = notch::engine::Engine::start().expect("engine start");
    let snap = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|e| panic!("no snapshot within 15s: {e:?}"));

    assert_eq!(snap.version, 1);
    assert!(snap.sessions.is_empty(), "fresh state dir must have no sessions: {:?}", snap.sessions);
    assert!(snap.supervisors.is_empty());
    assert_eq!(snap.orchestrator.id, "ORCH");

    // State dir + registry.json were created (the registry is the only allocator).
    assert!(dir.join("registry.json").exists(), "registry.json must exist after engine start");
    assert!(dir.join("events.jsonl").exists(), "events.jsonl must exist after engine start");

    // A second snapshot built directly must not panic and must stay consistent.
    let snap2 = engine.build_snapshot();
    assert_eq!(snap2.version, 1);
    assert!(snap2.sessions.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}
