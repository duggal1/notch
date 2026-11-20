//! A7 REAL persistence attacks — no fakes, no LLM, no filesystem mocking.
//!
//! Every test here exercises the real `Registry` against a real temp
//! `registry.json` on the real filesystem, with real OS threads. The registry
//! is the single source of ID truth; a race or a lost write must surface as a
//! duplicate ID, a missing persisted entry, or a silent SV-1 reset.

use notchd::registry::{allocate_session_at, allocate_supervisor_at, Registry};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("notch-a7-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// 12,800 session IDs handed out across 8 real threads against the one real
/// registry file. Each allocation is persisted before the id is returned; the
/// authoritative on-disk registry must end with every single id exactly once.
/// A duplicate or a lost entry is a fence failure, not a rounding error.
#[test]
fn real_threads_allocate_12800_ids_with_no_duplicates_or_loss() {
    const TOTAL: usize = 12_800;
    const THREADS: usize = 8;

    let dir = tmp_root("idstorm");
    let path = dir.join("registry.json");

    // The real SV-1 allocation path, persisted to the real file.
    let sv1 = allocate_supervisor_at(&path);
    assert_eq!(sv1, "SV-1", "first supervisor must be SV-1");

    let collected: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let per_thread = TOTAL / THREADS;
    assert_eq!(per_thread * THREADS, TOTAL);

    let mut handles = Vec::with_capacity(THREADS);
    for _ in 0..THREADS {
        let p = path.clone();
        let set = Arc::clone(&collected);
        handles.push(std::thread::spawn(move || {
            let mut local = Vec::with_capacity(per_thread);
            for _ in 0..per_thread {
                local.push(allocate_session_at(&p, 1));
            }
            let mut guard = set.lock().unwrap();
            for id in local {
                assert!(guard.insert(id.clone()), "duplicate id handed out: {id}");
            }
        }));
    }
    for h in handles {
        h.join().expect("allocator thread panicked");
    }

    let ids = collected.lock().unwrap();
    assert_eq!(ids.len(), TOTAL, "expected {TOTAL} unique ids, got {}", ids.len());

    // Reload from disk: no allocation may have been lost to a stale clone or a
    // torn/lost write, and the registry must not have silently reset to SV-1.
    let persisted = Registry::load_strict(&path).expect("registry must be recoverable after storm");
    let persisted_ids: HashSet<&str> = persisted.ids().iter().map(String::as_str).collect();
    assert!(
        persisted_ids.contains("SV-1"),
        "registry silently reset to SV-1: {:?}",
        &persisted.ids()[..persisted.ids().len().min(5)]
    );
    assert_eq!(
        persisted.ids().len(),
        TOTAL + 1,
        "registry lost persisted entries: {} of {}",
        persisted.ids().len(),
        TOTAL + 1
    );
    for id in ids.iter() {
        assert!(persisted_ids.contains(id.as_str()), "id not persisted: {id}");
    }
    println!(
        "A7 ID storm: {TOTAL} ids across {THREADS} real threads, {} persisted, 0 duplicates",
        persisted.ids().len()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The ledger is a bounded 8 MiB read window over a real file. A line pushed
/// out of the window must not re-enter state on replay, and a final torn line
/// must never be applied. Both are real on-disk conditions, not mocks.
#[test]
fn ledger_8mib_window_drops_old_lines_and_torn_tail_is_skipped() {
    use notchd::ledger::{read_lines, MAX_LEDGER_BYTES};

    let dir = tmp_root("ledgerwin");
    let path = dir.join("ledger.jsonl");
    let line = |seq: u64, target: &str| -> String {
        format!(
            "{{\"seq\":{seq},\"ts\":1,\"source\":\"system\",\"target\":\"{target}\",\"op\":{{\"type\":\"release\"}}}}\n"
        )
    };

    let mut buf = String::new();
    buf.push_str(&line(1, "OLD")); // first line: will fall outside the window
    let mut seq = 2u64;
    while buf.len() < MAX_LEDGER_BYTES as usize + 64 * 1024 {
        buf.push_str(&line(seq, "filler"));
        seq += 1;
    }
    buf.push_str(&line(seq, "NEW"));
    buf.push_str("{\"seq\":999999,\"ts\":1,\"source\":\"system\",\"target\":\"TORN\""); // no newline
    std::fs::write(&path, &buf).unwrap();

    let lines = read_lines(&path).expect("read real ledger file");
    let targets: Vec<&str> = lines.iter().map(|l| l.target.as_str()).collect();
    assert!(!targets.contains(&"OLD"), "line older than the 8 MiB window re-entered replay");
    assert!(targets.contains(&"NEW"), "newest line must be inside the window");
    assert!(!targets.contains(&"TORN"), "torn last line must be skipped");
    assert!(
        lines.iter().all(|l| l.target == "filler" || l.target == "NEW"),
        "unexpected line survived the window"
    );
    println!(
        "A7 ledger window: {} bytes on disk, {} lines in window, OLD dropped, TORN skipped",
        buf.len(),
        lines.len()
    );

    let _ = std::fs::remove_dir_all(&dir);
}
