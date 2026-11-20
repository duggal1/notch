//! Supervisor failure policy. Deterministic — no model calls.
//!
//! On a session failure (status `failed` after a herdr-reported reason):
//! 1. `blocked_waiting` means the agent is waiting on a human (a permission
//!    prompt, a confirmation, a missing credential). It is NOT a failure to
//!    retry: the pane and temp dir are kept, `needs_you` is escalated, and
//!    nothing is ever typed into the permission prompt.
//! 2. Other failures: first failure re-prompts exactly once, appending the
//!    tail of pane output; a second failure marks the session failed and
//!    escalates.
//!
//! `--opencode-decider` is accepted by the CLI but NOT implemented; when
//! passed, the deterministic policy is used and a warning is logged.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

type SnapshotFn = fn(&Path, &str, &str) -> Result<Option<String>, String>;

static SNAPSHOT_HOOK: OnceLock<SnapshotFn> = OnceLock::new();

/// A5 registers `worktree::snapshot` here at daemon startup. Until then the
/// git fallback below still snapshots WIP before a stuck pane is closed.
pub fn register_snapshot_hook(f: SnapshotFn) -> Result<(), SnapshotFn> {
    SNAPSHOT_HOOK.set(f)
}

/// Commit WIP before any kill. `None` means the worktree was already clean;
/// `Err` vetoes the pane close so uncommitted work is never lost.
pub fn snapshot_worktree(wt: &Path, id: &str, reason: &str) -> Result<Option<String>, String> {
    if let Some(hook) = SNAPSHOT_HOOK.get() {
        return hook(wt, id, reason);
    }
    let status = crate::worktree::git_run(&["status", "--porcelain"], Some(wt))?;
    if !status.ok {
        return Err(format!(
            "git status failed: {}",
            crate::model::truncate(status.stderr.trim(), 160)
        ));
    }
    if status.stdout.is_empty() {
        return Ok(None);
    }
    let add = crate::worktree::git_run(&["add", "-A"], Some(wt))?;
    if !add.ok {
        return Err(format!(
            "git add failed: {}",
            crate::model::truncate(add.stderr.trim(), 160)
        ));
    }
    let message = format!("notch: WIP snapshot ({reason}) — {id}");
    let commit = crate::worktree::git_run(&["commit", "-m", &message], Some(wt))?;
    if !commit.ok {
        return Err(format!(
            "git commit failed: {}",
            crate::model::truncate(commit.stderr.trim(), 160)
        ));
    }
    let head = crate::worktree::git_run(&["rev-parse", "HEAD"], Some(wt))?;
    Ok(Some(head.stdout.trim().to_string()))
}

#[derive(Debug)]
pub enum FailureAction {
    /// Re-prompt once. `tail` is the pane output to append.
    Reprompt { tail: String },
    /// Budget exhausted. Mark failed + escalate.
    Escalate,
    /// Waiting on a human. Keep the pane/temp, never prompt the permission
    /// dialog, escalate as `needs_you`.
    NeedsHuman { reason: String },
}

/// Decide what a supervisor does with a newly-failed session.
///
/// `failure_count` is the number of failures already observed for this
/// session (0 on first failure).
pub fn decide_on_failure(failure_count: u32, tail: Option<String>) -> FailureAction {
    decide_on_failure_with_reason(None, failure_count, tail)
}

/// Reason-aware variant used by the poll path. `reason` is the herdr mapping
/// reason (e.g. `blocked_waiting`); only that exact value means "human gate".
pub fn decide_on_failure_with_reason(
    reason: Option<&str>,
    failure_count: u32,
    tail: Option<String>,
) -> FailureAction {
    if reason == Some("blocked_waiting") {
        return FailureAction::NeedsHuman {
            reason: reason.unwrap_or_default().to_string(),
        };
    }
    if failure_count == 0 {
        FailureAction::Reprompt {
            tail: tail.unwrap_or_else(|| "<pane output unavailable>".to_string()),
        }
    } else {
        FailureAction::Escalate
    }
}

/// Build the re-prompt text: original task head + tail of the pane output.
pub fn reprompt_text(task_head: &str, tail: &str) -> String {
    format!(
        "{task_head}\n\n---\nPrevious attempt failed. Tail of pane output:\n{tail}\nDiagnose and continue from here."
    )
}

