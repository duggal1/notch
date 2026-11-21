//! Agent watchdog-deep — aggressive watchdog / supervisor-poll cases.
//!
//! I own this file only. Fake clock everywhere, zero real sleeps, and the
//! herdr / opencode CLIs are scripted through their injected runners. The
//! only real subprocess involved anywhere here is `git` (the attribution
//! sampler, unconfigurable from outside `src/`): the 50-session timing bound
//! therefore uses idle sessions so it measures the poll bookkeeping, not
//! 50 git spawns. See the `timing_*` tests and the final report.
//!
//! Case coverage map (bridge.md v1.3 §8, docs/PLAN.md "stuck session" demo):
//!   1  stuck_*                         threshold exactness at the boundary
//!   2  done_in_stuck_window_*          working->done wins in the stuck window
//!   3  paused_*                        pause gates decisions, never the watchdog
//!   4  readopt_* / ids_never_*         re-adoption + registry monotonicity
//!   5  timing_*                        50 sessions, poll_once < 150 ms
//!   6  unicode_* / task_field_* / nul_* / revision_u64_max  hostile data
//!   7  herdr_fail_*                    mid-poll herdr failure, daemon survives
//!   8  kill_reprompt_*                 interleaved kill+re-prompt, exactly-once
//!   9  blocked_*                       blocked -> failed/blocked_waiting + esc kind

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::status::Status;
use notchd::watchdog::Watchdog;

use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

const T0: u64 = 1_763_520_000_000;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-wd-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn test_config(root: &Path, stuck_after_secs: u64) -> Config {
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

/// Advance elapsed time in poll-sized steps, polling between every step.
/// A single large jump is indistinguishable from a scheduler/VM stall to the
/// hardened watchdog (which grants grace); real sessions go quiet across many
/// ordinary ticks. Tests that expect a kill must therefore step through the
/// ladder, not teleport past it.
fn step_advance(clock: &Arc<FakeClock>, daemon: &Daemon, ms: u64) {
    let mut left = ms;
    while left > 0 {
        let step = 2_000u64.min(left);
        clock.advance_ms(step);
        daemon.poll_once();
        left -= step;
    }
}

/// A stuck kill snapshots WIP first; without a git repo at the session cwd
/// the snapshot vetoes the close (correct safety behavior). Test fixtures
/// create their cwds here so they model real per-session worktrees.
/// Judge jobs run on A2's worker threads. Yield until the expected call count
/// is observed (bounded, so a real failure still fails the test).
fn wait_for_decider(oc: &Arc<FakeOpencode>, expected: usize) {
    for _ in 0..10_000 {
        if oc.decider_calls() >= expected {
            return;
        }
        std::thread::yield_now();
    }
}

fn init_git(dir: &Path) {
    assert!(
        std::process::Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success(),
        "git init failed for {}",
        dir.display()
    );
}

// ---------- fake herdr ----------

struct FakeHerdr {
    calls: Mutex<Vec<Vec<String>>>,
    agents: Mutex<Vec<Value>>,
    read_text: Mutex<String>,
    fail_subcommand: Mutex<Option<String>>,
    split_seq: Mutex<u32>,
    panes: Mutex<Vec<String>>,
}

impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            agents: Mutex::new(Vec::new()),
            read_text: Mutex::new("last line of pane output\n".to_string()),
            fail_subcommand: Mutex::new(None),
            split_seq: Mutex::new(0),
            panes: Mutex::new(Vec::new()),
        })
    }

    fn set_agents(&self, agents: Vec<Value>) {
        *self.agents.lock().unwrap() = agents;
    }

    fn set_read_text(&self, text: &str) {
        *self.read_text.lock().unwrap() = text.to_string();
    }

    fn fail(&self, subcommand: &str) {
        *self.fail_subcommand.lock().unwrap() = Some(subcommand.to_string());
    }

    fn clear_failure(&self) {
        *self.fail_subcommand.lock().unwrap() = None;
    }


    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    fn clear_calls(&self) {
        self.calls.lock().unwrap().clear();
    }

    fn pane_close_count(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| {
                c.get(1).map(String::as_str) == Some("pane")
                    && c.get(2).map(String::as_str) == Some("close")
            })
            .count()
    }

    fn panes(&self) -> Vec<String> {
        self.panes.lock().unwrap().clone()
    }

    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| fake.run(args))
    }

    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        self.calls.lock().unwrap().push(args.to_vec());
        let sub = args.get(1).cloned().unwrap_or_default();
        let sub2 = args.get(2).cloned().unwrap_or_default();
        if let Some(fail) = self.fail_subcommand.lock().unwrap().clone() {
            if fail == format!("{sub} {sub2}") {
                return Err(HerdrError::Failed {
                    args: args.to_vec(),
                    code: Some(1),
                    stderr: "simulated herdr failure".to_string(),
                });
            }
        }
        match (sub.as_str(), sub2.as_str()) {
            ("agent", "list") => Ok(serde_json::json!({
                "id": "cli:agent:list",
                "result": { "type": "agent_list", "agents": self.agents.lock().unwrap().clone() }
            })
            .to_string()),
            ("pane", "split") => {
                let mut seq = self.split_seq.lock().unwrap();
                *seq += 1;
                let pane = format!("wT:p{seq}");
                self.panes.lock().unwrap().push(pane.clone());
                Ok(serde_json::json!({
                    "id": "cli:pane:split",
                    "result": { "type": "pane_info", "pane": { "pane_id": pane, "tab_id": "w1:t1", "workspace_id": "w1" } }
                })
                .to_string())
            }
            ("agent", "start") => Ok(serde_json::json!({
                "id": "cli:agent:start",
                "result": { "type": "agent_started" }
            })
            .to_string()),
            ("agent", "prompt") => Ok(serde_json::json!({
                "id": "cli:agent:prompt",
                "result": { "type": "agent_prompted" }
            })
            .to_string()),
            ("agent", "read") => Ok(self.read_text.lock().unwrap().clone()),
            ("pane", "close") => {
                Ok(serde_json::json!({
                    "id": "cli:pane:close",
                    "result": { "type": "ok" }
                })
                .to_string())
            }
            ("agent", "focus") => Ok(serde_json::json!({
                "id": "cli:agent:focus",
                "result": { "type": "ok" }
            })
            .to_string()),
            ("agent", "wait") => Ok(serde_json::json!({
                "id": "cli:agent:wait",
                "result": { "type": "ok" }
            })
            .to_string()),
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected call: {}", args.join(" ")),
            }),
        }
    }
}

fn agent_entry(pane_id: &str, status: &str, revision: u64) -> Value {
    serde_json::json!({
        "agent": "opencode",
        "agent_status": status,
        "agent_session": {"agent": "opencode", "kind": "id", "source": "herdr:opencode", "value": format!("ses_{pane_id}")},
        "cwd": "/tmp",
        "focused": false,
        "pane_id": pane_id,
        "revision": revision,
        "terminal_title": format!("OC | {pane_id}"),
        "tab_id": "w1:t1",
        "workspace_id": "w1",
        "screen_detection_skipped": true
    })
}

