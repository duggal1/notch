//! ID registry — the only allocator for notch IDs. Permanent, never reused.
//!
//! Formats (bridge.md §1.2):
//!   Orchestrator:  ORCH
//!   Supervisor:    SV-<n>            (1-based)
//!   Session:       S<sup>-OC<nn>     (nn zero-padded to 2)
//!
//! `registry.json` is loaded on daemon start; allocation is monotonic, and a
//! candidate that collides with a recorded ID is bumped until free.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Serializes every allocation across all `Registry` clones (socket threads,
/// plan runs, submit_task storms). A clone loaded before a peer's allocation
/// cannot hand out the same id: the authoritative on-disk registry is
/// re-read under this lock, advanced, and persisted *before* the id escapes.
static ALLOC_LOCK: Mutex<()> = Mutex::new(());

fn alloc_guard() -> MutexGuard<'static, ()> {
    ALLOC_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The single authoritative registry per `registry.json` path, loaded from
/// disk exactly once. All allocations go through this instance, so no thread
/// can advance a stale clone. Persistence still happens on every allocation.
static CACHES: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Registry>>>>> = OnceLock::new();

fn caches() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<Registry>>>> {
    CACHES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn registry_cache(path: &Path) -> Arc<Mutex<Registry>> {
    let mut map = caches().lock().unwrap_or_else(|e| e.into_inner());
    map.entry(path.to_path_buf())
        .or_insert_with(|| {
            let mut reg = Registry::load_or_error(path).unwrap_or_else(|_| Registry::fresh());
            reg.path = Some(path.to_path_buf());
            Arc::new(Mutex::new(reg))
        })
        .clone()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    #[serde(rename = "supervisorCount", default)]
    pub supervisor_count: u32,
    /// Owning supervisor number (as string) -> sessions allocated so far.
    #[serde(rename = "sessionCounts", default)]
    pub session_counts: HashMap<String, u32>,
    /// Every ID ever allocated. Never shrunk.
    #[serde(default)]
    pub ids: Vec<String>,
    /// Absolute path of `registry.json`. Set by `load`/`load_or_error` so a
    /// cloned registry still persists before handing out an id.
    #[serde(skip)]
    pub(crate) path: Option<PathBuf>,
    /// Parse failures seen while recovering; surfaced into state.warnings.
    #[serde(skip)]
    pub(crate) warnings: Vec<String>,
}

impl Registry {
    /// Missing or corrupt registry -> empty registry (fresh start). Never
    /// panics. `load_or_recover` in `daemon/state.rs` is the strict entry
    /// point that turns unrecoverable corruption into a hard error; this
    /// lenient form is kept for call sites that only need a value.
    pub fn load(path: &Path, _clock: &dyn crate::clock::Clock) -> Registry {
        match Self::load_or_error(path) {
            Ok(reg) => reg,
            Err(e) => {
                let mut reg = Registry::fresh();
                reg.path = Some(path.to_path_buf());
                reg.warnings.push(format!("registry recovery failed: {e}"));
                reg
            }
        }
    }

    /// Public strict form used by crash-consistency harnesses: a hard error
    /// when the live file and all three backups are missing or corrupt.
    /// `.tmp` files are never considered.
    pub fn load_strict(path: &Path) -> io::Result<Registry> {
        Self::load_or_error(path)
    }

    /// Try the live file then the three rolling backups. Select the most
    /// advanced parseable registry (never a `.tmp` file). A hard error means
    /// there is nothing to rebuild from — callers must not fabricate SV-1.
    pub(crate) fn load_or_error(path: &Path) -> io::Result<Registry> {
        let mut best: Option<Registry> = None;
        let mut warnings: Vec<String> = Vec::new();
        for candidate in super::durable::read_candidates(path) {
            // A missing candidate is normal; only parse files that exist.
            if let Ok(bytes) = std::fs::read(&candidate) {
                match serde_json::from_slice::<Registry>(&bytes) {
                    Ok(mut reg) => {
                        reg.path = Some(path.to_path_buf());
                        if is_more_advanced(&reg, best.as_ref()) {
                            best = Some(reg);
                        }
                    }
                    Err(e) => warnings.push(format!(
                        "{} parse failed: {e}; using backup",
                        candidate.display()
                    )),
                }
            }
        }
        match best {
            Some(mut reg) => {
                reg.warnings = warnings;
                Ok(reg)
            }
            None => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "registry unrecoverable: {} and its 3 backups are missing or corrupt",
                    path.display()
                ),
            )),
        }
    }

    fn fresh() -> Registry {
        Registry {
            version: 1,
            supervisor_count: 0,
            session_counts: HashMap::new(),
            ids: Vec::new(),
            path: None,
            warnings: Vec::new(),
        }
    }

    /// Clone handed to a caller that only needs to read / allocate. The path
    /// and warnings are preserved so a later allocation still persists.
    #[allow(dead_code)] // public contract helper for registry snapshots
    pub(crate) fn clone_for_caller(&self) -> Registry {
        self.clone()
    }

    /// Rebuild allocation high-water marks from a recovered state file. This
    /// is the fallback when registry.json and all backups are unreadable:
    /// the derived numbers come from real persisted sessions, never from a
    /// hardcoded SV-1 default.
    #[allow(dead_code)] // contract API: wired by load_or_recover in daemon/state.rs
    pub(crate) fn rebuild_from_state(state: &crate::model::StateFile) -> Registry {
        let mut reg = Registry::fresh();
        reg.supervisor_count = state
            .supervisors
            .iter()
            .map(|s| s.number)
            .max()
            .unwrap_or(0);
        for sup in &state.supervisors {
            reg.ids.push(sup.id.clone());
        }
        let mut counts: HashMap<String, u32> = HashMap::new();
        for session in &state.sessions {
            let number: u32 = session
                .sup
                .strip_prefix("SV-")
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            let seq: u32 = session
                .id
                .strip_prefix(&format!("S{number}-OC"))
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            counts
                .entry(number.to_string())
                .and_modify(|v| *v = (*v).max(seq))
                .or_insert(seq);
            reg.ids.push(session.id.clone());
        }
        reg.session_counts = counts;
        reg.ids.sort();
        reg.ids.dedup();
        reg
    }

    /// Persist an allocation before returning its id. Both the global
    /// allocation lock and the single cached registry serialize callers, so
    /// two threads holding stale clones cannot collide. A failed write is a
    /// hard error: the id is never handed out unpersisted.
    pub(crate) fn try_allocate_session(&mut self, supervisor_number: u32) -> io::Result<String> {
        let _guard = alloc_guard();
        let Some(path) = self.path.clone() else {
            let id = self.allocate_session_inner(supervisor_number);
            return Ok(id);
        };
        let cached = registry_cache(&path);
        let mut authoritative = cached.lock().unwrap_or_else(|e| e.into_inner());
        let id = authoritative.allocate_session_inner(supervisor_number);
        authoritative.save(&path)?;
        *self = authoritative.clone();
        Ok(id)
    }

    pub(crate) fn try_allocate_supervisor(&mut self) -> io::Result<String> {
        let _guard = alloc_guard();
        let Some(path) = self.path.clone() else {
            return Ok(self.allocate_supervisor_inner());
        };
        let cached = registry_cache(&path);
        let mut authoritative = cached.lock().unwrap_or_else(|e| e.into_inner());
        let id = authoritative.allocate_supervisor_inner();
        authoritative.save(&path)?;
        *self = authoritative.clone();
        Ok(id)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::durable::write_atomic(
            path,
            &serde_json::to_vec_pretty(self).unwrap_or_else(|_| b"{}".to_vec()),
        )
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// SV-<n>. Monotonic; collision-proof even against hand-edited registries.
    /// Persists before the id is returned.
    pub fn allocate_supervisor(&mut self) -> String {
        self.try_allocate_supervisor()
            .expect("registry: refusing to hand out an id that could not be persisted")
    }

    /// S<sup>-OC<nn>. nn is 1-based per supervisor, never reused.
    /// Persists before the id is returned.
    pub fn allocate_session(&mut self, supervisor_number: u32) -> String {
        self.try_allocate_session(supervisor_number)
            .expect("registry: refusing to hand out an id that could not be persisted")
    }

    fn allocate_supervisor_inner(&mut self) -> String {
        let mut n = self.supervisor_count + 1;
        loop {
            let id = format!("SV-{n}");
            if !self.ids.iter().any(|x| x == &id) {
                self.supervisor_count = n;
                self.ids.push(id.clone());
                return id;
            }
            n += 1;
        }
    }

    fn allocate_session_inner(&mut self, supervisor_number: u32) -> String {
        let key = supervisor_number.to_string();
        let mut n = self.session_counts.get(&key).copied().unwrap_or(0) + 1;
        loop {
            let id = format!("S{supervisor_number}-OC{n:02}");
            if !self.ids.iter().any(|x| x == &id) {
                self.session_counts.insert(key, n);
                self.ids.push(id.clone());
                return id;
            }
            n += 1;
        }
    }
}

