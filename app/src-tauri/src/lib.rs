//! notch — regular macOS app for the OpenCode orchestration control plane:
//! a menu-bar status item, an always-on-top notch popover (transparent,
//! borderless, NSMainMenuWindowLevel+3), and a standard dashboard window.
//! Library + thin main; integration tests live in tests/.

pub mod commands;
pub mod engine;
pub mod frame;
pub mod macos;
pub mod snapshot;
pub mod symbols;
pub mod tray;
pub mod trayfmt;

use std::sync::Arc;

use tauri::webview::WebviewWindowBuilder;
use tauri::window::Color;
use tauri::{LogicalPosition, LogicalSize, TitleBarStyle, WebviewUrl};

use notchd::{Config, Daemon, Herdr, SystemClock};

use crate::frame::{dashboard_frame, frame_for_mode, Mode};

/// Resolve the herdr binary. `open`-launched apps get LaunchServices' minimal
/// PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), which never includes
/// `~/.local/bin` — where herdr usually lives — so a bare `herdr` silently
/// fails to spawn and re-adoption is skipped. Probe the common install
/// locations and hand notchd an absolute path via NOTCH_HERDR_CMD.
/// `NOTCH_HERDR_CMD` remains the user's explicit override.
pub fn resolve_herdr_cmd() -> String {
    if let Ok(p) = std::env::var("NOTCH_HERDR_CMD") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        for rel in [
            ".local/bin/herdr",
            ".cargo/bin/herdr",
            ".bun/bin/herdr",
            "bin/herdr",
            "go/bin/herdr",
        ] {
            candidates.push(home.join(rel));
        }
    }
    candidates.push(std::path::PathBuf::from("/opt/homebrew/bin/herdr"));
    candidates.push(std::path::PathBuf::from("/usr/local/bin/herdr"));
    for cand in candidates {
        if cand.is_file() {
            return cand.to_string_lossy().into_owned();
        }
    }
    "herdr".to_string() // fall back to PATH (works from a dev shell)
}

/// Resolve the opencode binary the same way (LaunchServices minimal PATH) —
/// the engine's Opencode client reads NOTCH_OPENCODE_CMD (default bare
/// `opencode`, which a LaunchServices-launched app cannot find).
pub fn resolve_opencode_cmd() -> String {
    if let Ok(p) = std::env::var("NOTCH_OPENCODE_CMD") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        for rel in [
            ".bun/bin/opencode",
            ".local/bin/opencode",
            ".cargo/bin/opencode",
            "bin/opencode",
        ] {
            candidates.push(home.join(rel));
        }
    }
    candidates.push(std::path::PathBuf::from("/opt/homebrew/bin/opencode"));
    candidates.push(std::path::PathBuf::from("/usr/local/bin/opencode"));
    for cand in candidates {
        if cand.is_file() {
            return cand.to_string_lossy().into_owned();
        }
    }
    "opencode".to_string() // fall back to PATH (works from a dev shell)
}