// ---------- fake opencode ----------

struct FakeOpencode {
    calls: Mutex<Vec<Vec<String>>>,
    api_session_raw: Mutex<String>,
    models: Mutex<Vec<String>>,
    decisions: Mutex<VecDeque<String>>,
}

impl FakeOpencode {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            api_session_raw: Mutex::new(
                serde_json::json!({"cursor": null, "data": []}).to_string(),
            ),
            models: Mutex::new(vec!["opencode/hy4-flash-free".to_string()]),
            decisions: Mutex::new(VecDeque::new()),
        })
    }

    fn push_decision(&self, s: &str) {
        self.decisions.lock().unwrap().push_back(s.to_string());
    }

    fn decider_calls(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.get(1).map(String::as_str) == Some("run"))
            .count()
    }

    fn opencode(self: &Arc<Self>) -> notchd::opencode::Opencode {
        let fake = Arc::clone(self);
        notchd::opencode::Opencode::with_runner(move |args: &[String]| {
            fake.calls.lock().unwrap().push(args.to_vec());
            let rest = &args[1..];
            match rest.join(" ").as_str() {
                "api GET /api/session" => Ok(fake.api_session_raw.lock().unwrap().clone()),
                "models" => Ok(fake.models.lock().unwrap().join("\n")),
                s if s.starts_with("run --model") => {
                    let mut q = fake.decisions.lock().unwrap();
                    q.pop_front().ok_or(HerdrError::Failed {
                        args: args.to_vec(),
                        code: Some(1),
                        stderr: "no scripted decision queued".to_string(),
                    })
                }
                other => Err(HerdrError::Failed {
                    args: args.to_vec(),
                    code: Some(2),
                    stderr: format!("unexpected opencode call: {other}"),
                }),
            }
        })
    }
}

// ---------- builders ----------

/// The pre-spawn hook shells out to `node` and the real `opencode` binary and
/// can rewrite the user's opencode config. Point it at /bin/true so the test
/// suite never touches it. Edition 2021 -> `set_var` is safe; every test sets
/// the same value, and every test that can spawn a session goes through here.
fn neutralize_pre_spawn_hook() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/bin/true");
}

fn build_daemon(
    root: &Path,
    fake: &Arc<FakeHerdr>,
    oc: &Arc<FakeOpencode>,
    clock: Arc<FakeClock>,
    stuck_after_secs: u64,
) -> Daemon {
    neutralize_pre_spawn_hook();
    let config = test_config(root, stuck_after_secs);
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    let daemon = Daemon::open(config, fake.herdr(), clock).unwrap();
    daemon.set_opencode(oc.opencode());
    daemon
}

