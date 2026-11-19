//! Hard-timeout coverage for the opencode subprocess seam (`opencode.rs`).
//!
//! Every opencode CLI call (`api GET /api/session`, `models`, `run`) is now
//! wrapped in a bounded `spawn` + `mpsc::recv_timeout`. A hung child is killed
//! and reaped, and a typed `HerdrError::Io("opencode timeout after <ms>ms")`
//! is returned so the daemon's existing `.ok()` fallback degrades to herdr
//! `agent_status` instead of wedging the poll.
//!
//! The shim is a self-contained Python script (no new deps) copied to a unique
//! path per test. Its filename is the `pgrep -f` marker used to prove the hung
//! child was reaped (no orphan/zombie). All tests share one global lock
//! because they mutate process-global environment variables.
//!
//! Run: cargo test -p notchd --test opencode_timeout

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::{is_timeout_error, timeout_ms, Opencode, DEFAULT_TIMEOUT_MS};

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const TEST_TIMEOUT_MS: u64 = 400;
const TIMEOUT_MARGIN_MS: u64 = 150;
const T0: u64 = 1_763_520_000_000;

static TMP_N: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: Mutex<()> = Mutex::new(());
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// A hung child must be a single, directly-killable process: `time.sleep`
// keeps the shim's unique filename in its cmdline for the pgrep proof.
const SHIM_SRC: &str = r#"#!/usr/bin/env python3
import os, sys, time
mode = os.environ.get("SHIM_MODE", "healthy")
counter = os.environ.get("SHIM_COUNTER", "")
if counter:
    try:
        with open(counter, "a") as f:
            f.write((sys.argv[1] if len(sys.argv) > 1 else "?") + "\n")
    except Exception:
        pass
if mode == "hang":
    time.sleep(float(os.environ.get("SHIM_SLEEP_SECS", "30")))
    sys.exit(0)
sub = sys.argv[1] if len(sys.argv) > 1 else ""
if sub == "api":
    print('{"cursor":null,"data":[{"id":"ses_test","time":{"busy":1000,"idle":2000},"location":{"directory":"/tmp"}}]}')
elif sub == "models":
    print("opencode/hy4-flash-free")
    print("mock/second-model")
elif sub == "run":
    print("CONTINUE: keep going")
else:
    sys.stderr.write("unsupported invocation\n")
    sys.exit(2)
"#;

