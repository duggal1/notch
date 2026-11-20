//! B1 regression — writer A1 (Maya), tester A8 (Zoe).
//!
//! A hung `git` inside the snapshot-before-kill path must not wedge the daemon.
//! The `snapshot` command (the in-process `Daemon::snapshot()` the CLI serves)
//! must return in < 1 s while that git is hung.
//!
//! Unfixed (e544b8c): `poll.rs` holds `inner` across `snapshot_worktree`, so
//! `snapshot()` blocks until the bounded git call times out (~5 s). RED.
//! After A1's fix: `inner` is dropped before that IO. GREEN.
//!
//! Run: cargo test -p notchd --test a8_b1_hung_git

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::status::Status;

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a8b1-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn config(r: &Path) -> Config {
    std::fs::create_dir_all(r.join("tmp")).unwrap();
    Config {
        state_dir: r.join("state"),
        tmp_dir: r.join("tmp"),
        stuck_after_secs: 15,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

/// Real git repo with one committed file and one dirty file: the WIP snapshot
/// has work to do, so `worktree::snapshot` reaches `git add -A` (the call the
/// shim hangs on). The commit makes `git status` meaningful.
fn init_dirty_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    // Global core.fsmonitor=true makes plain `git add` wait on a dead socket;
    // disable it exactly as worktree::git_run does.
    let git = |args: &[&str]| {
        let mut full = vec!["-c", "core.fsmonitor=false"];
        full.extend_from_slice(args);
        assert!(
            std::process::Command::new("git")
                .args(&full)
                .current_dir(dir)
                .status()
                .unwrap()
                .success(),
            "git {full:?} failed"
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
    std::fs::write(dir.join("wip.txt"), "uncommitted work\n").unwrap();
}

/// Fake herdr: one working pane. No real herdr binary is involved.
struct FakeHerdr {
    agents: Arc<std::sync::Mutex<Vec<Value>>>,
    panes: Arc<std::sync::Mutex<Vec<String>>>,
}
impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            agents: Arc::new(std::sync::Mutex::new(Vec::new())),
            panes: Arc::new(std::sync::Mutex::new(Vec::new())),
        })
    }
    fn herdr(self: &Arc<Self>) -> Herdr {
        let agents = Arc::clone(&self.agents);
        let panes = Arc::clone(&self.panes);
        Herdr::with_runner(move |args: &[String]| {
            match (
                args.get(1).map(String::as_str),
                args.get(2).map(String::as_str),
            ) {
                (Some("pane"), Some("split")) => {
                    let p = "wB:p1".to_string();
                    panes.lock().unwrap().push(p.clone());
                    Ok(json!({"result":{"pane":{"pane_id":p}}}).to_string())
                }
                (Some("agent"), Some("list")) => {
                    Ok(json!({"result":{"agents": agents.lock().unwrap().clone()}}).to_string())
                }
                (Some("agent"), Some("read")) => Ok("still working\n".to_string()),
                _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
            }
        })
    }
}

fn agent_entry(pane: &str, status: &str, revision: u64) -> Value {
    json!({
        "agent": "opencode",
        "agent_status": status,
        "agent_session": {"value": format!("ses_{pane}")},
        "cwd": "/tmp",
        "pane_id": pane,
        "revision": revision,
        "terminal_title": format!("OC | {pane}"),
        "focused": false,
        "name": "b1"
    })
}

/// Shim: delegates status/diff to the REAL git; on `add -A` it writes a marker
/// and sleeps 60 s. Only the WIP-snapshot path calls `add`, so the hung call is
/// unambiguously snapshot-before-kill. `proc::run` kills it at TIMEOUT_GIT=5 s.
const GIT_SHIM: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$SHIM_LOG"
for a in "$@"; do
  if [ "$a" = "add" ]; then
    printf 'started\n' > "$SHIM_MARKER"
    sleep 60
  fi
done
exec "$REAL_GIT" "$@"
"#;

