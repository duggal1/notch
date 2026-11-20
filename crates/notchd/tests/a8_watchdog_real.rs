//! Watchdog regression tests with a monotonic test clock, real Herdr panes and Git.
//!
//! Cases: wall-clock jump kills nothing; poll gap > 3x interval resets the
//! quiet timestamp; MAX_GRACE_STREAK=3 exhaustion classifies normally; a
//! genuinely failed WIP snapshot (real index.lock) vetoes the pane close.
//!
//! Run: cargo test -p notchd --test a8_watchdog_real

#[path = "fixtures/headless_herdr.rs"]
mod headless_herdr;

use headless_herdr::HeadlessHerdr;
use notchd::clock::{Clock, FakeClock};
use notchd::status::Status;
use notchd::watchdog::{Liveness, Obs, Watchdog, MAX_GRACE_STREAK};
use notchd::{Config, Daemon, Herdr};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

fn setup(server: &HeadlessHerdr, stuck_after_secs: u64) -> (Daemon, Arc<FakeClock>, String) {
    let root = &server.root;
    let cwd = root.join("repo");
    std::fs::create_dir(&cwd).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(["-c", "core.fsmonitor=false"])
            .args(args)
            .current_dir(&cwd)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=notch",
        "-c",
        "user.email=notch@localhost",
        "commit",
        "--allow-empty",
        "-qm",
        "base",
    ]);
    let worktree = root.join("worktree");
    git(&[
        "worktree",
        "add",
        "--detach",
        worktree.to_str().unwrap(),
        "HEAD",
    ]);

    let herdr = Herdr::from_env();
    let pane = herdr.pane_split(worktree.to_str().unwrap(), &[]).unwrap();
    server.report(&pane, "working");
    let state = root.join("state");
    std::fs::create_dir(&state).unwrap();
    let checkpoint = json!({
        "version": 1,
        "sessions": [{
            "id":"S1-OC01","sup":"SV-1","status":"working","title":"Watchdog","task":"Watchdog",
            "cwd":worktree,"paneId":pane
        }]
    });
    std::fs::write(state.join("state.json"), checkpoint.to_string()).unwrap();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = Daemon::open(
        Config {
            state_dir: state,
            tmp_dir: root.join("tmp"),
            stuck_after_secs,
            poll_interval_ms: 2000,
            opencode_decider: false,
        },
        herdr,
        clock.clone(),
    )
    .unwrap();
    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    (daemon, clock, pane)
}

fn pane_is_open(pane: &str) -> bool {
    Herdr::from_env()
        .pane_list()
        .unwrap()
        .iter()
        .any(|entry| entry.pane_id == pane)
}

fn warnings(d: &Daemon) -> Vec<String> {
    d.snapshot().warnings
}

// ---------------------------------------------------------------------------

#[test]
fn wall_clock_jump_alone_kills_nothing() {
    let server = HeadlessHerdr::start();
    let (daemon, clock, pane) = setup(&server, 15);
    let w0 = clock.now_ms();

    // +6 h wall clock, ZERO monotonic elapsed: no liveness decision may fire.
    clock.set(w0 + 6 * 3_600_000);
    for _ in 0..5 {
        daemon.poll_once();
    }
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert!(pane_is_open(&pane), "wall jump must never close a pane");
    assert!(
        !daemon
            .snapshot()
            .escalations
            .iter()
            .any(|e| e.kind == "stuck"),
        "no stuck verdict from a wall-clock jump"
    );
    eprintln!("WALL JUMP +6h, mono unchanged: HELD (0 closes, still working)");
}

#[test]
fn poll_gap_over_3x_interval_resets_quiet_timestamp() {
    // A 1 s stuck threshold makes the reset decisive: without it, 11 s quiet
    // on the next poll would be Stuck (>= 2 s threshold).
    let server = HeadlessHerdr::start();
    let (daemon, clock, pane) = setup(&server, 1);

    // One 10 s mono gap (> 3x the 2 s poll interval) is a scheduler stall:
    // timestamps reset, no kill.
    clock.advance_mono_ms(10_000);
    daemon.poll_once();
    assert!(pane_is_open(&pane), "stall grants grace, no kill");
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert!(
        warnings(&daemon)
            .iter()
            .any(|w| w.contains("grace granted")),
        "stall grace recorded: {:?}",
        warnings(&daemon)
    );

    // A small normal step after the reset: quiet is 1 s (< 2 s Stuck
    // threshold) — session stays Working. Without the reset it would be 11 s.
    clock.advance_mono_ms(1_000);
    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert!(pane_is_open(&pane));
    eprintln!("POLL GAP 10s > 3x 2s: HELD (timestamp reset, no kill after)");
}

