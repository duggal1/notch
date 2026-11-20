//! W2-2 RED: append-only ledger, epoch fencing, parked refused writes.
//!
//! Stolen from agentbox `main/store/work-items.mts` (single O_APPEND write,
//! short-write throws), `shared/work-items.mts` (epoch fence: an agent line
//! under a superseded epoch is discarded) and `main/store/pending-writes.mts`
//! (a refused write is parked for the next legitimate holder, never dropped).
//!
//! These tests are written BEFORE the implementation and must fail to compile
//! until `notchd::ledger` exists.

use notchd::ledger::{collect_pending, read_lines, read_pending, replay, Ledger, Op, Source};
use notchd::model::StateFile;
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notch-w2-fence-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn upsert_session(id: &str, title: &str) -> Op {
    Op::Upsert {
        entity: "session".into(),
        id: id.into(),
        value: json!({
            "id": id,
            "sup": "SV-1",
            "status": "working",
            "title": title,
            "task": title,
            "cwd": "/tmp"
        }),
    }
}

#[test]
fn stale_epoch_write_is_parked_and_never_applied() {
    let dir = root();
    let mut ledger = Ledger::open(&dir).unwrap();
    let e1 = ledger.claim("worker-A", "S1-OC01", 300_000, 1_000).unwrap();
    let e2 = ledger.claim("worker-B", "S1-OC01", 300_000, 1_001).unwrap();
    assert_eq!(
        (e1, e2),
        (1, 2),
        "claims must mint monotonically increasing epochs"
    );

    let seq_ok = ledger
        .append_guarded(
            "worker-B",
            e2,
            "S1-OC01",
            upsert_session("S1-OC01", "current"),
            1_002,
        )
        .expect("the current holder's write must land");
    let refused = ledger
        .append_guarded(
            "worker-A",
            e1,
            "S1-OC01",
            upsert_session("S1-OC01", "zombie"),
            1_003,
        )
        .expect_err("a stale-epoch write must be refused, not applied");
    assert!(
        refused.seq > seq_ok,
        "the refused line still consumed a seq"
    );
    assert_eq!(refused.reason, "stale_epoch");
    assert_eq!(refused.holder.as_deref(), Some("worker-A"));

    let lines = read_lines(&dir.join("ledger.jsonl")).unwrap();
    let rr = replay(StateFile::empty(), &lines);
    assert_eq!(rr.refused.len(), 1, "replay must re-park the stale line");
    assert_eq!(
        rr.applied, 3,
        "two claims + one fenced write; the zombie is not applied"
    );
    assert_eq!(rr.state.sessions.len(), 1);
    assert_eq!(
        rr.state.sessions[0].title, "current",
        "a zombie must not overwrite the live holder"
    );

    let pending = read_pending(&dir, "S1-OC01").unwrap();
    assert_eq!(pending.len(), 1, "the refused write must be parked on disk");
    assert_eq!(pending[0].reason, "stale_epoch");
    assert_eq!(pending[0].epoch, Some(e1));
}

#[test]
fn concurrent_stale_writers_are_all_parked_not_applied() {
    let dir = root();
    let shared = Arc::new(Mutex::new(Ledger::open(&dir).unwrap()));
    {
        let mut l = shared.lock().unwrap();
        l.claim("old", "S1-OC01", 300_000, 1_000).unwrap();
        l.claim("owner", "S1-OC01", 300_000, 1_001).unwrap();
    }

    const N: usize = 24;
    let barrier = Arc::new(Barrier::new(N + 1));
    let mut handles = Vec::new();
    for i in 0..N {
        let shared = shared.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            let mut l = shared.lock().unwrap();
            // Every racer writes with the SUPERSEDED epoch 1. None may land.
            l.append_guarded(
                &format!("racer-{i}"),
                1,
                "S1-OC01",
                upsert_session("S1-OC01", &format!("zombie-{i}")),
                2_000 + i as u64,
            )
        }));
    }
    barrier.wait();
    let mut parked = 0;
    for h in handles {
        assert!(
            h.join().unwrap().is_err(),
            "every stale racer must be refused"
        );
        parked += 1;
    }
    assert_eq!(parked, N);

    // The current holder (epoch 2) still writes successfully.
    shared
        .lock()
        .unwrap()
        .append_guarded(
            "owner",
            2,
            "S1-OC01",
            upsert_session("S1-OC01", "owner-value"),
            3_000,
        )
        .expect("the legitimate holder must still land");

    let lines = read_lines(&dir.join("ledger.jsonl")).unwrap();
    let rr = replay(StateFile::empty(), &lines);
    assert_eq!(rr.state.sessions.len(), 1);
    assert_eq!(
        rr.state.sessions[0].title, "owner-value",
        "no racer may win the fence"
    );
    assert_eq!(read_pending(&dir, "S1-OC01").unwrap().len(), N);
}