// ---------------------------------------------------------------------------
// Fleet-level start guard (steal #5 from agentbox supervisor.mts:
// `_noteHowTheRunStarted` / `_heldFromStarting` :7349-7510, `STORM_*` :1094-1099,
// `_preemptFor` / `_takeSlotFor` / `_settlePreemptions` :2801-2930,
// `HELD_RETRY_MS` :1099, CLAUDE.md:83-104).
//
// The per-session watchdog and the bounded judge pool are deliberately NOT
// touched: this guard is an additive admission layer for *starts*. It never
// closes a pane, never changes a liveness classification and never bypasses
// the stall grace / 2x Stuck ladder / verdict-before-close invariants.
// ---------------------------------------------------------------------------

/// Two starts closer than this are evidence that the previous run died before
/// the agent began (agentbox's "fast exit").
pub const FAST_START_MS: u64 = 45_000;
/// Six dead starts inside ten minutes is a fleet storm (agentbox constants).
pub const STORM_STARTS: usize = 6;
pub const STORM_WINDOW_MS: u64 = 10 * 60_000;
/// A burst of starts this close together briefly holds new starts.
pub const BURST_STARTS: usize = 3;
pub const BURST_WINDOW_MS: u64 = 2_000;
/// How long a burst hold lasts. Time-based, so it can never become permanent.
pub const START_HOLD_MS: u64 = 2_000;
/// Bounded self-retries of a held/storming worker, then it waits for a person.
pub const HELD_RETRY_MS: [u64; 3] = [30 * 60_000, 2 * 3_600_000, 8 * 3_600_000];
/// A preemption settles after this long even if the target never runs again.
pub const PREEMPTION_HOLD_MS: u64 = 30 * 60_000;

/// The decision the fleet guard made about one start attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartGate {
    /// Proceed with the prompt / re-prompt as before.
    Admit,
    /// Defer this start for a brief, bounded window.
    Hold {
        reason: String,
        until_ms: u64,
        /// Only the first hold of an episode should be logged (no spam).
        first: bool,
    },
    /// A dead-start storm was detected: the worker must be preempted.
    Storm { reason: String },
    /// This worker is already preempted and held out of the start loop.
    Preempt {
        reason: String,
        /// Only the first preemption notice of an episode should be logged.
        first: bool,
    },
}

impl StartGate {
    /// The reason string to put in the decision log, if any.
    pub fn reason(&self) -> Option<&str> {
        match self {
            StartGate::Admit => None,
            StartGate::Hold { reason, .. }
            | StartGate::Storm { reason }
            | StartGate::Preempt { reason, .. } => Some(reason),
        }
    }
}

#[derive(Default)]
struct SessionStartState {
    /// Monotonic timestamps of recent starts (dead-start ring).
    starts: VecDeque<u64>,
    last_start_ms: u64,
    /// Once a storm is seen the session only gets the bounded retry ladder.
    stormed: bool,
    retries: u8,
    next_retry_ms: u64,
    needs_human: bool,
    preempted_at_ms: u64,
    preempt_reason: Option<String>,
    preempt_logged: bool,
}

/// Runtime-only fleet state. Driven exclusively by monotonic milliseconds that
/// the caller supplies — it has no wall-clock access and cannot be skewed by
/// an NTP step, a manual clock change or a VM sleep.
#[derive(Default)]
pub struct FleetGuard {
    recent_starts: VecDeque<(String, u64)>,
    fleet_hold_until_ms: u64,
    fleet_hold_reason: Option<String>,
    fleet_hold_logged: bool,
    held: HashSet<String>,
    sessions: HashMap<String, SessionStartState>,
    held_total: u64,
    storm_total: u64,
    preempt_total: u64,
}

