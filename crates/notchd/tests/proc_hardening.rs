//! Hardening tests for the single bounded subprocess runner (`proc.rs`).
//!
//! T2 (fault-injection shim matrix): every child class — hang, ignore SIGTERM,
//! 1 GB stdout, garbage JSON, setsid grandchild, byte drip, fd leak — returns
//! within `timeout + 500ms`, leaves no survivor in its process group, keeps RSS
//! growth bounded, and the daemon still answers status.
//!
//! Phase 0: the API-shape test below is the frozen contract A1/A3/A5 code
//! against. Run: cargo test -p notchd --test proc_hardening

use notchd::proc::{
    run, Out, ProcError, Spec, PIPE_EOF_GRACE, TIMEOUT_DECIDER, TIMEOUT_GIT,
    TIMEOUT_HERDR_LIST, TIMEOUT_HERDR_MUTATE, TIMEOUT_HERDR_READ, TIMEOUT_OPENCODE_API,
};
use std::path::Path;
use std::time::Duration;

/// The frozen Phase 0 signatures compile and the one-place timeout constants
/// hold their documented values.
#[test]
fn proc_contract_signatures_and_timeouts() {
    let _run_ptr: fn(&Spec<'_>) -> Result<Out, ProcError> = run;
    assert_eq!(TIMEOUT_HERDR_LIST, Duration::from_secs(5));
    assert_eq!(TIMEOUT_HERDR_READ, Duration::from_secs(5));
    assert_eq!(TIMEOUT_HERDR_MUTATE, Duration::from_secs(10));
    assert_eq!(TIMEOUT_OPENCODE_API, Duration::from_secs(8));
    assert_eq!(TIMEOUT_GIT, Duration::from_secs(5));
    assert_eq!(TIMEOUT_DECIDER, Duration::from_secs(90));
    assert_eq!(PIPE_EOF_GRACE, Duration::from_millis(500));

    let args: Vec<String> = vec!["list".into()];
    let env: Vec<(String, String)> = Vec::new();
    let spec = Spec {
        cmd: "/usr/bin/true",
        args: &args,
        cwd: Some(Path::new("/")),
        env: &env,
        timeout: Duration::from_millis(100),
        max_out: 1024,
    };
    // Construction must not require `'static` or owned data.
    assert_eq!(spec.max_out, 1024);

    assert!(ProcError::Timeout { after: Duration::from_millis(250) }
        .to_string()
        .contains("250ms"));
    assert!(ProcError::Spawn("nope".into()).to_string().contains("nope"));
    assert!(ProcError::Failed { code: Some(7), stderr: "bad".into() }
        .to_string()
        .contains("bad"));
    let _ = Out::default();
}
