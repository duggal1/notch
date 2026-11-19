//! Supervisor failure policy. Deterministic — no model calls.
//!
//! On a session failure (status `failed` after a herdr-reported `blocked`):
//! 1. If this is the first failure, the supervisor re-prompts the session
//!    exactly once, appending the tail of pane output.
//! 2. If it fails again, the session is marked `failed` and the failure is
//!    escalated (event log + snapshot warning).
//!
//! `--opencode-decider` is accepted by the CLI but NOT implemented; when
//! passed, the deterministic policy is used and a warning is logged.

pub enum FailureAction {
    /// Re-prompt once. `tail` is the pane output to append.
    Reprompt { tail: String },
    /// Budget exhausted. Mark failed + escalate.
    Escalate,
}

/// Decide what a supervisor does with a newly-failed session.
///
/// `failure_count` is the number of failures already observed for this
/// session (0 on first failure).
pub fn decide_on_failure(failure_count: u32, tail: Option<String>) -> FailureAction {
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
