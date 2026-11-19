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
use std::path::Path;

#[derive(Serialize, Deserialize)]
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
}

impl Registry {
    /// Missing or corrupt registry -> empty registry (fresh start). Never panics.
    pub fn load(path: &Path, _clock: &dyn crate::clock::Clock) -> Registry {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<Registry>(&bytes).unwrap_or_else(|_| Registry::fresh()),
            Err(_) => Registry::fresh(),
        }
    }

    fn fresh() -> Registry {
        Registry {
            version: 1,
            supervisor_count: 0,
            session_counts: HashMap::new(),
            ids: Vec::new(),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::durable::write_atomic(path, &serde_json::to_vec_pretty(self).unwrap_or_else(|_| b"{}".to_vec()))
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// SV-<n>. Monotonic; collision-proof even against hand-edited registries.
    pub fn allocate_supervisor(&mut self) -> String {
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

    /// S<sup>-OC<nn>. nn is 1-based per supervisor, never reused.
    pub fn allocate_session(&mut self, supervisor_number: u32) -> String {
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
        let dir =
            std::env::temp_dir().join(format!("notch-reg-{tag}-{}-{n}", std::process::id()));
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
        let fake = canary.allocate_session(1);
        fs::write(crate::durable::tmp_path(&path), serde_json::to_vec(&canary).unwrap()).unwrap();
        fs::write(&path, b"not json").unwrap();

        let recovered = Registry::load(&path, &SystemClock);
        assert!(recovered.ids().contains(&real), "backup ids must be recovered");
        assert!(!recovered.ids().contains(&fake), "a .tmp file must NEVER be adopted");
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
