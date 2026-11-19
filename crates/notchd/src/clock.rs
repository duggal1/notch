//! Clock abstraction. Every duration/timeout in notchd uses the monotonic
//! half; only display and persistence use the wall clock.
//!
//! `SystemClock::mono_ms` is anchored once per process to a static
//! [`Instant`] via `OnceLock`. `Instant` is unaffected by NTP steps, manual
//! clock changes and (on macOS) wall-clock jumps across sleep: the elapsed
//! time since the anchor is the only thing that can drive a watchdog.
//! `FakeClock::set` moves ONLY the wall clock, so tests can inject arbitrary
//! skew while `mono_ms` keeps its own, strictly non-decreasing timeline.

use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static MONO_ANCHOR: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the process-wide monotonic anchor. Never goes
/// backwards, never follows a wall-clock jump.
pub fn mono_ms_now() -> u64 {
    MONO_ANCHOR
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch. Display and persistence only.
    fn now_ms(&self) -> u64;
    /// Monotonic milliseconds. Every duration and timeout uses this.
    fn mono_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn mono_ms(&self) -> u64 {
        mono_ms_now()
    }
}

/// Deterministic clock for tests. Not wall-clock based. `advance_ms` is real
/// elapsed time and moves both timelines; `set` is a wall-clock jump and
/// moves only `now_ms`.
pub struct FakeClock {
    t: Mutex<u64>,
    mono: Mutex<u64>,
}

impl FakeClock {
    pub fn new(t: u64) -> Self {
        Self {
            t: Mutex::new(t),
            mono: Mutex::new(t),
        }
    }

    /// Real elapsed time: both wall and monotonic timelines advance.
    pub fn advance_ms(&self, ms: u64) {
        let mut t = self.t.lock().unwrap_or_else(|e| e.into_inner());
        *t = t.saturating_add(ms);
        let mut mono = self.mono.lock().unwrap_or_else(|e| e.into_inner());
        *mono = mono.saturating_add(ms);
    }

    /// Advance only the monotonic timeline (e.g. to model mono drifting
    /// forward while the wall clock is frozen).
    pub fn advance_mono_ms(&self, ms: u64) {
        let mut mono = self.mono.lock().unwrap_or_else(|e| e.into_inner());
        *mono = mono.saturating_add(ms);
    }

    /// A wall-clock jump (NTP step, user change, Tauri clock correction).
    /// The monotonic timeline is deliberately untouched.
    pub fn set(&self, t: u64) {
        *self.t.lock().unwrap_or_else(|e| e.into_inner()) = t;
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        *self.t.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn mono_ms(&self) -> u64 {
        *self.mono.lock().unwrap_or_else(|e| e.into_inner())
    }
}
