//! Append-only event log (`events.jsonl`, rotated at 4 MB) and the rolling
//! 10-second EPS window used by the snapshot.

use crate::clock::Clock;
use serde::Serialize;
use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const WINDOW_MS: u64 = 10_000;
pub const ROTATE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Serialize)]
struct EventLine {
    ts: u64,
    session: Option<String>,
    kind: String,
    detail: String,
}

pub struct EventLog {
    path: PathBuf,
    max_bytes: u64,
    clock: Arc<dyn Clock>,
    window: Mutex<VecDeque<u64>>,
}

impl EventLog {
    pub fn new(path: PathBuf, max_bytes: u64, clock: Arc<dyn Clock>) -> Self {
        Self {
            path,
            max_bytes,
            clock,
            window: Mutex::new(VecDeque::new()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one event. Rotates the log first if it exceeds the threshold.
    pub fn append(&self, session: Option<String>, kind: &str, detail: &str) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rotate_if_oversized(&self.path, self.max_bytes)?;
        let now = self.clock.now_ms();
        let line = EventLine {
            ts: now,
            session,
            kind: kind.to_string(),
            detail: detail.to_string(),
        };
        let mut buf = serde_json::to_vec(&line).unwrap_or_else(|_| b"{}".to_vec());
        buf.push(b'\n');
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(&buf)?;
        let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        w.push_back(now);
        while w
            .front()
            .map(|t| now.saturating_sub(*t) > WINDOW_MS)
            .unwrap_or(false)
        {
            w.pop_front();
        }
        Ok(())
    }

    /// Events per second across the rolling 10 s window.
    pub fn eps(&self) -> f64 {
        let now = self.clock.now_ms();
        let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        while w
            .front()
            .map(|t| now.saturating_sub(*t) > WINDOW_MS)
            .unwrap_or(false)
        {
            w.pop_front();
        }
        w.len() as f64 / (WINDOW_MS as f64 / 1000.0)
    }
}

/// Rotate `events.jsonl` → `events.jsonl.1` when it exceeds `max_bytes`.
/// Overwrites any previous `.1`. Returns true when a rotation happened.
pub fn rotate_if_oversized(path: &Path, max_bytes: u64) -> io::Result<bool> {
    let len = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return Ok(false),
    };
    if len <= max_bytes {
        return Ok(false);
    }
    let rotated = path.with_extension("jsonl.1");
    std::fs::rename(path, rotated)?;
    Ok(true)
}
