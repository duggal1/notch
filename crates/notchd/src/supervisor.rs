//! Supervisor failure policy. Deterministic — no model calls.
//!
//! On a session failure (status `failed` after a herdr-reported reason):
//! 1. `blocked_waiting` means the agent is waiting on a human (a permission
//!    prompt, a confirmation, a missing credential). It is NOT a failure to
//!    retry: the pane and temp dir are kept, `needs_you` is escalated, and
//!    nothing is ever typed into the permission prompt.
//! 2. Other failures: first failure re-prompts exactly once, appending the
//!    tail of pane output; a second failure marks the session failed and
//!    escalates.
//!
//! `--opencode-decider` is accepted by the CLI but NOT implemented; when
//! passed, the deterministic policy is used and a warning is logged.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

type SnapshotFn = fn(&Path, &str, &str) -> Result<Option<String>, String>;

static SNAPSHOT_HOOK: OnceLock<SnapshotFn> = OnceLock::new();

/// A5 registers `worktree::snapshot` here at daemon startup. Until then the
/// git fallback below still snapshots WIP before a stuck pane is closed.
pub fn register_snapshot_hook(f: SnapshotFn) -> Result<(), SnapshotFn> {
    SNAPSHOT_HOOK.set(f)
}

/// Commit WIP before any kill. `None` means the worktree was already clean;
/// `Err` vetoes the pane close so uncommitted work is never lost.
pub fn snapshot_worktree(wt: &Path, id: &str, reason: &str) -> Result<Option<String>, String> {
    if let Some(hook) = SNAPSHOT_HOOK.get() {
        return hook(wt, id, reason);
    }
    let path = wt.to_string_lossy().to_string();
    let run = |args: &[&str]| -> Result<std::process::Output, String> {
        Command::new("git")
            .args(args)
            .output()
            .map_err(|e| format!("git {}: {e}", args.get(1).copied().unwrap_or("")))
    };
    let status = run(&["-C", &path, "status", "--porcelain"])?;
    if !status.status.success() {
        return Err(format!(
            "git status failed: {}",
            crate::model::truncate(String::from_utf8_lossy(&status.stderr).trim(), 160)
        ));
    }
    if status.stdout.is_empty() {
        return Ok(None);
    }
    let add = run(&["-C", &path, "add", "-A"])?;
    if !add.status.success() {
        return Err(format!(
            "git add failed: {}",
            crate::model::truncate(String::from_utf8_lossy(&add.stderr).trim(), 160)
        ));
    }
    let message = format!("notch WIP snapshot: {id} ({reason})");
    let commit = Command::new("git")
        .args(["-C", &path, "commit", "-m", &message])
        .output()
        .map_err(|e| format!("git commit: {e}"))?;
    if !commit.status.success() {
        return Err(format!(
            "git commit failed: {}",
            crate::model::truncate(String::from_utf8_lossy(&commit.stderr).trim(), 160)
        ));
    }
    let head = run(&["-C", &path, "rev-parse", "HEAD"])?;
    Ok(Some(
        String::from_utf8_lossy(&head.stdout).trim().to_string(),
    ))
}

#[derive(Debug)]
pub enum FailureAction {
    /// Re-prompt once. `tail` is the pane output to append.
    Reprompt { tail: String },
    /// Budget exhausted. Mark failed + escalate.
    Escalate,
    /// Waiting on a human. Keep the pane/temp, never prompt the permission
    /// dialog, escalate as `needs_you`.
    NeedsHuman { reason: String },
}

/// Decide what a supervisor does with a newly-failed session.
///
/// `failure_count` is the number of failures already observed for this
/// session (0 on first failure).
pub fn decide_on_failure(failure_count: u32, tail: Option<String>) -> FailureAction {
    decide_on_failure_with_reason(None, failure_count, tail)
}

/// Reason-aware variant used by the poll path. `reason` is the herdr mapping
/// reason (e.g. `blocked_waiting`); only that exact value means "human gate".
pub fn decide_on_failure_with_reason(
    reason: Option<&str>,
    failure_count: u32,
    tail: Option<String>,
) -> FailureAction {
    if reason == Some("blocked_waiting") {
        return FailureAction::NeedsHuman {
            reason: reason.unwrap_or_default().to_string(),
        };
    }
    if failure_count == 0 {
        FailureAction::Reprompt {
            tail: tail.unwrap_or_else(|| "<pane output unavailable>".to_string()),
        }
    } else {
        FailureAction::Escalate
    }
}

/// Build the re-prompt text: original task head + tail of the pane output.
pub fn reprompt_text(task_head: &str, tail: &str) -> String {
    format!(
        "{task_head}\n\n---\nPrevious attempt failed. Tail of pane output:\n{tail}\nDiagnose and continue from here."
    )
}
