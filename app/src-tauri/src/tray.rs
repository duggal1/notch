//! Menu-bar (tray) status item.
//!
//! • Template ring glyph (18×18 black PNG, generated programmatically —
//!   see app/assets/README.md). Monochrome, so macOS renders it correctly in
//!   both menu bar appearances.
//! • Text title `‹N›` while N sessions are in flight; icon-only at rest.
//!   `TrayIcon::set_title` EXISTS on macOS in tauri 2.12 (unlike the doubted
//!   assumption in the brief) — so no icon-drawn count glyph is needed.
//! • Tooltip and the disabled first menu row carry the same numbers.
//! • Left click toggles the popover (window is created with
//!   `show_menu_on_left_click(false)`); right click shows the menu.
//! • "Show / hide popover", "Sweep temp orphans", "Quit".

use std::sync::{Arc, Mutex};

use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItem, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, LogicalPosition, Manager, Wry,
};

use crate::engine::Engine;
use crate::macos;
use crate::snapshot::Snapshot;
use crate::trayfmt::{self, TrayStatus};

pub struct TrayState {
    tray: TrayIcon,
    inflight: MenuItem<Wry>,
}

fn toggle_popover(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
        }
        _ => {
            let _ = window.show();
            // Re-center after show (a screen reconfiguration between hide and
            // show would otherwise strand the popover). y = 0 always.
            if let Ok(size) = window.outer_size() {
                let scale = window.scale_factor().unwrap_or(2.0);
                let width = size.width as f64 / scale;
                let m = macos::cached_measurements();
                let x = ((m.screen_width - width) / 2.0).max(0.0);
                let _ = window.set_position(LogicalPosition::new(x, 0.0));
            }
        }
    }
}

pub fn build(app: &AppHandle) -> Result<(), String> {
    let inflight = MenuItemBuilder::with_id("inflight", trayfmt::menu_inflight_label(&TrayStatus { in_flight: 0, failed: 0 }))
        .enabled(false)
        .build(app)
        .map_err(|e| format!("menu item inflight: {e}"))?;
    let toggle = MenuItemBuilder::with_id("toggle", "Show / hide popover")
        .build(app)
        .map_err(|e| format!("menu item toggle: {e}"))?;
    let sweep = MenuItemBuilder::with_id("sweep", "Sweep temp orphans")
        .build(app)
        .map_err(|e| format!("menu item sweep: {e}"))?;
    let quit = MenuItemBuilder::with_id("quit", "Quit")
        .build(app)
        .map_err(|e| format!("menu item quit: {e}"))?;

    let menu = MenuBuilder::new(app)
        .item(&inflight)
        .separator()
        .item(&toggle)
        .item(&sweep)
        .separator()
        .item(&quit)
        .build()
        .map_err(|e| format!("menu build: {e}"))?;

    let glyph = Image::from_bytes(include_bytes!("../../assets/tray-glyph.png"))
        .map_err(|e| format!("tray-glyph.png: {e}"))?;

    let tray = TrayIconBuilder::with_id("notch")
        .icon(glyph)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("notch — 0 in flight")
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_popover(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_popover(app),
            "sweep" => {
                if let Some(engine) = app.try_state::<Arc<Engine>>() {
                    let _ = engine.cleanup_temp(None);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)
        .map_err(|e| format!("tray build: {e}"))?;

    app.manage(Mutex::new(TrayState { tray, inflight }));
    Ok(())
}

/// Refresh tray title/tooltip/status row from a fresh snapshot. Called by the
/// emit thread on every emit. Never errors visibly — a tray that can't
/// refresh is not worth a panic.
pub fn update_status_from_snapshot(app: &AppHandle, snapshot: &Snapshot) {
    let status = trayfmt::status_from_snapshot(snapshot);
    let Some(state) = app.try_state::<Mutex<TrayState>>() else {
        return; // tray not built yet (or torn down)
    };
    let Ok(state) = state.lock() else {
        return;
    };
    let _ = state.tray.set_title(trayfmt::tray_title(&status));
    let _ = state.tray.set_tooltip(Some(trayfmt::tooltip(&status)));
    let _ = state.inflight.set_text(trayfmt::menu_inflight_label(&status));
}
