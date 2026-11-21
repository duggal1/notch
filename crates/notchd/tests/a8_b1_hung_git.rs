#[path = "fixtures/headless_herdr.rs"]
mod headless_herdr;

use headless_herdr::HeadlessHerdr;
use notchd::clock::FakeClock;
use notchd::status::Status;
use notchd::{Config, Daemon, Herdr};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

#[test]
fn snapshot_returns_in_under_one_second_while_wip_git_is_hung() {
    let server = HeadlessHerdr::start();
    let herdr = Herdr::from_env();
    let root = &server.root;
    let cwd = root.join("repo");
    std::fs::create_dir(&cwd).unwrap();
    git(&cwd, &["init", "-q"]);
    git(&cwd, &["config", "--local", "core.hooksPath", ".git/hooks"]);
    git(&cwd, &["config", "--local", "user.name", "Notch"]);
    git(
        &cwd,
        &["config", "--local", "user.email", "notch@localhost"],
    );
    git(&cwd, &["commit", "--allow-empty", "-qm", "initial"]);
    std::fs::write(cwd.join("wip.txt"), "unsaved task result").unwrap();

    let pane = herdr.pane_split(cwd.to_str().unwrap(), &[]).unwrap();
    server.report(&pane, "working");
    let state = root.join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(
        state.join("state.json"),
        json!({
            "version": 1,
            "sessions": [{
                "id":"S1-OC01", "sup":"SV-1", "status":"working", "title":"Hung Git",
                "task":"save result", "cwd":cwd, "paneId":pane
            }]
        })
        .to_string(),
    )
    .unwrap();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = Arc::new(
        Daemon::open(
            Config {
                state_dir: state,
                tmp_dir: root.join("tmp"),
                stuck_after_secs: 15,
                poll_interval_ms: 2000,
                opencode_decider: false,
            },
            herdr.clone(),
            clock.clone(),
        )
        .unwrap(),
    );

    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    for _ in 0..14 {
        clock.advance_ms(2000);
        daemon.poll_once();
    }

    // A real Git pre-commit hook signals that Git reached the commit, then
    // blocks on a FIFO with no writer. proc::run enforces the Git deadline.
    let marker = cwd.join(".git/commit-started");
    assert!(Command::new("mkfifo")
        .arg(cwd.join(".git/hold-commit"))
        .status()
        .unwrap()
        .success());
    let hook = cwd.join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\nprintf started > .git/commit-started\nread -r line < .git/hold-commit\n",
    )
    .unwrap();
    std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let polling = daemon.clone();
    let poll = std::thread::spawn(move || {
        clock.advance_ms(2000);
        polling.poll_once();
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "WIP commit never entered its hook"
        );
        std::thread::yield_now();
    }
    let start = Instant::now();
    let snapshot = daemon.snapshot();
    let elapsed = start.elapsed();
    poll.join().unwrap();

    assert_eq!(snapshot.sessions[0].id, "S1-OC01");
    assert!(
        elapsed < Duration::from_secs(1),
        "snapshot blocked for {elapsed:?}"
    );
    assert!(herdr
        .pane_list()
        .unwrap()
        .iter()
        .any(|entry| entry.pane_id == pane));
    assert!(daemon
        .snapshot()
        .warnings
        .iter()
        .any(|warning| warning.contains("pane close skipped")));
    eprintln!("snapshot took {elapsed:?} while real Git was blocked");
    daemon.flush_state();
}
