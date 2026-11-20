//! V1 verifier finding: the decider glue in supervise.rs still used the shared
//! `decider_session_id()` and the daemon's cwd instead of A1's per-notch
//! session id (`ses_notch_<id>`) and the session's real cwd/worktree.
//! RED: the recorded judge argv carries `ses_notchd_supervisor_decider` and
//! the wrong cwd. Run: cargo test -p notchd --test judge_session_scope

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: Mutex<()> = Mutex::new(());

const SHIM: &str = r#"#!/usr/bin/env python3
import os, sys
with open(os.environ["JUDGE_LOG"], "a") as f:
    f.write(os.getcwd() + "\t" + " ".join(sys.argv[1:]) + "\n")
print("CONTINUE: keep going")
"#;

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-jscope-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn config(root: &Path) -> Config {
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 3600,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

fn idle_herdr() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("pane", "split") => Ok(json!({"result":{"pane":{"pane_id":"wJ:p1"}}}).to_string()),
            ("agent", "list") => Ok(json!({"result":{"agents":[{
                "agent":"opencode", "agent_status":"idle",
                "pane_id":"wJ:p1", "revision":1,
                "terminal_title":"OC | j", "focused":false, "name":"j"
            }]}})
            .to_string()),
            ("agent", "read") => Ok("idle tail\n".to_string()),
            _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
        }
    })
}

fn read_log(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(cwd, argv)| (cwd.to_string(), argv.to_string()))
        .collect()
}

#[test]
fn judge_uses_per_notch_session_id_and_real_cwd() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let r = root("scope");
    let cwd = r.join("session-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let shim = r.join("judge_shim");
    std::fs::write(&shim, SHIM).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = r.join("judge.log");

    std::env::set_var("NOTCH_OPENCODE_CMD", shim.to_str().unwrap());
    std::env::set_var("JUDGE_LOG", log.to_string_lossy().as_ref());
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");

    let daemon = Daemon::open(config(&r), idle_herdr(), Arc::new(FakeClock::new(T0))).unwrap();
    let ack = daemon.submit_task(
        "scope work".to_string(),
        Some(cwd.to_string_lossy().into_owned()),
        Some("scope".to_string()),
        None,
        None,
        None,
    );
    assert!(ack.ok, "submit failed: {:?}", ack.error);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        daemon.poll_once();
        if read_log(&log)
            .iter()
            .any(|(_, argv)| argv.contains("--session"))
            || Instant::now() >= deadline
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let calls = read_log(&log);
    assert!(
        !calls.is_empty(),
        "decoder judge never ran; snapshot={}",
        serde_json::to_value(daemon.snapshot()).unwrap()
    );

    let session_cwd = std::fs::canonicalize(&cwd)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut saw_scoped = false;
    for (call_cwd, argv) in &calls {
        if argv.contains("--session") {
            assert!(
                argv.contains("ses_notch_S1-OC01"),
                "shared judge id still wired: {argv}"
            );
            assert!(
                !argv.contains("ses_notchd_supervisor_decider"),
                "shared decider session leaked into per-session judgment: {argv}"
            );
            assert_eq!(
                call_cwd, &session_cwd,
                "judge ran in daemon cwd, not the session cwd/worktree: {call_cwd} != {session_cwd}"
            );
            saw_scoped = true;
        }
    }
    assert!(saw_scoped, "no persistent judge call recorded: {calls:?}");

    std::env::remove_var("NOTCH_OPENCODE_CMD");
    std::env::remove_var("JUDGE_LOG");
}
