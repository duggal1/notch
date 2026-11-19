//! Daemon configuration. Everything injectable for tests.

use std::path::PathBuf;

pub struct Config {
    /// Where registry.json / state.json / events.jsonl live.
    pub state_dir: PathBuf,
    /// Root under which per-session dirs `notch-<id>` are created and swept.
    /// Defaults to the OS temp dir.
    pub tmp_dir: PathBuf,
    /// A working session whose revision has not changed in this many seconds
    /// is considered stuck. Default 900; the demo plan sets 15.
    pub stuck_after_secs: u64,
    /// Watchdog poll cadence (used by `run_poll_loop`; `poll_once` is driven
    /// manually in tests).
    pub poll_interval_ms: u64,
    /// Off by default. If set, the deterministic policy is still used and a
    /// warning is logged — the OpenCode-backed decider is intentionally not
    /// implemented for the prototype.
    pub opencode_decider: bool,
}

impl Config {
    /// Reads NOTCH_STATE_DIR / NOTCH_STUCK_AFTER_SECS / NOTCH_POLL_INTERVAL_MS.
    /// herdr's target command is read by `Herdr::from_env` (NOTCH_HERDR_CMD).
    pub fn from_env() -> Self {
        let stuck_after_secs = std::env::var("NOTCH_STUCK_AFTER_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(900);
        let poll_interval_ms = std::env::var("NOTCH_POLL_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(2000);
        Self {
            state_dir: crate::paths::default_state_dir(),
            tmp_dir: std::env::temp_dir(),
            stuck_after_secs,
            poll_interval_ms,
            opencode_decider: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::from_env()
    }
}
