//! A8 zero-coverage modules: CLI, paths, plan, snapshot (A6 matrix).
//! Real filesystem, real git where relevant, real `notchd` binary for the CLI.
//! No fake CLI, no mock of the modules under test. snapshot.rs is A4's source
//! lane; these tests only call its public functions from a separate file.
//!
//! Run: cargo test -p notchd --test a8_zero_coverage

use notchd::model::BusMessageEntry;
use notchd::paths;
use notchd::plan::{load_plan, parse_plan};
use notchd::snapshot::{cap_bus_messages_newest_first, rollup};
use notchd::status::Status;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static N: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a8zc-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

// ------------------------------ paths.rs -----------------------------------

#[test]
fn paths_honor_env_overrides_and_fallbacks() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let r = root("paths");
    let prev_state = std::env::var_os("NOTCH_STATE_DIR");
    let prev_sock = std::env::var_os("NOTCH_SOCK");
    let prev_home = std::env::var_os("HOME");

    std::env::set_var("NOTCH_STATE_DIR", &r);
    assert_eq!(paths::default_state_dir(), r);
    assert_eq!(paths::default_sock_path(), r.join("notch.sock"));
    assert_eq!(paths::ws_token_path(), r.join("ws.token"));
    assert_eq!(paths::ws_token_path_in(&r), r.join("ws.token"));

    let custom = r.join("custom.sock");
    std::env::set_var("NOTCH_SOCK", &custom);
    assert_eq!(paths::default_sock_path(), custom);

    // Empty env values are treated as unset.
    std::env::set_var("NOTCH_STATE_DIR", "");
    std::env::set_var("HOME", &r);
    assert_eq!(
        paths::default_state_dir(),
        r.join("Library/Application Support/notch")
    );
    std::env::set_var("NOTCH_SOCK", "");
    assert_eq!(
        paths::default_sock_path(),
        r.join("Library/Application Support/notch/notch.sock")
    );
    assert_eq!(
        paths::fallback_sock_path(),
        std::env::temp_dir().join("notch").join("notch.sock")
    );

    match prev_state {
        Some(v) => std::env::set_var("NOTCH_STATE_DIR", v),
        None => std::env::remove_var("NOTCH_STATE_DIR"),
    }
    match prev_sock {
        Some(v) => std::env::set_var("NOTCH_SOCK", v),
        None => std::env::remove_var("NOTCH_SOCK"),
    }
    match prev_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    eprintln!("PATHS: HELD");
}

// ------------------------------- plan.rs -----------------------------------

#[test]
fn plan_parses_alias_fallbacks_and_rejects_broken_plans() {
    let r = root("plan");
    let a = r.join("a");
    let b = r.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let good = format!(
        r#"{{
            "stuckAfterSecs": 7,
            "supervisors": [
                {{"cwd": {a:?}, "sessions": [
                    {{"prompt": "  do the work \n and more  ", "cwd": {b:?}}},
                    {{"prompt": "second task", "title": "  Custom  "}}
                ]}}
            ]
        }}"#
    );
    let parsed = parse_plan(&good).expect("valid plan");
    assert_eq!(parsed.stuck_after_secs, Some(7)); // camelCase alias
    assert_eq!(parsed.supervisors.len(), 1);
    let sessions = &parsed.supervisors[0];
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].cwd, b.to_string_lossy()); // session cwd wins
    assert_eq!(sessions[1].cwd, a.to_string_lossy()); // supervisor fallback
    assert_eq!(sessions[0].title, "do the work"); // prompt first-line fallback
    assert_eq!(sessions[1].title, "Custom");
    assert_eq!(sessions[0].task_head(), "do the work");

    let path = r.join("plan.json");
    std::fs::write(&path, &good).unwrap();
    let loaded = load_plan(&path).unwrap();
    assert_eq!(loaded.stuck_after_secs, Some(7));

    let bad_cases = [
        ("not json", "invalid plan JSON"),
        (
            r#"{"supervisors":[{"sessions":[{"prompt":"x"}]}]}"#,
            "missing cwd",
        ),
        (
            r#"{"supervisors":[{"cwd":"/definitely/not/here","sessions":[{"prompt":"x"}]}]}"#,
            "is not a directory",
        ),
        (
            &format!(r#"{{"supervisors":[{{"cwd":{a:?},"sessions":[{{"prompt":"  "}}]}}]}}"#),
            "empty prompt",
        ),
    ];
    for (text, expect) in bad_cases {
        let err = parse_plan(text).unwrap_err();
        assert!(err.contains(expect), "expected `{expect}` in `{err}`");
    }
    let err = load_plan(&r.join("absent.json")).unwrap_err();
    assert!(err.contains("cannot read plan"), "{err}");
    eprintln!("PLAN: HELD");
}