/// One-supervisor / one-session plan. Returns the plan path and ensures the
/// cwd directory exists.
fn write_one_session_plan(root: &Path) -> PathBuf {
    let cwd = root.join("a");
    std::fs::create_dir_all(&cwd).unwrap();
    init_git(&cwd);
    let plan = serde_json::json!({
        "stuck_after_secs": 15,
        "supervisors": [
            { "sessions": [
                { "title": "Refactor auth middleware",
                  "prompt": "Refactor the auth middleware to remove the shared session cache",
                  "cwd": cwd }
            ]}
        ]
    });
    let path = root.join("plan.json");
    std::fs::write(&path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    path
}

fn write_two_by_two_plan(root: &Path) -> PathBuf {
    for d in ["a", "b", "c", "d"] {
        let dir = root.join(d);
        std::fs::create_dir_all(&dir).unwrap();
        init_git(&dir);
    }
    let plan = serde_json::json!({
        "stuck_after_secs": 15,
        "supervisors": [
            { "sessions": [
                { "title": "Refactor auth middleware", "prompt": "Refactor the auth middleware to remove the shared session cache", "cwd": root.join("a") },
                { "title": "Fix login race", "prompt": "Fix the race between token refresh and login", "cwd": root.join("b") }
            ]},
            { "sessions": [
                { "title": "Add rate limiter", "prompt": "Add a sliding-window rate limiter to the public API", "cwd": root.join("c") },
                { "title": "Instrument p99 latency", "prompt": "Instrument p99 latency on the charge endpoint", "cwd": root.join("d") }
            ]}
        ]
    });
    let path = root.join("plan.json");
    std::fs::write(&path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    path
}

fn spawn_one_session(root: &Path, fake: &Arc<FakeHerdr>, oc: &Arc<FakeOpencode>, clock: Arc<FakeClock>) -> (Daemon, String) {
    let daemon = build_daemon(root, fake, oc, clock, 15);
    let plan = write_one_session_plan(root);
    let ack = daemon.run_plan(&plan);
    assert!(ack.ok, "plan must spawn");
    let pane = fake.panes().into_iter().next().expect("one pane split");
    (daemon, pane)
}

fn events(root: &Path) -> String {
    std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap_or_default()
}

fn event_count(log: &str, kind: &str) -> usize {
    let needle = format!("\"kind\":\"{kind}\"");
    log.matches(&needle).count()
}

/// Watchdog events include Suspect verdicts and stall-grace notices. Count
/// only actual Stuck verdicts when asserting "done was never killed".
fn stuck_verdicts(log: &str) -> usize {
    log.matches("\"detail\":\"stuck:").count()
}

fn read_state(root: &Path) -> Value {
    let bytes = std::fs::read(root.join("state").join("state.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn read_registry(root: &Path) -> Value {
    let bytes = std::fs::read(root.join("state").join("registry.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn write_state_fixture(root: &Path, value: Value) {
    let state_dir = root.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("state.json"), serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn write_registry_fixture(root: &Path, value: Value) {
    let state_dir = root.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("registry.json"), serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

/// A state.json fixture with one supervisor SV-1 and one working session
/// S1-OC01 in pane `pane`, revision `revision`, optional sessionRef.
fn working_session_state(pane: &str, revision: u64, session_ref: Option<&str>) -> Value {
    let mut session = serde_json::json!({
        "id": "S1-OC01",
        "sup": "SV-1",
        "status": "working",
        "title": "fixture session",
        "task": "fixture task",
        "cwd": "/tmp",
        "paneId": pane,
        "startedAt": T0,
        "lastAction": null,
        "lastRevision": revision,
        "latencyMs": null,
        "failureReason": null,
        "goal": "fixture goal",
        "iterations": 0,
        "maxIterations": 8,
        "wallClockDeadline": T0 + 3_600_000u64
    });
    if let Some(r) = session_ref {
        session["sessionRef"] = serde_json::Value::String(r.to_string());
    }
    serde_json::json!({
        "version": 1,
        "supervisors": [ { "id": "SV-1", "number": 1, "title": "fixture sup" } ],
        "sessions": [ session ]
    })
}

fn registry_fixture() -> Value {
    serde_json::json!({
        "version": 1,
        "supervisorCount": 1,
        "sessionCounts": { "1": 1 },
        "ids": ["SV-1", "S1-OC01"]
    })
}

// ============================================================
// Case 1 — stuck threshold exactness
// ============================================================

#[test]
fn stuck_no_kill_before_threshold_minus_one() {
    let root = tmp_root("stuck-pre");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    // Priming poll: first observation, revision adopts Some(7).
    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);

    step_advance(&clock, &daemon, 14_000); // stays below the suspicion threshold
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Working, "T-1s must not kill");
    assert_eq!(snap.sessions[0].failure_reason, None);
    assert_eq!(fake.pane_close_count(), 0);
    eprintln!("case1 delta=+14s obs=working/not-killed verdict=PASS");
}

#[test]
fn stuck_kill_exact_boundary() {
    let root = tmp_root("stuck-exact");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime at T0
    step_advance(&clock, &daemon, 30_000); // Stuck fires at 2x the suspicion threshold
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Failed);
    assert_eq!(snap.sessions[0].failure_reason.as_deref(), Some("stuck"));
    assert_eq!(fake.pane_close_count(), 1);
    eprintln!("case1 delta=+15s (exact) obs=failed/stuck, 1 pane close verdict=PASS");
}

#[test]
fn stuck_kill_plus_one() {
    let root = tmp_root("stuck-plus1");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime at T0
    step_advance(&clock, &daemon, 31_001); // just past the 2x Stuck threshold
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Failed);
    assert_eq!(snap.sessions[0].failure_reason.as_deref(), Some("stuck"));
    assert_eq!(fake.pane_close_count(), 1);
    eprintln!("case1 delta=+16s obs=failed/stuck, 1 pane close verdict=PASS");
}

#[test]
fn stuck_two_polls_at_boundary_do_not_double_kill() {
    let root = tmp_root("stuck-twice");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime
    step_advance(&clock, &daemon, 30_000); // stepped ticks reach the Stuck boundary
    daemon.poll_once(); // same instant, stale herdr view — must not re-kill
    step_advance(&clock, &daemon, 30_000); // deep into overtime
    daemon.poll_once(); // and again — must not re-kill
    assert_eq!(fake.pane_close_count(), 1, "one stuck kill across repeat boundary polls");
    // Two verdicts on the way: Suspect at 1x, Stuck at 2x. Exactly one of
    // them is a kill — asserted above by pane_close_count().
    assert_eq!(event_count(&events(&root), "watchdog"), 2, "suspect + stuck verdicts");
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Failed);
    eprintln!("case1 delta=+15s/+45s obs=1 pane close, 1 watchdog event verdict=PASS");
}

#[test]
fn watchdog_unit_boundary_matches_code_semantics() {
    let w = Watchdog::new(15);
    // Code: now - revision_changed_at >= stuck_after_ms, Working only.
    assert!(!w.is_stuck(Status::Working, 10_000, 10_000 + 14_999), "T-1ms: not stuck");
    assert!(w.is_stuck(Status::Working, 10_000, 10_000 + 15_000), "exact boundary: stuck (>=)");
    assert!(w.is_stuck(Status::Working, 10_000, 10_000 + 15_001), "T+1ms: stuck");
    // Non-working statuses are never stuck.
    for s in [Status::Idle, Status::Done, Status::Failed] {
        assert!(!w.is_stuck(s, 10_000, 10_000 + 999_000_000), "{s} must never be stuck");
    }
    // Future revision timestamp (clock skew) saturates to 0, never underflows.
    assert!(!w.is_stuck(Status::Working, u64::MAX, 42));
    assert!(w.is_stuck(Status::Working, 0, u64::MAX));
    // saturating_mul for an absurd threshold must not panic.
    let huge = Watchdog::new(u64::MAX);
    assert!(huge.is_stuck(Status::Working, 0, u64::MAX), "MAX-0 saturates to u64::MAX and >= u64::MAX");
}

// ============================================================
// Case 2 — working->done in the stuck window: done wins
// ============================================================

#[test]
fn done_in_stuck_window_not_killed_and_done_logged() {
    let root = tmp_root("done-window");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime at T0
    clock.advance_ms(60_000); // 4x the stuck budget, revision frozen
    fake.set_agents(vec![agent_entry(&pane, "done", 7)]); // same frozen revision, but done
    daemon.poll_once();

    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Done, "done must win over the stuck check");
    assert_eq!(snap.sessions[0].failure_reason, None);
    assert_eq!(fake.pane_close_count(), 0, "done panes are never watchdog-closed");
    let log = events(&root);
    assert_eq!(stuck_verdicts(&log), 0, "no stuck verdict for done");
    assert!(log.contains("\"detail\":\"done\""), "done evidence logged: {log}");
    assert!(snap.escalations.iter().all(|e| e.kind != "stuck"), "no stuck escalation for done");
    eprintln!("case2 delta=+60s obs=done, 0 pane closes, evidence logged verdict=PASS");
}

#[test]
fn done_survives_repeat_polls_in_stuck_window() {
    let root = tmp_root("done-repeat");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);
    daemon.poll_once(); // prime
    clock.advance_ms(90_000);
    fake.set_agents(vec![agent_entry(&pane, "done", 7)]);
    daemon.poll_once();
    daemon.poll_once();
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Done);
    assert_eq!(fake.pane_close_count(), 0);
    assert_eq!(stuck_verdicts(&events(&root)), 0);
    eprintln!("case2 delta=+90s obs=done across repeat polls verdict=PASS");
}

// ============================================================
// Case 3 — paused supervisor: watchdog still kills; decider stays silent
// ============================================================

#[test]
fn paused_supervisor_stuck_session_killed_reason_intact() {
    let root = tmp_root("paused-stuck");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.pause_supervisor("SV-1").unwrap();
    assert!(daemon.snapshot().supervisors.iter().any(|s| s.id == "SV-1" && s.paused));

    daemon.poll_once(); // prime at T0 (pause active)
    step_advance(&clock, &daemon, 30_000); // stepped ticks; watchdog fires while paused

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    assert_eq!(s.status, Status::Failed);
    assert_eq!(s.failure_reason.as_deref(), Some("stuck"), "reason intact through pause");
    assert_eq!(fake.pane_close_count(), 1);
    assert!(
        snap.warnings.iter().any(|w| w.contains("stuck")),
        "stuck warning recorded: {:?}",
        snap.warnings
    );
    assert!(snap.escalations.iter().any(|e| e.kind == "stuck"), "stuck escalation recorded");
    eprintln!("case3 delta=+20s paused obs=failed/stuck, reason intact, 1 close verdict=PASS");
}

#[test]
fn paused_supervisor_decision_loop_does_not_fire() {
    let root = tmp_root("paused-loop");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "idle", 7)]);
    // The session has a goal and is idle — the decision loop would fire if unpaused.
    for _ in 0..4 {
        oc.push_decision("CONTINUE: keep going");
    }

    daemon.pause_supervisor("SV-1").unwrap();
    daemon.poll_once(); // prime
    clock.advance_ms(100);
    daemon.poll_once();
    daemon.poll_once();

    assert_eq!(oc.decider_calls(), 0, "paused supervisor: zero decider calls");
    assert!(!events(&root).contains("\"kind\":\"idle_signal\""), "no idle signal logged while paused");
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Idle, "idle session untouched");

    daemon.resume_supervisor("SV-1").unwrap();
    daemon.poll_once();
    wait_for_decider(&oc, 1);
    assert_eq!(oc.decider_calls(), 1, "after resume the decider fires once for the idle session");
    // Drain the completed judge job; its verdict is applied on a later poll.
    for _ in 0..1_000 {
        if daemon.snapshot().sessions[0].iterations == Some(1) {
            break;
        }
        daemon.poll_once();
        std::thread::yield_now();
    }
    assert_eq!(daemon.snapshot().sessions[0].iterations, Some(1));
    eprintln!("case3 delta=+0.1s paused obs=0 decider calls; resumed obs=1 verdict=PASS");
}

