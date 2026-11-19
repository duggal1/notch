//! P1 (E2E finding): a DoD-bearing session may not become `done` just because
//! herdr said `done`. A `decision{action:"done"}` from the evidence verifier
//! must be logged first. RED: the current apply path flips the status with
//! `decisions=[]`. Run: cargo test -p notchd --test dod_gate

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

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

fn root(tag: &str) -> std::path::PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-dod-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// herdr reports the pane as `done` with a stable revision. No verifier has
/// spoken, so the daemon must keep the session non-terminal.
fn herdr_says_done() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("pane", "split") => Ok(json!({
                "result": {"pane": {"pane_id": "wD:p1"}}
            })
            .to_string()),
            ("agent", "list") => Ok(json!({
                "result": {"agents": [{
                    "agent": "opencode",
                    "agent_status": "done",
                    "agent_session": {"value": "ses_dod"},
                    "cwd": "/tmp",
                    "pane_id": "wD:p1",
                    "revision": 1,
                    "terminal_title": "OC | dod",
                    "focused": false,
                    "name": "dod"
                }]}
            })
            .to_string()),
            ("agent", "read") => Ok("session claims it is complete\n".to_string()),
            _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
        }
    })
}

fn snap_value(daemon: &Daemon) -> Value {
    serde_json::to_value(daemon.snapshot()).unwrap()
}

fn done_decisions(snap: &Value) -> usize {
    snap["decisions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|d| d["action"] == json!("done"))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn dod_session_is_not_done_before_verifier_decision() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root("gate");
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = Daemon::open(config(&r), herdr_says_done(), clock).unwrap();
    // No model verdict available: the verifier never produces a `done`
    // decision, so the session must stay non-terminal across polls.
    daemon.set_opencode(Opencode::with_runner(|_| {
        Err(HerdrError::Io("model unavailable".to_string()))
    }));
    let ack = daemon.submit_task(
        "do the DoD work".to_string(),
        Some(r.to_string_lossy().into_owned()),
        Some("dod".to_string()),
        Some("file:RESULT.md exists".to_string()),
        None,
        None,
    );
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);

    daemon.poll_once();
    let snap = snap_value(&daemon);
    let sessions = snap["sessions"].as_array().unwrap();
    let session = sessions
        .iter()
        .find(|s| s["id"] == ack.session_id.as_deref().unwrap_or("S1-OC01"))
        .expect("session present");
    assert_ne!(
        session["status"],
        json!("done"),
        "herdr `done` must NOT flip a DoD-bearing session to done with no verifier decision; snapshot={snap}"
    );
    assert_eq!(
        done_decisions(&snap),
        0,
        "no `done` decision may exist before verification; snapshot={snap}"
    );

    // A second poll with the same herdr verdict is still gated (no spin into
    // a premature done, no duplicate verifier decisions).
    daemon.poll_once();
    let snap2 = snap_value(&daemon);
    let s2 = snap2["sessions"].as_array().unwrap().iter().find(|s| s["id"] == session["id"]).unwrap();
    assert_ne!(s2["status"], json!("done"), "second poll still gated; snapshot={snap2}");
}

/// Control: with a real decider that returns DONE then a verifier YES, the
/// session becomes done AND a `done` decision is logged — the gate must not
/// block legitimate evidence verification.
#[test]
fn dod_session_becomes_done_after_verifier_yes() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root("yes");
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = Daemon::open(config(&r), herdr_says_done(), clock).unwrap();
    daemon.set_opencode(Opencode::with_runner(|argv: &[String]| {
        let prompt = argv.last().cloned().unwrap_or_default();
        if prompt.starts_with("You verify a definition of done") {
            Ok("YES: RESULT.md exists with the required content".to_string())
        } else {
            Ok("DONE: RESULT.md exists\nEVIDENCE: file:RESULT.md exists".to_string())
        }
    }));
    let ack = daemon.submit_task(
        "write RESULT.md".to_string(),
        Some(r.to_string_lossy().into_owned()),
        Some("dod-yes".to_string()),
        Some("file:RESULT.md exists".to_string()),
        None,
        None,
    );
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);
    let id = ack.session_id.unwrap_or_else(|| "S1-OC01".to_string());

    let mut became_done = false;
    for _ in 0..8 {
        daemon.poll_once();
        let snap = snap_value(&daemon);
        if snap["sessions"].as_array().unwrap().iter().any(|s| s["id"] == json!(id) && s["status"] == json!("done")) {
            assert!(done_decisions(&snap) >= 1, "done must be accompanied by a verifier decision; snapshot={snap}");
            became_done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50)); // worker queue drain, not sync of logic
    }
    assert!(became_done, "session never reached done via verifier; snapshot={}", snap_value(&daemon));
}
