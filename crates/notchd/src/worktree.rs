//! Per-session git worktrees — A5 isolation (HARDENING §3.6).
//!
//! Every session gets its own branch `notch/<id>` and a linked worktree under
//! its scratch dir. The pane's cwd is that worktree, so `git status` is an
//! EXACT signal: no other agent can dirty it. Attribution is commits since
//! `base_rev` plus porcelain status (see [`crate::attribution`]); conflicts are
//! decided by `git merge-tree --write-tree` (exit 1 = conflict), never by
//! comparing path names.
//!
//! Safety: WIP is committed by [`snapshot`] before a pane is closed; a dirty
//! worktree is never removed; no merge is ever performed automatically; the
//! snapshot branch is KEPT so committed WIP stays recoverable.
//!
//! All git shell-outs funnel through one [`git_run`] seam. CONTRACT(A2): once
//! `proc.rs` lands, its body becomes `proc::run(&Spec{cmd:"git", ..})` with the
//! frozen 5 s git timeout — no other call site needs to change.

use crate::model::SessionState;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

pub const BRANCH_PREFIX: &str = "notch/";
pub const META_FILE: &str = "notch-worktree.json";
pub const DEFAULT_MAX_TMP_BYTES: u64 = 5 * 1024 * 1024 * 1024;


/// A session's worktree, persisted as `<tmp>/notch-<id>/notch-worktree.json`
/// so a daemon restart can re-adopt it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Worktree {
    /// Absolute path of the worktree — used as the pane cwd.
    pub path: PathBuf,
    /// `notch/<session id>`.
    pub branch: String,
    /// Full commit sha the worktree was branched from (attribution base).
    pub base_rev: String,
    /// Canonical main checkout, used for `worktree remove` / merge-tree.
    pub base_repo: PathBuf,
}