// ============================================================
// Case 4 — re-adoption and ID monotonicity
// ============================================================

#[test]
fn readopt_state_json_deleted_registry_intact_no_ids_reissued() {
    let root = tmp_root("readopt-gone");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));

    // Stage 1 — a real daemon builds the world: SV-1..SV-2, S1-OC01..S2-OC02.
    let plan = write_two_by_two_plan(&root);
    let panes_after_plan;
    {
        let daemon = build_daemon(&root, &fake, &oc, clock.clone(), 15);
        let ack = daemon.run_plan(&plan);
        assert!(ack.ok);
        assert_eq!(ack.spawned, vec!["SV-1".to_string(), "SV-2".to_string()]);
        assert_eq!(daemon.snapshot().sessions.len(), 4);
        let reg = read_registry(&root);
        assert_eq!(reg["supervisorCount"], 2);
        panes_after_plan = fake.panes();
        // Daemon dropped without a shutdown path; panes "survive" in herdr.
    }
    assert!(panes_after_plan.len() == 4);

    // The disaster: state.json is gone. registry.json survives.
    std::fs::remove_file(root.join("state").join("state.json")).unwrap();
    assert!(!root.join("state").join("state.json").exists());

    // herdr still shows the old panes — but the daemon has no record of them.
    fake.set_agents(panes_after_plan.iter().map(|p| agent_entry(p, "working", 9)).collect());

    let clock2 = Arc::new(FakeClock::new(T0 + 100_000));
    let daemon = build_daemon(&root, &fake, &oc, clock2.clone(), 15);
    daemon.reconcile(); // must not panic, must not invent sessions
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions.len(), 0, "state.json deleted: nothing to re-adopt (no crash)");
    assert_eq!(snap.supervisors.len(), 0);

    // Registry monotonicity: the next allocation must not reuse SV-1/SV-2.
    let ack = daemon.submit_task(
        "brand new task".to_string(),
        Some(root.join("a").to_string_lossy().to_string()),
        Some("Brand fresh".to_string()),
        None,
        Some("opencode/hy4-flash-free".to_string()),
        None,
    );
    assert!(ack.ok, "submit_task ok: {:?}", ack.error);
    assert_eq!(ack.supervisor_id.as_deref(), Some("SV-3"), "registry monotonic, no SV-1 reissue");
    assert_eq!(ack.session_id.as_deref(), Some("S3-OC01"), "no S1-OC*/S2-OC* reissue");
    let reg = read_registry(&root);
    assert_eq!(reg["supervisorCount"], 3);
    assert!(reg["ids"].as_array().unwrap().iter().any(|v| v == "S3-OC01"));
    eprintln!("case4 state.json deleted obs=fresh tree, next ids SV-3/S3-OC01 verdict=PASS");
}

#[test]
fn readopt_pane_present_keeps_session_and_binds_session_ref() {
    let root = tmp_root("readopt-live");
    write_registry_fixture(&root, registry_fixture());
    write_state_fixture(
        &root,
        working_session_state("w5:p7", 5, Some("ses_fixture123")),
    );

    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    // Paned kept working through the restart: revision moved 5 -> 6.
    fake.set_agents(vec![agent_entry("w5:p7", "working", 6)]);
    let clock = Arc::new(FakeClock::new(T0 + 50_000));
    let daemon = build_daemon(&root, &fake, &oc, clock.clone(), 15);

    daemon.reconcile();
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions.len(), 1);
    assert_eq!(snap.sessions[0].status, Status::Working, "live pane is re-adopted, not done");
    assert_eq!(snap.sessions[0].id, "S1-OC01");
    assert_eq!(snap.sessions[0].last_revision, Some(6), "revision re-adopted at reconcile");
    // Registry untouched: allocation continues from what it recorded.
    assert_eq!(read_registry(&root)["ids"].as_array().unwrap().len(), 2);
    // sessionRef survived the roundtrip through state.json.
    assert_eq!(read_state(&root)["sessions"][0]["sessionRef"], "ses_fixture123");
    eprintln!("case4 pane present obs=working, sessionRef ses_fixture123 preserved verdict=PASS");
}

#[test]
fn readopt_lazy_records_session_ref_on_first_poll() {
    let root = tmp_root("readopt-bind");
    write_registry_fixture(&root, registry_fixture());
    // No sessionRef recorded yet — herdr reports it at list time.
    write_state_fixture(&root, working_session_state("w5:p7", 5, None));

    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    fake.set_agents(vec![agent_entry("w5:p7", "working", 6)]); // agent_session.value = ses_w5:p7
    let clock = Arc::new(FakeClock::new(T0 + 50_000));
    let daemon = build_daemon(&root, &fake, &oc, clock.clone(), 15);

    daemon.reconcile();
    daemon.poll_once(); // lazy session-ref bind happens inside poll
    daemon.flush_state(); // coalesced writer is 1 Hz; flush before reading disk
    assert_eq!(
        read_state(&root)["sessions"][0]["sessionRef"],
        "ses_w5:p7",
        "first poll after restart binds the recorded sessionRef"
    );
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    eprintln!("case4 lazy sessionRef obs=ses_w5:p7 bound on first poll verdict=PASS");
}

/// ENGINE BUG (flagged, not fixed — src/ is off-limits for this agent):
/// `revision_changed_at_ms` is `#[serde(skip)]`, so any session loaded from
/// state.json gets `0`. If herdr then reports the *same* frozen revision on
/// the first poll after a restart, `revision_changed` is false, the timestamp
/// is never re-adopted, `now - 0` dwarfs `stuck_after_ms`, and the watchdog
/// kills a session that may have made progress one second before the
/// restart. LOAD-TIME SHOULD SET revision_changed_at_ms = now (a grace
/// window), like a fresh adoption. Desired-behavior regression:
#[test]
#[ignore = "ENGINE BUG: frozen revision across a daemon restart is treated as stuck with zero grace. flagged to lead; do not edit src/ from this agent"]
fn frozen_revision_across_restart_gets_grace_window() {
    let root = tmp_root("readopt-frozen");
    write_registry_fixture(&root, registry_fixture());
    write_state_fixture(&root, working_session_state("w5:p7", 5, Some("ses_frozen")));

    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    // Paned is alive and working, but revision has NOT moved since the save.
    fake.set_agents(vec![agent_entry("w5:p7", "working", 5)]);
    let clock = Arc::new(FakeClock::new(T0 + 1_000)); // 1s after restart
    let daemon = build_daemon(&root, &fake, &oc, clock.clone(), 15);

    daemon.reconcile();
    daemon.poll_once();
    // Desired: the restart re-adopts the pane and gives the session a fresh
    // grace window; it must NOT be watchdog-killed on the first poll.
    assert_eq!(
        daemon.snapshot().sessions[0].status,
        Status::Working,
        "grace window after restart (currently: instantly killed as stuck)"
    );
}

