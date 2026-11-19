//! macOS reach-through: NSScreen notch measurements and the NSWindow chrome
//! (level, collection behavior, borderless style mask).
//!
//! Everything here is isolated in this one module; the rest of the app talks
//! in logical points (Tauri coordinates) and pure math (frame.rs).
//!
//! Measurements are taken ONCE on the main thread (setup) and cached — AppKit
//! screen geometry is only guaranteed from the main thread, and Tauri commands
//! run off it. Screen reconfiguration (docking/undocking, closing a lid) does
//! not re-measure until the app relaunches; acceptable for v1, noted in the
//! report. The window's frame is recomputed from cache + frame.rs on every
//! `set_frame`, so at least resizes are always consistent.
//!
//! Known limitation (recorded in the report): Tauri cannot host an
//! NSPanel. This window is a borderless, non-opaque NSWindow at
//! NSMainMenuWindowLevel+3 — NOT a non-activating panel. A click on the
//! popover therefore participates in app activation like a normal (borderless)
//! window; every other notch-host property from DESIGN.md §5.1 is applied.

use std::sync::OnceLock;

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSHapticFeedbackManager, NSHapticFeedbackPerformanceTime, NSHapticFeedbackPerformer,
    NSHapticFeedbackPattern, NSScreen, NSWindow, NSWindowCollectionBehavior, NSWindowLevel,
    NSWindowStyleMask, NSMainMenuWindowLevel,
};
use objc2_foundation::{NSEdgeInsets, NSRect};

use crate::frame::{notch_from_measurements, NotchSize};

#[derive(Clone, Copy, Debug)]
pub struct ScreenMeasurements {
    /// NSScreen.frame.width, logical points.
    pub screen_width: f64,
    /// Detected notch size, or None on notchless displays (callers fall back).
    pub notch: Option<NotchSize>,
}

static SCREEN: OnceLock<ScreenMeasurements> = OnceLock::new();

/// Fallback used when AppKit cannot be queried (never on a normal main-thread
/// setup; the tests run headless-less, so this path is also what tests see).
pub const FALLBACK_MEASUREMENTS: ScreenMeasurements = ScreenMeasurements {
    screen_width: 1512.0, // 14" MacBook Pro logical width; centering still works
    notch: None,          // frame.rs maps None → 200×32 fallback
};

/// Take the measurements. Must run on the main thread (called from setup).
/// Returns the cached value on subsequent calls.
pub fn measure_screen() -> ScreenMeasurements {
    *SCREEN.get_or_init(|| {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("[notch] NSScreen measured off the main thread; using fallback measurements");
            return FALLBACK_MEASUREMENTS;
        };
        let Some(screen) = NSScreen::mainScreen(mtm) else {
            eprintln!("[notch] NSScreen.mainScreen returned nil; using fallback measurements");
            return FALLBACK_MEASUREMENTS;
        };
        let frame: NSRect = screen.frame();
        let insets: NSEdgeInsets = screen.safeAreaInsets();
        let aux_left: NSRect = screen.auxiliaryTopLeftArea();
        let aux_right: NSRect = screen.auxiliaryTopRightArea();
        let notch = notch_from_measurements(
            insets.top,
            aux_left.size.width,
            aux_right.size.width,
            frame.size.width,
        );
        ScreenMeasurements {
            screen_width: frame.size.width,
            notch,
        }
    })
}

pub fn cached_measurements() -> ScreenMeasurements {
    SCREEN.get().copied().unwrap_or(FALLBACK_MEASUREMENTS)
}

/// Apply the notch-host chrome to the (already borderless, transparent)
/// Tauri window: main-menu level + 3, all-Spaces / stationary / fullscreen
/// auxiliary / ignores-cycle collection behavior, borderless style mask.
///
/// SAFETY: `ns_window_ptr` comes from `tauri::window::Window::ns_window()` /
/// `WebviewWindow::ns_window()`, which returns an autoreleased NSWindow*
/// owned by the running app. We borrow it for the duration of this call only,
/// send the AppKit setters (all documented thread-affine to the main thread),
/// and never retain, release, or hand the pointer out.
pub fn apply_window_chrome(ns_window_ptr: *mut std::ffi::c_void) {
    if ns_window_ptr.is_null() {
        eprintln!("[notch] ns_window() returned null; window chrome not applied");
        return;
    }
    unsafe {
        let window = &*(ns_window_ptr as *const NSWindow);
        // NSMainMenuWindowLevel is 24; the notch host sits 3 above the menu
        // bar (DESIGN.md §5.1) — high enough that the popover is never
        // covered by fullscreen menu bars, below status items and pop-ups.
        let level: NSWindowLevel = NSMainMenuWindowLevel + 3;
        window.setLevel(level);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setStyleMask(NSWindowStyleMask::Borderless);
        window.setHidesOnDeactivate(false);
        window.setMovable(false);
    }
}

/// One haptic tick (DESIGN.md §5.1: alignment on open, levelChange on
/// failure). Best effort — never an error, never a no-op crash.
pub fn perform_haptic(kind: &str) {
    let pattern = match kind {
        "alignment" => NSHapticFeedbackPattern::Alignment,
        "levelChange" => NSHapticFeedbackPattern::LevelChange,
        other => {
            eprintln!("[notch] unknown haptic kind {other:?}; ignoring");
            return;
        }
    };
    let performer = NSHapticFeedbackManager::defaultPerformer();
    performer.performFeedbackPattern_performanceTime(pattern, NSHapticFeedbackPerformanceTime::Now);
}
