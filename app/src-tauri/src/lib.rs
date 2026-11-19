//! notch — macOS notch + menu-bar surface for the OpenCode orchestration
//! control plane. Library + thin main; integration tests live in tests/.

pub mod commands;
pub mod engine;
pub mod frame;
pub mod macos;
pub mod snapshot;
pub mod tray;
pub mod trayfmt;

use std::sync::Arc;

use tauri::{LogicalPosition, LogicalSize, WebviewUrl};
use tauri::webview::WebviewWindowBuilder;

use crate::frame::{frame_for_mode, Mode};

pub fn run() -> Result<(), String> {
    // Engine first (background thread + channel); the emit thread is attached
    // after the window and tray exist.
    let (engine, snapshot_rx) = engine::Engine::start()?;

    tauri::Builder::default()
        .manage(Arc::clone(&engine))
        .setup(move |app| {
            // Notch measurement must happen on the main thread (AppKit).
            let measurements = macos::measure_screen();
            let notch = frame::notch_or_fallback(measurements.notch);
            let rect = frame_for_mode(Mode::Rest, measurements.screen_width, measurements.notch);

            // The UI reads window.__NOTCH__ at document start (DESIGN.md §5.1,
            // ui/src/lib/shell-geometry.ts). Integer pixels.
            let init_script = format!(
                "window.__NOTCH__ = {{ w: {}, h: {} }};",
                notch.width.round() as i64,
                notch.height.round() as i64
            );

            // NOTE: the static `windows` array in tauri.conf.json cannot carry
            // an initialization script, so the popover window is created here
            // with the same flags the static config would have used.
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
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
                    "[notch] notch {}x{} at ({},{}) — screen {}, window {:.0}x{:.0}",
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

            tray::build(&app.handle())?;
            engine::spawn_emit_thread(app.handle().clone(), snapshot_rx);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::kill,
            commands::reprompt,
            commands::logs,
            commands::focus,
            commands::cleanup,
            commands::spawn,
            commands::run_plan,
            commands::set_frame,
            commands::haptic,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| format!("tauri run: {e}"))
}