#[test]
fn readopt_pane_missing_marks_done_and_cleans() {
    let root = tmp_root("readopt-gone-pane");
    write_registry_fixture(&root, registry_fixture());
    write_state_fixture(&root, working_session_state("w5:p9", 5, Some("ses_fixture999")));
    std::fs::create_dir_all(root.join("tmp").join("notch-S1-OC01")).unwrap();
    std::fs::write(root.join("tmp").join("notch-S1-OC01").join("marker"), b"x").unwrap();

    let fake = FakeHerdr::new(); // agent list will be empty: pane vanished
    let oc = FakeOpencode::new();
    fake.set_agents(vec![]);
    let clock = Arc::new(FakeClock::new(T0 + 50_000));
    let daemon = build_daemon(&root, &fake, &oc, clock.clone(), 15);

    daemon.reconcile();
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Done, "vanished pane -> done");
    assert_eq!(snap.sessions[0].failure_reason, None);
    assert!(!root.join("tmp").join("notch-S1-OC01").exists(), "cleanup removed the temp dir");
    assert!(events(&root).contains("marked done"), "done re-adoption logged");
    eprintln!("case4 pane missing obs=done, temp dir removed verdict=PASS");
}

#[test]
fn ids_never_reissued_after_kill_and_cleanup() {
    let root = tmp_root("ids-monotonic");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let plan = write_two_by_two_plan(&root);
    {
        let daemon = build_daemon(&root, &fake, &oc, clock.clone(), 15);
        assert!(daemon.run_plan(&plan).ok);
        daemon.kill("S1-OC01", Some("operator stop".to_string())).unwrap();
        // Drop: state persists through kill().
    }
    let clock2 = Arc::new(FakeClock::new(T0 + 1_000));
    let daemon = build_daemon(&root, &fake, &oc, clock2, 15);
    let panes = fake.panes();
    fake.set_agents(panes.iter().skip(1).map(|p| agent_entry(p, "working", 3)).collect()); // S1-OC01's pane gone

    let ack = daemon.spawn("SV-1", "follow-up work", &root.join("a").to_string_lossy(), "follow-up");
    assert!(ack.ok, "spawn ok: {:?}", ack.error);
    assert_eq!(ack.id.as_deref(), Some("S1-OC03"), "killed S1-OC01 is never reissued");
    let snap = daemon.snapshot();
    let ids: Vec<&str> = snap.sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids.iter().filter(|i| **i == "S1-OC01").count(), 1, "S1-OC01 appears once (failed)");
    assert!(ids.contains(&"S1-OC03"));
    eprintln!("case4 killed S1-OC01 obs=next id S1-OC03 verdict=PASS");
}

// ============================================================
// Case 5 — 50 concurrent fake sessions, poll_once timing regression
// ============================================================

fn write_fifty_session_plan(root: &Path) -> PathBuf {
    let cwd = root.join("sess");
    std::fs::create_dir_all(&cwd).unwrap();
    init_git(&cwd);
    let sessions: Vec<Value> = (0..50)
        .map(|i| {
            serde_json::json!({
                "title": format!("Session number {i}"),
                "prompt": format!("Do the thing for session number {i} with some care"),
                "cwd": cwd
            })
        })
        .collect();
    let plan = serde_json::json!({
        "stuck_after_secs": 15,
        "supervisors": [ { "sessions": sessions } ]
    });
    let path = root.join("plan.json");
    std::fs::write(&path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    path
}

#[test]
fn timing_poll_once_50_sessions_under_150ms() {
    let root = tmp_root("timing50");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = build_daemon(&root, &fake, &oc, clock.clone(), 15);
    let plan = write_fifty_session_plan(&root);
    assert!(daemon.run_plan(&plan).ok);
    let panes = fake.panes();
    assert_eq!(panes.len(), 50);

    // All 50 idle in herdr -> the poll exercises the 50-session bookkeeping,
    // the lazy session-ref binding, and the decision loop without paying
    // 50 real `git status` spawns (attribution sampling is unconfigurable
    // from outside src/; see the module docs).
    fake.set_agents(panes.iter().map(|p| agent_entry(p, "idle", 3)).collect());
    for _ in 0..200 {
        oc.push_decision("CONTINUE: keep going");
    }

    // Poll 1: priming (revision adoption + session-ref binding).
    let t = std::time::Instant::now();
    daemon.poll_once();
    let first = t.elapsed();

    clock.advance_ms(2_000);
    // Poll 2: steady state.
    let t = std::time::Instant::now();
    daemon.poll_once();
    let second = t.elapsed();

    let snap = daemon.snapshot();
    assert_eq!(snap.sessions.len(), 50);
    println!(
        "timing poll_once 50 sessions: first={}ms second={}ms (limit 150ms)",
        first.as_millis(),
        second.as_millis()
    );
    assert!(
        second < std::time::Duration::from_millis(150),
        "steady-state poll_once must stay under 150ms, got {}ms",
        second.as_millis()
    );
    eprintln!(
        "case5 sessions=50 obs first={}ms second={}ms verdict=PASS",
        first.as_millis(),
        second.as_millis()
    );
}

/// Ignored probe (not a pass/fail gate): 50 *working* sessions force one real
/// `git status` subprocess each (attribution sampling, unconfigurable from
/// outside src/). Logged for the report, asserted only against an absurd bound.
#[test]
#[ignore = "probe: measures real git cost; not a regression gate"]
fn timing_poll_once_50_working_sessions_git_probe() {
    let root = tmp_root("timing50w");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = build_daemon(&root, &fake, &oc, clock.clone(), 15);
    let plan = write_fifty_session_plan(&root);
    assert!(daemon.run_plan(&plan).ok);
    let panes = fake.panes();
    // Working, revision frozen, cwd does not exist -> git fails fast (~10ms each).
    fake.set_agents(panes.iter().map(|p| agent_entry(p, "working", 3)).collect());

    let t = std::time::Instant::now();
    daemon.poll_once();
    let elapsed = t.elapsed();
    println!("timing 50 working sessions (missing cwd): {}ms", elapsed.as_millis());
    assert!(elapsed < std::time::Duration::from_secs(30));
}

// ============================================================
// Case 6 — hostile data: unicode, length, collisions, NULs, u64::MAX
// ============================================================

#[test]
fn unicode_titles_length_and_collision_attacks() {
    let root = tmp_root("unicode-titles");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = build_daemon(&root, &fake, &oc, clock, 15);
    let cwd = root.join("u");
    std::fs::create_dir_all(&cwd).unwrap();

    // Visually identical, byte-distinct (bidi embeddings vs bidi overrides).
    let rtl_a = "deploy \u{2066}infra\u{2069} now";
    let rtl_b = "deploy \u{2067}infra\u{2069} now";
    assert_ne!(rtl_a, rtl_b);
    // Homoglyph collision: Cyrillic 'а' vs Latin 'a'.
    let homoglyph = "deplоy infra now"; // contains U+043E
    // Emoji + ZWJ rocket.
    let zwj = "ship it 🚀\u{200d}✨ 🚀\u{200d}✨";

    for (i, task) in [rtl_a, rtl_b, homoglyph, zwj].iter().enumerate() {
        let ack = daemon.submit_task(
            task.to_string(),
            Some(cwd.to_string_lossy().to_string()),
            None,
            None,
            None,
            None,
        );
        assert!(ack.ok, "task {i} must be accepted: {:?}", ack.error);
    }
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions.len(), 4, "all four hostile titles spawned");
    let ids: Vec<&str> = snap.sessions.iter().map(|s| s.id.as_str()).collect();
    let unique: std::collections::HashSet<&&str> = ids.iter().collect();
    assert_eq!(unique.len(), 4, "colliding titles still get distinct IDs");
    // Every exposed title is a sane single line; none exceeds 48 chars.
    for s in &snap.sessions {
        assert!(s.title.chars().count() <= 48, "title within 48 chars");
        assert!(!s.title.contains('\n'), "title is single-line");
    }
    // Everything round-trips through serde.
    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains("deploy"));
    eprintln!("case6 unicode titles obs=4 sessions, distinct ids, titles<=48 chars verdict=PASS");
}

