//! Dashboard frame math (bridge.md §7): 1120×760, centered, min enforced.

use notch::frame::{
    dashboard_frame, DASHBOARD_HEIGHT, DASHBOARD_MIN_HEIGHT, DASHBOARD_MIN_WIDTH, DASHBOARD_WIDTH,
};

#[test]
fn dashboard_is_1120_by_760() {
    assert_eq!(DASHBOARD_WIDTH, 1120.0);
    assert_eq!(DASHBOARD_HEIGHT, 760.0);
}

#[test]
fn dashboard_exceeds_its_minimums() {
    // The window server enforces min_inner_size; the defaults must respect it.
    const { assert!(DASHBOARD_WIDTH >= DASHBOARD_MIN_WIDTH) };
    const { assert!(DASHBOARD_HEIGHT >= DASHBOARD_MIN_HEIGHT) };
    // …with sane margins, not merely-equal
    const { assert!(DASHBOARD_MIN_WIDTH >= 880.0) };
    const { assert!(DASHBOARD_MIN_HEIGHT >= 600.0) };
}

#[test]
fn dashboard_frame_centered_on_1440x900() {
    let r = dashboard_frame(1440.0, 900.0);
    assert_eq!((r.width, r.height), (1120.0, 760.0));
    assert!((r.x - (1440.0 - 1120.0) / 2.0).abs() < 0.001);
    assert!((r.y - (900.0 - 760.0) / 2.0).abs() < 0.001);
    assert_eq!(r.x, 160.0);
    assert_eq!(r.y, 70.0);
}

#[test]
fn dashboard_frame_centered_on_2560x1440() {
    let r = dashboard_frame(2560.0, 1440.0);
    assert_eq!((r.width, r.height), (1120.0, 760.0));
    assert_eq!(r.x, 720.0);
    assert_eq!(r.y, 340.0);
}

#[test]
fn dashboard_frame_small_screen_does_not_go_negative() {
    let r = dashboard_frame(800.0, 600.0);
    assert_eq!((r.x, r.y), (0.0, 0.0));
}