#[derive(Debug)]
pub enum WorktreeError {
    /// The cwd is not a git worktree; caller must fall back with a warning.
    NotGit(String),
    /// A git command failed (with the command and stderr).
    Git(String),
    /// Refused to remove a worktree with uncommitted changes.
    Dirty(String),
    /// Filesystem error creating/locating the scratch layout.
    Io(String),
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotGit(s) | Self::Git(s) | Self::Dirty(s) | Self::Io(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for WorktreeError {}

/// One git result. `ok=false` with `code=Some(1)` is how merge-tree reports a
/// conflict — it is data, not an error.
#[derive(Debug, Clone)]
pub(crate) struct GitOut {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// THE single git seam — delegates to A2's `proc::run` (bounded 5 s hard
/// deadline, process-group kill, drained pipes). `ProcError::Failed` is data,
/// not an exception: merge-tree exits 1 to report a conflict, so the exit code
/// is preserved in `GitOut.code`.
pub(crate) fn git_run(args: &[&str], cwd: Option<&Path>) -> Result<GitOut, String> {
    let owned: Vec<String> = std::iter::once("-c".to_string())
        .chain(std::iter::once("core.fsmonitor=false".to_string()))
        .chain(args.iter().map(|s| (*s).to_string()))
        .collect();
    let spec = crate::proc::Spec {
        cmd: "git",
        args: &owned,
        cwd,
        env: &[],
        timeout: crate::proc::TIMEOUT_GIT,
        max_out: 1 << 20,
    };
    match crate::proc::run(&spec) {
        Ok(out) => Ok(GitOut {
            ok: true,
            code: Some(0),
            stdout: out.stdout,
            stderr: out.stderr,
        }),
        Err(crate::proc::ProcError::Failed { code, stderr }) => Ok(GitOut {
            ok: false,
            code,
            stdout: String::new(),
            stderr,
        }),
        Err(e) => Err(e.to_string()),
    }
}

fn git_text(args: &[&str], cwd: Option<&Path>) -> Result<String, WorktreeError> {
    // proc::run allows only 500 ms for pipe EOF after the child exits; under a
    // heavy parallel git storm that grace can expire and drop stdout. Every
    // git_text caller needs the text, so retry once rather than treating an
    // empty capture as a valid branch/sha. (Reported to A2 in COORD.md.)
    for attempt in 0..2 {
        let out = git_run(args, cwd).map_err(WorktreeError::Git)?;
        if !out.ok {
            return Err(WorktreeError::Git(format!(
                "git {} failed: {}",
                args.join(" "),
                out.stderr.trim()
            )));
        }
        let text = out.stdout.trim().to_string();
        if !text.is_empty() || attempt == 1 {
            return Ok(text);
        }
    }
    unreachable!()
}

/// `notch/<id>`, sanitized to a safe ref component.
pub fn branch_name(id: &str) -> String {
    let clean: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{BRANCH_PREFIX}{clean}")
}

/// `<tmp_root>/notch-<id>` — the session scratch dir.
pub fn session_root(tmp_root: &Path, id: &str) -> PathBuf {
    tmp_root.join(format!("notch-{id}"))
}

/// Create `build/` and `judge/` under the session scratch dir and return the
/// root. `worktree/` is created by `git worktree add` itself (git refuses an
/// existing target dir, even an empty one). `build/` is the optional
/// CARGO_TARGET_DIR; `judge/` is A1's decider scratch root (COORD layout).
pub fn ensure_layout(tmp_root: &Path, id: &str) -> Result<PathBuf, WorktreeError> {
    let root = session_root(tmp_root, id);
    for sub in ["build", "judge"] {
        std::fs::create_dir_all(root.join(sub))
            .map_err(|e| WorktreeError::Io(format!("{}: {e}", root.join(sub).display())))?;
    }
    Ok(root)
}

fn save_meta(wt: &Worktree) -> Result<(), WorktreeError> {
    let path = wt
        .path
        .parent()
        .ok_or_else(|| WorktreeError::Io("worktree path has no parent".into()))?
        .join(META_FILE);
    let json = serde_json::to_string_pretty(wt)
        .map_err(|e| WorktreeError::Io(format!("serialize worktree: {e}")))?;
    std::fs::write(&path, json)
        .map_err(|e| WorktreeError::Io(format!("write {}: {e}", path.display())))
}

/// Load a session's worktree metadata (restart / cleanup path).
pub fn load(tmp_root: &Path, id: &str) -> Option<Worktree> {
    read_meta(&session_root(tmp_root, id).join(META_FILE))
}

/// Load metadata when all we have is a pane cwd (the poll path).
pub fn meta_for_cwd(cwd: &Path) -> Option<Worktree> {
    read_meta(&cwd.parent()?.join(META_FILE))
}

fn read_meta(path: &Path) -> Option<Worktree> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Create (or re-adopt) the per-session worktree.
///
/// `Ok(None)` means `repo` is not a git worktree — the caller spawns with the
/// original cwd, emits a loud warning, and uses the old shared-cwd heuristic.
pub fn create(
    repo: &Path,
    id: &str,
    tmp_root: &Path,
) -> Result<Option<Worktree>, WorktreeError> {
    let repo = std::fs::canonicalize(repo)
        .map_err(|e| WorktreeError::Io(format!("cwd {}: {e}", repo.display())))?;
    let inside = git_text(&["rev-parse", "--is-inside-work-tree"], Some(&repo)).unwrap_or_default();
    if inside != "true" {
        return Ok(None);
    }
    // An unborn HEAD has no base to branch from — fall back loudly.
    let base_rev = match git_text(&["rev-parse", "HEAD"], Some(&repo)) {
        Ok(rev) if rev.len() >= 7 => rev,
        _ => return Ok(None),
    };
    let root = ensure_layout(tmp_root, id)?;
    let path = root.join("worktree");
    let branch = branch_name(id);

    if path.exists() {
        let current = git_text(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&path)).unwrap_or_default();
        if !current.is_empty() && current != branch {
            return Err(WorktreeError::Git(format!(
                "{} exists but is on branch `{current}`, expected `{branch}`",
                path.display()
            )));
        }
        let base_rev = load(tmp_root, id).map(|m| m.base_rev).unwrap_or(base_rev);
        let wt = Worktree {
            path,
            branch,
            base_rev,
            base_repo: repo,
        };
        save_meta(&wt)?;
        return Ok(Some(wt));
    }

    let exists = git_run(
        &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")],
        Some(&repo),
    )
    .map_err(WorktreeError::Git)?
    .ok;
    let path_s = path.to_string_lossy().to_string();
    let add = if exists {
        vec!["worktree", "add", "-q", path_s.as_str(), branch.as_str()]
    } else {
        vec![
            "worktree",
            "add",
            "-q",
            "-b",
            branch.as_str(),
            path_s.as_str(),
            base_rev.as_str(),
        ]
    };
    let out = git_run(&add, Some(&repo)).map_err(WorktreeError::Git)?;
    if !out.ok {
        return Err(WorktreeError::Git(format!(
            "worktree add failed: {}",
            out.stderr.trim()
        )));
    }
    let wt = Worktree {
        path,
        branch,
        base_rev,
        base_repo: repo,
    };
    save_meta(&wt)?;
    Ok(Some(wt))
}

/// Install `snapshot` as the daemon-wide WIP hook used before any pane close.
/// Idempotent (`register_snapshot_hook` is a `OnceLock`). A2 should call this
/// from `Daemon::open` so restart-recovered sessions are covered too.
pub fn register_snapshot_hook() {
    let _ = crate::supervisor::register_snapshot_hook(snapshot);
}

/// Uncommitted changes in this worktree (porcelain, untracked included).
pub fn is_dirty(wt: &Worktree) -> Result<bool, String> {
    let out = git_run(&["status", "--porcelain=v1"], Some(&wt.path))?;
    if !out.ok {
        return Err(format!("git status: {}", out.stderr.trim()));
    }
    Ok(!out.stdout.trim().is_empty())
}

/// Commit WIP before a kill. Returns `Some(sha)` if anything was committed,
/// `None` when the worktree was already clean. Never merges, never force-adds
/// past the worktree boundary (`git add -A` runs with the worktree as cwd).
pub fn snapshot(wt: &Path, session_id: &str, reason: &str) -> Result<Option<String>, String> {
    let status = git_run(&["status", "--porcelain=v1"], Some(wt))?;
    if !status.ok {
        return Err(format!("git status: {}", status.stderr.trim()));
    }
    if status.stdout.trim().is_empty() {
        return Ok(None);
    }
    let add = git_run(&["add", "-A"], Some(wt))?;
    if !add.ok {
        return Err(format!("git add: {}", add.stderr.trim()));
    }
    // Canonical string agreed with A3 (a3_liveness.rs:496): every WIP
    // snapshot, hook or fallback, must contain `notch: WIP snapshot (<reason>)`.
    let message = format!("notch: WIP snapshot ({reason}) — {session_id}");
    let commit = git_run(
        &[
            "-c",
            "user.name=notch",
            "-c",
            "user.email=notch@localhost",
            "commit",
            "-qm",
            message.as_str(),
        ],
        Some(wt),
    )?;
    if !commit.ok {
        return Err(format!("git commit: {}", commit.stderr.trim()));
    }
    let sha = git_text(&["rev-parse", "HEAD"], Some(wt)).map_err(|e| e.to_string())?;
    Ok(Some(sha))
}

/// Would merging `branch_b` into `branch_a` conflict, relative to `base_rev`?
/// `git merge-tree --write-tree` exit 0 = clean, exit 1 = conflict (data), any
/// other exit = error. This is a pure read: neither worktree is modified.
pub fn conflicts(
    repo: &Path,
    base_rev: &str,
    branch_a: &str,
    branch_b: &str,
) -> Result<bool, String> {
    let out = git_run(
        &[
            "merge-tree",
            "--write-tree",
            "--merge-base",
            base_rev,
            branch_a,
            branch_b,
        ],
        Some(repo),
    )?;
    match out.code {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(c) => Err(format!("merge-tree exit {c}: {}", out.stderr.trim())),
        None => Err(format!("merge-tree terminated: {}", out.stderr.trim())),
    }
}

/// Exact attribution for one session: committed files since `base_rev` plus
/// uncommitted porcelain paths. Sorted and deduped.
pub fn sample_files(wt: &Worktree) -> Vec<String> {
    crate::attribution::sample_git_at(&wt.path, Some(&wt.base_rev))
}

/// Remove a CLEAN worktree. A dirty one is refused with its data intact.
/// The branch is intentionally KEPT: snapshot commits remain recoverable and
/// nothing is auto-merged or dropped.
pub fn remove(wt: &Worktree) -> Result<(), WorktreeError> {
    if is_dirty(wt).map_err(WorktreeError::Git)? {
        return Err(WorktreeError::Dirty(format!(
            "{} has uncommitted changes — left in place",
            wt.path.display()
        )));
    }
    let path_s = wt.path.to_string_lossy().to_string();
    let out = git_run(&["worktree", "remove", path_s.as_str()], Some(&wt.base_repo))
        .map_err(WorktreeError::Git)?;
    if !out.ok {
        // A manually deleted worktree leaves a stale admin entry; prune once.
        let _ = git_run(&["worktree", "prune"], Some(&wt.base_repo));
        if wt.path.exists() {
            return Err(WorktreeError::Git(format!(
                "worktree remove: {}",
                out.stderr.trim()
            )));
        }
    }
    if let Some(parent) = wt.path.parent() {
        let _ = std::fs::remove_file(parent.join(META_FILE));
    }
    Ok(())
}

/// Environment passed to `herdr pane split`. `TMPDIR` scopes every temp file
/// a pane (or its LLM judge) creates to this session; `CARGO_TARGET_DIR` is
/// only set under `NOTCH_ISOLATE_BUILD=1` so build caches do not collide.
pub fn pane_env(tmp_root: &Path, id: &str, isolate_build: bool) -> Vec<(String, String)> {
    let root = session_root(tmp_root, id);
    let mut env = vec![
        ("TMPDIR".to_string(), root.to_string_lossy().to_string()),
        ("NOTCH_SESSION_ID".to_string(), id.to_string()),
    ];
    if isolate_build {
        env.push((
            "CARGO_TARGET_DIR".to_string(),
            root.join("build").to_string_lossy().to_string(),
        ));
    }
    env
}

/// Read the `NOTCH_ISOLATE_BUILD` opt-in (anything but empty/`0` is on).
pub fn isolate_build_enabled() -> bool {
    std::env::var("NOTCH_ISOLATE_BUILD")
        .map(|v| {
            let v = v.trim();
            !v.is_empty() && v != "0" && v.to_ascii_lowercase() != "false"
        })
        .unwrap_or(false)
}

/// Total bytes under a directory tree (file lengths; symlinks not followed).
pub fn dir_size(path: &Path) -> u64 {
    fn walk(p: &Path, total: &mut u64) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(&path, total),
                Ok(t) if t.is_file() => {
                    if let Ok(m) = entry.metadata() {
                        *total += m.len();
                    }
                }
                _ => {}
            }
        }
    }
    let mut total = 0;
    walk(path, &mut total);
    total
}