fn install_shim(r: &Path, real_git: &str) -> (PathBuf, String) {
    let bin = r.join("shim-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("git");
    std::fs::write(&shim, GIT_SHIM).unwrap();
    std::fs::set_permissions(&shim, PermissionsExt::from_mode(0o755)).unwrap();
    let marker = r.join("git_add_started");
    let log = r.join("git_shim.log");
    std::env::set_var("SHIM_MARKER", &marker);
    std::env::set_var("SHIM_LOG", &log);
    std::env::set_var("REAL_GIT", real_git);
    let old_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", bin.display(), old_path));
    (marker, old_path)
}

fn wait_for_marker(marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "shim never reached git add");
        std::thread::yield_now();
    }
}

#[test]
fn snapshot_returns_in_under_one_second_while_wip_git_is_hung() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root("hung");
    let cwd = r.join("worktree");
    init_dirty_repo(&cwd);
    let real_git = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real_git = String::from_utf8_lossy(&real_git.stdout).trim().to_string();
    assert!(!real_git.is_empty(), "cannot locate real git");

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = Arc::new(Daemon::open(config(&r), fake.herdr(), clock.clone()).unwrap());
    let plan = json!({
        "stuck_after_secs": 15,
        "supervisors": [{"sessions": [{
            "title": "hung git regression",
            "prompt": "work that gets stuck",
            "cwd": cwd
        }]}]
    });
    let plan_path = r.join("plan.json");
    std::fs::write(&plan_path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    assert!(daemon.run_plan(&plan_path).ok);
    let pane = fake
        .panes
        .lock()
        .unwrap()
        .clone()
        .into_iter()
        .next()
        .unwrap();
    // run_plan gives the session its OWN git worktree under tmp/. Dirty that
    // exact cwd so the WIP snapshot reaches `git add -A`.
    let session_cwd = daemon.snapshot().sessions[0].cwd.clone();
    std::fs::write(
        Path::new(&session_cwd).join("wip.txt"),
        "uncommitted work\n",
    )
    .unwrap();
    *fake.agents.lock().unwrap() = vec![agent_entry(&pane, "working", 7)];

    // Prime, then step to 28 s quiet (Suspect, not yet Stuck). Real git is on PATH.
    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    for _ in 0..14 {
        clock.advance_ms(2000);
        daemon.poll_once();
    }
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert_eq!(fake.panes.lock().unwrap().len(), 1, "no kill yet");

    // Arm the hung git and drive the killing poll on another thread.
    let (marker, old_path) = install_shim(&r, &real_git);
    let d = Arc::clone(&daemon);
    let c = Arc::clone(&clock);
    let killer = std::thread::spawn(move || {
        c.advance_ms(2000); // quiet = 30 s = 2x stuck_after -> Stuck
        d.poll_once();
        eprintln!(
            "B1 killer done: status={:?} warnings={:?}",
            d.snapshot().sessions[0].status,
            d.snapshot().warnings
        );
    });

    if let Err(e) = std::panic::catch_unwind(|| wait_for_marker(&marker)) {
        let _ = e;
        eprintln!(
            "B1 DEBUG shim log:\n{}",
            std::fs::read_to_string(r.join("git_shim.log")).unwrap_or_default()
        );
        std::env::set_var("PATH", &old_path);
        let _ = killer.join();
        panic!("shim never reached git add (see B1 DEBUG above)");
    }
    // The snapshot-before-kill git is now hung. A snapshot command must not wait.
    let t = Instant::now();
    let snap = daemon.snapshot();
    let elapsed = t.elapsed();
    eprintln!(
        "B1 snapshot while WIP git hung: {:?} (limit 1 s), sessions={}",
        elapsed,
        snap.sessions.len()
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "snapshot wedged behind hung git: {elapsed:?} (B1 regression)"
    );

    std::env::set_var("PATH", &old_path);
    std::env::remove_var("SHIM_MARKER");
    std::env::remove_var("SHIM_LOG");
    std::env::remove_var("REAL_GIT");
    let _ = killer.join();
    eprintln!("B1 VERDICT=HELD");
}