/// Persist a registry snapshot under the same lock allocations use, so a
/// redundant `save_registry` from a caller cannot race an in-flight atomic
/// write on the shared temp file.
/// Allocate through the cached registry without requiring the caller to hold
/// (and therefore re-enter) the registry mutex. Used by the daemon's `alloc_*`
/// APIs; the public `Registry::allocate_*` methods remain for standalone
/// clones such as plan building.
pub fn allocate_session_at(path: &Path, supervisor_number: u32) -> String {
    let _guard = alloc_guard();
    let cached = registry_cache(path);
    let mut reg = cached.lock().unwrap_or_else(|e| e.into_inner());
    let id = reg.allocate_session_inner(supervisor_number);
    if let Some(p) = reg.path.clone() {
        reg.save(&p)
            .expect("registry: refusing to hand out an id that could not be persisted");
    }
    id
}

pub fn allocate_supervisor_at(path: &Path) -> String {
    let _guard = alloc_guard();
    let cached = registry_cache(path);
    let mut reg = cached.lock().unwrap_or_else(|e| e.into_inner());
    let id = reg.allocate_supervisor_inner();
    if let Some(p) = reg.path.clone() {
        reg.save(&p)
            .expect("registry: refusing to hand out an id that could not be persisted");
    }
    id
}

