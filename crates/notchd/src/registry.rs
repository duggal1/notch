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