impl FleetGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a start attempt for `id` at monotonic `mono_ms` and decide
    /// whether it may proceed, must be held briefly, or is a storm/preemption.
    pub fn note_start(&mut self, id: &str, mono_ms: u64) -> StartGate {
        if id.is_empty() {
            return StartGate::Admit;
        }
        let st = self.sessions.entry(id.to_string()).or_default();

        // A stormed worker is held out of the start loop. It gets at most three
        // self-retries (30 min / 2 h / 8 h) and then waits for a human — there
        // is no path that re-prompts it forever.
        if st.stormed {
            if st.needs_human {
                let first = !st.preempt_logged;
                st.preempt_logged = true;
                return StartGate::Preempt {
                    reason: format!(
                        "fleet storm preempt: {id} exhausted 3 bounded self-retries; needs human"
                    ),
                    first,
                };
            }
            if mono_ms < st.next_retry_ms {
                let first = !st.preempt_logged;
                st.preempt_logged = true;
                let wait = st.next_retry_ms.saturating_sub(mono_ms);
                return StartGate::Preempt {
                    reason: format!(
                        "fleet storm preempt: {id} held {wait}ms more (self-retry {}/3)",
                        st.retries
                    ),
                    first,
                };
            }
            // Exactly one bounded self-retry is now due.
            st.retries = st.retries.saturating_add(1);
            st.preempt_logged = false;
            if st.retries as usize >= HELD_RETRY_MS.len() {
                st.needs_human = true;
                st.next_retry_ms = u64::MAX;
            } else {
                st.next_retry_ms = mono_ms.saturating_add(HELD_RETRY_MS[st.retries as usize]);
            }
            return StartGate::Admit;
        }

        // Ring of dead-start candidates, bounded to the storm window.
        st.starts
            .retain(|t| mono_ms.saturating_sub(*t) < STORM_WINDOW_MS);
        st.starts.push_back(mono_ms);
        st.last_start_ms = mono_ms;

        let all_fast = st
            .starts
            .iter()
            .zip(st.starts.iter().skip(1))
            .all(|(a, b)| b.saturating_sub(*a) < FAST_START_MS);
        if st.starts.len() >= STORM_STARTS && all_fast {
            st.stormed = true;
            st.retries = 0;
            st.next_retry_ms = mono_ms.saturating_add(HELD_RETRY_MS[0]);
            st.preempted_at_ms = mono_ms;
            st.preempt_reason = Some(format!(
                "fleet storm preempt: {id} had {} starts each < {FAST_START_MS}ms apart in {}min; stopped auto re-prompts",
                st.starts.len(),
                STORM_WINDOW_MS / 60_000
            ));
            st.preempt_logged = false;
            self.storm_total = self.storm_total.saturating_add(1);
            self.preempt_total = self.preempt_total.saturating_add(1);
            return StartGate::Storm {
                reason: st.preempt_reason.clone().unwrap_or_default(),
            };
        }

        // Fleet-wide burst: hold new starts briefly so a wave of failures does
        // not stampede the judge pool / herdr panes.
        self.recent_starts
            .retain(|(_, t)| mono_ms.saturating_sub(*t) <= BURST_WINDOW_MS);
        self.recent_starts.push_back((id.to_string(), mono_ms));
        if self.recent_starts.len() >= BURST_STARTS {
            if self.fleet_hold_until_ms <= mono_ms {
                self.fleet_hold_until_ms = mono_ms.saturating_add(START_HOLD_MS);
                self.fleet_hold_reason = Some(format!(
                    "start burst: {} starts within {}ms; holding new starts briefly for {}ms",
                    self.recent_starts.len(),
                    BURST_WINDOW_MS,
                    START_HOLD_MS
                ));
                self.fleet_hold_logged = false;
                self.held_total = self.held_total.saturating_add(1);
            }
            self.held.insert(id.to_string());
            let first = !self.fleet_hold_logged;
            self.fleet_hold_logged = true;
            return StartGate::Hold {
                reason: self.fleet_hold_reason.clone().unwrap_or_default(),
                until_ms: self.fleet_hold_until_ms,
                first,
            };
        }
        StartGate::Admit
    }

    /// A model re-prompt is NOT a new process start: it must not feed the
    /// dead-start storm detector. It still honours an existing per-session
    /// storm preemption (bounded self-retry / needs-human), so the guard's
    /// authority is preserved without misclassifying normal supervisor loops.
    pub fn note_reprompt(&mut self, id: &str, mono_ms: u64) -> StartGate {
        if id.is_empty() || !self.sessions.get(id).map(|st| st.stormed).unwrap_or(false) {
            return StartGate::Admit;
        }
        let st = self.sessions.get_mut(id).expect("checked above");
        if st.needs_human {
            let first = !st.preempt_logged;
            st.preempt_logged = true;
            return StartGate::Preempt {
                reason: format!(
                    "fleet storm preempt: {id} exhausted 3 bounded self-retries; needs human"
                ),
                first,
            };
        }
        if mono_ms < st.next_retry_ms {
            let first = !st.preempt_logged;
            st.preempt_logged = true;
            let wait = st.next_retry_ms.saturating_sub(mono_ms);
            return StartGate::Preempt {
                reason: format!(
                    "fleet storm preempt: {id} held {wait}ms more (self-retry {}/3)",
                    st.retries
                ),
                first,
            };
        }
        st.retries = st.retries.saturating_add(1);
        st.preempt_logged = false;
        if st.retries as usize >= HELD_RETRY_MS.len() {
            st.needs_human = true;
            st.next_retry_ms = u64::MAX;
        } else {
            st.next_retry_ms = mono_ms.saturating_add(HELD_RETRY_MS[st.retries as usize]);
        }
        StartGate::Admit
    }

    /// Why a start is currently refused, if at all. Pure observation.
    pub fn admission(&self, id: &str, mono_ms: u64) -> Option<String> {
        if self.fleet_hold_until_ms > mono_ms {
            return Some(self.fleet_hold_reason.clone().unwrap_or_default());
        }
        let st = self.sessions.get(id)?;
        if !st.stormed {
            return None;
        }
        if st.needs_human {
            return Some(format!("fleet storm preempt: {id} needs human"));
        }
        if mono_ms < st.next_retry_ms {
            return Some(format!("fleet storm preempt: {id} held"));
        }
        None
    }

    /// Release holds whose deadline has passed. Returns one `(id, reason)` per
    /// released hold so the caller can log it. A hold is time-based: even if
    /// nobody calls this, [`Self::admission`] stops refusing after the window.
    pub fn release_due(&mut self, mono_ms: u64) -> Vec<(String, String)> {
        if self.fleet_hold_until_ms == 0 || mono_ms < self.fleet_hold_until_ms {
            return Vec::new();
        }
        let reason = format!("start hold released after {START_HOLD_MS}ms");
        self.fleet_hold_until_ms = 0;
        self.fleet_hold_reason = None;
        self.fleet_hold_logged = false;
        self.held.clear();
        self.recent_starts.clear();
        vec![("<fleet>".to_string(), reason)]
    }

    /// Every storming worker still holding a preemption, with its reason.
    pub fn preempted_victims(&self, mono_ms: u64) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .sessions
            .iter()
            .filter(|(_, st)| {
                st.stormed
                    && st.preempted_at_ms != 0
                    && mono_ms.saturating_sub(st.preempted_at_ms) <= PREEMPTION_HOLD_MS
            })
            .map(|(id, st)| (id.clone(), st.preempt_reason.clone().unwrap_or_default()))
            .collect();
        out.sort();
        out
    }

    /// The worst storming worker that is still preempted, with its reason.
    /// Never takes the same worker twice until it is settled or times out.
    pub fn preempt_victim(&self, mono_ms: u64) -> Option<(String, String)> {
        let mut candidates: Vec<(usize, String, String)> = self
            .sessions
            .iter()
            .filter(|(_, st)| {
                st.stormed
                    && st.preempted_at_ms != 0
                    && mono_ms.saturating_sub(st.preempted_at_ms) <= PREEMPTION_HOLD_MS
            })
            .map(|(id, st)| {
                (
                    st.starts.len(),
                    id.clone(),
                    st.preempt_reason.clone().unwrap_or_default(),
                )
            })
            .collect();
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        candidates
            .into_iter()
            .next()
            .map(|(_, id, reason)| (id, reason))
    }

    /// Settle a preemption (the target ran, or a human answered). It is not
    /// taken again as a victim.
    pub fn settle(&mut self, id: &str) {
        if let Some(st) = self.sessions.get_mut(id) {
            st.preempted_at_ms = 0;
            st.preempt_reason = None;
        }
    }

    /// Sessions whose starts are currently deferred by a burst hold.
    pub fn held_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.held.iter().cloned().collect();
        ids.sort();
        ids
    }

    /// Sessions that have been classified as a storm.
    pub fn stormed_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, st)| st.stormed)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Sessions currently held by a preemption.
    pub fn preempted_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, st)| st.stormed && st.preempted_at_ms != 0)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Cumulative `(holds, storms, preemptions)` since construction.
    pub fn counters(&self) -> (u64, u64, u64) {
        (self.held_total, self.storm_total, self.preempt_total)
    }
}

/// Guards are keyed by state dir (the same pattern as `JUDGE_POOLS`) so test
/// daemons never share fleet state, and no field has to be added to `Daemon`.
static FLEET_GUARDS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<FleetGuard>>>>> = OnceLock::new();

/// The runtime fleet guard for a daemon's state dir.
pub fn fleet_guard_for(state_dir: &Path) -> Arc<Mutex<FleetGuard>> {
    let map = FLEET_GUARDS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        map.entry(state_dir.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(FleetGuard::new()))),
    )
}
