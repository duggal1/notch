//! SF Symbol rasterization — the pure guards. Rasterizing itself requires
//! the main thread (AppKit) and is covered indirectly by the hidden-launch
//! smoke, not by tests.

use notch::symbols::{is_valid_symbol_name, weight_value};

#[test]
fn valid_symbol_names() {
    for name in [
        "bolt.fill",
        "xmark.circle",
        "chevron.right",
        "tray.full",
        "checkmark.circle.fill",
        "o.square",
        "a",
    ] {
        assert!(is_valid_symbol_name(name), "{name} should be valid");
    }
}

#[test]
fn invalid_symbol_names() {
    for name in [
        "",
        ".",
        "..",
        ".bolt",
        "bolt.",
        "bolt..fill",
        "Bolt",
        "BOLT.FILL",
        "bolt fill",
        "bolt;rm -rf /",
        "bolt/fill",
        "bolt.fill\n",
        &"x".repeat(65),
    ] {
        assert!(!is_valid_symbol_name(name), "{name:?} should be invalid");
    }
}

#[test]
fn weight_mapping() {
    // Compare against the AppKit constants themselves — their literal values
    // are the OS's business (Regular = 0.0 on current SDKs), not ours.
    unsafe {
        use objc2_app_kit::{NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold};
        assert_eq!(weight_value("regular").unwrap(), NSFontWeightRegular);
        assert_eq!(weight_value("medium").unwrap(), NSFontWeightMedium);
        assert_eq!(weight_value("semibold").unwrap(), NSFontWeightSemibold);
    }
    assert!(weight_value("bold").is_err());
    assert!(weight_value("regular;").is_err());
}

#[test]
fn rasterize_off_main_thread_is_an_error_not_a_crash() {
    // cargo tests run on worker threads; AppKit requires the main thread.
    let err = notch::symbols::rasterize_symbol("bolt.fill", 16.0, "semibold");
    assert!(
        err.is_err(),
        "off-main-thread rasterization must fail cleanly"
    );
}
