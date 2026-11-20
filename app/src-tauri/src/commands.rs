//! Tauri commands — thin wrappers over `notchd::Daemon`. Names are
//! snake_case exactly as the wire contract (bridge.md §2) and the webview's
//! `invoke(type, args)` expect (ui/src/lib/bridge.ts): `snapshot, kill,
//! reprompt, logs, focus, cleanup, set_frame, haptic`.
//!
//! All payload fields are camelCase on the wire (notchd's acks already are).
//! Errors are `Result<T, String>`. Commands that shell out to herdr are
//! `async` (never block the main thread); `set_frame` and `haptic` are sync
//! (AppKit / main-thread friendly, no blocking IO).

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State};

use notchd::approvals::card_digest as engine_card_digest;
use notchd::daemon::{KillAck, KillSwitchAck, ModelsAck, OkAck, SubmitTaskAck};
use notchd::snapshot::fleet_view;
use notchd::Daemon;

use crate::frame::{frame_for_mode, frame_for_mode_content, Mode, OPEN_HEIGHT};
use crate::macos;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameRectOut {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub mode: String,
}

/// Payload returned by the `logs` command (the UI polls it; no `logs` event).
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
    daemon: State<'_, Arc<Daemon>>,
) -> Result<LogsPayload, String> {
    let lines = daemon.logs(&id, tail.map(|t| t as usize))?;
    Ok(LogsPayload { id, lines })
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

/// bridge.md §7 v1.3: the second surface — the standard macOS dashboard
/// window (regular decorations, titled, resizable, min 880×600, 1120×760
/// default, normal level, dock-visible). Created hidden at startup;
/// this command shows + unminimizes + focuses it.
/// Shared by the command and the tray's "Open dashboard" item.
pub(crate) fn show_dashboard(app: &AppHandle) -> Result<serde_json::Value, String> {
    eprintln!("[notch] show_dashboard: enter");
    if let Some(window) = app.get_webview_window("dashboard") {
        if let Err(e) = window.show() {
            eprintln!("[notch] show_dashboard show failed: {e}");
        }
        if let Err(e) = window.unminimize() {
            eprintln!("[notch] show_dashboard unminimize failed: {e}");
        }
        if let Err(e) = window.set_focus() {
            eprintln!("[notch] show_dashboard set_focus failed: {e}");
        }
        // A borderless always-on-top popover can own focus; AppKit activation is what
        // actually pulls the dashboard in front of the app the user was in (Ghostty).
        if let Err(e) = app.run_on_main_thread(macos::activate_foreground) {
            eprintln!("[notch] show_dashboard activation dispatch failed: {e}");
        }
        eprintln!("[notch] show_dashboard: shown + focused");
        return Ok(serde_json::json!({ "ok": true, "focused": true }));
    }
    eprintln!("[notch] show_dashboard: dashboard window missing; recreating");

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
    .title_bar_style(tauri::TitleBarStyle::Overlay)
    .hidden_title(true)
    .background_color(tauri::window::Color(0, 0, 0, 255))
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
    eprintln!("[notch] open_dashboard invoked");
    show_dashboard(&app)
}

/// Apply the native popover frame for the requested mode.
#[tauri::command]
pub fn set_frame(
    mode: String,
    content_h: Option<f64>,
    app: AppHandle,
) -> Result<FrameRectOut, String> {
    let mode = Mode::parse(&mode)
        .ok_or_else(|| format!("unknown mode {mode:?} (expected rest|peek|open)"))?;
    let m = macos::cached_measurements();
    let rect = match content_h {
        Some(h) if h.is_finite() => frame_for_mode_content(mode, m.screen_width, m.notch, h),
        _ => frame_for_mode(mode, m.screen_width, m.notch),
    };
    eprintln!(
        "[notch] set_frame mode={} content_h={:?} -> {}x{} at ({},{})",
        mode.as_str(),
        content_h.unwrap_or(OPEN_HEIGHT),
        rect.width,
        rect.height,
        rect.x,
        rect.y
    );

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

/// One answerable card from the engine's signed approval inbox. `signed: true`
/// means the HMAC answer channel can consume it; the digest is the SHA-256 of
/// this exact card.
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

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn array_at<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// W2-1: the harness surface is the registry's real definitions — herdr is
/// the sole admitted/spawn-capable engine; opencode declares Status + Models
/// only and cannot make a managed pane.
fn declare_harnesses() -> Vec<HarnessOut> {
    use notchd::harness_registry::HarnessOperation;
    notchd::harness_registry::harness_listing()
        .into_iter()
        .map(|def| {
            let capabilities: Vec<HarnessCapabilityOut> = HarnessOperation::ALL
                .iter()
                .map(|op| HarnessCapabilityOut {
                    name: op.as_str().to_string(),
                    supported: def.supports(*op),
                })
                .collect();
            HarnessOut {
                id: def.id,
                label: def.label,
                capabilities,
            }
        })
        .collect()
}

/// Approval cards from the engine's REAL pending inbox, oldest first. The
/// digest is the engine's SHA-256 of the exact card the UI displays — not a
/// snapshot projection and not a local FNV hash.
pub fn approvals_from_engine(daemon: &Daemon) -> Vec<ApprovalOut> {
    let snap = serde_json::to_value(daemon.snapshot()).unwrap_or_else(|_| json!({}));
    approval_cards(daemon, &snap)
}

fn approval_cards(daemon: &Daemon, snap: &Value) -> Vec<ApprovalOut> {
    let mut out: Vec<ApprovalOut> = Vec::new();
    for card in daemon.pending_approvals() {
        let supervisor = array_at(snap, "sessions")
            .iter()
            .find(|v| str_at(v, "id") == card.session.as_str())
            .map(|v| str_at(v, "sup").to_string())
            .unwrap_or_default();
        out.push(ApprovalOut {
            digest: engine_card_digest(&card),
            id: card.id,
            session: card.session,
            supervisor,
            kind: card.kind,
            reason: card.reason,
            at: card.at,
            signed: true,
        });
    }
    out.sort_by_key(|a| a.at);
    out.truncate(12);
    out
}

/// Signed answer path shared by the Tauri command and its real-Daemon test.
/// Verify the digest supplied by the UI against the current pending card.
pub fn answer_approval_payload(
    daemon: &Daemon,
    id: &str,
    allow: bool,
    note: Option<String>,
    digest: &str,
) -> Result<Value, String> {
    let card = daemon
        .pending_approvals()
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| format!("unknown approval {id:?}"))?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let answer = daemon.answer_approval(id, allow, note, at, digest, "answer")?;
    Ok(json!({
        "ok": true,
        "id": id,
        "session": card.session,
        "verdict": if allow { "allow" } else { "deny" },
        "signed": true,
        "sig": answer.sig,
        "source": "engine",
    }))
}

/// Real ledger seq/epoch for the fleet view, from the daemon's state-dir
/// ledger (the same source A2's C2 accessors read). Errors surface, never
/// silently become fake zeroes.
fn ledger_numbers(daemon: &Daemon) -> Result<(u64, u64), String> {
    let ledger = notchd::ledger::Ledger::open(&daemon.config.state_dir)
        .map_err(|e| format!("fleet ledger: {e}"))?;
    Ok((ledger.last_seq(), ledger.current_epoch("ORCH")))
}

/// `fleet_state` — the ONE backend-derived fleet view. The UI renders it
/// verbatim; no TypeScript re-derivation.
#[tauri::command]
pub async fn fleet_state(daemon: State<'_, Arc<Daemon>>) -> Result<Value, String> {
    let snap = daemon.snapshot();
    let value = serde_json::to_value(&snap).map_err(|e| e.to_string())?;
    let (seq, epoch) = ledger_numbers(&daemon)?;
    Ok(fleet_view(&value, seq, epoch))
}

/// `list_harnesses` — the registry's real capabilities (herdr spawns;
/// opencode is Status + Models only).
#[tauri::command]
pub async fn list_harnesses() -> Result<Value, String> {
    Ok(json!({ "source": "registry", "harnesses": declare_harnesses() }))
}

/// `list_approvals` — one decision per real engine card, oldest first. The
/// digest is the exact card digest that a signed answer must carry.
#[tauri::command]
pub async fn list_approvals(daemon: State<'_, Arc<Daemon>>) -> Result<Value, String> {
    Ok(json!({ "source": "engine", "approvals": approvals_from_engine(&daemon) }))
}

/// `answer_approval` — the ONLY way to answer a `needs_you` card. Signs over the
/// displayed card's real digest inside the engine and consumes it once; invalid,
/// expired and replayed answers return an honest error to the UI.
#[tauri::command]
pub async fn answer_approval(
    id: String,
    allow: bool,
    note: Option<String>,
    digest: String,
    daemon: State<'_, Arc<Daemon>>,
) -> Result<Value, String> {
    answer_approval_payload(&daemon, &id, allow, note, &digest)
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
            Ok(json!({ "ok": true, "action": "hold", "id": id, "source": "engine" }))
        }
        "release" | "resume" => {
            let id = id.ok_or_else(|| "release requires a supervisor id".to_string())?;
            daemon.resume_supervisor(&id)?;
            Ok(json!({ "ok": true, "action": "release", "id": id, "source": "engine" }))
        }
        "kill_all" => {
            let ack = kill_switch_impl(&daemon);
            Ok(
                json!({ "ok": ack.ok, "action": "kill_all", "closed": ack.closed, "source": "engine" }),
            )
        }
        other => Err(format!(
            "unknown fleet action {other:?} (hold|release|kill_all)"
        )),
    }
}

/// Second, additive event channel. The frozen `snapshot` event is unchanged;
/// `fleet` carries the same §1.3 object plus the ONE backend-derived fleet view.
/// It taps a second daemon subscriber — no second poll, no change to engine.rs.
pub fn spawn_fleet_emit_thread(app: AppHandle, rx: Receiver<String>, daemon: Arc<Daemon>) {
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
                    if let Some(v) = parse(&line) {
                        pending = Some(v);
                    }
                }
                if let Some(snap) = pending.take() {
                    if last.elapsed() >= Duration::from_millis(150) {
                        if let Ok((seq, epoch)) = ledger_numbers(&daemon) {
                            let fleet = fleet_view(&snap, seq, epoch);
                            let approvals = approval_cards(&daemon, &snap);
                            let _ = app.emit(
                                "fleet",
                                json!({ "snapshot": snap, "fleet": fleet, "approvals": approvals }),
                            );
                            last = Instant::now();
                        }
                    } else {
                        pending = Some(snap);
                    }
                }
            }
        })
        .ok();
}