#[test]
fn unicode_last_action_truncated_at_80_chars() {
    let root = tmp_root("unicode-lastact");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock);
    fake.set_agents(vec![agent_entry(&pane, "working", 3)]);
    // Last line is 200 flame emojis — multi-byte, must cut on chars not bytes.
    let flames = "🔥".repeat(200);
    fake.set_read_text(&format!("earlier line\n{flames}\n"));

    daemon.poll_once(); // prime: revision adoption only (last_action not read on priming? read is mapped==Working -> read happens)
    let snap = daemon.snapshot();
    let action = snap.sessions[0].last_action.clone().expect("last_action recorded");
    assert!(action.chars().count() <= 80, "truncated at 80 chars, got {}", action.chars().count());
    assert!(action.starts_with('🔥'));
    eprintln!("case6 lastAction obs={} chars (cap 80) verdict=PASS", action.chars().count());
}

#[test]
fn nul_bytes_in_pane_output_survive_roundtrip() {
    let root = tmp_root("nul-pane");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock);
    fake.set_agents(vec![agent_entry(&pane, "working", 3)]);
    fake.set_read_text("clean first line\n\0\0\0\x01\x02NULs and control bytes \0\n");

    daemon.poll_once(); // prime
    let snap = daemon.snapshot();
    let action = snap.sessions[0].last_action.clone().expect("last_action recorded");
    assert!(action.contains('\0'), "NULs preserved verbatim");
    assert!(!action.contains('\n'), "last_action is single-line");

    // state.json must survive serialize/deserialize with NULs embedded.
    daemon.flush_state();
    let state_bytes = std::fs::read(root.join("state").join("state.json")).unwrap();
    assert!(state_bytes.windows(4).any(|w| w == b"\\u00"), "NUL escaped in JSON");
    drop(daemon);
    let clock2 = Arc::new(FakeClock::new(T0 + 500));
    let daemon2 = build_daemon(&root, &fake, &oc, clock2, 15);
    let snap2 = daemon2.snapshot();
    let action2 = snap2.sessions[0].last_action.clone().expect("last_action survived reload");
    assert!(action2.contains('\0'), "NULs survive a daemon restart");
    eprintln!("case6 NULs in pane output obs=survive poll + restart verdict=PASS");
}

#[test]
fn task_field_10mb_no_blowup() {
    let root = tmp_root("task10mb");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = build_daemon(&root, &fake, &oc, clock, 15);
    let cwd = root.join("big");
    std::fs::create_dir_all(&cwd).unwrap();

    // 10 MB of realistic wordy text, no leading/trailing whitespace
    // (submit_task trims, and last_action/goal round-trip through JSON).
    let task = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor magna aliqua"
        .repeat(110_000); // 96 chars * 110k ~= 10.5 MB
    assert!(task.len() > 9_000_000, "task really is ~10MB ({} bytes)", task.len());
    assert_eq!(task.trim().len(), task.len());

    let t = std::time::Instant::now();
    let ack = daemon.submit_task(
        task.clone(),
        Some(cwd.to_string_lossy().to_string()),
        None,
        Some("definition of done: everything passes".to_string()),
        None,
        None,
    );
    let submit_ms = t.elapsed().as_millis();
    assert!(ack.ok, "10MB task accepted: {:?}", ack.error);

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    assert!(s.task.chars().count() <= 78, "wire task truncated to 78 chars");
    assert!(s.title.chars().count() <= 48, "wire title truncated to 48 chars");
    // The full goal really landed in state.json.
    let state = read_state(&root);
    let goal = state["sessions"][0]["goal"].as_str().unwrap();
    assert_eq!(goal.len(), task.len(), "full 10MB goal persisted");

    // A single 10MB token (no whitespace) is the worst case for the collapser.
    let token = "a".repeat(10 * 1024 * 1024);
    let t = std::time::Instant::now();
    let ack2 = daemon.submit_task(token, Some(cwd.to_string_lossy().to_string()), None, None, None, None);
    let token_ms = t.elapsed().as_millis();
    assert!(ack2.ok, "10MB single token accepted");
    let snap = daemon.snapshot();
    for s in &snap.sessions {
        assert!(s.task.chars().count() <= 78);
        assert!(s.title.chars().count() <= 48);
    }
    println!("timing 10MB task: submit={submit_ms}ms token={token_ms}ms");
    eprintln!("case6 10MB task obs submit={}ms token={}ms verdict=PASS", submit_ms, token_ms);
}

#[test]
fn revision_u64_max_no_overflow() {
    let root = tmp_root("u64max");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", u64::MAX)]);

    daemon.poll_once(); // prime at T0
    assert_eq!(daemon.snapshot().sessions[0].last_revision, Some(u64::MAX));
    step_advance(&clock, &daemon, 30_000);
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Failed);
    assert_eq!(snap.sessions[0].failure_reason.as_deref(), Some("stuck"));
    assert_eq!(fake.pane_close_count(), 1);
    // state.json serialized u64::MAX without loss.
    daemon.flush_state();
    assert_eq!(read_state(&root)["sessions"][0]["lastRevision"], u64::MAX);
    eprintln!("case6 revision u64::MAX obs=stuck kill at boundary, persisted verdict=PASS");
}

// ============================================================
// Case 7 — herdr fails mid-poll; daemon survives; next poll clean
// ============================================================

