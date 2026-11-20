//! Snapshot — the daemon → Swift/Tauri → UI contract, bridge.md §1.3.
//! CamelCase keys, exactly this shape. Extra fields may be added; none may
//! be removed or renamed in v1.

use crate::model::{
    AttributionEntry, BusMessageEntry, ConflictEntry, DecisionEntry, EscalationEntry,
};
use crate::status::Status;
use serde::Serialize;
use serde_json::{json, Value};

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
    #[serde(
        rename = "lastSupervisorAction",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_supervisor_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Newest-first, capped projection of the bus log into the frozen
/// `snapshot.busMessages` array. Duplicates (the same message recorded both by
/// the `message` command and by `bus_drain`'s event log) collapse to one entry.
pub fn cap_bus_messages_newest_first(
    entries: &[BusMessageEntry],
    cap: usize,
) -> Vec<BusMessageEntry> {
    let mut out = entries.to_vec();
    out.sort_by_key(|e| std::cmp::Reverse(e.at));
    out.dedup_by(|a, b| {
        a.from == b.from
            && a.to == b.to
            && a.kind == b.kind
            && a.preview == b.preview
            && a.at == b.at
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

/// One honest line each fleet unit gets. Derived only from the four legal
/// statuses plus the escalation/failure reason — never a fifth status value.
fn unit_line(status: &str, reason: &str, kind: &str) -> (&'static str, String) {
    match status {
        "working" => ("Working", "An agent is on this session right now.".to_string()),
        "failed" if kind == "blocked" || reason == "blocked_waiting" => (
            "Stopped",
            "The agent stopped on a request only you can answer. Nothing moves until it is answered.".to_string(),
        ),
        "failed" if kind == "stuck" || reason == "stuck" => (
            "Stopped",
            "The watchdog stopped this session and kept its work. Send it a fresh prompt to continue.".to_string(),
        ),
        "failed" => ("Stopped", "This session failed. Its last action is kept on the row.".to_string()),
        "done" => ("Done", "The session finished and its work was verified.".to_string()),
        _ => ("Idle", "No agent is on this session, and none is waiting to start.".to_string()),
    }
}

fn fleet_str<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn fleet_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// The snapshot vocabulary is exactly the four §1.1 statuses (unlike herdr's
/// raw `blocked`/`unknown`), so parse it directly into `Status`.
fn fleet_status(s: &str) -> Status {
    match s {
        "working" => Status::Working,
        "failed" => Status::Failed,
        "done" => Status::Done,
        _ => Status::Idle,
    }
}

/// The ONE fleet projection (C3). Pure over a §1.3 snapshot plus the REAL
/// ledger seq/epoch supplied by the caller from the C2 accessors. `topStatus`
/// comes from [`rollup`] — the same truth the daemon uses for
/// `orchestrator.status` — so the Tauri event and the WS frame never disagree.
/// Every consumer (Tauri command, Tauri fleet event, WS fleet frame) renders
/// this object; no caller re-derives the fleet.
pub fn fleet_view(snap: &Value, seq: u64, epoch: u64) -> Value {
    let at = snap.get("generatedAt").cloned().unwrap_or(Value::Null);
    let sessions = snap
        .get("sessions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let supervisors = snap
        .get("supervisors")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let escalations = snap
        .get("escalations")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let decisions = snap
        .get("decisions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let bus = snap
        .get("busMessages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let top = rollup(
        sessions
            .iter()
            .map(|s| fleet_status(fleet_str(s, "status"))),
    )
    .as_str();

    let mut units: Vec<Value> = Vec::with_capacity(sessions.len() + supervisors.len() + 1);
    units.push(json!({
        "id": "ORCH", "kind": "orchestrator", "status": top, "topStatus": top,
        "paneId": "", "workspace": "", "lastAction": "", "revision": 0u64,
    }));

    for sup in supervisors {
        units.push(json!({
            "id": fleet_str(sup, "id"), "kind": "supervisor", "status": fleet_str(sup, "status"),
            "topStatus": top, "title": fleet_str(sup, "title"),
            "paused": sup.get("paused").cloned().unwrap_or(Value::Null),
            "paneId": "", "workspace": "", "lastAction": fleet_str(sup, "title"), "revision": 0u64,
        }));
    }

    let mut working = 0usize;
    let mut idle = 0usize;
    let mut stopped = 0usize;
    let mut done = 0usize;
    for s in sessions {
        let id = fleet_str(s, "id");
        let status = fleet_str(s, "status");
        let reason = fleet_str(s, "failureReason");
        let kind = escalations
            .iter()
            .find(|e| fleet_str(e, "session") == id)
            .map(|e| fleet_str(e, "kind"))
            .unwrap_or("");
        let (word, detail) = unit_line(status, reason, kind);
        match status {
            "working" => working += 1,
            "failed" => stopped += 1,
            "done" => done += 1,
            _ => idle += 1,
        }
        units.push(json!({
            "id": id, "kind": "session", "sup": fleet_str(s, "sup"), "status": status,
            "topStatus": top, "word": word, "detail": detail, "title": fleet_str(s, "title"),
            "paneId": fleet_str(s, "paneId"), "workspace": fleet_str(s, "cwd"),
            "lastAction": fleet_str(s, "lastAction"), "revision": fleet_u64(s, "lastRevision"),
            "escalation": kind,
        }));
    }

    let holds = supervisors
        .iter()
        .filter(|s| s.get("paused").and_then(Value::as_bool).unwrap_or(false))
        .count();
    json!({
        "source": "snapshot",
        "generatedAt": at,
        "topStatus": top,
        "units": units,
        "groups": { "working": working, "idle": idle, "stopped": stopped, "done": done },
        "storm": {
            "holds": holds,
            "guard": if holds > 0 { "holding" } else { "open" },
            "starts": decisions.iter().filter(|d| fleet_str(d, "action") == "reprompt").count(),
        },
        "ledger": {
            "seq": seq,
            "fence": format!("{epoch:016x}{seq:016x}"),
            "epoch": epoch,
            "decisions": decisions.len(),
            "messages": bus.len(),
        },
    })
}
