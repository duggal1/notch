//! Snapshot — the daemon → Swift/Tauri → UI contract, bridge.md §1.3.
//! CamelCase keys, exactly this shape. Extra fields may be added; none may
//! be removed or renamed in v1.

use crate::model::{AttributionEntry, ConflictEntry};
use crate::status::Status;
use serde::Serialize;

#[derive(Serialize)]
pub struct Snapshot {
    pub version: u32,
    #[serde(rename = "generatedAt")]
    pub generated_at: u64,
    pub orchestrator: OrchestratorSnapshot,
    pub supervisors: Vec<SupervisorSnapshot>,
    pub sessions: Vec<SessionSnapshot>,
    pub eps: f64,
    pub conflicts: Vec<ConflictEntry>,
    pub attribution: Vec<AttributionEntry>,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct OrchestratorSnapshot {
    pub id: String,
    pub status: Status,
}

#[derive(Serialize)]
pub struct SupervisorSnapshot {
    pub id: String,
    pub status: Status,
    pub working: u32,
    pub idle: u32,
    pub failed: u32,
    pub done: u32,
}

#[derive(Serialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub sup: String,
    pub status: Status,
    pub title: String,
    pub task: String,
    pub cwd: String,
    #[serde(rename = "paneId")]
    pub pane_id: String,
    #[serde(rename = "startedAt")]
    pub started_at: u64,
    #[serde(rename = "lastAction")]
    pub last_action: Option<String>,
    #[serde(rename = "lastRevision")]
    pub last_revision: Option<u64>,
    #[serde(rename = "latencyMs")]
    pub latency_ms: Option<u64>,
    #[serde(rename = "failureReason")]
    pub failure_reason: Option<String>,
}

/// Rollup status for a supervisor / the orchestrator.
/// working if anything is working; else failed if anything is failed;
/// else done if everything is done (and at least one exists); else idle.
pub fn rollup(statuses: impl Iterator<Item = Status>) -> Status {
    let mut any = false;
    let mut all_done = true;
    let mut any_failed = false;
    let mut any_working = false;
    for s in statuses {
        any = true;
        match s {
            Status::Working => {
                any_working = true;
                all_done = false;
            }
            Status::Idle => all_done = false,
            Status::Failed => any_failed = true,
            Status::Done => {}
        }
    }
    if !any {
        return Status::Idle;
    }
    if any_working {
        Status::Working
    } else if any_failed {
        Status::Failed
    } else if all_done {
        Status::Done
    } else {
        Status::Idle
    }
}
