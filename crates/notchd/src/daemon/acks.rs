//! Ack types returned by the daemon commands. Shared with the socket
//! layer (which flattens them into NDJSON `ack` frames) and with the Tauri
//! shell. The JSON shape is the wire contract — do not rename fields.

use serde::Serialize;

/// `run_plan` (bridge.md §2). `spawned` lists the allocated supervisor ids,
/// in plan order; `error` is set only when the plan was rejected.
#[derive(Serialize)]
pub struct RunPlanAck {
    pub ok: bool,
    pub spawned: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `kill` (bridge.md §2). The session is marked `failed` with `reason`
/// and its temp dir removed; `error` carries the failure when the pane
/// close itself failed (the kill still stands).
#[derive(Serialize)]
pub struct KillAck {
    pub ok: bool,
    pub id: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Generic ok-ack for `reprompt`, `cleanup`, `pause_supervisor`,
/// `resume_supervisor` and `focus`. `removed` is `cleanup` only.
#[derive(Serialize)]
pub struct OkAck {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `status` (bridge.md §2). `event` is always `"status"`; `sessions` is
/// the count of non-terminal sessions; `uptime_s` is seconds since open.
#[derive(Serialize)]
pub struct StatusInfo {
    pub event: &'static str,
    pub pid: u32,
    pub sessions: u64,
    #[serde(rename = "uptimeS")]
    pub uptime_s: u64,
}

/// `spawn` (bridge.md §2). `id` is set when a session was allocated.
#[derive(Serialize)]
pub struct SpawnAck {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `submit_task` (bridge.md v1.2 §2/§3). Deterministic; no model calls.
/// On validation failure, nothing is allocated and the ack carries only
/// `ok:false` + `error`. On spawn failure the session exists and is marked
/// `failed` with `spawn_failed`; both ids are returned.
#[derive(Serialize)]
pub struct SubmitTaskAck {
    pub ok: bool,
    #[serde(rename = "supervisorId", skip_serializing_if = "Option::is_none")]
    pub supervisor_id: Option<String>,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `kill_switch` (bridge.md v1.3 §2/§3). One click: every managed pane is
/// closed, its session marked failed with reason `kill_switch`, cleanup run.
#[derive(Serialize)]
pub struct KillSwitchAck {
    pub ok: bool,
    pub closed: u64,
}

/// `message` (bridge.md v1.3 §8 A). Bus message delivered to the target
/// session as a normal prompt.
#[derive(Serialize)]
pub struct MessageAck {
    pub ok: bool,
    #[serde(rename = "messageId", skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `list_models` (bridge.md v1.3 §2). `opencode models`, cached 5 minutes.
#[derive(Serialize)]
pub struct ModelsAck {
    pub ok: bool,
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
