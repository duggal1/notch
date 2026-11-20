//! A5's open contract: the WIP-snapshot hook must be registered in
//! `Daemon::open`, not only in `spawn_one`, so a session recovered from
//! state.json (never spawned in this process) is snapshotted before a stuck
//! pane close. Two assertions: (1) open registered exactly one hook, (2) a
//! recovered dirty worktree is committed with the agreed WIP message before
//! the pane closes.
//!
//! Run: cargo test -p notchd --test a2_review_a5_hook

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::supervisor::register_snapshot_hook;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a2rev-a5-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git available");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn recovered_session_snapshots_wip_before_stuck_close() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root();
    let repo = r.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "notch@localhost"]);
    git(&repo, &["config", "user.name", "notch"]);
    std::fs::write(repo.join("EVIDENCE.txt"), "initial\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "initial"]);
    std::fs::write(repo.join("EVIDENCE.txt"), "STATUS: PASS\n").unwrap(); // uncommitted WIP

    let state_dir = r.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let state: Value = json!({
        "version": 1,
        "supervisors": [{"id": "SV-1", "number": 1, "title": "recovered"}],
        "sessions": [{
            "id": "S1-OC01", "sup": "SV-1", "status": "working",
            "title": "recovered", "task": "recovered work",
            "cwd": repo.to_string_lossy(), "paneId": "wR:p1",
            "startedAt": T0, "lastRevision": 7, "goal": null,
            "definitionOfDone": null, "iterations": 0, "maxIterations": 8,
            "escalated": false
        }],
        "attribution": [], "conflicts": [], "warnings": [],
        "stuck_after_secs": 2, "paused_supervisors": [],
        "decisions": [], "escalations": [], "bus_messages": [],
        "esc_seq": 0, "msg_seq": 0
    });
    std::fs::write(state_dir.join("state.json"), state.to_string()).unwrap();

    let config = Config {
        state_dir,
        tmp_dir: r.join("tmp"),
        stuck_after_secs: 2,
        poll_interval_ms: 1000,
        opencode_decider: false,
    };
    std::fs::create_dir_all(&config.tmp_dir).unwrap();

    let calls = Arc::new(AtomicU64::new(0));
    let pane_closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let closed_flag = Arc::clone(&pane_closed);
    let herdr = {
        let counter = Arc::clone(&calls);
        Herdr::with_runner(move |args: &[String]| {
            let sub = args.get(1).map(String::as_str).unwrap_or("");
            let sub2 = args.get(2).map(String::as_str).unwrap_or("");
            match (sub, sub2) {
                ("agent", "list") => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let rev = 8;
                    Ok(json!({"result":{"agents":[{
                        "agent":"opencode", "agent_status":"working",
                        "pane_id":"wR:p1", "revision":rev,
                        "terminal_title":"OC | recovered", "focused":false, "name":"recovered"
                    }]}})
                    .to_string())
                }
                ("agent", "read") => Ok("no progress\n".to_string()),
                ("pane", "close") => {
                    closed_flag.store(true, Ordering::SeqCst);
                    Ok(r#"{"result":{"type":"ok"}}"#.to_string())
                }
                _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
            }
        })
    };

    let clock = Arc::new(FakeClock::new(T0));
    let daemon = Daemon::open(config, herdr, clock.clone()).unwrap();
    // (1) open() must have installed the exact hook before any kill path.
    let second_registration = register_snapshot_hook(|_, _, _| Ok(None));
    assert!(
        second_registration.is_err(),
        "Daemon::open must register the worktree snapshot hook for recovered sessions"
    );

    // No spawn_one in this process. First poll adopts the pane + new revision.
    daemon.poll_once();
    for _ in 0..4 {
        clock.advance_ms(1_000);
        daemon.poll_once();
    }

    // (2) the WIP was committed before the pane was closed.
    let subjects = git(&repo, &["log", "--format=%s"]);
    assert!(
        subjects.contains("notch: WIP snapshot (stuck) — S1-OC01"),
        "recovered dirty worktree was not snapshotted before kill; git log: {subjects}"
    );
    assert!(
        pane_closed.load(Ordering::SeqCst),
        "pane should have been closed after the snapshot"
    );
    assert_eq!(
        git(&repo, &["status", "--porcelain"]),
        "",
        "WIP must be committed, not left dirty"
    );
}
