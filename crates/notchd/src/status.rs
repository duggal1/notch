//! Status vocabulary. Exactly the four DESIGN.md / bridge.md §1.1 states.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Working,
    Idle,
    Failed,
    Done,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Idle => "idle",
            Status::Failed => "failed",
            Status::Done => "done",
        }
    }

    /// bridge.md §1.1 mapping. `blocked` maps to `failed` with reason
    /// `blocked_waiting`; `unknown` (and anything unrecognized) maps to idle.
    pub fn from_herdr(s: &str) -> (Status, Option<String>) {
        match s {
            "working" => (Status::Working, None),
            "idle" => (Status::Idle, None),
            "done" => (Status::Done, None),
            "blocked" => (Status::Failed, Some("blocked_waiting".to_string())),
            _ => (Status::Idle, None),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Failed | Status::Done)
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