#[test]
fn herdr_fail_agent_read_second_command_daemon_survives() {
    let root = tmp_root("fail-read");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime; herdr healthy: list + read both OK
    fake.fail("agent read"); // 2nd herdr command of the poll fails
    step_advance(&clock, &daemon, 30_000); // stuck check still fires; read failure tolerated

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    assert_eq!(s.status, Status::Failed, "watchdog kill proceeds despite read failure");
    assert_eq!(s.failure_reason.as_deref(), Some("stuck"));
    assert_eq!(
        s.last_action.as_deref(),
        Some("last line of pane output"),
        "failed read leaves the stale last_action untouched — non-fatal, as designed"
    );
    assert_eq!(fake.pane_close_count(), 1, "close (3rd command) still ran");
    assert!(
        snap.warnings.iter().any(|w| w.contains("stuck")),
        "warning recorded for the stuck kill: {:?}",
        snap.warnings
    );

    fake.clear_failure();
    let status = daemon.status(); // daemon is responsive
    assert_eq!(status.sessions, 0);
    daemon.poll_once(); // clean poll after the fault
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Failed);
    eprintln!("case7 2nd-of-3 fails obs=survived, warning recorded, next poll clean verdict=PASS");
}

#[test]
fn herdr_fail_pane_close_warning_and_still_failed() {
    let root = tmp_root("fail-close");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);
    std::fs::create_dir_all(root.join("tmp").join("notch-S1-OC01")).unwrap();

    daemon.poll_once(); // prime
    fake.fail("pane close"); // the kill itself fails
    step_advance(&clock, &daemon, 30_000);

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    // A pane-close failure aborts the kill: the session stays live and its
    // temp dir is untouched so no uncommitted work is lost.
    assert_eq!(s.status, Status::Working, "failed close leaves the session alive");
    assert_eq!(s.failure_reason, None);
    assert!(
        snap.warnings.iter().any(|w| w.contains("pane close failed")),
        "close failure warning recorded: {:?}",
        snap.warnings
    );
    assert!(snap.escalations.iter().any(|e| e.kind == "stuck"), "stuck escalation recorded anyway");
    assert!(root.join("tmp").join("notch-S1-OC01").exists(), "no cleanup when the close failed");

    let failed_attempts = fake.pane_close_count();
    assert!(failed_attempts >= 1, "every stuck tick retries the failing close");
    fake.clear_failure();
    daemon.poll_once(); // next tick retries the close with herdr healthy
    assert_eq!(fake.pane_close_count(), failed_attempts + 1, "exactly one retry after recovery");
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Failed, "retry completes the kill");
    eprintln!("case7 pane close fails obs=survived, warning recorded, retry kills verdict=PASS");
}

#[test]
fn herdr_fail_agent_prompt_daemon_survives_blocked_path() {
    let root = tmp_root("fail-prompt");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime (spawn prompt happened before this)
    fake.clear_calls();
    fake.set_agents(vec![agent_entry(&pane, "blocked", 8)]); // human gate begins
    fake.fail("agent prompt"); // a prompt failure cannot matter: none is attempted
    clock.advance_ms(100);
    daemon.poll_once();

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    assert!(!s.status.is_terminal(), "blocked waits on a human, not failed");
    assert_eq!(s.status, Status::Idle);
    assert_eq!(s.failure_reason.as_deref(), Some("blocked_waiting"));
    assert!(snap.escalations.iter().any(|e| e.kind == "needs_you"), "human escalation recorded");
    assert!(
        snap.warnings.iter().any(|w| w.contains("escalated")),
        "escalation warning recorded: {:?}",
        snap.warnings
    );

    fake.clear_failure();
    daemon.poll_once(); // clean
    assert!(!daemon.snapshot().sessions[0].status.is_terminal());
    eprintln!("case7 prompt would fail obs=survived, needs_you escalation, no prompt verdict=PASS");
}

#[test]
fn herdr_fail_agent_list_at_gate_next_poll_clean() {
    let root = tmp_root("fail-list");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime
    fake.fail("agent list"); // the gate itself fails
    clock.advance_ms(1_000);
    daemon.poll_once(); // must not panic; must record a warning; tree unchanged

    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Working, "session untouched by the failed poll");
    assert!(
        snap.warnings.iter().any(|w| w.contains("herdr unavailable")),
        "gate failure warning recorded: {:?}",
        snap.warnings
    );

    fake.clear_failure();
    step_advance(&clock, &daemon, 30_000); // clean poll: stuck kill proceeds on the frozen revision
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Failed);
    eprintln!("case7 gate fails obs=survived, warning recorded, next poll clean verdict=PASS");
}

// ============================================================
// Case 8 — interleaved kill + re-prompt: no double close, exactly-once cleanup
// ============================================================

#[test]
fn kill_then_stale_herdr_no_double_close_cleanup_exactly_once() {
    let root = tmp_root("kill-stale");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);
    let tmp_dir = root.join("tmp").join("notch-S1-OC01");
    assert!(tmp_dir.exists(), "spawn creates the per-session temp dir");

    daemon.poll_once(); // prime at T0
    step_advance(&clock, &daemon, 30_000); // stuck kill fires: close #1, cleanup runs
    assert_eq!(fake.pane_close_count(), 1);
    assert!(!tmp_dir.exists(), "cleanup removed the temp dir on the kill poll");

    // Cleanup exactly once: recreate the dir; a later poll must not re-clean.
    std::fs::create_dir_all(&tmp_dir).unwrap();
    std::fs::write(tmp_dir.join("marker"), b"x").unwrap();
    step_advance(&clock, &daemon, 40_000); // stale herdr view still says working/frozen
    assert_eq!(fake.pane_close_count(), 1, "stale herdr data must not re-close the pane");
    assert!(tmp_dir.exists() && tmp_dir.join("marker").exists(), "cleanup did not run twice");

    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Failed);
    assert_eq!(snap.sessions[0].failure_reason.as_deref(), Some("stuck"));
    assert_eq!(event_count(&events(&root), "watchdog"), 2, "suspect + stuck verdicts");
    eprintln!("case8 stale-after-kill obs=1 close, cleanup exactly once verdict=PASS");
}

#[test]
fn reprompt_then_stuck_kill_stuck_wins_no_double_close() {
    let root = tmp_root("reprompt-stuck");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);
    let tmp_dir = root.join("tmp").join("notch-S1-OC01");

    daemon.poll_once(); // prime at T0
    // Poll 2 at T0+1s: herdr reports blocked -> the human owns the gate. The
    // session stays non-terminal, nothing is typed into the prompt, pane/temp
    // are kept, and a `needs_you` escalation is recorded.
    fake.set_agents(vec![agent_entry(&pane, "blocked", 8)]);
    clock.advance_ms(1_000);
    daemon.poll_once();
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Idle, "blocked waits on a human");
    assert_eq!(snap.sessions[0].failure_reason.as_deref(), Some("blocked_waiting"));
    assert_eq!(fake.pane_close_count(), 0, "a human gate never closes the pane");
    assert!(tmp_dir.exists(), "temp dir kept for the human");
    assert!(snap.escalations.iter().any(|e| e.kind == "needs_you"));

    // Poll 3: back to working with revision frozen at 8. Stepped ticks reach
    // the 2x Stuck boundary; the watchdog verdict still wins.
    fake.set_agents(vec![agent_entry(&pane, "working", 8)]);
    step_advance(&clock, &daemon, 30_000);
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Failed, "verified stuck wins over a human gate");
    assert_eq!(snap.sessions[0].failure_reason.as_deref(), Some("stuck"));
    assert_eq!(fake.pane_close_count(), 1, "exactly one close after the stuck verdict");
    assert!(!tmp_dir.exists(), "cleanup ran once on the kill");

    // Poll 4: stale herdr still reporting working/8 — nothing happens.
    clock.advance_ms(1_000);
    daemon.poll_once();
    assert_eq!(fake.pane_close_count(), 1);
    let kinds: Vec<String> = daemon
        .snapshot()
        .escalations
        .iter()
        .map(|e| e.kind.clone())
        .collect();
    assert!(kinds.contains(&"stuck".to_string()), "stuck verdict recorded: {kinds:?}");
    assert!(kinds.contains(&"needs_you".to_string()), "human gate was escalated: {kinds:?}");
    assert!(!kinds.iter().any(|k| k == "blocked"), "blocked is never a plain failure escalation");
    eprintln!("case8 reprompt->stuck obs=1 close, failed/stuck wins verdict=PASS");
}

