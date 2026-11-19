//! Durable state persistence and recovery.
//!
//! `state.json` is written atomically (temp + fsync + rename + parent-dir
//! fsync) with three time-spaced backups (`state.json.bak1..3`, 60/900/3600 s)
//! — see `crate::durable`. Recovery tries the live file and all three backups,
//! picks the most advanced parseable snapshot, and treats *unrecoverable*
//! corruption as a hard error (`load_or_recover`) rather than silently
//! resetting the supervisor registry to SV-1. Parse failures are surfaced as
//! warnings in the loaded `StateFile`.
//!
//! Writes are coalesced: `save_state` only marks a dirty snapshot and a single
//! writer thread flushes it at most once per second. `save_now` is the
//! synchronous escape hatch used on spawn / kill / escalate / shutdown.

use super::types::{lock, Daemon, Inner};
use crate::config::Config;
use crate::durable;
use crate::model::{SessionState, StateFile};
use crate::registry::Registry;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Coalesced writer cadence: at most one state-file write per second.
pub(crate) const WRITE_INTERVAL: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Registry: one mutex-protected instance per state dir, loaded once.
// ---------------------------------------------------------------------------

pub(crate) fn registry_path(state_dir: &Path) -> PathBuf {
    state_dir.join("registry.json")
}

/// The daemon's single registry instance for this state dir. Created on first
/// use from disk (live + backups), then shared by every socket thread.
pub(crate) fn registry_for(state_dir: &Path) -> Arc<Mutex<Registry>> {
    crate::registry::registry_cache(&registry_path(state_dir))
}

/// Eager variant for `Daemon::open`: fails loudly when the registry and all
/// backups are corrupt. Contracted to A2 in COORD.md.
#[allow(dead_code)] // contract API: wired by A2 in lifecycle.rs
pub(crate) fn prime_registry(state_dir: &Path) -> io::Result<Arc<Mutex<Registry>>> {
    let path = registry_path(state_dir);
    // Strict check first: unrecoverable corruption is a hard error.
    Registry::load_or_error(&path)?;
    Ok(registry_for(state_dir))
}

// ---------------------------------------------------------------------------
// Coalesced state writer: one thread per state path, at most 1 Hz.
// ---------------------------------------------------------------------------

struct StateWriter {
    /// Latest serialized snapshot waiting to be flushed.
    pending: Mutex<Option<Vec<u8>>>,
    cv: Condvar,
    stop: AtomicBool,
    last_write: Mutex<Instant>,
    /// Serializing the whole tree on every mutation is O(n²) over a storm.
    /// Only one snapshot per second is enqueued; intervening mutations set
    /// `dirty` and are captured by the next eligible enqueue or `save_now`.
    last_enqueue: Mutex<Instant>,
    dirty: AtomicBool,
}

static WRITERS: OnceLock<Mutex<HashMap<PathBuf, Arc<StateWriter>>>> = OnceLock::new();

fn writers() -> &'static Mutex<HashMap<PathBuf, Arc<StateWriter>>> {
    WRITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn writer_for(path: &Path) -> Arc<StateWriter> {
    let mut map = lock(writers());
    if let Some(w) = map.get(path) {
        return w.clone();
    }
    let writer = Arc::new(StateWriter {
        pending: Mutex::new(None),
        cv: Condvar::new(),
        stop: AtomicBool::new(false),
        last_write: Mutex::new(Instant::now() - WRITE_INTERVAL), // allow an immediate first flush
        last_enqueue: Mutex::new(Instant::now() - WRITE_INTERVAL),
        dirty: AtomicBool::new(false),
    });
    let thread_path = path.to_path_buf();
    let thread_writer = writer.clone();
    let name = format!("notch-state-writer:{}", path.file_name().unwrap_or_default().to_string_lossy());
    let built = std::thread::Builder::new().name(name).spawn(move || {
        writer_loop(thread_path, thread_writer);
    });
    // A failure to start the writer must not take the daemon down; save_now
    // remains available as a synchronous path.
    if built.is_err() {
        // The writer is still registered; pending bytes will simply not be
        // flushed by a thread. Leave the slot so save_now can clear it.
    }
    map.insert(path.to_path_buf(), writer.clone());
    writer
}

