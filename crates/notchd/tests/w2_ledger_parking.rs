//! W2-2: parking semantics — a delivered write never resurrects, and a
//! hostile target/holder id cannot escape the state dir.

use notchd::ledger::{
    collect_pending, park, read_pending, replay_over_checkpoint, Ledger, Op, ParkedWrite,
};
use notchd::model::StateFile;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notch-w2-park-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn upsert() -> Op {
    Op::Upsert {
        entity: "session".into(),
        id: "S1-OC01".into(),
        value: json!({"id":"S1-OC01","sup":"SV-1","status":"working","title":"notes","task":"notes","cwd":"/tmp"}),
    }
}

#[test]
fn collected_write_is_not_resurrected_by_replay() {
    let dir = root();
    let mut ledger = Ledger::open(&dir).unwrap();
    ledger.claim("old", "S1-OC01", 300_000, 1_000).unwrap();
    ledger.claim("new", "S1-OC01", 300_000, 1_001).unwrap();
    ledger
        .append_guarded("old", 1, "S1-OC01", upsert(), 1_002)
        .expect_err("stale write must be parked");
    assert_eq!(read_pending(&dir, "S1-OC01").unwrap().len(), 1);

    let moved = collect_pending(&dir, "S1-OC01", "new-holder").unwrap();
    assert_eq!(moved.len(), 1);
    assert!(read_pending(&dir, "S1-OC01").unwrap().is_empty());

    // A fresh replay sees the refused ledger line again, but an already
    // collected message must not be handed out a second time.
    let rr = replay_over_checkpoint(&dir, StateFile::empty()).unwrap();
    assert_eq!(rr.refused.len(), 1, "the audit count still includes the fenced line");
    assert!(
        read_pending(&dir, "S1-OC01").unwrap().is_empty(),
        "a delivered write must not resurrect in the pending queue"
    );
    assert!(moved[0].exists(), "collected evidence must survive");
}

#[test]
fn hostile_target_cannot_escape_pending_dir() {
    let dir = root();
    let parked = ParkedWrite {
        seq: 1,
        ts: 1_000,
        target: "../escape".into(),
        holder: Some("../../evil".into()),
        epoch: Some(1),
        reason: "stale_epoch".into(),
        op: Op::Release,
        file: None,
    };
    let file = park(&dir, &parked).unwrap();
    assert!(file.starts_with(&dir), "parked file escaped state dir: {}", file.display());
    assert!(!dir.parent().unwrap().join("escape").exists());
}

#[test]
fn symlinked_pending_target_is_refused() {
    use std::os::unix::fs::symlink;

    let dir = root();
    let outside = dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let pending_root = dir.join(notchd::ledger::PENDING_DIR);
    std::fs::create_dir_all(&pending_root).unwrap();
    symlink(&outside, pending_root.join("S1-OC01")).unwrap();

    let parked = ParkedWrite {
        seq: 1,
        ts: 1_000,
        target: "S1-OC01".into(),
        holder: Some("old".into()),
        epoch: Some(1),
        reason: "stale_epoch".into(),
        op: Op::Release,
        file: None,
    };
    assert!(park(&dir, &parked).is_err(), "a planted symlink must not be written through");
    assert!(read_pending(&dir, "S1-OC01").unwrap().is_empty());
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
}
