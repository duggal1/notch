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

/// How much of `events.jsonl` the snapshot's bus-message projection reads.
/// The log rotates at 4 MB but `busMessages` is capped at 30 entries, so a
/// bounded tail is sufficient and keeps snapshot cost independent of log size.
pub const BUS_LOG_TAIL_BYTES: u64 = 512 * 1024;

/// Read at most `max_bytes` from the end of `path`, starting on a line boundary
/// so the returned bytes always contain complete JSON lines.
pub fn read_tail(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len <= max_bytes {
        let mut bytes = Vec::with_capacity(len as usize);
        f.read_to_end(&mut bytes)?;
        return Ok(bytes);
    }
    f.seek(SeekFrom::Start(len - max_bytes))?;
    let mut bytes = vec![0u8; max_bytes as usize];
    f.read_exact(&mut bytes)?;
    if let Some(pos) = bytes.iter().position(|b| *b == b'\n') {
        bytes.drain(..=pos);
    }
    Ok(bytes)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_tail_starts_on_a_line_boundary_and_is_bounded() {
        let dir = std::env::temp_dir().join(format!("notch-events-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        let mut expected = String::new();
        for i in 0..100 {
            expected.push_str(&format!("{{\"n\":{i}}}\n"));
        }
        std::fs::write(&path, &expected).unwrap();

        let tail = String::from_utf8(read_tail(&path, 32).unwrap()).unwrap();
        assert!(tail.starts_with("{\"n\":"), "tail began mid-line: {tail:?}");
        assert!(tail.lines().all(|l| l.starts_with("{\"n\":")), "{tail:?}");
        assert!(tail.len() <= 32);

        let whole = String::from_utf8(read_tail(&path, 1024).unwrap()).unwrap();
        assert_eq!(whole, expected);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