fn writer_loop(path: PathBuf, writer: Arc<StateWriter>) {
    let mut guard = lock(&writer.pending);
    loop {
        let wait = WRITE_INTERVAL.saturating_sub(lock(&writer.last_write).elapsed());
        let (next, timed_out) = writer
            .cv
            .wait_timeout(guard, wait)
            .unwrap_or_else(|e| e.into_inner());
        guard = next;

        if writer.stop.load(Ordering::SeqCst) {
            // Final drain: never lose the last dirty snapshot on shutdown.
            if let Some(bytes) = guard.take() {
                let _ = durable::write_atomic(&path, &bytes);
                *lock(&writer.last_write) = Instant::now();
            }
            break;
        }

        let due = timed_out.timed_out()
            || lock(&writer.last_write).elapsed() >= WRITE_INTERVAL;
        if due && guard.is_some() && lock(&writer.last_write).elapsed() >= WRITE_INTERVAL {
            if let Some(bytes) = guard.take() {
                let _ = durable::write_atomic(&path, &bytes);
                *lock(&writer.last_write) = Instant::now();
            }
        }
    }
}

fn mark_dirty(path: &Path, bytes: Vec<u8>) {
    let writer = writer_for(path);
    *lock(&writer.pending) = Some(bytes);
    writer.cv.notify_one();
}

#[allow(dead_code)] // reached via Daemon::save_now / shutdown_save once call sites land
fn write_now(path: &Path, bytes: &[u8]) {
    let writer = writer_for(path);
    // Drop any older pending snapshot: it must not overwrite this one later.
    lock(&writer.pending).take();
    let _ = durable::write_atomic(path, bytes);
    *lock(&writer.last_write) = Instant::now();
    writer.dirty.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Load / recover.
// ---------------------------------------------------------------------------

/// Result of a strict load. `warnings` must be merged into `Inner.warnings` so
/// corruption is observable in `snapshot.warnings`, not swallowed.
#[allow(dead_code)] // contract API: wired by A2 in lifecycle.rs
pub(crate) struct Recovered {
    pub state: StateFile,
    pub registry: Registry,
    pub warnings: Vec<String>,
}

fn rank_state(s: &StateFile) -> (usize, usize, u64, u64) {
    (
        s.sessions.len(),
        s.supervisors.len(),
        s.esc_seq,
        s.msg_seq,
    )
}

fn load_state_recovered(path: &Path, warnings: &mut Vec<String>) -> io::Result<StateFile> {
    let mut best: Option<StateFile> = None;
    for candidate in durable::read_candidates(path) {
        match std::fs::read(&candidate) {
            Ok(bytes) => match serde_json::from_slice::<StateFile>(&bytes) {
                Ok(state) => {
                    if best.as_ref().map_or(true, |b| rank_state(&state) > rank_state(b)) {
                        best = Some(state);
                    }
                }
                Err(e) => warnings.push(format!(
                    "{} parse failed: {e}; trying backup",
                    candidate.display()
                )),
            },
            Err(_) => {}
        }
    }
    best.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "state unrecoverable: {} and its 3 backups are missing or corrupt",
                path.display()
            ),
        )
    })
}

/// Strict entry point. HARD ERROR when no parseable state file exists; a
/// corrupt registry alone is rebuilt from the recovered state, never reset to
/// SV-1. Warnings from every parse failure are returned to the caller.
#[allow(dead_code)] // contract API: wired by A2 in lifecycle.rs
pub(crate) fn load_or_recover(config: &Config) -> io::Result<Recovered> {
    let state_path = config.state_dir.join("state.json");
    let reg_path = registry_path(&config.state_dir);
    let mut warnings: Vec<String> = Vec::new();
    let mut state = load_state_recovered(&state_path, &mut warnings)?;
    if state.version == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "state version 0 is not a valid checkpoint",
        ));
    }
    let registry = match Registry::load_or_error(&reg_path) {
        Ok(reg) => reg,
        Err(e) => {
            warnings.push(format!("{e}; rebuilding registry from state"));
            let rebuilt = Registry::rebuild_from_state(&state);
            // Persist the rebuilt registry immediately so a later crash cannot
            // lose the reconstructed high-water marks.
            let _ = rebuilt.save(&reg_path);
            rebuilt
        }
    };
    warnings.extend(registry.warnings.iter().cloned());
    state.warnings.extend(warnings.iter().cloned());
    Ok(Recovered {
        state,
        registry,
        warnings,
    })
}

/// Lenient form used by the current `open()` until A2 lands the strict call.
/// Warnings still travel to `Inner.warnings` through `StateFile.warnings`.
pub(crate) fn load_state(path: &Path) -> StateFile {
    let mut warnings = Vec::new();
    match load_state_recovered(path, &mut warnings) {
        Ok(mut state) => {
            state.warnings.extend(warnings);
            state
        }
        Err(e) => {
            let mut state = StateFile::empty();
            state
                .warnings
                .push(format!("state recovery failed: {e}; starting empty"));
            state
        }
    }
}

