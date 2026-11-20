//! Pure geometry tests: notch detection, fallback, mode→frame table.

use notch::frame::{
    frame_for_mode, notch_from_measurements, notch_or_fallback, Mode, NotchSize,
    FALLBACK_NOTCH_HEIGHT, FALLBACK_NOTCH_WIDTH, OPEN_HEIGHT, OPEN_WIDTH,
};

#[test]
fn notch_detection_typical_14_inch() {
    // 14" MBP: 1512pt screen, ~206pt notch, 32pt safe area.
    let n = notch_from_measurements(32.0, 653.0, 653.0, 1512.0).expect("notch");
    assert!((n.width - 206.0).abs() < 0.001);
    assert!((n.height - 32.0).abs() < 0.001);
}

#[test]
fn notch_detection_16_inch() {
    // 16" MBP: 1728pt wide screen; same 32pt safe area.
    let n = notch_from_measurements(32.0, 761.0, 761.0, 1728.0).expect("notch");
    assert!((n.width - 206.0).abs() < 0.001);
    assert_eq!(n.height, 32.0);
}

#[test]
fn notch_detection_notchless_external() {
    // Notchless displays report a zero top safe-area inset.
    assert_eq!(notch_from_measurements(0.0, 100.0, 100.0, 1920.0), None);
}

#[test]
fn notch_detection_garbage_rejected() {
    assert_eq!(notch_from_measurements(f64::NAN, 1.0, 1.0, 1512.0), None);
    assert_eq!(
        notch_from_measurements(32.0, f64::INFINITY, 653.0, 1512.0),
        None
    );
    assert_eq!(notch_from_measurements(-1.0, 653.0, 653.0, 1512.0), None);
    assert_eq!(notch_from_measurements(32.0, -1.0, 653.0, 1512.0), None);
    assert_eq!(notch_from_measurements(32.0, 653.0, 653.0, 0.0), None);
    // aux areas covering the whole screen ⇒ nonsense
    assert_eq!(notch_from_measurements(32.0, 800.0, 800.0, 1512.0), None);
    // implausible notch width (would be > 480pt wide)
    assert_eq!(notch_from_measurements(32.0, 10.0, 10.0, 1512.0), None);
    // implausible notch height (> 64pt)
    assert_eq!(notch_from_measurements(100.0, 653.0, 653.0, 1512.0), None);
}

#[test]
fn fallback_frame_is_200_by_32() {
    assert_eq!(FALLBACK_NOTCH_WIDTH, 200.0);
    assert_eq!(FALLBACK_NOTCH_HEIGHT, 32.0);
    let n = notch_or_fallback(None);
    assert_eq!(n, NotchSize::FALLBACK);
    assert_eq!(
        notch_or_fallback(Some(NotchSize {
            width: 206.0,
            height: 32.0
        }))
        .width,
        206.0
    );
}

#[test]
fn mode_frame_table_with_notch() {
    let notch = Some(NotchSize {
        width: 206.0,
        height: 32.0,
    });
    let rest = frame_for_mode(Mode::Rest, 1512.0, notch);
    assert_eq!(rest.width, 206.0 + 120.0);
    // P0: the whole pill is one real button, never shorter than 44px.
    assert_eq!(rest.height, 44.0);
    assert_eq!(rest.y, 0.0);
    assert!((rest.x - (1512.0 - 326.0) / 2.0).abs() < 0.001);

    let peek = frame_for_mode(Mode::Peek, 1512.0, notch);
    assert_eq!(peek.width, 206.0 + 120.0 + 120.0);
    assert_eq!(peek.height, 32.0 + 44.0);
    assert_eq!(peek.y, 0.0);
    assert!((peek.x - (1512.0 - 446.0) / 2.0).abs() < 0.001);

    let open = frame_for_mode(Mode::Open, 1512.0, notch);
    assert_eq!((open.width, open.height), (OPEN_WIDTH, OPEN_HEIGHT));
    assert_eq!(open.y, 0.0);
}

#[test]
fn mode_frame_table_fallback_notchless() {
    let rest = frame_for_mode(Mode::Rest, 1920.0, None);
    assert_eq!((rest.width, rest.height), (320.0, 44.0));
    assert_eq!(rest.y, 0.0);
    assert!((rest.x - (1920.0 - 320.0) / 2.0).abs() < 0.001);

    let peek = frame_for_mode(Mode::Peek, 1920.0, None);
    assert_eq!((peek.width, peek.height), (440.0, 76.0));

    // Open is the same everywhere — the shell hangs from the bar at its size.
    let open = frame_for_mode(Mode::Open, 1920.0, None);
    assert_eq!((open.width, open.height), (560.0, 640.0));
    assert!((open.x - (1920.0 - 560.0) / 2.0).abs() < 0.001);
}

#[test]
fn open_frame_stays_560_by_640() {
    for screen in [1440.0, 1512.0, 1728.0, 2560.0] {
        for notch in [
            None,
            Some(NotchSize {
                width: 206.0,
                height: 32.0,
            }),
            Some(NotchSize {
                width: 379.0,
                height: 59.0,
            }),
        ] {
            let open = frame_for_mode(Mode::Open, screen, notch);
            assert_eq!(
                (open.width, open.height),
                (560.0, 640.0),
                "open must stay 560x640 on {screen} with {notch:?}"
            );
        }
    }
}

#[test]
fn mode_parsing() {
    assert_eq!(Mode::parse("rest"), Some(Mode::Rest));
    assert_eq!(Mode::parse("peek"), Some(Mode::Peek));
    assert_eq!(Mode::parse("open"), Some(Mode::Open));
    assert_eq!(Mode::parse("REST"), Some(Mode::Rest));
    assert_eq!(Mode::parse(" peek "), Some(Mode::Peek));
    assert_eq!(Mode::parse("expanded"), None);
    assert_eq!(Mode::parse(""), None);
    assert_eq!(Mode::Rest.as_str(), "rest");
    assert_eq!(Mode::Peek.as_str(), "peek");
    assert_eq!(Mode::Open.as_str(), "open");
}
