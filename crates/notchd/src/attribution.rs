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

/// Run git in `cwd`. Returns relative file paths, or an empty vec on any
/// failure — never an error. A hung git (e.g. fsmonitor/IPC trouble on a
/// huge worktree) is killed after 1 second and counts as no signal.
pub fn sample_git(cwd: &Path) -> Vec<String> {
    let child = std::process::Command::new("git")
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false", "-C"])
        .arg(cwd)
        .args(["status", "--porcelain=v1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    // Drain stdout on a helper thread so a huge porcelain listing can't
    // block the child on a full pipe.
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            return Vec::new();
        }
    };
    let drain = std::thread::spawn(move || {
        let mut buf = Vec::new();
        use std::io::Read;
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                let out = drain.join().unwrap_or_default();
                return parse_porcelain(&String::from_utf8_lossy(&out));
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Vec::new();
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return Vec::new(),
        }
    }
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
