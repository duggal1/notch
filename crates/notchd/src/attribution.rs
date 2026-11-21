//! Per-session attribution: which session touched which file.
//!
//! `git --no-optional-locks -C <cwd> status --porcelain=v1 -z` is sampled while
//! a session is working. Any git failure (not a repo, no git, timeout) is a
//! NON-signal, never an error — the daemon keeps running and attribution
//! simply has no entry for that poll.
//!
//! Attribution is tracked per session (cap `PER_SESSION_CAP` files per
//! session), deduped, and surfaced newest-first (cap `SNAPSHOT_CAP`).

use std::path::Path;

const PER_SESSION_CAP: usize = 64;
const SNAPSHOT_CAP: usize = 24;
const TRACKED_CAP: usize = 512;

// Preserve exact artifact identity, as AgentBox does in
// shared/artifact-marks.mts:779-796. Git's NUL mode keeps whitespace and
// punctuation literal. Ignore an incomplete final record after output capping.
fn paths(out: &str) -> impl Iterator<Item = &str> {
    out.split_inclusive('\0')
        .filter_map(|record| record.strip_suffix('\0'))
}

fn parse_porcelain(out: &str) -> Vec<String> {
    let mut records = paths(out);
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        let Some(path) = record.get(3..) else {
            continue;
        };
        if record.as_bytes()[..2]
            .iter()
            .any(|b| matches!(b, b'R' | b'C'))
            && records.next().is_none()
        {
            break;
        }
        files.push(path.to_string());
    }
    files
}

/// Sample one worktree's exact file set: commits since this session's base
/// plus porcelain status. Metadata under `<session>/notch-worktree.json`
/// supplies the base for per-session worktrees; plain repos keep the legacy
/// porcelain-only behavior. Any git failure is a non-signal, never an error.
pub fn sample_git(cwd: &Path) -> Vec<String> {
    let base = crate::worktree::meta_for_cwd(cwd).map(|m| m.base_rev);
    sample_git_at(cwd, base.as_deref())
}

/// Exact attribution for `cwd` relative to `base_rev`:
/// Committed paths since `base_rev` plus working tree changes. Sorted,
/// deduped, capped by caller.
pub fn sample_git_at(cwd: &Path, base_rev: Option<&str>) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(out) = crate::worktree::git_run(
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
        Some(cwd),
    ) {
        if out.ok {
            files.extend(parse_porcelain(&out.stdout));
        }
    }
    if let Some(base) = base_rev {
        let range = format!("{base}..HEAD");
        if let Ok(out) =
            crate::worktree::git_run(&["diff", "--name-only", "-z", range.as_str()], Some(cwd))
        {
            if out.ok {
                files.extend(paths(&out.stdout).map(str::to_string));
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

pub const fn per_session_cap() -> usize {
    PER_SESSION_CAP
}

pub const fn snapshot_cap() -> usize {
    SNAPSHOT_CAP
}

pub const fn tracked_cap() -> usize {
    TRACKED_CAP
}
