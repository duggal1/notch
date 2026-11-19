//! Watchdog — stuck-session detection.
//!
//! A session is *stuck* when it is `working` and its herdr `revision` has
//! not changed for `stuck_after` milliseconds. The threshold is a config
//! value (default 900 s; the demo plan uses 15 s).

use crate::status::Status;

pub struct Watchdog {
    stuck_after_ms: u64,
}

impl Watchdog {
    pub fn new(stuck_after_secs: u64) -> Self {
        Self {
            stuck_after_ms: stuck_after_secs.saturating_mul(1000),
        }
    }

    pub fn set_stuck_after_secs(&mut self, secs: u64) {
        self.stuck_after_ms = secs.saturating_mul(1000);
    }

    pub fn stuck_after_ms(&self) -> u64 {
        self.stuck_after_ms
    }

    /// Is this session stuck right now?
    ///
    /// Only `working` sessions can be stuck, and only when the revision
    /// timestamp is at least `stuck_after_ms` in the past.
    pub fn is_stuck(&self, status: Status, revision_changed_at_ms: u64, now_ms: u64) -> bool {
        status == Status::Working && now_ms.saturating_sub(revision_changed_at_ms) >= self.stuck_after_ms
    }
}