/// True when any field the daemon persists/acts on actually changed. A3 uses
/// this in `poll_once` so `changed` (and therefore the dirty flag) is set only
/// on a real status / last_action / revision transition, not on every tick.
#[allow(dead_code)] // contract API: wired by A3 in daemon/poll.rs
pub(crate) fn session_changed(before: &SessionState, after: &SessionState) -> bool {
    before.status != after.status
        || before.last_action != after.last_action
        || before.last_revision != after.last_revision
}

impl Daemon {
    pub(crate) fn state_path(&self) -> PathBuf {
        self.config.state_dir.join("state.json")
    }

    pub(crate) fn registry_path(&self) -> PathBuf {
        registry_path(&self.config.state_dir)
    }

    /// Clone of the daemon's single registry instance. Allocation itself goes
    /// through `alloc_session` / `alloc_supervisor` so it stays serialized.
    #[allow(dead_code)] // kept for callers that still build registries explicitly
    pub(crate) fn registry(&self) -> Registry {
        let registry = registry_for(&self.config.state_dir);
        let clone = lock(&*registry).clone_for_caller();
        clone
    }

    #[allow(dead_code)] // contract API: direct mutex handle for A5's batch allocators
    pub(crate) fn registry_mutex(&self) -> Arc<Mutex<Registry>> {
        registry_for(&self.config.state_dir)
    }

    /// Allocate + persist under the registry mutex, BEFORE the id escapes.
    #[allow(dead_code)] // contract API: wired by A5 in daemon/spawn.rs
    pub(crate) fn alloc_session(&self, supervisor_number: u32) -> String {
        crate::registry::allocate_session_at(&self.registry_path(), supervisor_number)
    }

    #[allow(dead_code)] // contract API: wired by A5 in daemon/spawn.rs
    pub(crate) fn alloc_supervisor(&self) -> String {
        crate::registry::allocate_supervisor_at(&self.registry_path())
    }

    #[allow(dead_code)] // kept for callers that still persist registries explicitly
    pub(crate) fn save_registry(&self, reg: &Registry) {
        let _ = crate::registry::persist_locked(reg, &self.registry_path());
    }

    fn state_file(&self, inner: &Inner) -> StateFile {
        StateFile {
            version: 1,
            supervisors: inner.supervisors.clone(),
            sessions: inner.sessions.clone(),
            attribution: inner.attribution.clone(),
            conflicts: inner.conflicts.clone(),
            warnings: inner.warnings.clone(),
            stuck_after_secs: Some(lock(&self.watchdog).stuck_after_ms() / 1000),
            paused_supervisors: inner.paused_supervisors.iter().cloned().collect(),
            decisions: inner.decisions.clone(),
            escalations: inner.escalations.clone(),
            bus_messages: inner.bus_messages.clone(),
            esc_seq: inner.esc_seq,
            msg_seq: inner.msg_seq,
        }
    }

    fn serialize_state(&self, inner: &Inner) -> Vec<u8> {
        serde_json::to_vec_pretty(&self.state_file(inner)).unwrap_or_else(|_| b"{}".to_vec())
    }

    /// Mark the tree dirty; the single writer thread flushes at most 1 Hz.
    /// Serialization itself is also capped at 1 Hz so a 12,800-call burst does
    /// not rebuild the whole tree 12,800 times.
    pub(crate) fn save_state(&self, inner: &Inner) {
        let writer = writer_for(&self.state_path());
        writer.dirty.store(true, Ordering::SeqCst);
        let mut last = lock(&writer.last_enqueue);
        if last.elapsed() < WRITE_INTERVAL {
            return;
        }
        *last = Instant::now();
        drop(last);
        writer.dirty.store(false, Ordering::SeqCst);
        mark_dirty(&self.state_path(), self.serialize_state(inner));
    }

    /// Synchronous flush for spawn / kill / escalate / shutdown.
    #[allow(dead_code)] // contract API: wired by spawn/kill/escalate/shutdown call sites
    pub(crate) fn save_now(&self, inner: &Inner) {
        write_now(&self.state_path(), &self.serialize_state(inner));
    }

    /// Flush the final snapshot and stop this daemon's writer thread.
    #[allow(dead_code)] // contract API: wired by A2 in the shutdown path
    pub(crate) fn shutdown_save(&self) {
        {
            let inner = lock(&self.inner);
            self.save_now(&inner);
        }
        let writer = writer_for(&self.state_path());
        writer.stop.store(true, Ordering::SeqCst);
        writer.cv.notify_all();
    }
}