#[test]
fn blocked_twice_escalates_blocked_no_pane_close_cleanup_once() {
    let root = tmp_root("blocked-twice");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);
    let tmp_dir = root.join("tmp").join("notch-S1-OC01");

    daemon.poll_once(); // prime at T0
    fake.set_agents(vec![agent_entry(&pane, "blocked", 8)]);
    clock.advance_ms(1_000);
    daemon.poll_once(); // blocked -> needs_you, pane and temp kept
    fake.set_agents(vec![agent_entry(&pane, "blocked", 9)]);
    clock.advance_ms(1_000);
    daemon.poll_once(); // still waiting on the human

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    assert_eq!(s.status, Status::Idle, "blocked is not failed");
    assert_eq!(s.failure_reason.as_deref(), Some("blocked_waiting"));
    assert!(snap.escalations.iter().any(|e| e.kind == "needs_you"));
    assert_eq!(fake.pane_close_count(), 0, "a human gate never closes the pane");
    assert!(tmp_dir.exists(), "temp dir kept for the human");
    assert_eq!(event_count(&events(&root), "watchdog"), 0);

    std::fs::write(tmp_dir.join("marker"), b"x").unwrap();
    clock.advance_ms(10_000);
    daemon.poll_once();
    assert!(tmp_dir.exists(), "cleanup must never run for a human gate");
    assert!(tmp_dir.join("marker").exists());
    eprintln!("case8 blocked x2 obs=0 closes, idle/blocked_waiting, temp kept verdict=PASS");
}

// ============================================================
// Case 9 — blocked maps to failed/blocked_waiting + escalation kind "blocked"
// ============================================================

#[test]
fn blocked_herdr_mapping_is_failed_blocked_waiting() {
    use notchd::status::Status as S;
    assert_eq!(Status::from_herdr("blocked"), (S::Failed, Some("blocked_waiting".to_string())));
    assert_eq!(Status::from_herdr("unknown"), (S::Idle, None));
    assert_eq!(Status::from_herdr("working"), (S::Working, None));
    assert_eq!(Status::from_herdr("idle"), (S::Idle, None));
    assert_eq!(Status::from_herdr("done"), (S::Done, None));
    // Garbage input never panics and maps to idle.
    assert_eq!(Status::from_herdr(""), (S::Idle, None));
    assert_eq!(Status::from_herdr("\u{0}\u{1}"), (S::Idle, None));
}

#[test]
fn blocked_first_failure_is_needs_human_no_prompt() {
    let root = tmp_root("blocked-first");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime
    fake.set_agents(vec![agent_entry(&pane, "blocked", 8)]);
    clock.advance_ms(1_000);
    daemon.poll_once();

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    assert!(!s.status.is_terminal(), "blocked waits on a human, not failed");
    assert_eq!(s.status, Status::Idle);
    assert_eq!(s.failure_reason.as_deref(), Some("blocked_waiting"));
    assert_eq!(fake.pane_close_count(), 0);
    assert!(snap.escalations.iter().any(|e| e.session == "S1-OC01" && e.kind == "needs_you"));
    assert_eq!(event_count(&events(&root), "reprompt"), 0, "never type into a permission prompt");
    eprintln!("case9 first blocked obs=idle/needs_you, no prompt verdict=PASS");
}

#[test]
fn blocked_escalates_needs_you_and_excludes_decider() {
    let root = tmp_root("blocked-esc");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime
    fake.set_agents(vec![agent_entry(&pane, "blocked", 8)]);
    clock.advance_ms(1_000);
    daemon.poll_once(); // blocked -> needs_you escalation

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    assert!(!s.status.is_terminal(), "blocked is not failed");
    assert_eq!(s.status, Status::Idle);
    assert_eq!(s.failure_reason.as_deref(), Some("blocked_waiting"));
    let esc = snap
        .escalations
        .iter()
        .find(|e| e.session == "S1-OC01")
        .expect("needs_you escalation recorded in snapshot");
    assert_eq!(esc.kind, "needs_you", "blocked waits on a human");
    assert!(esc.reason.contains("blocked_waiting"), "reason carries blocked_waiting: {}", esc.reason);
    assert!(esc.id.starts_with("esc_"));
    assert_eq!(event_count(&events(&root), "escalate"), 1);

    // The escalated session is excluded from the decision loop forever.
    fake.set_agents(vec![agent_entry(&pane, "idle", 10)]);
    clock.advance_ms(5_000);
    daemon.poll_once();
    assert_eq!(oc.decider_calls(), 0, "escalated session: no decider fires");
    eprintln!("case9 blocked obs=idle/needs_you, loop excluded verdict=PASS");
}

#[test]
fn blocked_never_prompts_even_if_prompt_would_fail() {
    let root = tmp_root("blocked-promptfail");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, pane) = spawn_one_session(&root, &fake, &oc, clock.clone());
    fake.set_agents(vec![agent_entry(&pane, "working", 7)]);

    daemon.poll_once(); // prime
    fake.set_agents(vec![agent_entry(&pane, "blocked", 8)]);
    fake.fail("agent prompt"); // re-prompt delivery fails outright
    clock.advance_ms(1_000);
    daemon.poll_once();

    let snap = daemon.snapshot();
    let s = &snap.sessions[0];
    assert!(!s.status.is_terminal(), "blocked is not failed");
    assert_eq!(s.status, Status::Idle);
    assert_eq!(s.failure_reason.as_deref(), Some("blocked_waiting"));
    let esc = snap
        .escalations
        .iter()
        .find(|e| e.session == "S1-OC01")
        .expect("needs_you escalation recorded");
    assert_eq!(esc.kind, "needs_you");
    assert_eq!(fake.pane_close_count(), 0, "a human gate never closes a pane");
    assert!(
        !fake.calls().iter().any(|c| c.get(4).map(String::as_str)
            .map(|t| t.contains("Diagnose and continue"))
            .unwrap_or(false)),
        "blocked never triggers the diagnose-and-continue re-prompt"
    );
    eprintln!("case9 blocked obs=idle/needs_you, zero prompts verdict=PASS");
}
