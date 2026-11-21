//! A1 verification of the V1 gate: the judge must use a per-notch-session
//! `ses_notch_<id>` id and the session's REAL worktree/cwd, not the shared
//! decider id or the daemon cwd (/).
//!
//! RED until A2 lands the 3-line glue in daemon/supervise.rs. This test uses a
//! REAL subprocess shim as `NOTCH_OPENCODE_CMD` so cwd is observable (the
//! in-process runner seam only receives argv).
//!
//! Run: cargo test -p notchd --test a1_judge_wiring

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::opencode::Opencode;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static ENV_LOCK: Mutex<()> = Mutex::new(());
const T0: u64 = 1_763_520_000_000;

fn tmp_root(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("notchd-a1-wiring-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fake_herdr() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        let a = args.get(1).map(String::as_str).unwrap_or("");
        let b = args.get(2).map(String::as_str).unwrap_or("");
        match (a, b) {
            ("pane", "split") => Ok(json!({"id":"x","result":{"type":"pane_info","pane":{
                "pane_id":"wT:p1","tab_id":"w1:t1","workspace_id":"w1"}}}).to_string()),
            ("agent", "start") => Ok(json!({"id":"x","result":{"type":"agent_started","agent":{
                "pane_id":"wT:p1","agent_status":"working","revision":1}}}).to_string()),
            ("agent", "list") => Ok(json!({"id":"x","result":{"type":"agent_list","agents":[{
                "pane_id":"wT:p1","agent_status":"idle","agent_session":{"value":"ses_test"},
                "cwd":"/tmp","revision":1,"name":"test"}]}}).to_string()),
            ("agent", "read") => Ok("working\n".to_string()),
            _ => Ok(r#"{"id":"x","result":{"type":"ok"}}"#.to_string()),
        }
    })
}

#[test]
fn judge_uses_per_session_id_and_real_worktree_cwd() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_root("judge");
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    // A real git repo makes A5 allocate a real per-session worktree; the test
    // must follow that worktree, never the daemon cwd.
    for args in [
        vec!["init", "-q"],
        vec!["-c", "user.email=a@b", "-c", "user.name=a", "commit", "--allow-empty", "-qm", "init"],
    ] {
        let status = std::process::Command::new("git")
            .args(&args)
            .current_dir(&work)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    let log = root.join("judge-calls.jsonl");
    let shim = root.join("opencode_shim");
    std::fs::write(
        &shim,
        format!(
            "#!/usr/bin/env python3\nimport json, os, sys\n\
             with open({:?}, 'a') as f:\n\
             \x20   f.write(json.dumps({{'cwd': os.getcwd(), 'argv': sys.argv[1:]}}) + '\\n')\n\
             print('CONTINUE: keep going')\n",
            log.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

    std::env::set_var("NOTCH_OPENCODE_CMD", &shim);
    std::env::set_var("NOTCH_OPENCODE_TIMEOUT_MS", "5000");
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/bin/true");

    let config = Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 3,
        poll_interval_ms: 1000,
        opencode_decider: false,
    };
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    // A5's layout: <tmp_dir>/notch-<id>/worktree. The judge must run there.
    let real_cwd = std::fs::canonicalize(config.tmp_dir.join("notch-S1-OC01").join("worktree"))
        .unwrap_or_else(|_| config.tmp_dir.join("notch-S1-OC01").join("worktree"))
        .to_string_lossy()
        .into_owned();
    let clock = ArcClock(Arc::new(FakeClock::new(T0)));
    let daemon = Daemon::open(config, fake_herdr(), clock.0.clone()).unwrap();
    daemon.set_opencode(Opencode::from_env());

    let ack = daemon.submit_task(
        "verify real evidence".into(),
        Some(work.to_string_lossy().into_owned()),
        None,
        Some("cargo test passes on main".into()), // not locally checkable
        None,
        None,
    );
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);
    let sid = ack.session_id.unwrap();
    assert_eq!(sid, "S1-OC01");

    daemon.poll_once();
    clock.0.advance_ms(4_000);
    daemon.poll_once();
    // Drain the async judge result once so the worker is definitely done.
    std::thread::sleep(Duration::from_millis(200));
    daemon.poll_once();

    // Condition-based wait for the shim log (not a fixed sleep as sync).
    let deadline = Instant::now() + Duration::from_secs(5);
    while !log.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    // The daemon also probes `api GET /api/session`; pick the `run` call.
    let judge_line = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["argv"].as_array().map(|a| a.first().and_then(Value::as_str) == Some("run")).unwrap_or(false))
        .unwrap_or_else(|| panic!("no decider run call in shim log: {text}"));
    let argv: Vec<String> = judge_line["argv"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let cwd = judge_line["cwd"].as_str().unwrap_or("").to_string();

    let expected_sid = notchd::decider::decider_session_id_for(&sid);
    assert!(
        argv.iter().any(|a| a == &expected_sid),
        "judge must use {expected_sid}; argv={argv:?}"
    );
    assert!(
        !argv.iter().any(|a| a == notchd::decider::DEFAULT_DECIDER_SESSION),
        "shared decider id leaked into judge: argv={argv:?}"
    );

    // macOS exposes /var and /private/var as the same directory; compare
    // canonical forms so a symlink spelling never causes a false failure.
    let cwd_canon = std::fs::canonicalize(&cwd).unwrap_or_else(|_| PathBuf::from(&cwd));
    let expected_canon = std::fs::canonicalize(&real_cwd).unwrap_or_else(|_| PathBuf::from(&real_cwd));
    assert_eq!(cwd_canon, expected_canon, "judge cwd must be the real worktree/cwd, not {cwd}");
    assert!(Path::new(&cwd).ends_with("worktree"), "judge cwd must be the worktree: {cwd}");
    assert_ne!(
        cwd,
        std::env::current_dir().unwrap_or_default().to_string_lossy(),
        "judge must not inherit the daemon cwd"
    );

    std::env::remove_var("NOTCH_OPENCODE_CMD");
    std::env::remove_var("NOTCH_OPENCODE_TIMEOUT_MS");
    eprintln!("A1 wiring verified: sid={expected_sid} cwd={cwd}");
}

/// Tiny wrapper so the FakeClock Arc is easy to advance in the test body.
struct ArcClock(Arc<FakeClock>);
