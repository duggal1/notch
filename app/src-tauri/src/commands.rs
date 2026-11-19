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
use std::sync::Arc;

use serde::Serialize;
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