pub fn persist_locked(reg: &Registry, path: &Path) -> io::Result<()> {
    let _guard = alloc_guard();
    let cached = registry_cache(path);
    let cache = cached.lock().unwrap_or_else(|e| e.into_inner());
    // Every allocation already persists synchronously. A caller's redundant
    // save_registry(&reg) writes the identical snapshot; skip it rather than
    // rewriting (and fsyncing) the whole growing JSON a second time.
    if cache.version == reg.version
        && cache.supervisor_count == reg.supervisor_count
        && cache.session_counts == reg.session_counts
        && cache.ids == reg.ids
    {
        return Ok(());
    }
    reg.save(path)
}

/// Pick the candidate that has allocated the most ids. Ties break on
/// supervisor count then total session count, so a hand-edited or truncated
/// live file never wins over a fuller backup.
fn is_more_advanced(candidate: &Registry, current: Option<&Registry>) -> bool {
    match current {
        None => true,
        Some(best) => {
            let rank = |r: &Registry| {
                (
                    r.ids.len(),
                    r.supervisor_count as usize,
                    r.session_counts
                        .values()
                        .map(|n| *n as usize)
                        .sum::<usize>(),
                )
            };
            rank(candidate) > rank(best)
        }
    }
}

#[cfg(test)]
mod a4_tests {
    //! Crash-consistency contracts for the ID allocator. These tests compile
    //! against the pre-fix API and FAIL until recovery / persist-before-use
    //! land (RED first, HARDENING §0.1).
    use super::*;
    use crate::clock::SystemClock;
    use serde_json::Value;
    use std::fs;
    use std::path::PathBuf;

    fn test_dir(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notch-reg-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn backup(path: &Path, n: usize) -> PathBuf {
        PathBuf::from(format!("{}.bak{n}", path.display()))
    }

    #[test]
    fn load_recovers_ids_from_backup_when_live_is_corrupt() {
        let dir = test_dir("recover");
        let path = dir.join("registry.json");
        let mut reg = Registry::fresh();
        reg.allocate_supervisor();
        let a = reg.allocate_session(1);
        let b = reg.allocate_session(1);
        reg.save(&path).unwrap();
        fs::copy(&path, backup(&path, 1)).unwrap();

        // Simulate a torn live file after a crash.
        fs::write(&path, b"{\"version\":1,\"supervisorC").unwrap();
        let mut recovered = Registry::load(&path, &SystemClock);
        assert!(
            recovered.ids().contains(&a) && recovered.ids().contains(&b),
            "recovery must find ids in the rolling backup, not reset to SV-1"
        );
        // The next allocation must not repeat a recovered id.
        let c = recovered.allocate_session(1);
        assert_ne!(c, a);
        assert_ne!(c, b);
    }

    #[test]
    fn load_never_adopts_a_tmp_file() {
        let dir = test_dir("tmp");
        let path = dir.join("registry.json");
        let mut good = Registry::fresh();
        good.allocate_supervisor();
        let real = good.allocate_session(1);
        good.save(&path).unwrap();
        fs::copy(&path, backup(&path, 1)).unwrap();

        let mut canary = Registry::fresh();
        canary.allocate_supervisor();
        let _ = canary.allocate_session(1);
        // Use a distinct id so the assertion is meaningful (S1-OC01 would
        // otherwise be identical in both registries).
        let fake = canary.allocate_session(1);
        assert_ne!(fake, real);
        fs::write(
            crate::durable::tmp_path(&path),
            serde_json::to_vec(&canary).unwrap(),
        )
        .unwrap();
        fs::write(&path, b"not json").unwrap();

        let recovered = Registry::load(&path, &SystemClock);
        assert!(
            recovered.ids().contains(&real),
            "backup ids must be recovered"
        );
        assert!(
            !recovered.ids().contains(&fake),
            "a .tmp file must NEVER be adopted"
        );
    }

    #[test]
    fn allocation_is_persisted_before_the_id_is_returned() {
        let dir = test_dir("persist");
        let path = dir.join("registry.json");
        let mut reg = Registry::fresh();
        reg.allocate_supervisor();
        reg.save(&path).unwrap();
        // Re-load so the registry knows its own path (as open() does).
        let mut reg = Registry::load(&path, &SystemClock);
        let id = reg.allocate_session(1);
        let on_disk: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            on_disk["ids"].as_array().unwrap().iter().any(|v| v == &id),
            "the id must be on disk before allocate returns it"
        );
    }
}