pub fn run() -> Result<(), String> {
    // Single instance FIRST: a second launch activates the running instance
    // and exits before any state file is read or written.
    if macos::focus_existing_instance() {
        eprintln!("[notch] already running — focused the existing instance, exiting");
        std::process::exit(0);
    }

    // Point notchd at a herdr it can actually exec from a LaunchServices context.
    std::env::set_var("NOTCH_HERDR_CMD", resolve_herdr_cmd());
    // …and at a real opencode binary for the engine's idle-detection client.
    std::env::set_var("NOTCH_OPENCODE_CMD", resolve_opencode_cmd());

    // In-process orchestrator — the same wiring `notchd serve` uses:
    // open (load state + registry, create state dir) → reconcile (re-adopt,
    // vanished panes → done) → poll thread → subscriber flusher.
    // NOT used: the Unix socket (that is for external CLI clients).
    let config = Config::from_env();
    let herdr = Herdr::from_env();
    let daemon = Daemon::open(config, herdr, Arc::new(SystemClock))
        .map_err(|e| format!("open notchd: {e}"))?;
    daemon.reconcile();
    let daemon = Arc::new(daemon);

    // Subscriber channel carries the daemon's snapshot pushes (NDJSON lines,
    // `{"event":"snapshot","snapshot":{…}}`); the emit thread unwraps them.
    let snapshot_rx = engine::spawn_daemon_threads(Arc::clone(&daemon));
    // W2-5: a SECOND subscriber taps the same pushes for the additive `fleet`
    // event. Same source, same debounce, no second poll and no engine.rs edit.
    let (_fleet_sub_id, fleet_rx) = daemon.register_subscriber();

    tauri::Builder::default()
        .manage(Arc::clone(&daemon))
        .setup(move |app| {
            // Notch measurement must happen on the main thread (AppKit).
            let measurements = macos::measure_screen();
            let notch = frame::notch_or_fallback(measurements.notch);
            let rect = frame_for_mode(Mode::Rest, measurements.screen_width, measurements.notch);

            // The popover UI reads window.__NOTCH__ at document start
            // (ui/src/lib/shell-geometry.ts). Integer pixels.
            let init_script = format!(
                "window.__NOTCH__ = {{ w: {}, h: {} }};",
                notch.width.round() as i64,
                notch.height.round() as i64
            );

            // The popover: notch-fused, borderless, transparent, always on top.
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Notch")
                    .inner_size(rect.width, rect.height)
                    .position(rect.x, rect.y)
                    .transparent(true)
                    .decorations(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .resizable(false)
                    .shadow(false)
                    .visible(true)
                    .initialization_script(&init_script)
                    .build()
                    .map_err(|e| format!("create popover window: {e}"))?;

            if let Ok(ptr) = window.ns_window() {
                macos::apply_window_chrome(ptr);
                eprintln!(
                    "[notch] notch {}x{} at ({},{}) — screen {}, popover {:.0}x{:.0}",
                    notch.width.round(),
                    notch.height.round(),
                    rect.x.round(),
                    rect.y.round(),
                    measurements.screen_width.round(),
                    rect.width,
                    rect.height
                );
            } else {
                eprintln!("[notch] ns_window() unavailable; window chrome not applied");
            }
            // Enforce the measured frame after chrome (AppKit can adjust).
            let _ = window.set_size(LogicalSize::new(rect.width, rect.height));
            let _ = window.set_position(LogicalPosition::new(rect.x, rect.y));

            // The dashboard: standard macOS window, created hidden at startup.
            // `open_dashboard` shows + unminimizes + focuses it.
            let dash = dashboard_frame(measurements.screen_width, measurements.screen_height);
            WebviewWindowBuilder::new(
                app,
                "dashboard",
                WebviewUrl::App("index.html?surface=dashboard".into()),
            )
            .title("Notch")
            .inner_size(dash.width, dash.height)
            .min_inner_size(
                crate::frame::DASHBOARD_MIN_WIDTH,
                crate::frame::DASHBOARD_MIN_HEIGHT,
            )
            .position(dash.x, dash.y)
            .decorations(true)
            // The native bar is overlaid, not removed — this window's header IS the top edge.
            .title_bar_style(TitleBarStyle::Overlay)
            .hidden_title(true)
            .background_color(Color(0, 0, 0, 255))
            .resizable(true)
            .always_on_top(false)
            .skip_taskbar(false)
            .visible(false)
            .build()
            .map_err(|e| format!("create dashboard window: {e}"))?;

            let tray_handle = app.handle().clone();
            tray::build(&tray_handle)?;
            engine::spawn_emit_thread(app.handle().clone(), snapshot_rx);
            commands::spawn_fleet_emit_thread(app.handle().clone(), fleet_rx);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::kill,
            commands::reprompt,
            commands::logs,
            commands::focus,
            commands::cleanup,
            commands::submit_task,
            commands::message,
            commands::pause_supervisor,
            commands::resume_supervisor,
            commands::kill_switch,
            commands::list_models,
            commands::open_dashboard,
            commands::set_frame,
            commands::haptic,
            commands::fleet_state,
            commands::list_harnesses,
            commands::list_approvals,
            commands::answer_approval,
            commands::fleet_control,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| format!("tauri run: {e}"))
}
