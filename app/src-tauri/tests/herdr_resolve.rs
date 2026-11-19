//! herdr resolution: env override wins; otherwise probe standard install
//! locations; otherwise fall back to bare `herdr` (PATH).
//!
//! All cases mutate process env, so they run as ONE test — the harness runs
//! tests on parallel threads and env is process-global.

use std::path::Path;

#[test]
fn herdr_resolution_order() {
    let prev_home = std::env::var_os("HOME");
    std::env::remove_var("NOTCH_HERDR_CMD");

    // 1) Env override wins, verbatim.
    std::env::set_var("NOTCH_HERDR_CMD", "/custom/path/to/herdr-test");
    assert_eq!(notch::resolve_herdr_cmd(), "/custom/path/to/herdr-test");
    std::env::remove_var("NOTCH_HERDR_CMD");

    // 2) Unreachable home: absolute candidates or bare fallback, never a
    //    typo'd home-relative path.
    std::env::set_var("HOME", "/definitely-not-here-xyz-123");
    let got = notch::resolve_herdr_cmd();
    assert!(got == "herdr" || Path::new(&got).is_file());

    // 3) Real machine: resolves to something usable.
    match prev_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    let got = notch::resolve_herdr_cmd();
    assert!(
        Path::new(&got).is_file() || got == "herdr",
        "expected a usable herdr path, got {got}"
    );
}