// ----------------------------- snapshot.rs ---------------------------------

fn bus(id: &str, at: u64) -> BusMessageEntry {
    BusMessageEntry {
        id: id.to_string(),
        from: "S1-OC01".into(),
        to: "S1-OC02".into(),
        kind: "handoff".into(),
        preview: "same body".into(),
        at,
    }
}

#[test]
fn snapshot_bus_projection_dedups_caps_and_orders_newest_first() {
    let entries = vec![
        bus("old", 1),
        bus("new", 9),
        bus("dup-new", 9), // identical payload/at -> collapses with "new"
        bus("mid", 5),
    ];
    let out = cap_bus_messages_newest_first(&entries, 2);
    assert_eq!(out.len(), 2, "dedup then cap");
    assert_eq!(out[0].id, "new", "newest first");
    assert_eq!(out[1].id, "mid");
    let zero = cap_bus_messages_newest_first(&entries, 0);
    assert!(zero.is_empty());
    eprintln!("SNAPSHOT bus projection: HELD");
}

#[test]
fn snapshot_rollup_precedence_is_working_then_failed_then_done() {
    assert_eq!(rollup(std::iter::empty()), Status::Idle);
    assert_eq!(
        rollup([Status::Failed, Status::Working, Status::Done].into_iter()),
        Status::Working,
        "working beats failed"
    );
    assert_eq!(
        rollup([Status::Done, Status::Failed, Status::Idle].into_iter()),
        Status::Failed,
        "failed beats done/idle"
    );
    assert_eq!(
        rollup([Status::Done, Status::Done].into_iter()),
        Status::Done
    );
    assert_eq!(
        rollup([Status::Idle, Status::Done].into_iter()),
        Status::Idle
    );
    eprintln!("SNAPSHOT rollup: HELD");
}

// -------------------------------- cli.rs -----------------------------------

/// Run the REAL built binary with an isolated state dir and socket. `status`
/// and `snapshot` never call herdr, so no external binary is needed.
fn cli(r: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_notchd"))
        .args(args)
        .env("NOTCH_STATE_DIR", r.join("state"))
        .env("NOTCH_SOCK", r.join("absent.sock"))
        .env("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn cli_status_and_snapshot_print_real_state_json() {
    let r = root("cli-state");
    let (code, stdout, stderr) = cli(&r, &["status"]);
    assert_eq!(code, 0, "status failed: {stderr}");
    let v: Value = serde_json::from_str(stdout.trim()).expect("status must be JSON");
    assert!(v.get("sessions").is_some(), "{v}");

    let (code, stdout, stderr) = cli(&r, &["snapshot"]);
    assert_eq!(code, 0, "snapshot failed: {stderr}");
    let v: Value = serde_json::from_str(stdout.trim()).expect("snapshot must be pretty JSON");
    assert_eq!(v["version"], 1, "{v}");
    assert!(
        v.get("sessions").is_some() && v.get("supervisors").is_some(),
        "{v}"
    );
    assert!(
        r.join("state").exists(),
        "CLI created the isolated state dir"
    );
    eprintln!("CLI status/snapshot (real binary): HELD");
}

#[test]
fn cli_run_plan_with_no_sessions_and_bad_usage_exit_codes() {
    let r = root("cli-plan");
    let plan = r.join("empty-plan.json");
    std::fs::write(&plan, r#"{"stuck_after_secs":5,"supervisors":[]}"#).unwrap();
    let (code, stdout, stderr) = cli(&r, &["run-plan", plan.to_str().unwrap()]);
    assert_eq!(code, 0, "empty run-plan must ack: {stdout} {stderr}");
    let v: Value = serde_json::from_str(stdout.trim()).expect("ack must be JSON");
    assert_eq!(v["ok"], true, "{v}");

    // Bad usage never touches herdr or state: exact exit codes from cli::run.
    let (code, _, _) = cli(&r, &["submit"]);
    assert_eq!(code, 2, "submit without task is usage error");
    let (code, _, _) = cli(&r, &["nonsense"]);
    assert_eq!(code, 2, "unknown command is usage error");
    let (code, _, _) = cli(&r, &["logs"]);
    assert_eq!(code, 2, "logs without id is usage error");
    eprintln!("CLI run-plan/usage exit codes (real binary): HELD");
}
