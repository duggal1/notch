//! Per-session `$TMPDIR/notch-<id>/` lifecycle and the global sweep.
//!
//! - `session_tmp_dir` — the dir for a session id under the configured root.
//! - `cleanup_session` — remove one dir (done/failed/kill hook).
//! - `sweep` — remove tracked temp dirs of dead sessions, exact-name empty
//!   orphans, and rotate `events.jsonl` if it exceeds the 4 MB threshold.
//!
//! Sweep safety (lead ruling, 2026-11-19): `notch-*` is NOT a valid match
//! basis. A candidate must be a directory whose name is EXACTLY
//! `notch-S<sup>-OC<nn>` (`<sup>` = one or more ASCII digits, `<nn>` = exactly
//! two). Before any deletion the dir must be (a) tracked as some session's
//! recorded temp dir, or (b) an exact-name EMPTY orphan. A nonempty
//! exact-name dir with no session behind it is skipped with a warning — never
//! deleted. Anything outside the exact grammar is ignored entirely.
//!
//! Returns (dirs removed, warnings).

use crate::model::SessionState;
use crate::status::Status;
use std::io;
use std::path::{Path, PathBuf};

pub fn session_tmp_dir(root: &Path, id: &str) -> PathBuf {
    root.join(format!("notch-{id}"))
}

/// Remove one session's temp dir. WIP is snapshotted into its branch BEFORE
/// anything is deleted; a dirty worktree (snapshot failed) is left intact.
/// Returns true if a directory was removed.
pub fn cleanup_session(root: &Path, id: &str) -> io::Result<bool> {
    if let Some(wt) = crate::worktree::load(root, id) {
        if wt.path.exists()
            && crate::worktree::snapshot(&wt.path, id, "cleanup").is_err()
        {
            return Ok(false); // snapshot failed — never delete unseen work
        }
        if crate::worktree::remove(&wt).is_err() {
            return Ok(false); // dirty or git failure — keep everything
        }
    }
    let dir = session_tmp_dir(root, id);
    remove_dir_if_exists(&dir)
}

/// Configured scratch budget (`NOTCH_MAX_TMP_BYTES`, default 5 GiB).
pub fn max_tmp_bytes() -> u64 {
    std::env::var("NOTCH_MAX_TMP_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(crate::worktree::DEFAULT_MAX_TMP_BYTES)
}

/// Re-export of the worktree-aware budget eviction.
pub use crate::worktree::enforce_budget;

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

fn is_empty_dir(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut rd) => rd.next().is_none(),
        Err(_) => false, // unreadable: treat as nonempty (never delete on uncertainty)
    }
}

/// Exact grammar of an engine temp dir: `notch-S<sup>-OC<nn>` where `<sup>`
/// is one or more ASCII digits and `<nn>` is exactly two. Anything else is
/// not ours.
fn is_exact_temp_dir_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("notch-S") else { return false };
    let Some((sup, rest)) = rest.split_once('-') else { return false };
    if sup.is_empty() || !sup.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(nn) = rest.strip_prefix("OC") else { return false };
    nn.len() == 2 && nn.bytes().all(|b| b.is_ascii_digit())
}

/// Global sweep. `sessions` is the full tracked tree. Returns how many
/// directories were removed plus safety warnings for skipped candidates.
pub fn sweep(
    root: &Path,
    sessions: &[SessionState],
    events_path: &Path,
    events_max_bytes: u64,
) -> io::Result<(u64, Vec<String>)> {
    let mut removed = 0u64;
    let mut warnings: Vec<String> = Vec::new();

    // Tracked set: paths recorded at session creation. Legacy sessions with
    // no recorded temp_dir are treated as untracked.
    let tracked: std::collections::HashSet<PathBuf> = sessions
        .iter()
        .filter_map(|s| s.temp_dir.as_ref())
        .map(PathBuf::from)
        .collect();

    // 1. Dead (done/failed) sessions: remove their RECORDED temp dir.
    for s in sessions {
        if s.status != Status::Done && s.status != Status::Failed {
            continue;
        }
        let Some(dir) = s.temp_dir.as_ref() else {
            continue; // legacy untracked entry: orphan pass handles it
        };
        if remove_dir_if_exists(Path::new(dir))? {
            removed += 1;
        }
    }

    // 2. Orphan pass: exact-name, untracked dirs. Empty ones are swept;
    //    nonempty ones survive with a warning.
    if let Ok(read) = std::fs::read_dir(root) {
        for entry in read.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_exact_temp_dir_name(&name) {
                continue; // not an engine-shaped name — ignore entirely
            }
            if tracked.contains(&path) {
                continue; // handled by pass 1 (if it was dead) or live — keep
            }
            if is_empty_dir(&path) {
                if let Ok(true) = remove_dir_if_exists(&path) {
                    removed += 1;
                }
            } else {
                warnings.push(format!(
                    "orphan `{name}` has no tracked session — left in place (contains files)"
                ));
            }
        }
    }

    // 3. Rotate stale events.jsonl if oversized.
    crate::events::rotate_if_oversized(events_path, events_max_bytes)?;

    // 4. Real disk budget: dead sessions evicted oldest-first. Dirty
    //    worktrees and live sessions are refused by enforce_budget itself.
    let before: std::collections::HashSet<PathBuf> = sessions
        .iter()
        .filter_map(|s| s.temp_dir.as_ref())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect();
    let (_freed, budget_warnings) = enforce_budget(root, sessions, max_tmp_bytes())?;
    warnings.extend(budget_warnings);
    for dir in before {
        if !dir.exists() {
            removed += 1;
        }
    }

    Ok((removed, warnings))
}
