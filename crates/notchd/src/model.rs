//! State model. `SessionState`/`SupervisorMeta` persist to `state.json`
//! (hot fields the daemon needs). `AttributionEntry`/`ConflictEntry` persist
//! inside the same file and are also the wire shape for the snapshot.

use crate::status::Status;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub id: String,
    pub sup: String,
    pub status: Status,
    pub title: String,
    pub task: String,
    pub cwd: String,
    #[serde(rename = "paneId", default)]
    pub pane_id: String,
    #[serde(rename = "startedAt", default)]
    pub started_at: u64,
    #[serde(rename = "lastAction", default)]
    pub last_action: Option<String>,
    #[serde(rename = "lastRevision", default)]
    pub last_revision: Option<u64>,
    #[serde(rename = "latencyMs", default)]
    pub latency_ms: Option<u64>,
    #[serde(rename = "failureReason", default)]
    pub failure_reason: Option<String>,

    // Runtime bookkeeping. Not persisted to state.json.
    #[serde(skip)]
    pub failure_count: u32,
    #[serde(skip)]
    pub revision_changed_at_ms: u64,
    /// Files already dirty when this session was first sampled. A file in
    /// this set cannot be attributed to this session (in a shared repo the
    /// dirty file may belong to another agent). Runtime-only; after restart,
    /// the first sample becomes the new baseline.
    #[serde(skip)]
    pub attribution_baseline: Option<std::collections::HashSet<String>>,
}

impl SessionState {
    pub fn task_from_prompt(prompt: &str) -> String {
        single_line(prompt)
    }
}

/// First line, collapsed whitespace, truncated to 78 chars.
pub fn single_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&line, 78)
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SupervisorMeta {
    pub id: String,
    pub number: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttributionEntry {
    pub session: String,
    pub file: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConflictEntry {
    pub file: String,
    pub sessions: Vec<String>,
}

/// Persisted form of the whole control plane.
#[derive(Serialize, Deserialize)]
pub struct StateFile {
    pub version: u32,
    #[serde(default)]
    pub supervisors: Vec<SupervisorMeta>,
    #[serde(default)]
    pub sessions: Vec<SessionState>,
    #[serde(default)]
    pub attribution: Vec<AttributionEntry>,
    #[serde(default)]
    pub conflicts: Vec<ConflictEntry>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Owning supervisor number -> nothing; kept for forward compatibility.
    #[serde(default)]
    pub stuck_after_secs: Option<u64>,
}

impl StateFile {
    pub fn empty() -> Self {
        Self {
            version: 1,
            supervisors: Vec::new(),
            sessions: Vec::new(),
            attribution: Vec::new(),
            conflicts: Vec::new(),
            warnings: Vec::new(),
            stuck_after_secs: None,
        }
    }
}