#[test]
fn grace_streak_exhaustion_after_three_classifies_normally() {
    // Unit-level: the public Watchdog API, no fake, no clock needed.
    assert_eq!(MAX_GRACE_STREAK, 3);
    let mut w = Watchdog::new(1);
    for i in 0..MAX_GRACE_STREAK {
        assert!(w.grant_stall_grace(), "grant {} must succeed", i + 1);
        assert_eq!(w.grace_streak(), i + 1);
    }
    assert!(!w.grant_stall_grace(), "4th large gap must NOT be forgiven");
    assert_eq!(w.grace_streak(), MAX_GRACE_STREAK);
    // From here a Working/quiet observation is classified normally -> Stuck.
    assert_eq!(
        w.classify(Obs {
            status: Status::Working,
            revision_changed_now: false,
            oc_busy: None,
            quiet_for_ms: 2_000,
        }),
        Liveness::Stuck,
        "grace exhausted -> normal classification"
    );
    w.clear_stall_grace_streak();
    assert_eq!(w.grace_streak(), 0);
    assert!(w.grant_stall_grace(), "cleared streak can grant again");

    // Daemon level: four consecutive large gaps. The 4th is classified
    // normally and kills; the first three only reset timestamps.
    let server = HeadlessHerdr::start();
    let (daemon, clock, pane) = setup(&server, 1);
    for i in 0..3 {
        clock.advance_mono_ms(10_000);
        daemon.poll_once();
        assert!(pane_is_open(&pane), "grace {i} must not kill");
        assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    }
    clock.advance_mono_ms(10_000);
    daemon.poll_once();
    assert!(
        !pane_is_open(&pane),
        "4th large gap after streak exhaustion must kill"
    );
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Failed);
    assert_eq!(
        daemon.snapshot().sessions[0].failure_reason.as_deref(),
        Some("stuck")
    );
    assert!(
        warnings(&daemon)
            .iter()
            .any(|w| w.contains("grace streak exhausted")),
        "exhaustion logged: {:?}",
        warnings(&daemon)
    );
    eprintln!("MAX_GRACE_STREAK=3: HELD (3 grace, 4th kills normally)");
}

#[test]
fn failed_wip_snapshot_vetoes_pane_close() {
    let server = HeadlessHerdr::start();
    let (daemon, clock, pane) = setup(&server, 15);

    // Make the WIP snapshot genuinely fail: dirty the SESSION worktree (not
    // the plan's source repo) and place a real index.lock at the path git
    // actually uses for that worktree. No fake git, no injected error.
    let session_cwd = PathBuf::from(daemon.snapshot().sessions[0].cwd.clone());
    std::fs::write(session_cwd.join("wip.txt"), "uncommitted\n").unwrap();
    let lock_path = std::process::Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-C",
            session_cwd.to_str().unwrap(),
            "rev-parse",
            "--git-path",
            "index.lock",
        ])
        .output()
        .unwrap();
    assert!(lock_path.status.success());
    let lock_path = PathBuf::from(String::from_utf8_lossy(&lock_path.stdout).trim());
    let lock_path = if lock_path.is_absolute() {
        lock_path
    } else {
        session_cwd.join(lock_path)
    };
    std::fs::write(&lock_path, "").unwrap();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);

    clock.advance_mono_ms(30_000); // large single gap -> grace, no kill
    daemon.poll_once();
    // Step through ordinary ticks (< 3x interval) to reach Stuck.
    for _ in 0..15 {
        clock.advance_ms(2_000);
        daemon.poll_once();
    }
    let snap = daemon.snapshot();
    assert!(
        warnings(&daemon)
            .iter()
            .any(|w| w.contains("WIP snapshot failed") && w.contains("pane close skipped")),
        "veto warning missing: {:?}",
        warnings(&daemon)
    );
    assert_eq!(
        snap.sessions[0].status,
        Status::Working,
        "failed snapshot must not flip the session to Failed/closed"
    );
    assert!(
        pane_is_open(&pane),
        "vetoed snapshot must not close the pane"
    );
    // The uncommitted file is still there: no work was lost or committed.
    assert!(session_cwd.join("wip.txt").exists());
    let log =
        std::fs::read_to_string(server.root.join("state").join("events.jsonl")).unwrap_or_default();
    assert!(
        log.contains("WIP snapshot failed")
            || warnings(&daemon)
                .iter()
                .any(|w| w.contains("WIP snapshot failed")),
        "failure is on the record"
    );
    eprintln!("WIP SNAPSHOT VETO: HELD (real index.lock, pane close skipped)");
    std::fs::remove_file(&lock_path).ok();
}
