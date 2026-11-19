//! opencode resolution: env override wins; otherwise probe standard install
//! locations; otherwise fall back to bare `opencode` (PATH).
//!
//! Mirrors herdr_resolve.rs. Mutates process env, so it is one test.

use std::path::Path;

#[test]
fn opencode_resolution_order() {
    let prev_home = std::env::var_os("HOME");
    std::env::remove_var("NOTCH_OPENCODE_CMD");

    // 1) Env override wins, verbatim.
    std::env::set_var("NOTCH_OPENCODE_CMD", "/custom/path/to/opencode-test");
    assert_eq!(
        notch::resolve_opencode_cmd(),
        "/custom/path/to/opencode-test"
    );
    std::env::remove_var("NOTCH_OPENCODE_CMD");

    // 2) Unreachable home: absolute candidates or bare fallback.
    std::env::set_var("HOME", "/definitely-not-here-xyz-123");
    let got = notch::resolve_opencode_cmd();
    assert!(got == "opencode" || Path::new(&got).is_file());

    // 3) Real machine: resolves to something usable.
    match prev_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    let got = notch::resolve_opencode_cmd();
    assert!(
        Path::new(&got).is_file() || got == "opencode",
        "expected a usable opencode path, got {got}"
    );
}
