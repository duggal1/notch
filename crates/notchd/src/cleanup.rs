//! Per-session `$TMPDIR/notch-<id>/` lifecycle and the global sweep.
//!
//! - `session_tmp_dir` — the dir for a session id under the configured root.
//! - `cleanup_session` — remove one dir (done/failed/kill hook).
//! - `sweep` — remove dirs for dead (done/failed) sessions, orphan
//!   `notch-*` dirs that match no live session, and rotate `events.jsonl`
//!   if it exceeds the 4 MB threshold. Returns the number of dirs removed.

use crate::model::SessionState;
use crate::status::Status;
use std::io;
use std::path::{Path, PathBuf};

pub fn session_tmp_dir(root: &Path, id: &str) -> PathBuf {
    root.join(format!("notch-{id}"))
}

/// Remove one session's temp dir. Returns true if a directory was removed.
pub fn cleanup_session(root: &Path, id: &str) -> io::Result<bool> {
    let dir = session_tmp_dir(root, id);
    remove_dir_if_exists(&dir)
}

fn remove_dir_if_exists(dir: &Path) -> io::Result<bool> {
    match std::fs::metadata(dir) {
        Ok(m) if m.is_dir() => {
            std::fs::remove_dir_all(dir)?;
            Ok(true)
        }
        Ok(_) => {
            // Not a directory — refuse to touch it.
            Ok(false)
        }
        Err(_) => Ok(false),
    }
}

/// Global sweep. `sessions` is the full tracked tree. Returns how many
/// directories were removed.
pub fn sweep(root: &Path, sessions: &[SessionState], events_path: &Path, events_max_bytes: u64) -> io::Result<u64> {
    let mut removed = 0u64;

    // 1. Dead sessions (done / failed): remove their temp dirs.
    for s in sessions {
        if (s.status == Status::Done || s.status == Status::Failed)
            && remove_dir_if_exists(&session_tmp_dir(root, &s.id))?
        {
            removed += 1;
        }
    }

    // 2. Orphan notch-* dirs under the root that match no live session.
    let live: std::collections::HashSet<String> = sessions
        .iter()
        .filter(|s| !s.status.is_terminal())
        .map(|s| s.id.clone())
        .collect();
    if let Ok(read) = std::fs::read_dir(root) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_prefix("notch-") else { continue };
            if live.contains(id) {
                continue;
            }
            if remove_dir_if_exists(&entry.path())? {
                removed += 1;
            }
        }
    }

    // 3. Rotate stale events.jsonl if oversized.
    crate::events::rotate_if_oversized(events_path, events_max_bytes)?;

    Ok(removed)
}
