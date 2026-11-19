//! Wall-clock abstraction. Everything time-based in notchd goes through a
//! `Clock`, so tests can drive the daemon with a fake clock.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Deterministic clock for tests. Not wall-clock based.
pub struct FakeClock {
    t: Mutex<u64>,
}

impl FakeClock {
    pub fn new(t: u64) -> Self {
        Self { t: Mutex::new(t) }
    }

    pub fn advance_ms(&self, ms: u64) {
        let mut t = self.t.lock().unwrap_or_else(|e| e.into_inner());
        *t += ms;
    }

    pub fn set(&self, t: u64) {
        *self.t.lock().unwrap_or_else(|e| e.into_inner()) = t;
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        *self.t.lock().unwrap_or_else(|e| e.into_inner())
    }
}
