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
