//! Per-session attribution: which session touched which file.
//!
//! `git --no-optional-locks -C <cwd> status --porcelain=v1` is sampled while
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

/// Parse `git status --porcelain=v1` output into relative file paths.
/// Handles `XY path`, untracked `?? path`, and rename lines `R  old -> new`
/// (the new path wins). Paths with quotes are unquoted and C-unescaped.
pub fn parse_porcelain(out: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in out.lines() {
        if line.len() < 4 {
            continue;
        }
        let path = &line[3..];
        let path = if let Some((_, new)) = path.split_once(" -> ") {
            new
        } else {
            path
        };
        let path = unquote_git_path(path.trim());
        if !path.is_empty() {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    files
}

fn unquote_git_path(p: &str) -> String {
    let p = p.trim();
    if p.len() >= 2 && p.starts_with('"') && p.ends_with('"') {
        // git quotes paths with special chars as C-style strings; strips quotes
        // and leave the interior as-is (good enough for tracking purposes).
        return p[1..p.len() - 1].to_string();
    }
    p.to_string()
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
/// `git diff --name-only <base>..HEAD` (committed by this session) unioned
/// with `git status --porcelain=v1` (WIP). Sorted, deduped, capped by caller.
pub fn sample_git_at(cwd: &Path, base_rev: Option<&str>) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(out) = crate::worktree::git_run(&["status", "--porcelain=v1"], Some(cwd)) {
        if out.ok {
            files.extend(parse_porcelain(&out.stdout));
        }
    }
    if let Some(base) = base_rev {
        let range = format!("{base}..HEAD");
        if let Ok(out) =
            crate::worktree::git_run(&["diff", "--name-only", range.as_str()], Some(cwd))
        {
            if out.ok {
                files.extend(parse_name_only(&out.stdout));
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// `git diff --name-only` emits bare paths (no `XY ` status prefix).
fn parse_name_only(out: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in out.lines() {
        let path = unquote_git_path(line.trim());
        if !path.is_empty() {
            files.push(path);
        }
    }
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