#[test]
fn system_writes_are_never_fenced() {
    let dir = root();
    let mut ledger = Ledger::open(&dir).unwrap();
    ledger.claim("agent", "S1-OC01", 300_000, 1_000).unwrap();

    // A daemon (system) line carries no epoch; it outranks any agent epoch.
    ledger
        .append(
            Source::System,
            None,
            None,
            "S1-OC01",
            upsert_session("S1-OC01", "system"),
            1_001,
        )
        .unwrap();
    let lines = read_lines(&dir.join("ledger.jsonl")).unwrap();
    let rr = replay(StateFile::empty(), &lines);
    assert!(rr.refused.is_empty());
    assert_eq!(rr.state.sessions[0].title, "system");
}

#[test]
fn torn_and_oversize_lines_are_skipped() {
    let dir = root();
    let ledger = Ledger::open(&dir).unwrap();
    let path = ledger.path().to_path_buf();
    let mut f = OpenOptions::new().append(true).open(&path).unwrap();
    // A half-written (torn) line with no trailing newline must never be read.
    f.write_all(br#"{"seq":2,"ts":1,"source":"system","target":"S1-OC01","op":{"type":"upsert""#)
        .unwrap();
    // An oversize line is treated as hostile and skipped.
    let mut huge = format!("{{\"seq\":3,\"pad\":\"{}\"}}\n", "x".repeat(300_000));
    huge.push('\n');
    f.write_all(huge.as_bytes()).unwrap();
    f.write_all(b"{ not json at all }\n").unwrap();
    drop(f);

    // One legitimate line written through the API after the junk.
    let mut ledger = Ledger::open(&dir).unwrap();
    ledger
        .append(
            Source::System,
            None,
            None,
            "S1-OC01",
            upsert_session("S1-OC01", "valid"),
            9_000,
        )
        .unwrap();
    let lines = read_lines(&path).unwrap();
    assert_eq!(
        lines.len(),
        1,
        "only the whole, in-budget, parseable line survives"
    );
    let rr = replay(StateFile::empty(), &lines);
    assert_eq!(rr.state.sessions[0].title, "valid");
    assert!(
        rr.warnings.is_empty(),
        "junk lines are skipped silently: {:?}",
        rr.warnings
    );
}

#[test]
fn parked_writes_survive_restart_and_collect_moves_them() {
    let dir = root();
    {
        let mut ledger = Ledger::open(&dir).unwrap();
        ledger.claim("old", "S1-OC01", 300_000, 1_000).unwrap();
        ledger.claim("new", "S1-OC01", 300_000, 1_001).unwrap();
        ledger
            .append_guarded(
                "old",
                1,
                "S1-OC01",
                upsert_session("S1-OC01", "lost-notes"),
                1_002,
            )
            .expect_err("stale write is parked");
    }
    // Reopen (fresh handle = fresh process); the parked file must still be there.
    let pending = read_pending(&dir, "S1-OC01").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].op, upsert_session("S1-OC01", "lost-notes"));

    let moved = collect_pending(&dir, "S1-OC01", "new-holder").unwrap();
    assert_eq!(moved.len(), 1);
    assert!(
        moved[0].to_string_lossy().contains("collected"),
        "collect moves, never deletes"
    );
    assert!(read_pending(&dir, "S1-OC01").unwrap().is_empty());
    let collected = std::fs::read(&moved[0]).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&collected).unwrap();
    assert_eq!(v["collectedBy"], json!("new-holder"));
}

#[test]
fn bounded_window_keeps_whole_records_and_rejects_partial_prefixes() {
    use notchd::ledger::{LEDGER_FILE, MAX_LEDGER_BYTES};

    let dir = root();
    let path = dir.join(LEDGER_FILE);
    let mut observed = Vec::new();
    for prefix in [b"torn", b"old\n"] {
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(prefix).unwrap();
        for seq in [1, 2] {
            writeln!(file, r#"{{"seq":{seq},"ts":0,"op":{{"type":"release"}}}}"#).unwrap();
        }
        file.set_len(MAX_LEDGER_BYTES + prefix.len() as u64)
            .unwrap();
        drop(file);
        observed.push(
            read_lines(&path)
                .unwrap()
                .into_iter()
                .map(|line| line.seq)
                .collect::<Vec<_>>(),
        );
    }

    assert_eq!(observed, vec![vec![2], vec![1, 2]]);
}

#[test]
fn reopen_repairs_a_torn_suffix_beyond_the_replay_window() {
    let dir = root();
    let mut ledger = Ledger::open(&dir).unwrap();
    ledger.claim("worker", "S1-OC01", 300_000, 1_000).unwrap();
    let path = ledger.path().to_path_buf();
    drop(ledger);
    let original = std::fs::read(&path).unwrap();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(original.len() as u64 + 256 * 1024 * 1024)
        .unwrap();

    let mut reopened = Ledger::open(&dir).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(reopened.current_epoch("S1-OC01"), 1);
    assert_eq!(
        reopened.claim("next", "S1-OC01", 300_000, 1_001).unwrap(),
        2
    );
    assert_eq!(read_lines(&path).unwrap().len(), 2);
}
