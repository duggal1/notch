//! A3's adversarial review test for A4 (identity/state).
//!
//! Target: defect #4, duplicate IDs under concurrent socket threads. Two
//! `Registry` handles are loaded from the SAME path BEFORE either allocates,
//! then 64 threads allocate from them concurrently. A stale in-memory clone
//! that hands out an id without re-reading and persisting authoritatively
//! will produce duplicates here.

use notchd::clock::SystemClock;
use notchd::registry::Registry;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

fn review_dir() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("notch-a3-review-a4-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a4_adversarial_stale_clone_concurrency_never_reissues_ids() {
    let path = review_dir().join("registry.json");
    let mut seed = Registry::load(&path, &SystemClock);
    seed.allocate_supervisor().unwrap();
    seed.save(&path).unwrap();

    // Both handles are stale: neither knows about the other's allocations.
    let a = Arc::new(Mutex::new(Registry::load(&path, &SystemClock)));
    let b = Arc::new(Mutex::new(Registry::load(&path, &SystemClock)));

    let handles: Vec<_> = (0..64)
        .map(|i| {
            let holder = if i % 2 == 0 { a.clone() } else { b.clone() };
            thread::spawn(move || {
                let mut reg = holder.lock().unwrap();
                reg.allocate_session(1).unwrap()
            })
        })
        .collect();
    let ids: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().expect("allocator thread panicked"))
        .collect();

    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        64,
        "64 concurrent allocations produced duplicate IDs: {ids:?}"
    );
    assert_eq!(ids.iter().filter(|id| *id == "S1-OC01").count(), 1);

    // A fresh process must see every allocation and must not reissue any id.
    let mut after = Registry::load(&path, &SystemClock);
    for id in &ids {
        assert!(after.ids().contains(id), "id {id} was not persisted");
    }
    let next = after.allocate_session(1).unwrap();
    assert!(
        !ids.contains(&next),
        "next allocation reissued a used id: {next}"
    );
}
