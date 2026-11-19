//! Tauri commands — thin wrappers over `engine::Engine`. Names are
//! snake_case exactly as the wire contract (bridge.md §2) and the webview's
//! `invoke(type, args)` expect (ui/src/lib/bridge.ts): `snapshot, kill,
//! reprompt, logs, focus, cleanup, spawn, run_plan, set_frame, haptic`.
//!
//! All payload fields are camelCase on the wire. Errors are `Result<T, String>`.
//! Commands that shell out to herdr are `async` (never block the main thread);
//! `set_frame` is sync deliberately (AppKit wants the main thread and the
//! work is pure window math).

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State};

use crate::engine::{Engine, LogsPayload, RunPlanResult};
use crate::frame::{frame_for_mode, Mode};
use crate::macos;
use crate::snapshot::Snapshot;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameRectOut {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub mode: String,
}

#[tauri::command]
pub async fn snapshot(engine: State<'_, Arc<Engine>>) -> Result<Snapshot, String> {
    // Fresh build (one herdr call). On failure the registry state is returned
    // with `warnings` populated — never an error for the popover.
    Ok(engine.build_snapshot())
}

#[tauri::command]
pub async fn kill(id: String, reason: Option<String>, engine: State<'_, Arc<Engine>>) -> Result<(), String> {
    engine.kill(&id, reason.as_deref())?;
    Ok(())
}

#[tauri::command]
pub async fn reprompt(id: String, text: String, engine: State<'_, Arc<Engine>>) -> Result<(), String> {
    engine.reprompt(&id, &text)?;
    Ok(())
}

#[tauri::command]
pub async fn logs(
    id: String,
    tail: Option<u32>,
    app: AppHandle,
    engine: State<'_, Arc<Engine>>,
) -> Result<LogsPayload, String> {
    let payload = engine.logs(&id, tail)?;
    // The frontend consumes logs via the `logs` EVENT (bridge.ts: listen('logs')),
    // so emit it here in addition to returning the value.
    let _ = app.emit("logs", &payload);
    Ok(payload)
}

#[tauri::command]
pub async fn focus(id: String, engine: State<'_, Arc<Engine>>) -> Result<(), String> {
    engine.focus(&id)
}

#[tauri::command]
pub async fn cleanup(id: Option<String>, engine: State<'_, Arc<Engine>>) -> Result<u32, String> {
    engine.cleanup_temp(id.as_deref())
}

#[tauri::command]
pub async fn spawn(
    supervisor: String,
    prompt: String,
    cwd: String,
    title: String,
    engine: State<'_, Arc<Engine>>,
) -> Result<String, String> {
    engine.spawn_session(&supervisor, &prompt, &cwd, &title)
}

#[tauri::command]
pub async fn run_plan(path: String, engine: State<'_, Arc<Engine>>) -> Result<RunPlanResult, String> {
    engine.run_plan(&path)
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
    let mode = Mode::parse(&mode).ok_or_else(|| format!("unknown mode {mode:?} (expected rest|peek|open)"))?;
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
