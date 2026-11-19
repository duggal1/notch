//! Tray status formatting — pure functions, unit-tested.
//!
//! The menu-bar item is a quiet status plate, not a scoreboard:
//!   • title (macOS): `‹N›` shown only while N sessions are in flight;
//!     no count glyph when the fleet is idle — a blank plate says "nothing
//!     needs you" better than a `‹0›`.
//!   • tooltip: carries the numbers (`notch — N in flight`; failures are
//!     appended only when there is at least one — `0 failed` is noise).
//!   • the disabled menu item repeats the same truth in words.
//!
//! `status_from_value` reads the snapshot as untrusted JSON (that's what
//! crosses the emit boundary), tolerating anything unexpected by returning
//! a zeroed count rather than failing.

use serde_json::Value;

/// N in flight = sessions whose status is `working`. Failures are reported
/// separately (they need eyes, not just a count).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayStatus {
    pub in_flight: usize,
    pub failed: usize,
}

pub fn status_from_value(snapshot: &Value) -> TrayStatus {
    let mut status = TrayStatus {
        in_flight: 0,
        failed: 0,
    };
    if let Some(sessions) = snapshot.get("sessions").and_then(Value::as_array) {
        for s in sessions {
            match s.get("status").and_then(Value::as_str) {
                Some("working") => status.in_flight += 1,
                Some("failed") => status.failed += 1,
                _ => {}
            }
        }
    }
    status
}

/// Menu-bar title. `None` ⇒ no title text (icon only).
pub fn tray_title(s: &TrayStatus) -> Option<String> {
    if s.in_flight == 0 {
        None
    } else {
        Some(format!("‹{}›", s.in_flight))
    }
}

/// Tooltip, e.g. `notch — 2 in flight` or `notch — 2 in flight · 1 failed`.
pub fn tooltip(s: &TrayStatus) -> String {
    let base = format!("notch — {} in flight", s.in_flight);
    if s.failed > 0 {
        format!("{} · {} failed", base, s.failed)
    } else {
        base
    }
}

/// Disabled status row at the top of the tray menu.
pub fn menu_inflight_label(s: &TrayStatus) -> String {
    match s.in_flight {
        0 => "Nothing in flight".to_string(),
        1 => "1 in flight".to_string(),
        n => format!("{n} in flight"),
    }
}
