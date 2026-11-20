//! Tauri commands — thin wrappers over `notchd::Daemon`. Names are
//! snake_case exactly as the wire contract (bridge.md §2) and the webview's
//! `invoke(type, args)` expect (ui/src/lib/bridge.ts): `snapshot, kill,
//! reprompt, logs, focus, cleanup, spawn, run_plan, set_frame, haptic`.
//!
//! All payload fields are camelCase on the wire (notchd's acks already are).
//! Errors are `Result<T, String>`. Commands that shell out to herdr are
//! `async` (never block the main thread); `set_frame` and `haptic` are sync
//! (AppKit / main-thread friendly, no blocking IO).

use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State};

use notchd::daemon::{
    KillAck, KillSwitchAck, MessageAck, ModelsAck, OkAck, RunPlanAck, SpawnAck, SubmitTaskAck,
};
use notchd::Daemon;

use crate::frame::{frame_for_mode, Mode};
use crate::macos;
use crate::symbols;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameRectOut {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub mode: String,
}

/// Payload for the `logs` EVENT (ui/src/lib/bridge.ts listens for it).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogsPayload {
    pub id: String,
    pub lines: Vec<String>,
}

#[tauri::command]
pub async fn snapshot(daemon: State<'_, Arc<Daemon>>) -> Result<serde_json::Value, String> {
    serde_json::to_value(daemon.snapshot()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kill(
    id: String,
    reason: Option<String>,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<KillAck, String> {
    let ack = daemon.kill(&id, reason)?;
    if ack.ok {
        Ok(ack)
    } else {
        Err(ack.error.clone().unwrap_or_else(|| "kill failed".into()))
    }
}

#[tauri::command]
pub async fn reprompt(
    id: String,
    text: String,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<OkAck, String> {
    daemon.reprompt(&id, &text)
}

#[tauri::command]
pub async fn logs(
    id: String,
    tail: Option<u32>,
    app: AppHandle,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<LogsPayload, String> {
    let lines = daemon.logs(&id, tail.map(|t| t as usize))?;
    let payload = LogsPayload { id, lines };
    // The frontend consumes logs via the `logs` EVENT (bridge.ts: listen('logs')),
    // so emit it here in addition to returning the value.
    let _ = app.emit("logs", &payload);
    Ok(payload)
}

#[tauri::command]
pub async fn focus(id: String, daemon: State<'_, Arc<Daemon>>) -> Result<OkAck, String> {
    // Focus moves herdr's own focus to the pane; never opens a window.
    daemon.focus(&id)
}

#[tauri::command]
pub async fn cleanup(id: Option<String>, daemon: State<'_, Arc<Daemon>>) -> Result<u64, String> {
    let ack = daemon.cleanup(id.as_deref());
    Ok(ack.removed.unwrap_or(0))
}

#[tauri::command]
pub async fn spawn(
    supervisor: String,
    prompt: String,
    cwd: String,
    title: String,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<SpawnAck, String> {
    if supervisor.trim().is_empty()
        || prompt.trim().is_empty()
        || cwd.trim().is_empty()
        || title.trim().is_empty()
    {
        return Err("supervisor, prompt, cwd and title are required".into());
    }
    let ack = daemon.spawn(&supervisor, &prompt, &cwd, &title);
    if ack.ok {
        Ok(ack)
    } else {
        Err(ack.error.clone().unwrap_or_else(|| "spawn failed".into()))
    }
}

#[tauri::command]
pub async fn run_plan(path: String, daemon: State<'_, Arc<Daemon>>) -> Result<RunPlanAck, String> {
    let ack = daemon.run_plan(Path::new(&path));
    if ack.ok {
        Ok(ack)
    } else {
        Err(ack
            .error
            .clone()
            .unwrap_or_else(|| "run_plan failed".into()))
    }
}

/// bridge.md §3 v1.3: allocate `SV-<n>` (title = provided title or task
/// head ≤ 48 chars on a word boundary) + `S<n>-OC01`, spawn through the same
/// path as a plan. With `supervisor_id` set, routes into that existing
/// supervisor instead of allocating a new one. `definition_of_done` is stored
/// on the session; `model` is passed to the harness at spawn. Validation
/// failure allocates nothing.
#[tauri::command]
pub async fn submit_task(
    task: String,
    cwd: Option<String>,
    title: Option<String>,
    definition_of_done: Option<String>,
    model: Option<String>,
    supervisor_id: Option<String>,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<SubmitTaskAck, String> {
    if task.trim().is_empty() {
        return Err("task must not be empty".into());
    }
    let ack = daemon.submit_task(task, cwd, title, definition_of_done, model, supervisor_id);
    if ack.ok {
        Ok(ack)
    } else {
        Err(ack
            .error
            .clone()
            .unwrap_or_else(|| "submit_task failed".into()))
    }
}

/// bridge.md §3 v1.3: agent-to-agent message over the bus. The only ingress.
/// Delivery = `herdr agent prompt <target> <body>` + attribution in the bus.
///
/// bridge.md §3 v1.3: agent-to-agent message over the bus — the only
/// ingress. Validated by the engine (type, body, known sessions); delivered
/// verbatim to the target as a normal prompt (`herdr agent prompt <to>
/// <body>`) and logged to the comms view.
#[tauri::command]
pub async fn message(
    from: String,
    to: String,
    r#type: String,
    body: String,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<MessageAck, String> {
    if from.trim().is_empty() || to.trim().is_empty() {
        return Err("message requires from and to".into());
    }
    let ack = daemon.message(&from, &to, &r#type, &body);
    if ack.ok {
        Ok(ack)
    } else {
        Err(ack.error.unwrap_or_else(|| "message failed".into()))
    }
}

/// bridge.md §3 v1.3: stop the supervisor's re-prompting/new-work decisions.
/// Live sessions keep running.
#[tauri::command]
pub async fn pause_supervisor(id: String, daemon: State<'_, Arc<Daemon>>) -> Result<OkAck, String> {
    daemon.pause_supervisor(&id)
}

/// bridge.md §3 v1.3: inverse of pause_supervisor.
#[tauri::command]
pub async fn resume_supervisor(
    id: String,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<OkAck, String> {
    daemon.resume_supervisor(&id)
}

/// Shared implementation for the `kill_switch` command and the tray's
/// "Kill all sessions" item. bridge.md §3 v1.3: one click; every managed
/// pane is closed, its session marked failed with reason `kill_switch`,
/// cleanup runs. Infallible by design.
pub(crate) fn kill_switch_impl(daemon: &Daemon) -> KillSwitchAck {
    daemon.kill_switch()
}

#[tauri::command]
pub async fn kill_switch(daemon: State<'_, Arc<Daemon>>) -> Result<KillSwitchAck, String> {
    Ok(kill_switch_impl(&daemon))
}

/// bridge.md §2/§3 v1.3: `opencode models`, read-only, cached 5 minutes.
/// On failure: `{ models: [], error }` — the UI falls back to its default
/// entry. Never errors out.
#[tauri::command]
pub async fn list_models(daemon: State<'_, Arc<Daemon>>) -> Result<ModelsAck, String> {
    Ok(daemon.list_models())
}

/// bridge.md v1.3: on-device SF Symbol rasterization → data URL. 2× raster,
/// in-memory cached. Sync command — AppKit requires the main thread.
#[tauri::command]
pub fn symbol(name: String, pt: Option<f64>, weight: Option<String>) -> Result<String, String> {
    symbols::rasterize_symbol(
        &name,
        pt.unwrap_or(16.0),
        weight.as_deref().unwrap_or("semibold"),
    )
}

/// bridge.md §7 v1.3: the second surface — the standard macOS dashboard
/// window (regular decorations, titled, resizable, min 880×600, 1120×760
/// default, normal level, dock-visible). Created hidden at startup;
/// this command shows + unminimizes + focuses it.
/// Shared by the command and the tray's "Open dashboard" item.
pub(crate) fn show_dashboard(app: &AppHandle) -> Result<serde_json::Value, String> {
    if let Some(window) = app.get_webview_window("dashboard") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return Ok(serde_json::json!({ "ok": true, "focused": true }));
    }

    // The window is destroyed on close — recreate it.
    use tauri::webview::WebviewWindowBuilder;
    let m = macos::cached_measurements();
    let dash = crate::frame::dashboard_frame(m.screen_width, m.screen_height);
    WebviewWindowBuilder::new(
        app,
        "dashboard",
        tauri::WebviewUrl::App("index.html?surface=dashboard".into()),
    )
    .title("Notch")
    .inner_size(dash.width, dash.height)
    .min_inner_size(
        crate::frame::DASHBOARD_MIN_WIDTH,
        crate::frame::DASHBOARD_MIN_HEIGHT,
    )
    .position(dash.x, dash.y)
    .decorations(true)
    .resizable(true)
    .always_on_top(false)
    .skip_taskbar(false)
    .visible(true)
    .build()
    .map_err(|e| format!("recreate dashboard window: {e}"))?;
    let window = app
        .get_webview_window("dashboard")
        .ok_or_else(|| "dashboard window vanished after creation".to_string())?;
    let _ = window.set_focus();
    Ok(serde_json::json!({ "ok": true, "recreated": true }))
}

#[tauri::command]
pub fn open_dashboard(app: AppHandle) -> Result<serde_json::Value, String> {
    show_dashboard(&app)
}

/// Resize/reposition the popover INSTANTLY. The webview's clip-path morph owns
/// the smoothness; this only moves the frame (DESIGN.md §5.1: resize once per
/// mode change, never per frame).
///
/// Cursor events stay enabled during and after the morph: the popover may be
/// clicked mid-flight and, in `rest`, the wings must remain hoverable for the
/// intent system. The transient oversized click target is ≤ 460 ms and by
/// design (`peek` grows downward from the bar, never over neighbours' work).
#[tauri::command]
pub fn set_frame(mode: String, app: AppHandle) -> Result<FrameRectOut, String> {
    let mode = Mode::parse(&mode)
        .ok_or_else(|| format!("unknown mode {mode:?} (expected rest|peek|open)"))?;
    let m = macos::cached_measurements();
    let rect = frame_for_mode(mode, m.screen_width, m.notch);

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is gone".to_string())?;
    window
        .set_size(LogicalSize::new(rect.width, rect.height))
        .map_err(|e| format!("set_size: {e}"))?;
    window
        .set_position(LogicalPosition::new(rect.x, rect.y))
        .map_err(|e| format!("set_position: {e}"))?;

    Ok(FrameRectOut {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        mode: mode.as_str().to_string(),
    })
}

/// The webview sends `haptic` on status transitions (bridge.ts). Maps onto
/// NSHapticFeedbackManager: alignment (open), levelChange (failure).
#[tauri::command]
pub fn haptic(kind: String) -> Result<(), String> {
    macos::perform_haptic(&kind);
    Ok(())
}

// ===========================================================================
// Wave 2 capability surface (W2-5 wiring).
//
// These handlers are the app-side contract for the four wave-2 engine
// capabilities: harness registry (W2-1), ledger/fencing (W2-2), approvals +
// A2A bridge (W2-3) and supervisor storm controls (W2-4). The engine modules
// are owned by those agents and land in parallel, so every handler has ONE
// marked seam: today it projects the answer from the frozen §1.3 snapshot
// and reports `source: "snapshot-projection"`; when the engine exposes its
// method, replace only that seam. No wire shape changes for the webview.
// ===========================================================================

/// Declared capability of a harness. `supported: false` is a DECLINE, not a
/// missing method: optional reads refuse before any side effect (stolen from
/// agentbox `main/harnesses.mts`, structure only).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilityOut {
    pub name: String,
    pub supported: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessOut {
    pub id: String,
    pub label: String,
    pub capabilities: Vec<HarnessCapabilityOut>,
}

/// An approval card derived from the unresolved escalations. `signed: false`
/// is honest: the ed25519 answer channel is W2-3's work, not the app's.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOut {
    pub id: String,
    pub session: String,
    pub supervisor: String,
    pub kind: String,
    pub reason: String,
    pub at: u64,
    pub digest: String,
    pub signed: bool,
}

const PROJECTION: &str = "snapshot-projection";
const OPTIONAL_READS: &[&str] = &["signIn", "usage", "profiles", "remoteControl"];

/// FNV-1a 64 over the answerable fields — a stable, in-process card digest.
/// No crypto claim is made; the signed channel is the engine's job (W2-3).
fn card_digest(id: &str, session: &str, kind: &str, reason: &str, at: u64) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{id}|{session}|{kind}|{reason}|{at}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn u64_at(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}
fn array_at<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// The one honest line each fleet unit gets. Derived only from the four legal
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

/// W2-1 seam. Declared registry shape: what the single harness can actually
/// do, with optional reads declining. Replace with `notchd::harness_registry`.
fn declare_harnesses() -> Vec<HarnessOut> {
    let operations = [
        "spawn", "read", "prompt", "kill", "focus", "status",
        "signIn", "usage", "profiles", "remoteControl",
    ];
    vec![HarnessOut {
        id: "opencode".into(),
        label: "OpenCode".into(),
        capabilities: operations
            .iter()
            .map(|name| HarnessCapabilityOut {
                name: (*name).to_string(),
                supported: !OPTIONAL_READS.contains(name),
            })
            .collect(),
    }]
}

/// W2-2/W2-3 seam: approval cards from unresolved escalations + blocked
/// sessions. Oldest first, bounded, one card per session.
fn approvals_from_snapshot(snap: &Value) -> Vec<ApprovalOut> {
    let sessions = array_at(snap, "sessions");
    let mut out: Vec<ApprovalOut> = Vec::new();
    for e in array_at(snap, "escalations") {
        let session = str_at(e, "session").to_string();
        let kind = str_at(e, "kind").to_string();
        if session.is_empty() || (kind != "blocked" && kind != "stuck") {
            continue;
        }
        let id = str_at(e, "id").to_string();
        let reason = str_at(e, "reason").to_string();
        let at = u64_at(e, "at");
        let supervisor = str_at(e, "supervisor").to_string();
        out.push(ApprovalOut {
            digest: card_digest(&id, &session, &kind, &reason, at),
            id, session, supervisor, kind, reason, at, signed: false,
        });
    }
    for s in sessions {
        if str_at(s, "failureReason") != "blocked_waiting" {
            continue;
        }
        let session = str_at(s, "id").to_string();
        if out.iter().any(|a| a.session == session) {
            continue;
        }
        let id = format!("blocked-{session}");
        let reason = "The agent is waiting on input only you can give.".to_string();
        let supervisor = str_at(s, "sup").to_string();
        let at = u64_at(s, "startedAt");
        out.push(ApprovalOut {
            digest: card_digest(&id, &session, "blocked", &reason, at),
            id, session, supervisor, kind: "blocked".into(), reason, at, signed: false,
        });
    }
    out.sort_by_key(|a| a.at);
    out.truncate(12);
    out
}

/// W2-2/W2-4 seam: the fleet projection — one unit per session, honest words,
/// a ledger sequence/fence and the storm guard reading. All from the snapshot.
fn fleet_from_snapshot(snap: &Value) -> Value {
    let sessions = array_at(snap, "sessions");
    let supervisors = array_at(snap, "supervisors");
    let escalations = array_at(snap, "escalations");
    let decisions = array_at(snap, "decisions");
    let bus = array_at(snap, "busMessages");

    let escalated: Vec<&Value> = escalations
        .iter()
        .filter(|e| !str_at(e, "session").is_empty())
        .collect();
    let mut units = Vec::with_capacity(sessions.len());
    let mut working = 0usize;
    let mut idle = 0usize;
    let mut stopped = 0usize;
    let mut done = 0usize;
    for s in sessions {
        let id = str_at(s, "id");
        if id.is_empty() { continue; }
        let status = str_at(s, "status");
        let reason = str_at(s, "failureReason");
        let esc = escalated.iter().find(|e| str_at(e, "session") == id).copied();
        let kind = esc.map(|e| str_at(e, "kind")).unwrap_or("");
        let (word, detail) = unit_line(status, reason, kind);
        match status {
            "working" => working += 1,
            "failed" => stopped += 1,
            "done" => done += 1,
            _ => idle += 1,
        }
        units.push(json!({
            "id": id,
            "kind": "session",
            "sup": str_at(s, "sup"),
            "status": status,
            "word": word,
            "detail": detail,
            "title": str_at(s, "title"),
            "paneId": str_at(s, "paneId"),
            "workspace": str_at(s, "cwd"),
            "lastAction": str_at(s, "lastAction"),
            "revision": u64_at(s, "lastRevision"),
            "escalation": kind,
        }));
    }
    let top = if stopped > 0 { "failed" } else if working > 0 { "working" }
        else if done > 0 && idle == 0 && done == sessions.len() { "done" } else { "idle" };
    let holds = supervisors.iter().filter(|s| {
        s.get("paused").and_then(Value::as_bool).unwrap_or(false)
    }).count();
    let seq = decisions.len() + bus.len();
    let fence_src = format!("{}|{}|{}", u64_at(snap, "generatedAt"), decisions.len(), bus.len());
    let mut fh: u64 = 0xcbf29ce484222325;
    for b in fence_src.bytes() { fh ^= b as u64; fh = fh.wrapping_mul(0x100000001b3); }
    json!({
        "source": PROJECTION,
        "generatedAt": u64_at(snap, "generatedAt"),
        "topStatus": top,
        "units": units,
        "groups": { "working": working, "idle": idle, "stopped": stopped, "done": done },
        "storm": {
            "holds": holds,
            "guard": if holds > 0 { "holding" } else { "open" },
            "starts": decisions.iter().filter(|d| str_at(d, "action") == "reprompt").count(),
        },
        "ledger": {
            "seq": seq,
            "fence": format!("{fh:016x}"),
            "epoch": u64_at(snap, "version"),
            "decisions": decisions.len(),
            "messages": bus.len(),
        },
    })
}

/// `fleet_state` — one call returns the whole derived fleet surface.
#[tauri::command]
pub async fn fleet_state(daemon: State<'_, Arc<Daemon>>) -> Result<Value, String> {
    let snap = daemon.snapshot();
    let value = serde_json::to_value(&snap).map_err(|e| e.to_string())?;
    Ok(fleet_from_snapshot(&value))
}

/// `list_harnesses` — declared capabilities, declines before side effects.
#[tauri::command]
pub async fn list_harnesses() -> Result<Value, String> {
    Ok(json!({ "source": PROJECTION, "harnesses": declare_harnesses() }))
}

/// `list_approvals` — one decision per card, oldest first, digest bound to the
/// card actually shown. Never an engine write.
#[tauri::command]
pub async fn list_approvals(daemon: State<'_, Arc<Daemon>>) -> Result<Value, String> {
    let snap = daemon.snapshot();
    let value = serde_json::to_value(&snap).map_err(|e| e.to_string())?;
    Ok(json!({ "source": PROJECTION, "approvals": approvals_from_snapshot(&value) }))
}

/// `answer_approval` — today routes an answer through the existing reprompt /
/// bus commands so the control is real, and is explicitly `signed: false`.
/// W2-3 replaces the route with the signed answer channel (same wire shape).
#[tauri::command]
pub async fn answer_approval(
    id: String,
    allow: bool,
    note: Option<String>,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<Value, String> {
    let snap = daemon.snapshot();
    let value = serde_json::to_value(&snap).map_err(|e| e.to_string())?;
    let approval = approvals_from_snapshot(&value)
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| format!("unknown approval {id:?}"))?;
    let note = note.unwrap_or_default();
    let text = if allow {
        format!("approved: {}", if note.is_empty() { "continue" } else { &note })
    } else {
        format!("needs you: {}", if note.is_empty() { "answer this request" } else { &note })
    };
    let outcome: Result<Value, String> = if allow {
        daemon.reprompt(&approval.session, &text).map(|_| json!({}))
    } else {
        let ack = daemon.message("ORCH", &approval.session, "escalate", &text);
        if ack.ok {
            Ok(json!({}))
        } else {
            Err(ack.error.unwrap_or_else(|| "message failed".into()))
        }
    };
    outcome?;
    Ok(json!({
        "ok": true,
        "id": id,
        "session": approval.session,
        "verdict": if allow { "allow" } else { "deny" },
        "signed": false,
        "source": PROJECTION,
    }))
}

/// `fleet_control` — storm guard. `hold` / `release` reuse the frozen
/// pause/resume commands (the decision loop only; live panes keep running).
/// W2-4 adds start-hold, storm detection and preemption behind the same action
/// names. `kill_all` is the existing two-step kill switch.
#[tauri::command]
pub async fn fleet_control(
    action: String,
    id: Option<String>,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<Value, String> {
    match action.as_str() {
        "hold" | "pause" => {
            let id = id.ok_or_else(|| "hold requires a supervisor id".to_string())?;
            daemon.pause_supervisor(&id)?;
            Ok(json!({ "ok": true, "action": "hold", "id": id, "source": PROJECTION }))
        }
        "release" | "resume" => {
            let id = id.ok_or_else(|| "release requires a supervisor id".to_string())?;
            daemon.resume_supervisor(&id)?;
            Ok(json!({ "ok": true, "action": "release", "id": id, "source": PROJECTION }))
        }
        "kill_all" => {
            let ack = kill_switch_impl(&daemon);
            Ok(json!({ "ok": ack.ok, "action": "kill_all", "closed": ack.closed, "source": PROJECTION }))
        }
        other => Err(format!("unknown fleet action {other:?} (hold|release|kill_all)")),
    }
}

/// Second, additive event channel. The frozen `snapshot` event is unchanged;
/// `fleet` carries the same §1.3 object plus the derived fleet projection.
/// It taps a second daemon subscriber — no second poll, no change to engine.rs.
pub fn spawn_fleet_emit_thread(app: AppHandle, rx: Receiver<String>) {
    std::thread::Builder::new()
        .name("notch-fleet-emit".into())
        .spawn(move || {
            let mut last = Instant::now() - Duration::from_millis(150);
            let mut pending: Option<Value> = None;
            let parse = |line: &str| -> Option<Value> {
                let v: Value = serde_json::from_str(line).ok()?;
                v.get("snapshot").cloned()
            };
            loop {
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(line) => pending = parse(&line).or(pending),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                while let Ok(line) = rx.try_recv() {
                    if let Some(v) = parse(&line) { pending = Some(v); }
                }
                if let Some(snap) = pending.take() {
                    if last.elapsed() >= Duration::from_millis(150) {
                        let fleet = fleet_from_snapshot(&snap);
                        let _ = app.emit("fleet", json!({ "snapshot": snap, "fleet": fleet }));
                        last = Instant::now();
                    } else {
                        pending = Some(snap);
                    }
                }
            }
        })
        .ok();
}
