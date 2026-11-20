//! A8 DoD gate test — real in-process Daemon, fake herdr (allowed external
//! binary stand-in), real worker threads, real decision log. No model binary:
//! the Opencode runner seam is used only to script the two real judge calls.
//!
//! A self-declared `done` with no verifier decision must revert to Working,
//! log `verify_pending`, queue a Judge job which queues a Verify job, and only
//! reach Done after the verifier says YES.
//!
//! Run: cargo test -p notchd --test a8_dod_verify

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::opencode::Opencode;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a8dod-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn config(r: &Path) -> Config {
    std::fs::create_dir_all(r.join("tmp")).unwrap();
    Config {
        state_dir: r.join("state"),
        tmp_dir: r.join("tmp"),
        stuck_after_secs: 3600,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

fn herdr_says_done() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        match (
            args.get(1).map(String::as_str),
            args.get(2).map(String::as_str),
        ) {
            (Some("pane"), Some("split")) => {
                Ok(json!({"result":{"pane":{"pane_id":"w8d:p1"}}}).to_string())
            }
            (Some("agent"), Some("list")) => Ok(json!({"result":{"agents":[{
                "agent": "opencode", "agent_status": "done",
                "agent_session": {"value": "ses_dod"},
                "cwd": "/tmp", "pane_id": "w8d:p1", "revision": 1,
                "terminal_title": "OC | dod", "focused": false, "name": "dod"
            }]}})
            .to_string()),
            (Some("agent"), Some("read")) => Ok("session claims done\n".to_string()),
            _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
        }
    })
}

fn snap_value(d: &Daemon) -> Value {
    serde_json::to_value(d.snapshot()).unwrap()
}

fn session(d: &Daemon) -> Value {
    snap_value(d)["sessions"][0].clone()
}

fn wait_for(pred: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !pred() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::yield_now();
    }
}

#[test]
fn self_declared_done_reverts_queues_verify_and_waits_for_verifier() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root("gate");
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    let oc = Opencode::with_runner(move |argv: &[String]| {
        let prompt = argv.last().cloned().unwrap_or_default();
        seen.lock().unwrap().push(prompt.clone());
        if prompt.starts_with("You verify a definition of done") {
            Ok("YES: RESULT.md exists with required content".to_string())
        } else {
            Ok("DONE: RESULT.md exists".to_string())
        }
    });

    let daemon = Daemon::open(config(&r), herdr_says_done(), Arc::new(FakeClock::new(T0))).unwrap();
    daemon.set_opencode(oc);
    let ack = daemon.submit_task(
        "write RESULT.md".to_string(),
        Some(r.to_string_lossy().into_owned()),
        Some("dod".to_string()),
        Some("file:RESULT.md exists".to_string()),
        None,
        None,
    );
    assert!(ack.ok, "submit failed: {:?}", ack.error);

    // Poll 1: herdr says done -> apply marks Done -> gate reverts to Working,
    // logs verify_pending("... reverted"), and queues a Judge job.
    daemon.poll_once();
    assert_eq!(
        session(&daemon)["status"],
        json!("working"),
        "self-declared done reverted"
    );
    let decisions = snap_value(&daemon)["decisions"].as_array().unwrap().clone();
    assert!(
        decisions.iter().any(|d| {
            d["action"] == json!("verify_pending")
                && d["detail"].as_str().unwrap_or("").contains("reverted")
        }),
        "revert decision missing: {decisions:?}"
    );
    wait_for(
        || {
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.starts_with("You are the supervisor"))
        },
        "judge job after gate revert",
    );
    eprintln!("DOD GATE step 1: reverted to working, Judge job queued: HELD");

    // Poll 2: drain the Judge result (DONE) -> a Verify job is queued and run.
    daemon.poll_once();
    wait_for(
        || {
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.starts_with("You verify a definition of done"))
        },
        "verify job queued after judge DONE",
    );
    eprintln!("DOD GATE step 2: Verify job queued after judge DONE: HELD");

    // Poll 3: drain the verifier YES -> Done, with a real `done` decision.
    let mut became_done = false;
    for _ in 0..20 {
        daemon.poll_once();
        if session(&daemon)["status"] == json!("done") {
            became_done = true;
            break;
        }
        std::thread::yield_now();
    }
    assert!(
        became_done,
        "verifier YES must flip to done: {}",
        snap_value(&daemon)
    );
    let decisions = snap_value(&daemon)["decisions"].as_array().unwrap().clone();
    assert!(
        decisions.iter().any(|d| d["action"] == json!("done")),
        "verifier YES must log a done decision: {decisions:?}"
    );
    eprintln!("DOD GATE step 3: verifier YES -> done + done decision: HELD");
}

#[test]
fn verifier_no_keeps_session_working_without_done_decision() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root("no");
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    let oc = Opencode::with_runner(move |argv: &[String]| {
        let prompt = argv.last().cloned().unwrap_or_default();
        seen.lock().unwrap().push(prompt.clone());
        if prompt.starts_with("You verify a definition of done") {
            Ok("NO: RESULT.md is missing the required section".to_string())
        } else {
            Ok("DONE: RESULT.md exists".to_string())
        }
    });
    let daemon = Daemon::open(config(&r), herdr_says_done(), Arc::new(FakeClock::new(T0))).unwrap();
    daemon.set_opencode(oc);
    let ack = daemon.submit_task(
        "write RESULT.md".to_string(),
        Some(r.to_string_lossy().into_owned()),
        Some("dod-no".to_string()),
        Some("file:RESULT.md exists".to_string()),
        None,
        None,
    );
    assert!(ack.ok, "submit failed: {:?}", ack.error);

    for _ in 0..20 {
        daemon.poll_once();
        if calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.starts_with("You verify a definition of done"))
        {
            break;
        }
        std::thread::yield_now();
    }
    wait_for(
        || {
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.starts_with("You verify a definition of done"))
        },
        "verify job",
    );
    for _ in 0..4 {
        daemon.poll_once();
        std::thread::yield_now();
    }
    let snap = snap_value(&daemon);
    assert_ne!(
        snap["sessions"][0]["status"],
        json!("done"),
        "verifier NO must not mark done: {snap}"
    );
    assert_eq!(
        snap["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["action"] == json!("done"))
            .count(),
        0,
        "no done decision without verifier YES"
    );
    eprintln!("DOD GATE verifier NO: HELD (no done, no done decision)");
}
