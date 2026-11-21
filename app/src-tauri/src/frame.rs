//! Pure frame math for the notch host window.
//!
//! The NSWindow is resized INSTANTLY per mode change (DESIGN.md §5.1: "resize
//! the NSPanel once per mode change, never per frame"); the smoothness lives
//! in the webview's clip-path morph. Everything in this module is a pure
//! function so the geometry is unit-testable without a screen.
//!
//! Content geometry mirrors `frameFor` in ui (shell-geometry.ts / DESIGN.md §5.2):
//!   rest: w = notch.w + 2*wing, h = max(notch.h, 44)   (wing = 44; >= 44px hit target)
//!   peek: w = notch.w + 2*wing + 8, h = max(notch.h+8, 44)
//!   open: w = 560, h = contentH                        (640 default, 320 when idle)
//! x is centered on the main screen; y is always 0 (the shell hangs from the bar).

pub const REST_WING: f64 = 44.0;
pub const PEEK_EXTRA: f64 = 8.0;
pub const PEEK_HEIGHT_EXTRA: f64 = 8.0;
pub const OPEN_WIDTH: f64 = 560.0;
pub const OPEN_HEIGHT: f64 = 640.0;
/// Concise idle open height (P1). Clamped by `set_frame`.
pub const OPEN_HEIGHT_IDLE: f64 = 320.0;
/// The whole rest/peek pill is one real button; never smaller than 44px tall.
pub const MIN_PILL_HEIGHT: f64 = 44.0;

/// Dashboard (bridge.md §7): the standard macOS window. 1120×760 default,
/// min 880×600 enforced by the window server; centered on the main screen.
pub const DASHBOARD_WIDTH: f64 = 1120.0;
pub const DASHBOARD_HEIGHT: f64 = 760.0;
pub const DASHBOARD_MIN_WIDTH: f64 = 880.0;
pub const DASHBOARD_MIN_HEIGHT: f64 = 600.0;

/// Notchless Macs and external displays get a 200×32 stand-in (DESIGN.md §5.1:
/// "Fallback (200×32) is for notchless Macs and external displays, so the
/// design stays identical there").
pub const FALLBACK_NOTCH_WIDTH: f64 = 200.0;
pub const FALLBACK_NOTCH_HEIGHT: f64 = 32.0;

/// Sanity bands for real notch measurements. Anything outside means the OS
/// gave us garbage (common on notchless or unusual setups) and we prefer the
/// documented fallback over a weird window.
const MIN_NOTCH_WIDTH: f64 = 120.0;
const MAX_NOTCH_WIDTH: f64 = 480.0;
const MAX_NOTCH_HEIGHT: f64 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotchSize {
    pub width: f64,
    pub height: f64,
}

impl NotchSize {
    pub const FALLBACK: NotchSize = NotchSize {
        width: FALLBACK_NOTCH_WIDTH,
        height: FALLBACK_NOTCH_HEIGHT,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Rest,
    Peek,
    Open,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rest" => Some(Mode::Rest),
            "peek" => Some(Mode::Peek),
            "open" => Some(Mode::Open),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Rest => "rest",
            Mode::Peek => "peek",
            Mode::Open => "open",
        }
    }
}

/// Result of `set_frame`: the exact rect applied to the window (logical
/// points, top-left origin — Tauri window coordinates).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Pure notch detection.
///
/// `safe_top`   = NSScreen.safeAreaInsets.top (menu-bar inset; > 0 only on notched Macs)
/// `aux_left`   = auxiliaryTopLeftArea.width
/// `aux_right`  = auxiliaryTopRightArea.width
/// `screen_width` = NSScreen.frame.width
///
/// Returns None when the measurements do not describe a real notch; callers
/// use `notch_or_fallback` in that case. The notch width is the screen width
/// minus the two aux (menu bar) areas, exactly like the Swift reference.
pub fn notch_from_measurements(
    safe_top: f64,
    aux_left_width: f64,
    aux_right_width: f64,
    screen_width: f64,
) -> Option<NotchSize> {
    for v in [safe_top, aux_left_width, aux_right_width, screen_width] {
        if !v.is_finite() {
            return None;
        }
    }
    if safe_top <= 0.0 {
        return None; // notchless display (safe area starts below the menu bar)
    }
    if aux_left_width <= 0.0 || aux_right_width <= 0.0 {
        return None;
    }
    if screen_width <= 0.0 || aux_left_width + aux_right_width >= screen_width {
        return None;
    }
    let width = screen_width - aux_left_width - aux_right_width;
    if !(MIN_NOTCH_WIDTH..=MAX_NOTCH_WIDTH).contains(&width) {
        return None;
    }
    if safe_top > MAX_NOTCH_HEIGHT {
        return None;
    }
    Some(NotchSize {
        width,
        height: safe_top,
    })
}

/// Notch measurements or the documented 200×32 fallback.
pub fn notch_or_fallback(measurements: Option<NotchSize>) -> NotchSize {
    measurements.unwrap_or(NotchSize::FALLBACK)
}

/// Dashboard window rect (bridge.md §7): 1120×760, centered on the main
/// screen (top-left origin — Tauri window coordinates).
pub fn dashboard_frame(screen_width: f64, screen_height: f64) -> FrameRect {
    let top_clearance = MIN_PILL_HEIGHT.max(MAX_NOTCH_HEIGHT + PEEK_HEIGHT_EXTRA) + 8.0;
    let height = DASHBOARD_HEIGHT.min((screen_height - top_clearance).max(DASHBOARD_MIN_HEIGHT));
    let width = DASHBOARD_WIDTH.min(screen_width.max(DASHBOARD_MIN_WIDTH));
    FrameRect {
        x: ((screen_width - width) / 2.0).max(0.0),
        y: ((screen_height - height) / 2.0).max(top_clearance),
        width,
        height,
    }
}

/// Compute the window rect for a mode. `notch: None` means "no usable notch
/// measurement for this screen" — the fallback frame is used.
pub fn frame_for_mode(mode: Mode, screen_width: f64, notch: Option<NotchSize>) -> FrameRect {
    frame_for_mode_content(mode, screen_width, notch, OPEN_HEIGHT)
}

/// Same math with an explicit open content height. The UI passes 320 when idle.
pub fn frame_for_mode_content(
    mode: Mode,
    screen_width: f64,
    notch: Option<NotchSize>,
    content_height: f64,
) -> FrameRect {
    let n = notch_or_fallback(notch);
    let h = content_height.clamp(OPEN_HEIGHT_IDLE, OPEN_HEIGHT);
    let (width, height) = match mode {
        Mode::Rest => (n.width + REST_WING * 2.0, n.height.max(MIN_PILL_HEIGHT)),
        Mode::Peek => (
            n.width + REST_WING * 2.0 + PEEK_EXTRA,
            (n.height + PEEK_HEIGHT_EXTRA).max(MIN_PILL_HEIGHT),
        ),
        Mode::Open => (OPEN_WIDTH, h),
    };
    let x = ((screen_width - width) / 2.0).max(0.0);
    FrameRect {
        x,
        y: 0.0,
        width,
        height,
    }
}
