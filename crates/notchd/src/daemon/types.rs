//! Shared internals: the [`Daemon`] struct, the mutable tree behind its
//! mutex, and the small helpers every submodule uses.
//!
//! Nothing in here is public API — the facade (`daemon::mod`) re-exports
//! only [`Daemon`] and the ack types.

use crate::bus::{BusMessage, OrchestratorBus, SupervisorSink};
use crate::clock::Clock;
use crate::config::Config;
use crate::events::EventLog;
use crate::herdr::Herdr;
use crate::model::{
    AttributionEntry, BusMessageEntry, ConflictEntry, DecisionEntry, EscalationEntry, SessionState,
    SupervisorMeta,
};
use crate::watchdog::Watchdog;

use std::collections::HashSet;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

pub(crate) const EVENTS_MAX_BYTES: u64 = crate::events::ROTATE_BYTES;
pub(crate) const MAX_WARNINGS: usize = 10;
pub(crate) const ATTRIBUTION_KEEP: usize = 512;
pub(crate) const DEBOUNCE_MS: u64 = 100;
pub(crate) const DECISIONS_CAP: usize = 12;
pub(crate) const BUS_MESSAGES_CAP: usize = 30;
pub(crate) const MAX_ESCALATIONS: usize = 64;

/// The long-lived control plane for a set of OpenCode sessions running in
/// herdr panes (bridge.md §6).
///
/// `Daemon` owns the global tree (supervisors, sessions, attribution,
/// conflicts, decisions, escalations, the message-bus log), persists it to
/// `state.json`, and exposes the command API the socket layer and the
/// Tauri shell consume. It makes no model calls in the control path; the
/// supervisor's decider is an optional, off-by-default OpenCode call
/// (see `supervise` in this module tree).
///
/// Concurrency layout: `inner` is the mutable tree behind one mutex.
/// herdr/git IO happens *outside* the mutex (a seed pass clones what's
/// needed, a side-effect pass does the IO, then one apply critical section
/// commits results). Lock order everywhere is `inner` → `watchdog` →
/// subscriber/pending locks — never the reverse. `events.jsonl` is
/// append-only via [`EventLog`]; [`OrchestratorBus`] mediates all
/// cross-supervisor traffic.
pub struct Daemon {
    /// Resolved configuration (state dir, temp dir, watchdog thresholds).
    pub config: Config,
    pub(crate) herdr: Herdr,
    pub(crate) clock: Arc<dyn Clock>,
    /// Append-only `events.jsonl` sink, shared with the CLI and the UI.
    pub event_log: EventLog,
    pub(crate) inner: Mutex<Inner>,
    pub(crate) watchdog: Mutex<Watchdog>,
    pub(crate) bus: OrchestratorBus,
    pub(crate) sinks: Mutex<std::collections::HashMap<String, SupervisorSink>>,
    pub(crate) mailboxes: Mutex<std::collections::HashMap<String, Receiver<BusMessage>>>,
    pub(crate) subscribers: Mutex<Subscribers>,
    pub(crate) pending_change: Mutex<Option<u64>>,
    pub(crate) start_ms: u64,
    pub(crate) opencode: Mutex<Option<crate::opencode::Opencode>>,
}

/// The mutable control-plane tree. Persisted to `state.json` on every
/// meaningful mutation (atomic write + rolling backups).
pub(crate) struct Inner {
    pub(crate) supervisors: Vec<SupervisorMeta>,
    pub(crate) sessions: Vec<SessionState>,
    pub(crate) attribution: Vec<AttributionEntry>,
    pub(crate) conflicts: Vec<ConflictEntry>,
    pub(crate) warnings: Vec<String>,
    pub(crate) paused_supervisors: HashSet<String>,
    pub(crate) decisions: Vec<DecisionEntry>,
    pub(crate) escalations: Vec<EscalationEntry>,
    pub(crate) bus_messages: Vec<BusMessageEntry>,
    pub(crate) esc_seq: u64,
    pub(crate) msg_seq: u64,
    pub(crate) models_cache: Option<(u64, Vec<String>)>,
    pub(crate) attribution_seen: HashSet<(String, String)>,
    pub(crate) session_tracked: std::collections::HashMap<String, usize>,
    pub(crate) snapshot_version: u64,
}

/// Connected socket subscribers waiting for debounced snapshot pushes.
#[derive(Default)]
pub(crate) struct Subscribers {
    pub(crate) senders: Vec<(u64, std::sync::mpsc::Sender<String>)>,
    pub(crate) next_id: u64,
}

/// herdr agent names must match ^[a-z][a-z0-9-_]{0,31}$ — session ids (S1-OC01)
/// are lowercased for every herdr target. Panes are referenced by pane id,
/// never by name.
pub(crate) fn agent_target(id: &str) -> String {
    id.to_lowercase()
}

/// Convert an internal `Result<T, NotchError>` to the wire-edge
/// `Result<T, String>` (socket acks and the Tauri shell consume plain
/// strings). `Display` for every `NotchError` variant renders the bare
/// payload, so the strings are byte-identical to what the code produced
/// before the type existed.
pub(crate) fn wire<T>(r: Result<T, crate::error::NotchError>) -> Result<T, String> {
    r.map_err(String::from)
}

/// Lock a mutex, recovering from poisoning (a panicking writer must not
/// wedge the control plane; the bytes under the lock stay coherent).
pub(crate) fn lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stable tail key for cross-call loop detection.
pub(crate) fn tail_key(tail: &str) -> String {
    let norm = crate::decider::normalize(tail);
    norm.chars()
        .rev()
        .take(240)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

/// Does this pane tail mention a rate limit (429 / quota / …)?
pub(crate) fn contains_rate_limit(tail: &str) -> bool {
    let lower = tail.to_lowercase();
    lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("quota")
        || lower.contains("resource_exhausted")
        || lower.contains("too many requests")
}

/// Append a warning, deduping consecutive duplicates, capping at
/// [`MAX_WARNINGS`] (oldest dropped first).
pub(crate) fn push_warning(inner: &mut Inner, warning: String) {
    if inner.warnings.last() == Some(&warning) {
        return; // dedupe consecutive duplicates
    }
    inner.warnings.push(warning);
    if inner.warnings.len() > MAX_WARNINGS {
        let drop = inner.warnings.len() - MAX_WARNINGS;
        inner.warnings.drain(0..drop);
    }
}