fn tmp_root(tag: &str) -> PathBuf {
    let n = TMP_N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-timeout-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Write the shim to a unique path and return it. The filename is the marker.
fn write_shim(root: &Path, tag: &str) -> PathBuf {
    let n = TMP_N.fetch_add(1, Ordering::SeqCst);
    let path = root.join(format!("opencode_timeout_hangshim_{tag}_{}_{n}", std::process::id()));
    std::fs::write(&path, SHIM_SRC).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn marker_of(shim: &Path) -> String {
    shim.file_name().unwrap().to_string_lossy().into_owned()
}

fn set_env(shim: &Path, timeout_ms: u64) {
    std::env::set_var("NOTCH_OPENCODE_CMD", shim);
    std::env::set_var("NOTCH_OPENCODE_TIMEOUT_MS", timeout_ms.to_string());
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
}

/// Count live processes whose cmdline contains the unique shim marker and
/// (optionally) a subcommand token. A2's async judge pool spawns decider
/// shims too; those are a different process from the API call under test.
fn live_shim_count_filtered(marker: &str, sub: Option<&str>) -> usize {
    let Ok(out) = Command::new("pgrep").args(["-f", marker]).output() else {
        eprintln!("pgrep unavailable; skipping orphan check");
        return 0;
    };
    if out.status.success() {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter(|l| sub.map(|s| l.contains(s)).unwrap_or(true))
            .count()
    } else {
        0
    }
}

fn live_shim_count(marker: &str) -> usize {
    live_shim_count_filtered(marker, None)
}

fn assert_timeout(err: Option<&HerdrError>, expected_ms: u64, ctx: &str) {
    let err = err.unwrap_or_else(|| panic!("{ctx}: expected a timeout error, got Ok"));
    assert!(
        is_timeout_error(err),
        "{ctx}: expected typed timeout error, got {err:?}"
    );
    assert!(
        err.to_string().contains(&format!("opencode timeout after {expected_ms}ms")),
        "{ctx}: error does not carry the deadline: {err}"
    );
}

// ------------------------------- tests -------------------------------------

/// Each of the three opencode surfaces returns within `timeout + 150ms` on a
/// hanging shim, carries the typed timeout error, and leaves no orphan child.
#[test]
fn each_call_is_bounded_and_reaps_hung_child() {
    let _g = env_lock();
    let root = tmp_root("each");
    let shim = write_shim(&root, "each");
    let marker = marker_of(&shim);
    set_env(&shim, TEST_TIMEOUT_MS);
    std::env::set_var("SHIM_MODE", "hang");
    std::env::set_var("SHIM_SLEEP_SECS", "30");

    let oc = Opencode::from_env();

    let t = Instant::now();
    let r = oc.api_session_list();
    let api_ms = t.elapsed().as_millis() as u64;
    assert_timeout(r.err().as_ref(), TEST_TIMEOUT_MS, "api_session_list");
    assert!(api_ms < TEST_TIMEOUT_MS + TIMEOUT_MARGIN_MS, "api_session_list took {api_ms}ms");
    assert_eq!(live_shim_count(&marker), 0, "api_session_list left a live shim");

    let t = Instant::now();
    let r = oc.models();
    let models_ms = t.elapsed().as_millis() as u64;
    assert_timeout(r.err().as_ref(), TEST_TIMEOUT_MS, "models");
    assert!(models_ms < TEST_TIMEOUT_MS + TIMEOUT_MARGIN_MS, "models took {models_ms}ms");
    assert_eq!(live_shim_count(&marker), 0, "models left a live shim");

    let t = Instant::now();
    let r = oc.run_decider("mock/model", "do the work");
    let run_ms = t.elapsed().as_millis() as u64;
    assert_timeout(r.err().as_ref(), TEST_TIMEOUT_MS, "run_decider");
    assert!(run_ms < TEST_TIMEOUT_MS + TIMEOUT_MARGIN_MS, "run_decider took {run_ms}ms");
    assert_eq!(live_shim_count(&marker), 0, "run_decider left a live shim");

    // The persistent-session variant shares the same seam.
    let t = Instant::now();
    let r = oc.run_decider_session("mock/model", "ses_test", "do the work");
    let sess_ms = t.elapsed().as_millis() as u64;
    assert_timeout(r.err().as_ref(), TEST_TIMEOUT_MS, "run_decider_session");
    assert!(sess_ms < TEST_TIMEOUT_MS + TIMEOUT_MARGIN_MS, "run_decider_session took {sess_ms}ms");
    assert_eq!(live_shim_count(&marker), 0, "run_decider_session left a live shim");
}

/// Three consecutive hangs neither panic nor wedge the client: all three are
/// bounded, all children are reaped, and the seam still works once healthy.
#[test]
fn three_consecutive_hangs_never_wedge() {
    let _g = env_lock();
    let root = tmp_root("triple");
    let shim = write_shim(&root, "triple");
    let marker = marker_of(&shim);
    set_env(&shim, TEST_TIMEOUT_MS);
    std::env::set_var("SHIM_MODE", "hang");
    std::env::set_var("SHIM_SLEEP_SECS", "30");

    let oc = Opencode::from_env();
    for i in 0..3 {
        let t = Instant::now();
        let r = oc.api_session_list();
        let ms = t.elapsed().as_millis() as u64;
        assert_timeout(r.err().as_ref(), TEST_TIMEOUT_MS, &format!("hang #{i}"));
        assert!(ms < TEST_TIMEOUT_MS + TIMEOUT_MARGIN_MS, "hang #{i} took {ms}ms");
        assert_eq!(live_shim_count(&marker), 0, "hang #{i} left a live shim");
    }

    std::env::set_var("SHIM_MODE", "healthy");
    let sessions = oc.api_session_list().expect("client wedged after three hangs");
    assert_eq!(sessions.len(), 1, "healthy call after hangs must work");
    assert_eq!(sessions[0].id, "ses_test");
}

/// A healthy shim still parses through every surface.
#[test]
fn healthy_shim_still_works() {
    let _g = env_lock();
    let root = tmp_root("healthy");
    let shim = write_shim(&root, "healthy");
    let counter = root.join("calls.log");
    set_env(&shim, TEST_TIMEOUT_MS);
    std::env::set_var("SHIM_MODE", "healthy");
    std::env::set_var("SHIM_COUNTER", &counter);

    let oc = Opencode::from_env();
    let sessions = oc.api_session_list().expect("healthy api_session_list");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "ses_test");
    assert!(notchd::opencode::is_idle(&sessions[0]));

    let models = oc.models().expect("healthy models");
    assert_eq!(models, vec!["opencode/hy4-flash-free", "mock/second-model"]);

    let out = oc.run_decider("m", "p").expect("healthy run_decider");
    assert!(out.contains("CONTINUE"), "unexpected decider output: {out}");

    let calls = std::fs::read_to_string(&counter).unwrap_or_default();
    assert_eq!(calls.lines().count(), 3, "each surface must hit the shim once");
}

/// The daemon's 5-minute `list_models` cache still works, and a hanging shim
/// cannot affect a warm cache (no subprocess is spawned on the cached path).
#[test]
fn models_cached_path_still_works_under_hang() {
    let _g = env_lock();
    let root = tmp_root("cache");
    let shim = write_shim(&root, "cache");
    let counter = root.join("models.log");
    set_env(&shim, TEST_TIMEOUT_MS);
    std::env::set_var("SHIM_MODE", "healthy");
    std::env::set_var("SHIM_COUNTER", &counter);

    let config = test_config(&root);
    let daemon = Daemon::open(config, ok_herdr(), Arc::new(FakeClock::new(T0))).unwrap();
    daemon.set_opencode(Opencode::from_env());

    let first = daemon.list_models();
    assert!(first.ok, "first list_models failed: {:?}", first.error);
    assert_eq!(first.models.len(), 2);
    assert_eq!(count_lines(&counter), 1, "first call should invoke the shim once");

    // Warm cache: switch the shim to hang. A cached answer must return fast
    // and must not spawn another child.
    std::env::set_var("SHIM_MODE", "hang");
    std::env::set_var("SHIM_SLEEP_SECS", "30");
    let t = Instant::now();
    let second = daemon.list_models();
    let ms = t.elapsed().as_millis() as u64;
    assert!(second.ok, "cached list_models failed: {:?}", second.error);
    assert_eq!(second.models, first.models);
    assert!(ms < TEST_TIMEOUT_MS, "cached list_models blocked for {ms}ms");
    assert_eq!(count_lines(&counter), 1, "cached path must not respawn the shim");
}

/// Real daemon + fake herdr: one `poll_once` under a hanging opencode returns
/// within the deadline and still falls back to herdr agent_status (stuck kill).
#[test]
fn poll_once_is_bounded_and_falls_back_to_herdr() {
    let _g = env_lock();
    let root = tmp_root("poll");
    let shim = write_shim(&root, "poll");
    let marker = marker_of(&shim);
    set_env(&shim, TEST_TIMEOUT_MS);
    std::env::set_var("SHIM_MODE", "healthy");
    std::env::set_var("SHIM_COUNTER", root.join("poll-calls.log"));

    let clock = Arc::new(FakeClock::new(T0));
    let config = test_config(&root);
    let daemon = Daemon::open(config, working_herdr(), clock.clone()).unwrap();
    daemon.set_opencode(Opencode::from_env());
    let ack = daemon.submit_task(
        "do bounded work".to_string(),
        Some(root.to_string_lossy().into_owned()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);

    // Baseline poll at T0 records the (unchanged) herdr revision.
    daemon.poll_once();

    // Hang the shim and advance past the stuck threshold. The poll must not
    // block for the child's 30s lifetime.
    std::env::set_var("SHIM_MODE", "hang");
    std::env::set_var("SHIM_SLEEP_SECS", "30");
    // Advance past the stuck threshold but stay inside the 3x-poll-interval
    // stall-grace window (A3): a 10 s jump would grant grace by design.
    clock.advance_ms(5_000);
    let t = Instant::now();
    daemon.poll_once();
    let ms = t.elapsed().as_millis() as u64;
    assert!(ms < TEST_TIMEOUT_MS + TIMEOUT_MARGIN_MS, "poll_once blocked for {ms}ms");
    // The API shim must be dead; judge-pool decider shims are a separate
    // (asynchronous) process and are not what this assertion measures.
    assert_eq!(
        live_shim_count_filtered(&marker, Some("api GET")),
        0,
        "poll_once left a live API shim"
    );

    // With /api/session unavailable, the watchdog fell back to the stable
    // herdr `agent_status` and classified the session as stuck. Since A3's
    // "stuck is not failure" change the honest signal is a stuck escalation,
    // not `failureReason` (which stays null when kill_on_stuck is OFF).
    let snap: Value = serde_json::to_value(daemon.snapshot()).unwrap();
    let stuck = snap["escalations"]
        .as_array()
        .map(|items| items.iter().any(|e| e["kind"] == json!("stuck")))
        .unwrap_or(false);
    assert!(stuck, "expected herdr-status fallback to escalate as stuck; snapshot: {snap}");
}

/// Env override is parsed safely: an invalid value falls back to the 1500ms
/// default (with a warning) instead of panicking or picking zero.
#[test]
fn timeout_env_override_is_safe() {
    let _g = env_lock();
    let root = tmp_root("env");
    let shim = write_shim(&root, "env");
    let marker = marker_of(&shim);
    set_env(&shim, 250);
    std::env::set_var("SHIM_MODE", "hang");
    std::env::set_var("SHIM_SLEEP_SECS", "30");

    assert_eq!(timeout_ms(), 250, "valid override should be honored");
    let oc = Opencode::from_env();
    let t = Instant::now();
    let r = oc.models();
    let ms = t.elapsed().as_millis() as u64;
    assert_timeout(r.err().as_ref(), 250, "250ms override");
    assert!(ms < 250 + TIMEOUT_MARGIN_MS, "override deadline missed: {ms}ms");
    assert_eq!(live_shim_count(&marker), 0);

    std::env::set_var("NOTCH_OPENCODE_TIMEOUT_MS", "definitely-not-a-number");
    assert_eq!(timeout_ms(), DEFAULT_TIMEOUT_MS, "invalid env should fall back");
    let t = Instant::now();
    let r = oc.models();
    let ms = t.elapsed().as_millis() as u64;
    assert_timeout(r.err().as_ref(), DEFAULT_TIMEOUT_MS, "invalid env fallback");
    assert!(ms >= DEFAULT_TIMEOUT_MS, "fallback deadline too short: {ms}ms");
    assert!(ms < DEFAULT_TIMEOUT_MS + 300, "fallback deadline too long: {ms}ms");
    assert_eq!(live_shim_count(&marker), 0);

    std::env::remove_var("NOTCH_OPENCODE_TIMEOUT_MS");
    assert_eq!(timeout_ms(), DEFAULT_TIMEOUT_MS, "unset env should use the default");
}

// ------------------------------- helpers -----------------------------------

fn test_config(root: &Path) -> Config {
    let config = Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 2,
        poll_interval_ms: 2000,
        opencode_decider: false,
    };
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    config
}

fn count_lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// Minimal herdr for paths that never spawn a session (list_models).
fn ok_herdr() -> Herdr {
    Herdr::with_runner(|_args: &[String]| Ok(r#"{"id":"x","result":{"type":"ok"}}"#.to_string()))
}

/// Fake herdr that reports one stable "working" agent with a fixed revision,
/// so the watchdog's stuck verdict is driven only by the fake clock.
fn working_herdr() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("pane", "split") => Ok(json!({
                "id": "cli:pane:split",
                "result": {"type": "pane_info", "pane": {
                    "pane_id": "wT:p1", "tab_id": "w1:t1", "workspace_id": "w1"
                }}
            })
            .to_string()),
            ("agent", "list") => Ok(json!({
                "id": "cli:agent:list",
                "result": {"type": "agent_list", "agents": [{
                    "agent": "opencode",
                    "agent_status": "working",
                    "agent_session": {"value": "ses_test"},
                    "cwd": "/tmp",
                    "focused": false,
                    "name": "test",
                    "pane_id": "wT:p1",
                    "revision": 1,
                    "terminal_title": "OC | test",
                    "tab_id": "w1:t1",
                    "workspace_id": "w1"
                }]}
            })
            .to_string()),
            ("agent", "read") => Ok("still working\n".to_string()),
            _ => Ok(r#"{"id":"x","result":{"type":"ok"}}"#.to_string()),
        }
    })
}
