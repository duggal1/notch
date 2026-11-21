//! T6 (HARDENING §4): 64 × 200 allocations under contention, then a restart.
//!
//! Split into phases that preserve the FULL invariant — 12,800 unique IDs,
//! persisted to disk, then continuation after restart — without paying the
//! O(n²) cost of 12,800 real spawns (each spawn runs A5's budget scan over all
//! existing sessions). Phase A drives the exact registry allocator the daemon
//! uses, at the full 64×200 shape. Phase B is a smaller live `submit_task`
//! integration proving the wiring, and Phase C is the restart.

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::registry::{allocate_session_at, allocate_supervisor_at};
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const T0: u64 = 1_700_000_000_000;
const THREADS: usize = 64;
const PER_THREAD: usize = 200;
/// Real spawns are exercised at a smaller shape; the 12,800-id invariant is
/// covered by Phase A against the same allocator and persistence path.
const SPAWN_THREADS: usize = 8;
const SPAWN_PER_THREAD: usize = 10;

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("notch-a4-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn config(root: &Path) -> Config {
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 600,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

fn fake_herdr() -> Herdr {
    let splits = Arc::new(AtomicUsize::new(0));
    Herdr::with_runner(move |args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("pane", "split") => {
                let n = splits.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(json!({
                    "id": "cli:pane:split",
                    "result": { "pane": { "pane_id": format!("w{n}:p{n}") } }
                })
                .to_string())
            }
            ("agent", "list") => Ok(json!({
                "id": "cli:agent:list",
                "result": { "type": "agent_list", "agents": [] }
            })
            .to_string()),
            ("agent", "start") | ("agent", "prompt") | ("pane", "close") | ("agent", "read") => {
                Ok(json!({ "id": "cli:ok", "result": { "type": "ok" } }).to_string())
            }
            _ => Ok("{}".to_string()),
        }
    })
}

#[test]
fn t6_64x200_allocations_then_restart_no_duplicate_ids() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/nonexistent/notch-a4-hook");

    let root = tmp_root("t6");
    let cfg = config(&root);
    let reg_path = cfg.state_dir.join("registry.json");
    std::fs::create_dir_all(&cfg.state_dir).unwrap();
    let started = std::time::Instant::now();

    // ---- Phase A: the full 64×200 allocator storm ------------------------
    let supervisor = allocate_supervisor_at(&reg_path);
    assert_eq!(supervisor, "SV-1");
    let ids: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let ids = ids.clone();
        let reg_path = reg_path.clone();
        handles.push(std::thread::spawn(move || {
            let mut mine = Vec::with_capacity(PER_THREAD);
            for _ in 0..PER_THREAD {
                mine.push(allocate_session_at(&reg_path, 1));
            }
            ids.lock().unwrap().extend(mine);
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let alloc_ms = started.elapsed().as_millis();
    let all = ids.lock().unwrap().clone();
    assert_eq!(all.len(), THREADS * PER_THREAD);
    let unique: HashSet<&String> = all.iter().collect();
    assert_eq!(
        unique.len(),
        all.len(),
        "duplicate session id under 64×200 concurrent allocation"
    );

    // Every id is on disk before it escaped, and the high-water marks agree.
    let disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&reg_path).unwrap()).unwrap();
    let disk_set: HashSet<String> = disk["ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    for id in &all {
        assert!(disk_set.contains(id), "allocated id missing on disk: {id}");
    }
    assert_eq!(disk["supervisorCount"].as_u64(), Some(1));
    assert_eq!(
        disk["sessionCounts"]["1"].as_u64(),
        Some((THREADS * PER_THREAD) as u64)
    );

    // ---- Phase B: live submit_task wiring (smaller shape) -----------------
    let daemon = Daemon::open(config(&root), fake_herdr(), Arc::new(FakeClock::new(T0))).unwrap();
    // Register SV-1 in the in-memory tree via one submission, then fan out.
    let seed = daemon.submit_task(
        "seed".to_string(),
        Some(root.to_string_lossy().to_string()),
        None,
        None,
        None,
        None,
    );
    assert!(seed.ok, "seed failed: {:?}", seed.error);
    let live_supervisor = seed.supervisor_id.clone().expect("seed supervisor id");
    let live_ids: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let daemon = Arc::new(daemon);
    let mut handles = Vec::new();
    for _ in 0..SPAWN_THREADS {
        let daemon = daemon.clone();
        let live_ids = live_ids.clone();
        let live_supervisor = live_supervisor.clone();
        let cwd = root.to_string_lossy().to_string();
        handles.push(std::thread::spawn(move || {
            for i in 0..SPAWN_PER_THREAD {
                let ack = daemon.submit_task(
                    format!("live {i}"),
                    Some(cwd.clone()),
                    None,
                    None,
                    None,
                    Some(live_supervisor.clone()),
                );
                if let Some(id) = ack.session_id {
                    live_ids.lock().unwrap().push(id);
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let spawn_ms = started.elapsed().as_millis() - alloc_ms;
    let live = live_ids.lock().unwrap().clone();
    assert_eq!(live.len(), SPAWN_THREADS * SPAWN_PER_THREAD);
    let live_unique: HashSet<&String> = live.iter().collect();
    assert_eq!(live_unique.len(), live.len(), "duplicate id in live submit_task");
    for id in &live {
        assert!(!all.contains(id), "live spawn reused a Phase A id: {id}");
    }

    // ---- Phase C: restart against a copy of the state dir -----------------
    let restart_root = tmp_root("t6restart");
    copy_dir_all(&cfg.state_dir, &restart_root.join("state"));
    std::fs::create_dir_all(restart_root.join("tmp")).unwrap();
    let _ = Daemon::open(config(&restart_root), fake_herdr(), Arc::new(FakeClock::new(T0))).unwrap();
    let after = allocate_session_at(&restart_root.join("state/registry.json"), 1);
    assert!(!all.contains(&after), "restart repeated an id: {after}");
    assert!(!live.contains(&after), "restart repeated a live id: {after}");
    let restart_reg: serde_json::Value = serde_json::from_slice(
        &std::fs::read(restart_root.join("state/registry.json")).unwrap(),
    )
    .unwrap();
    assert!(
        restart_reg["sessionCounts"]["1"].as_u64()
            >= Some((THREADS * PER_THREAD + 1) as u64),
        "post-restart registry did not advance"
    );

    println!(
        "T6 measured: {} unique alloc ids in {} ms; {} live spawns in {} ms; restart id {}",
        unique.len(),
        alloc_ms,
        live.len(),
        spawn_ms,
        after
    );
}

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_all(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}
