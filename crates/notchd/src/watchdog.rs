//! Watchdog — liveness classification and the suspect/stuck action ladder.
//!
//! A session is only ever classified from the *monotonic* timeline. Three
//! independent observations rescue a frozen `revision` from a false kill:
//! the status not being `Working`, a revision change in the current poll, and
//! OpenCode reporting the session as busy (a second opinion).
//!
//! `Suspect` (quiet >= threshold) gets a nudge and a `stuck_suspect`
//! escalation. `Stuck` (quiet >= 2x threshold) gets a `stuck` escalation;
//! the work is snapshotted, the escalation is recorded, and then the pane is
//! closed. `kill_on_stuck` defaults ON to honor the product promise that the
//! watchdog kills a verified-stuck session; operators can disable it with
//! `Daemon::set_kill_on_stuck(false)`.

use crate::status::Status;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Suspect,
    Stuck,
}

/// Everything `classify` needs, sampled BEFORE this poll's revision change is
/// applied to session state.
#[derive(Clone, Copy, Debug)]
pub struct Obs {
    pub status: Status,
    pub revision_changed_now: bool,
    /// Second opinion from OpenCode's `/api/session` busy markers.
    /// Only `Some(true)` counts as evidence of life.
    pub oc_busy: Option<bool>,
    pub quiet_for_ms: u64,
}

/// A single large poll gap is a scheduler/VM stall; a *repeating* large gap
/// is a permanently slow poll loop (e.g. a bounded subprocess that always
/// consumes most of the tick). Only the first few gaps get grace.
pub const MAX_GRACE_STREAK: u32 = 3;

pub struct Watchdog {
    stuck_after_ms: u64,
    /// Kill verified-Stuck sessions by default (product promise); an operator
    /// may opt out via `Daemon::set_kill_on_stuck(false)`.
    kill_on_stuck: bool,
    last_poll_mono_ms: Option<u64>,
    grace_streak: u32,
    suspect_taken: HashSet<String>,
    stuck_taken: HashSet<String>,
}

impl Watchdog {
    pub fn new(stuck_after_secs: u64) -> Self {
        Self {
            stuck_after_ms: stuck_after_secs.saturating_mul(1000),
            kill_on_stuck: true,
            last_poll_mono_ms: None,
            grace_streak: 0,
            suspect_taken: HashSet::new(),
            stuck_taken: HashSet::new(),
        }
    }

    pub fn set_stuck_after_secs(&mut self, secs: u64) {
        self.stuck_after_ms = secs.saturating_mul(1000);
    }

    pub fn stuck_after_ms(&self) -> u64 {
        self.stuck_after_ms
    }

    /// Stuck requires twice the ordinary suspicion threshold.
    pub fn stuck_threshold_ms(&self) -> u64 {
        self.stuck_after_ms.saturating_mul(2)
    }

    pub fn kill_on_stuck(&self) -> bool {
        self.kill_on_stuck
    }

    pub fn set_kill_on_stuck(&mut self, enabled: bool) {
        self.kill_on_stuck = enabled;
    }

    pub fn should_close_pane(&self, liveness: Liveness) -> bool {
        self.kill_on_stuck && liveness == Liveness::Stuck
    }

    /// A gap larger than three poll intervals means the scheduler/VM stalled:
    /// grant grace instead of interpreting the gap as session silence.
    pub fn is_stall_gap(gap_ms: u64, poll_interval_ms: u64) -> bool {
        poll_interval_ms > 0 && gap_ms > poll_interval_ms.saturating_mul(3)
    }

    /// Record this poll's monotonic instant and report the gap to the
    /// previous poll (None on the first observation).
    pub fn note_poll(&mut self, mono_ms: u64) -> Option<u64> {
        let gap = self.last_poll_mono_ms.map(|prev| mono_ms.saturating_sub(prev));
        self.last_poll_mono_ms = Some(mono_ms);
        gap
    }

    /// Spend one grace grant for a large poll gap. Returns false once the
    /// streak is exhausted: from then on the gap is classified normally, so
    /// a slow-but-progressing poll loop cannot disable the watchdog forever.
    pub fn grant_stall_grace(&mut self) -> bool {
        if self.grace_streak >= MAX_GRACE_STREAK {
            return false;
        }
        self.grace_streak += 1;
        true
    }

    pub fn clear_stall_grace_streak(&mut self) {
        self.grace_streak = 0;
    }

    pub fn grace_streak(&self) -> u32 {
        self.grace_streak
    }

    /// Take the nudge action once per silence episode. A revision change
    /// re-arms it via [`Self::mark_revision_change`].
    pub fn take_suspect_action(&mut self, id: &str, _mono_ms: u64) -> bool {
        self.suspect_taken.insert(id.to_string())
    }

    /// Take the stuck escalation once per silence episode.
    pub fn take_stuck_action(&mut self, id: &str, _mono_ms: u64) -> bool {
        self.stuck_taken.insert(id.to_string())
    }

    /// Re-arm the per-episode verdicts without touching the grace streak.
    pub fn rearm_episode(&mut self, id: &str) {
        self.suspect_taken.remove(id);
        self.stuck_taken.remove(id);
    }

    pub fn mark_revision_change(&mut self, id: &str, _mono_ms: u64) {
        self.rearm_episode(id);
        // Real progress after a stall means the next large gap is a fresh
        // one-off stall, not evidence of a permanently slow loop.
        self.grace_streak = 0;
    }

    pub fn classify(&self, o: Obs) -> Liveness {
        // The timestamp used to compute `quiet_for_ms` can predate this poll's
        // revision change; a fresh revision is absolute proof of life.
        if o.status != Status::Working || o.revision_changed_now || o.oc_busy == Some(true) {
            return Liveness::Alive;
        }
        if o.quiet_for_ms >= self.stuck_threshold_ms() {
            Liveness::Stuck
        } else if o.quiet_for_ms >= self.stuck_after_ms {
            Liveness::Suspect
        } else {
            Liveness::Alive
        }
    }

    /// Legacy 1x-threshold helper kept for callers that still ask the old
    /// binary question. The poll path uses [`Self::classify`] exclusively.
    pub fn is_stuck(&self, status: Status, revision_changed_at_ms: u64, now_ms: u64) -> bool {
        status == Status::Working
            && now_ms.saturating_sub(revision_changed_at_ms) >= self.stuck_after_ms
    }
}