/// Disk budget for the whole scratch root. Reads `NOTCH_MAX_TMP_BYTES`
/// (default 5 GiB). Dead sessions are evicted oldest-first; dirty worktrees
/// and live sessions are never touched. Returns (bytes freed, warnings).
pub fn enforce_budget(
    tmp_root: &Path,
    sessions: &[SessionState],
    max_bytes: u64,
) -> std::io::Result<(u64, Vec<String>)> {
    let mut entries: Vec<(u64, PathBuf, u64, bool)> = Vec::new(); // started, dir, bytes, dead
    for s in sessions {
        let dir = s
            .temp_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| session_root(tmp_root, &s.id));
        let bytes = dir_size(&dir);
        entries.push((s.started_at, dir, bytes, s.status.is_terminal()));
    }
    let total: u64 = entries.iter().map(|e| e.2).sum();
    if total <= max_bytes {
        return Ok((0, Vec::new()));
    }
    let mut candidates: Vec<(u64, PathBuf, u64)> = entries
        .into_iter()
        .filter(|e| e.3)
        .map(|e| (e.0, e.1, e.2))
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut freed = 0u64;
    let mut warnings = Vec::new();
    let mut remaining = total;
    for (started, dir, bytes) in candidates {
        if remaining <= max_bytes {
            break;
        }
        // A dirty worktree is never removed, even under budget pressure.
        let id = dir
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("notch-"))
            .unwrap_or("");
        if let Some(wt) = load(tmp_root, id) {
            match is_dirty(&wt) {
                Ok(true) => {
                    warnings.push(format!(
                        "budget: {id} worktree is dirty — not evicted (started {started})"
                    ));
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    warnings.push(format!("budget: {id} dirty check failed ({e}) — skipped"));
                    continue;
                }
            }
        }
        let _ = crate::cleanup::cleanup_session(tmp_root, id);
        if !dir.exists() {
            freed += bytes;
            remaining = remaining.saturating_sub(bytes);
        }
    }
    Ok((freed, warnings))
}
