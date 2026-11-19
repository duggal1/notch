//! Snapshot — the daemon → Swift/Tauri → UI contract, bridge.md §1.3.
//! CamelCase keys, exactly this shape. Extra fields may be added; none may
//! be removed or renamed in v1.

use crate::model::{AttributionEntry, BusMessageEntry, ConflictEntry, DecisionEntry, EscalationEntry};
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
    // v1.3 optional blocks: present only when data exists.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<DecisionEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub escalations: Vec<EscalationEntry>,
    #[serde(rename = "busMessages", skip_serializing_if = "Vec::is_empty")]
    pub bus_messages: Vec<BusMessageEntry>,
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
    /// Additive v1.2 field: supervisor display title (task head or id).
    pub title: String,
    /// Additive v1.3 field: present (and true) when the supervisor is paused.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
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
    // v1.3 optional control fields (additive; absent when unset).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(rename = "definitionOfDone", skip_serializing_if = "Option::is_none")]
    pub definition_of_done: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iterations: Option<u32>,
    #[serde(rename = "maxIterations", skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
    #[serde(rename = "wallClockDeadline", skip_serializing_if = "Option::is_none")]
    pub wall_clock_deadline: Option<u64>,
    #[serde(rename = "lastSupervisorAction", skip_serializing_if = "Option::is_none")]
    pub last_supervisor_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Newest-first, capped projection of the bus log into the frozen
/// `snapshot.busMessages` array. Duplicates (the same message recorded both by
/// the `message` command and by `bus_drain`'s event log) collapse to one entry.
pub fn cap_bus_messages_newest_first(entries: &[BusMessageEntry], cap: usize) -> Vec<BusMessageEntry> {
    let mut out = entries.to_vec();
    out.sort_by(|a, b| b.at.cmp(&a.at));
    out.dedup_by(|a, b| {
        a.from == b.from && a.to == b.to && a.kind == b.kind && a.preview == b.preview && a.at == b.at
    });
    out.truncate(cap);
    out
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
