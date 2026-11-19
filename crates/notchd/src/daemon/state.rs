//! Durable state persistence.
//!
//! `state.json` is written atomically (temp + fsync + rename + parent-dir
//! fsync) with a 3-deep rolling backup (`state.json.bak1..3`, rotated on
//! every write) — see `crate::durable`. Missing or corrupt files load as
//! `StateFile::empty()`: the daemon starts fresh rather than crash.
//! Recovery-from-backup is deliberately NOT automatic (that would change
//! observable behaviour); the backups are for operator/manual recovery.

use super::types::{lock, Daemon, Inner};
use crate::model::StateFile;
use crate::registry::Registry;
use std::path::{Path, PathBuf};

impl Daemon {
    pub(crate) fn state_path(&self) -> PathBuf {
        self.config.state_dir.join("state.json")
    }

    pub(crate) fn registry(&self) -> Registry {
        Registry::load(&registry_path(&self.config.state_dir), self.clock.as_ref())
    }

    pub(crate) fn save_registry(&self, reg: &Registry) {
        let _ = reg.save(&registry_path(&self.config.state_dir));
    }

    /// Persist the tree atomically (temp + fsync + rename + parent-dir
    /// fsync). The rolling backup chain `state.json.bak1..3` is rotated
    /// first — see `crate::durable`.
    pub(crate) fn save_state(&self, inner: &Inner) {
        let file = StateFile {
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
        };
        let bytes = match serde_json::to_vec_pretty(&file) {
            Ok(b) => b,
            Err(_) => return,
        };
        let _ = crate::durable::write_atomic(&self.state_path(), &bytes);
    }
}

pub(crate) fn registry_path(state_dir: &Path) -> PathBuf {
    state_dir.join("registry.json")
}

pub(crate) fn load_state(path: &Path) -> StateFile {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice::<StateFile>(&bytes).unwrap_or_else(|_| StateFile::empty())
        }
        Err(_) => StateFile::empty(),
    }
}
