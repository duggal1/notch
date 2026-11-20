//! A8 watchdog REAL tests. Injected monotonic clock (the one allowed fake),
//! real in-process Daemon, fake herdr (SOLUTION §6), real git for the WIP veto.
//!
//! Cases: wall-clock jump kills nothing; poll gap > 3x interval resets the
//! quiet timestamp; MAX_GRACE_STREAK=3 exhaustion classifies normally; a
//! genuinely failed WIP snapshot (real index.lock) vetoes the pane close.
//!
//! Run: cargo test -p notchd --test a8_watchdog_real

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::status::Status;
use notchd::watchdog::{Liveness, Obs, Watchdog, MAX_GRACE_STREAK};

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a8wd-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn config(r: &Path, stuck_after: u64) -> Config {
    std::fs::create_dir_all(r.join("tmp")).unwrap();
    Config {
        state_dir: r.join("state"),
        tmp_dir: r.join("tmp"),
        stuck_after_secs: stuck_after,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

struct FakeHerdr {
    agents: Arc<Mutex<Vec<Value>>>,
    closes: Arc<Mutex<usize>>,
}
impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            agents: Arc::new(Mutex::new(Vec::new())),
            closes: Arc::new(Mutex::new(0)),
        })
    }
    fn herdr(self: &Arc<Self>) -> Herdr {
        let agents = Arc::clone(&self.agents);
        let closes = Arc::clone(&self.closes);
        Herdr::with_runner(move |args: &[String]| {
            match (
                args.get(1).map(String::as_str),
                args.get(2).map(String::as_str),
            ) {
                (Some("pane"), Some("split")) => {
                    Ok(json!({"result":{"pane":{"pane_id":"w8:p1"}}}).to_string())
                }
                (Some("agent"), Some("list")) => {
                    Ok(json!({"result":{"agents": agents.lock().unwrap().clone()}}).to_string())
                }
                (Some("agent"), Some("read")) => Ok("still working\n".to_string()),
                (Some("pane"), Some("close")) => {
                    *closes.lock().unwrap() += 1;
                    Ok(r#"{"result":{"type":"ok"}}"#.to_string())
                }
                _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
            }
        })
    }
    fn close_count(&self) -> usize {
        *self.closes.lock().unwrap()
    }
}

fn agent_entry(pane: &str, status: &str, revision: u64) -> Value {
    json!({
        "agent": "opencode", "agent_status": status,
        "agent_session": {"value": format!("ses_{pane}")},
        "cwd": "/tmp", "pane_id": pane, "revision": revision,
        "terminal_title": "OC | a8", "focused": false, "name": "a8"
    })
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .unwrap()
                .success(),
            "git {args:?} failed"
        );
    };
    git(&["init", "-q"]);
    std::fs::write(dir.join("base.txt"), "base\n").unwrap();
    git(&["add", "base.txt"]);
    git(&[
        "-c",
        "user.name=notch",
        "-c",
        "user.email=notch@localhost",
        "commit",
        "-qm",
        "base",
    ]);
}

/// One working session in a real git cwd, primed at T0 with revision 7.
fn setup(r: &Path, cwd: &Path, stuck_after: u64) -> (Arc<Daemon>, Arc<FakeClock>, Arc<FakeHerdr>) {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let daemon =
        Arc::new(Daemon::open(config(r, stuck_after), fake.herdr(), clock.clone()).unwrap());
    let plan = json!({
        "stuck_after_secs": stuck_after,
        "supervisors": [{"sessions": [{
            "title": "watchdog case", "prompt": "watchdog case work", "cwd": cwd
        }]}]
    });
    let path = r.join("plan.json");
    std::fs::write(&path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    assert!(daemon.run_plan(&path).ok);
    *fake.agents.lock().unwrap() = vec![agent_entry("w8:p1", "working", 7)];
    daemon.poll_once(); // prime: adopts revision 7
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    (daemon, clock, fake)
}

fn warnings(d: &Daemon) -> Vec<String> {
    d.snapshot().warnings
}

// ---------------------------------------------------------------------------

#[test]
fn wall_clock_jump_alone_kills_nothing() {
    let r = root("walljump");
    let cwd = r.join("repo");
    init_repo(&cwd);
    let (daemon, clock, fake) = setup(&r, &cwd, 15);
    let w0 = clock.now_ms();

    // +6 h wall clock, ZERO monotonic elapsed: no liveness decision may fire.
    clock.set(w0 + 6 * 3_600_000);
    for _ in 0..5 {
        daemon.poll_once();
    }
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert_eq!(fake.close_count(), 0, "wall jump must never close a pane");
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
    let r = root("gapreset");
    let cwd = r.join("repo");
    init_repo(&cwd);
    let (daemon, clock, fake) = setup(&r, &cwd, 1);

    // One 10 s mono gap (> 3x the 2 s poll interval) is a scheduler stall:
    // timestamps reset, no kill.
    clock.advance_mono_ms(10_000);
    daemon.poll_once();
    assert_eq!(fake.close_count(), 0, "stall grants grace, no kill");
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
    assert_eq!(fake.close_count(), 0);
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
    let r = root("streak");
    let cwd = r.join("repo");
    init_repo(&cwd);
    let (daemon, clock, fake) = setup(&r, &cwd, 1);
    for i in 0..3 {
        clock.advance_mono_ms(10_000);
        daemon.poll_once();
        assert_eq!(fake.close_count(), 0, "grace {i} must not kill");
        assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    }
    clock.advance_mono_ms(10_000);
    daemon.poll_once();
    assert_eq!(
        fake.close_count(),
        1,
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
    let r = root("veto");
    let cwd = r.join("repo");
    init_repo(&cwd);
    let (daemon, clock, fake) = setup(&r, &cwd, 15);

    // Make the WIP snapshot genuinely fail: dirty file + a real index.lock
    // that blocks git's index write. No fake git, no injected error.
    std::fs::write(cwd.join("wip.txt"), "uncommitted\n").unwrap();
    std::fs::write(cwd.join(".git").join("index.lock"), "").unwrap();
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
    assert_eq!(
        fake.close_count(),
        0,
        "vetoed snapshot must not close the pane"
    );
    // The uncommitted file is still there: no work was lost or committed.
    assert!(cwd.join("wip.txt").exists());
    let log = std::fs::read_to_string(r.join("state").join("events.jsonl")).unwrap_or_default();
    assert!(
        log.contains("WIP snapshot failed")
            || warnings(&daemon)
                .iter()
                .any(|w| w.contains("WIP snapshot failed")),
        "failure is on the record"
    );
    eprintln!("WIP SNAPSHOT VETO: HELD (real index.lock, pane close skipped)");
    std::fs::remove_file(cwd.join(".git").join("index.lock")).ok();
}
