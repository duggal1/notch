//! A3 liveness — tests for the monotonic clock, watchdog classification,
//! stall guard, suspicion/stuck ladder, and blocked-is-human semantics.
//!
//! Every test here is driven by injected fake clocks and fake herdr/opencode
//! runners: no sleeps, no real subprocesses, no GUI, no `#[ignore]`.

use notchd::clock::{Clock, FakeClock, SystemClock};
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;
use notchd::status::Status;
use notchd::supervisor::{decide_on_failure_with_reason, FailureAction};
use notchd::watchdog::{Liveness, Obs, Watchdog};

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const T0: u64 = 1_763_520_000_000;

// ---------- Phase 0: Clock::mono_ms ----------

#[test]
fn phase0_mono_is_monotonic_and_shares_one_epoch() {
    let a = SystemClock.mono_ms();
    let b = SystemClock.mono_ms();
    let c = SystemClock.mono_ms();
    assert!(b >= a, "mono_ms must never go backwards: {a} -> {b}");
    assert!(c >= b, "mono_ms must never go backwards: {b} -> {c}");
    assert!(c - b < 5_000, "separate Clock instances share one OnceLock anchor");
}

#[test]
fn phase0_fake_mono_ignores_wall_clock_jumps() {
    let c = FakeClock::new(T0);
    assert_eq!(c.mono_ms(), T0);
    assert_eq!(c.now_ms(), T0);

    c.set(T0 + 3_600_000); // wall +1h (NTP / user change)
    assert_eq!(c.mono_ms(), T0, "mono must not follow a wall-clock jump");
    assert_eq!(c.now_ms(), T0 + 3_600_000);

    c.set(T0 - 3_600_000); // wall -1h
    assert_eq!(c.mono_ms(), T0, "mono must not follow a backwards wall jump");

    c.advance_ms(5_000); // real elapsed time
    assert_eq!(c.mono_ms(), T0 + 5_000, "mono advances only with elapsed time");
    assert_eq!(c.now_ms(), T0 - 3_600_000 + 5_000);
}

// ---------- section 3.4: classification ----------

fn obs(quiet_ms: u64) -> Obs {
    Obs {
        status: Status::Working,
        revision_changed_now: false,
        oc_busy: None,
        quiet_for_ms: quiet_ms,
    }
}

#[test]
fn classify_alive_when_not_working_or_revision_changed_or_oc_busy() {
    let w = Watchdog::new(15);
    assert_eq!(w.classify(obs(20_000)), Liveness::Suspect, "sanity: quiet working session");
    assert_eq!(w.classify(Obs { status: Status::Idle, ..obs(60_000) }), Liveness::Alive);
    assert_eq!(w.classify(Obs { status: Status::Done, ..obs(60_000) }), Liveness::Alive);
    assert_eq!(w.classify(Obs { status: Status::Failed, ..obs(60_000) }), Liveness::Alive);
    assert_eq!(
        w.classify(Obs { revision_changed_now: true, ..obs(60_000) }),
        Liveness::Alive,
        "revision changed in THIS poll means the old timestamp is irrelevant"
    );
    assert_eq!(
        w.classify(Obs { oc_busy: Some(true), ..obs(60_000) }),
        Liveness::Alive,
        "oc_busy is a second opinion that rescues a frozen revision"
    );
}

#[test]
fn classify_suspect_at_threshold_stuck_at_double_threshold() {
    let w = Watchdog::new(15);
    assert_eq!(w.classify(obs(14_999)), Liveness::Alive);
    assert_eq!(w.classify(obs(15_000)), Liveness::Suspect, "threshold is inclusive");
    assert_eq!(w.classify(obs(29_999)), Liveness::Suspect);
    assert_eq!(w.classify(obs(30_000)), Liveness::Stuck, "Stuck requires 2x threshold");
    assert_eq!(w.classify(obs(u64::MAX)), Liveness::Stuck, "no overflow panic");
}

#[test]
fn oc_busy_false_is_not_a_rescue() {
    let w = Watchdog::new(15);
    assert_eq!(
        w.classify(Obs { oc_busy: Some(false), ..obs(60_000) }),
        Liveness::Stuck,
        "only Some(true) counts; Some(false) and None are not evidence of life"
    );
}

#[test]
fn pane_close_defaults_on_and_only_for_stuck() {
    let mut w = Watchdog::new(15);
    assert!(w.kill_on_stuck(), "production default kills verified stuck sessions");
    assert!(w.should_close_pane(Liveness::Stuck));
    assert!(!w.should_close_pane(Liveness::Suspect), "Suspect is nudge-only");
    assert!(!w.should_close_pane(Liveness::Alive));
    w.set_kill_on_stuck(false);
    assert!(!w.should_close_pane(Liveness::Stuck), "operators can opt out");
}

#[test]
fn stall_gap_is_strictly_greater_than_three_poll_intervals() {
    assert!(!Watchdog::is_stall_gap(6_000, 2_000));
    assert!(Watchdog::is_stall_gap(6_001, 2_000));
    assert!(!Watchdog::is_stall_gap(0, 0), "zero interval never trips the guard");
}

#[test]
fn suspect_action_is_once_per_episode_and_reset_on_revision_change() {
    let mut w = Watchdog::new(15);
    assert_eq!(w.note_poll(1_000), None);
    assert_eq!(w.note_poll(3_000), Some(2_000), "gap reported for stall guard");
    assert!(w.take_suspect_action("S1-OC01", 20_000));
    assert!(!w.take_suspect_action("S1-OC01", 25_000), "no nudge spam every tick");
    w.mark_revision_change("S1-OC01", 30_000);
    assert!(w.take_suspect_action("S1-OC01", 50_000), "new episode re-arms nudge");
    assert!(w.take_stuck_action("S1-OC01", 70_000));
    assert!(!w.take_stuck_action("S1-OC01", 75_000), "stuck escalation is once per episode");
}

// ---------- section 3.8: blocked is waiting on a human ----------

#[test]
fn blocked_is_needs_human_on_first_and_later_failures() {
    for count in [0, 1, 7] {
        match decide_on_failure_with_reason(Some("blocked_waiting"), count, Some("tail".into())) {
            FailureAction::NeedsHuman { reason } => {
                assert!(reason.contains("blocked_waiting"), "reason carries the cause: {reason}");
            }
            other => panic!("blocked must never reprompt or plain-escalate (count {count}): {other:?}"),
        }
    }
}

#[test]
fn non_blocked_failures_keep_reprompt_then_escalate() {
    assert!(matches!(
        decide_on_failure_with_reason(None, 0, Some("tail".into())),
        FailureAction::Reprompt { .. }
    ));
    assert!(matches!(
        decide_on_failure_with_reason(Some("other_reason"), 0, None),
        FailureAction::Reprompt { .. }
    ));
    assert!(matches!(
        decide_on_failure_with_reason(None, 1, None),
        FailureAction::Escalate
    ));
}

// ---------- poll-level fakes and builders ----------

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a3-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

struct FakeHerdr {
    calls: Mutex<Vec<Vec<String>>>,
    agents: Mutex<Vec<Value>>,
    split_seq: Mutex<u32>,
    on_close: Mutex<Option<Box<dyn Fn() + Send + 'static>>>,
}

impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            agents: Mutex::new(Vec::new()),
            split_seq: Mutex::new(0),
            on_close: Mutex::new(None),
        })
    }

    fn on_close(&self, f: impl Fn() + Send + 'static) {
        *self.on_close.lock().unwrap() = Some(Box::new(f));
    }

    fn set_agents(&self, agents: Vec<Value>) {
        *self.agents.lock().unwrap() = agents;
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    fn clear_calls(&self) {
        self.calls.lock().unwrap().clear();
    }

    fn close_count(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.get(1).map(String::as_str) == Some("pane")
                && c.get(2).map(String::as_str) == Some("close"))
            .count()
    }

    fn prompts(&self) -> Vec<String> {
        self.calls()
            .iter()
            .filter(|c| c.get(1).map(String::as_str) == Some("agent")
                && c.get(2).map(String::as_str) == Some("prompt"))
            .map(|c| c.get(4).cloned().unwrap_or_default())
            .collect()
    }

    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args| fake.run(args))
    }

    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        self.calls.lock().unwrap().push(args.to_vec());
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("agent", "list") => Ok(serde_json::json!({
                "id": "cli:agent:list",
                "result": { "agents": self.agents.lock().unwrap().clone() }
            })
            .to_string()),
            ("pane", "split") => {
                let mut seq = self.split_seq.lock().unwrap();
                *seq += 1;
                Ok(serde_json::json!({
                    "id": "cli:pane:split",
                    "result": { "pane": { "pane_id": format!("wT:p{seq}") } }
                })
                .to_string())
            }
            ("agent", "start") | ("agent", "prompt") => {
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            }
            ("pane", "close") => {
                if let Some(f) = self.on_close.lock().unwrap().as_ref() {
                    f();
                }
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            }
            ("agent", "read") => Ok("still working on it\n".to_string()),
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected herdr call {}", args.join(" ")),
            }),
        }
    }
}

fn fake_opencode() -> Opencode {
    Opencode::with_runner(|args| {
        if args.get(1).map(String::as_str) == Some("api") {
            Ok(r#"{"cursor":null,"data":[]}"#.to_string())
        } else {
            Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: "no decider expected".to_string(),
            })
        }
    })
}

fn agent(pane: &str, status: &str, revision: u64) -> Value {
    serde_json::json!({
        "pane_id": pane,
        "agent_status": status,
        "revision": revision,
        "agent_session": { "kind": "id", "value": format!("ses_{pane}") },
        "cwd": "/tmp",
        "focused": false
    })
}

fn config(root: &Path) -> Config {
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 15,
        poll_interval_ms: 2_000,
        opencode_decider: false,
    }
}

fn build(root: &Path, fake: &Arc<FakeHerdr>, clock: Arc<FakeClock>) -> Daemon {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/bin/true");
    let cfg = config(root);
    std::fs::create_dir_all(&cfg.tmp_dir).unwrap();
    let daemon = Daemon::open(cfg, fake.herdr(), clock).unwrap();
    daemon.set_opencode(fake_opencode());
    daemon
}

fn plan(root: &Path, n: usize) -> PathBuf {
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    // A5's worktree::snapshot needs a git repo at the session cwd; without
    // one the snapshot vetoes the kill (correct safety behavior).
    assert!(
        std::process::Command::new("git")
            .args(["-C", cwd.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );
    let sessions: Vec<Value> = (0..n)
        .map(|i| {
            serde_json::json!({
                "title": format!("Task number {i}"),
                "prompt": format!("Do task number {i}"),
                "cwd": cwd
            })
        })
        .collect();
    let doc = serde_json::json!({
        "stuck_after_secs": 15,
        "supervisors": [{ "sessions": sessions }]
    });
    let path = root.join("plan.json");
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    path
}

fn spawn(root: &Path, fake: &Arc<FakeHerdr>, clock: Arc<FakeClock>, n: usize) -> (Daemon, Vec<String>) {
    let daemon = build(root, fake, clock);
    assert!(daemon.run_plan(&plan(root, n)).ok);
    let panes = (1..=n).map(|i| format!("wT:p{i}")).collect();
    (daemon, panes)
}

fn escalation_kinds(daemon: &Daemon) -> Vec<String> {
    daemon
        .snapshot()
        .escalations
        .iter()
        .map(|e| e.kind.clone())
        .collect()
}

/// Advance real elapsed time in poll-sized steps. Jumping straight to the
/// threshold would (correctly) trip the stall guard instead of the ladder.
fn tick_to(clock: &FakeClock, daemon: &Daemon, total_ms: u64) {
    let mut elapsed = 0;
    while elapsed < total_ms {
        let step = 2_000u64.min(total_ms - elapsed);
        clock.advance_ms(step);
        daemon.poll_once();
        elapsed += step;
    }
}

// ---------- T3: stall guard + the exact regression ----------

#[test]
fn stalled_hour_plus_revision_change_in_same_poll_never_kills() {
    let root = tmp_root("stall-rev");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];

    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once(); // prime: timestamp = mono T0
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);

    // The daemon did not tick for an hour, AND the session made progress in
    // the very poll that resumes. Both signals must veto a kill.
    clock.advance_mono_ms(3_600_000);
    fake.set_agents(vec![agent(pane, "working", 8)]);
    daemon.poll_once();

    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Working, "revision change wins over stalled timestamp");
    assert_eq!(snap.sessions[0].failure_reason, None);
    assert_eq!(fake.close_count(), 0, "no pane close on a stalled hour + revision change");
    assert!(!escalation_kinds(&daemon).iter().any(|k| k == "stuck" || k == "stuck_suspect"));
    assert!(snap.warnings.iter().any(|w| w.contains("watchdog stall")), "stall logged: {:?}", snap.warnings);
}

#[test]
fn stall_alone_grants_grace_without_killing() {
    let root = tmp_root("stall-alone");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];
    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once();

    clock.advance_mono_ms(3_600_000); // no revision change at all
    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert_eq!(fake.close_count(), 0);
    assert!(escalation_kinds(&daemon).is_empty(), "stall grants grace, no escalation");
    assert!(daemon.snapshot().warnings.iter().any(|w| w.contains("grace granted")));
}

// ---------- action ladder ----------

#[test]
fn suspect_nudges_and_escalates_once_without_closing_pane() {
    let root = tmp_root("suspect");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];
    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once();
    fake.clear_calls(); // discard the spawn prompt from run_plan

    tick_to(&clock, &daemon, 16_000); // crosses the 15s suspicion threshold
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert_eq!(fake.close_count(), 0, "Suspect never closes a pane");
    assert!(escalation_kinds(&daemon).contains(&"stuck_suspect".to_string()));
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "one nudge, not one per tick: {prompts:?}");
    assert!(!prompts[0].contains("Diagnose and continue"), "nudge is not a failure re-prompt");

    daemon.poll_once(); // same instant: no repeat nudge
    assert_eq!(fake.prompts().len(), 1, "nudge deduped within an episode");
}

#[test]
fn stuck_escalates_but_keeps_pane_when_kill_is_off() {
    let root = tmp_root("stuck-keep");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];
    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once();
    daemon.set_kill_on_stuck(false); // explicit opt-out for this legacy case

    tick_to(&clock, &daemon, 30_000); // 2x threshold in poll-sized steps
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Working, "Stuck with kill disabled is not failed");
    assert_eq!(fake.close_count(), 0);
    assert!(escalation_kinds(&daemon).contains(&"stuck".to_string()));
    assert!(snap.warnings.iter().any(|w| w.contains("kill_on_stuck is OFF")));
}

#[test]
fn stuck_with_kill_on_stuck_snapshots_wip_before_close() {
    let root = tmp_root("stuck-kill");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];
    let cwd = root.join("work"); // cwd from the plan

    // A real git repo with an uncommitted file: the snapshot must capture it.
    let run_git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git {:?} failed",
            args
        );
    };
    run_git(&["-C", cwd.to_str().unwrap(), "init", "-q"]);
    run_git(&["-C", cwd.to_str().unwrap(), "config", "user.email", "a3@example.test"]);
    run_git(&["-C", cwd.to_str().unwrap(), "config", "user.name", "A3 Test"]);
    std::fs::write(cwd.join("README.md"), "initial\n").unwrap();
    run_git(&["-C", cwd.to_str().unwrap(), "add", "-A"]);
    run_git(&["-C", cwd.to_str().unwrap(), "commit", "-qm", "initial"]);
    std::fs::write(cwd.join("WIP.txt"), "uncommitted work\n").unwrap();

    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once();
    daemon.set_kill_on_stuck(true);
    tick_to(&clock, &daemon, 30_000);

    let subject = String::from_utf8(
        std::process::Command::new("git")
            .args(["-C", cwd.to_str().unwrap(), "log", "-1", "--pretty=%s"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(
        subject.contains("notch: WIP snapshot (stuck)"),
        "canonical WIP snapshot message required; last commit was {subject:?}"
    );
    assert_eq!(fake.close_count(), 1, "opt-in kill closes exactly one pane");
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Failed);
    assert_eq!(daemon.snapshot().sessions[0].failure_reason.as_deref(), Some("stuck"));
}

// ---------- section 3.8 at the poll level ----------

#[test]
fn blocked_keeps_pane_and_temp_and_never_types_a_prompt() {
    let root = tmp_root("blocked-human");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];
    let temp = root.join("tmp").join("notch-S1-OC01");
    assert!(temp.exists(), "spawn creates the session temp dir");

    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once(); // prime (the spawn prompt happened before this)
    fake.clear_calls();
    fake.set_agents(vec![agent(pane, "blocked", 8)]);
    clock.advance_ms(1_000);
    daemon.poll_once();

    let snap = daemon.snapshot();
    assert!(!snap.sessions[0].status.is_terminal(), "blocked is not failed: {}", snap.sessions[0].status);
    assert_eq!(snap.sessions[0].failure_reason.as_deref(), Some("blocked_waiting"));
    assert_eq!(fake.prompts(), Vec::<String>::new(), "never type into a permission prompt");
    assert_eq!(fake.close_count(), 0, "pane kept for the human");
    assert!(temp.exists(), "temp dir kept for the human");
    assert!(escalation_kinds(&daemon).contains(&"needs_you".to_string()));
}

// ---------- verifier: production default kills a VERIFIED stuck session ----------

#[test]
fn stuck_default_config_closes_pane_after_escalation_is_recorded() {
    let root = tmp_root("stuck-default");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];

    // At the instant of pane_close, the `stuck` escalation must already be
    // in the append-only event log: escalation and WIP snapshot come first.
    let events_path = root.join("state").join("events.jsonl");
    fake.on_close(move || {
        let log = std::fs::read_to_string(&events_path).unwrap_or_default();
        assert!(
            log.contains("\"kind\":\"escalate\"") && log.contains("stuck"),
            "stuck escalation must be recorded before pane close: {log}"
        );
    });

    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once(); // prime on the default config
    fake.clear_calls();

    tick_to(&clock, &daemon, 30_000); // 2x threshold => verified Stuck

    assert_eq!(fake.close_count(), 1, "default config closes a verified stuck pane");
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Failed);
    assert_eq!(snap.sessions[0].failure_reason.as_deref(), Some("stuck"));
    assert!(escalation_kinds(&daemon).contains(&"stuck".to_string()));
}

#[test]
fn default_config_stall_grants_grace_and_kills_nobody() {
    let root = tmp_root("stall-default");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 3);
    fake.set_agents(panes.iter().map(|p| agent(p, "working", 7)).collect());
    daemon.poll_once(); // prime all three with the default (kill-on-stuck) config

    // A one-hour scheduler stall looks exactly like a one-hour silence. The
    // guard must grant grace even though the default config does kill Stuck.
    clock.advance_mono_ms(3_600_000);
    daemon.poll_once();

    assert_eq!(fake.close_count(), 0, "stall grants grace: zero kills");
    let snap = daemon.snapshot();
    assert!(snap.sessions.iter().all(|s| s.status == Status::Working));
    assert!(escalation_kinds(&daemon).is_empty());
}

// ---------- T3: 20 sessions, one truly stuck ----------

#[test]
fn twenty_sessions_one_truly_stuck_zero_false_positives() {
    let root = tmp_root("twenty");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 20);
    let stuck_pane = panes[7].clone();

    let mut ticks: Vec<Duration> = Vec::new();
    for tick in 1..=20u64 {
        let revision = 100 + tick;
        let agents = panes
            .iter()
            .map(|p| {
                if p == &stuck_pane {
                    agent(p, "working", 7) // frozen from the priming poll
                } else {
                    agent(p, "working", revision)
                }
            })
            .collect();
        fake.set_agents(agents);
        clock.advance_ms(2_000); // one poll interval per tick
        let t = Instant::now();
        daemon.poll_once();
        ticks.push(t.elapsed());
    }

    let snap = daemon.snapshot();
    assert_eq!(snap.sessions.len(), 20);
    assert_eq!(snap.sessions[7].status, Status::Failed, "default config kills the verified stuck session");
    let escalated: Vec<&str> = snap
        .escalations
        .iter()
        .filter(|e| e.kind == "stuck" || e.kind == "stuck_suspect")
        .map(|e| e.session.as_str())
        .collect();
    assert!(escalated.contains(&"S1-OC08"), "the one truly stuck session was flagged: {escalated:?}");
    assert_eq!(escalated.iter().filter(|s| **s != "S1-OC08").count(), 0, "zero false positives: {escalated:?}");
    assert_eq!(fake.close_count(), 1, "default config closes the one verified stuck pane");

    ticks.sort();
    let p99 = ticks[ticks.len() - 1]; // 20 samples: worst == p99 here
    let bound = Duration::from_millis(2 * config(&root).poll_interval_ms);
    assert!(p99 <= bound, "tick gap p99 {p99:?} exceeded 2x poll interval {bound:?}");
    eprintln!("A3 T3: 20 sessions, 20 ticks, p99={}ms, false_positives=0", p99.as_millis());
}

// ---------- T4: clock chaos ----------

#[test]
fn clock_chaos_wall_jumps_never_kill() {
    let root = tmp_root("clock-chaos");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];
    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once();

    // +1h wall jump, only 5s of real elapsed time.
    clock.set(T0 + 3_600_000);
    clock.advance_mono_ms(5_000);
    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert_eq!(fake.close_count(), 0);

    // -1h wall jump, another 5s of real elapsed time.
    clock.set(T0 - 3_600_000);
    clock.advance_mono_ms(5_000);
    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert_eq!(fake.close_count(), 0);
    assert!(!escalation_kinds(&daemon).iter().any(|k| k == "stuck"));
}

#[test]
fn wall_clock_alone_cannot_age_a_session() {
    let root = tmp_root("wall-only");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, panes) = spawn(&root, &fake, clock.clone(), 1);
    let pane = &panes[0];
    fake.set_agents(vec![agent(pane, "working", 7)]);
    daemon.poll_once();

    // A +2h wall jump with zero monotonic elapsed time. All liveness
    // decisions must be unaffected, even though wall now_ms moved hours.
    clock.set(T0 + 7_200_000);
    daemon.poll_once();
    assert_eq!(daemon.snapshot().sessions[0].status, Status::Working);
    assert!(escalation_kinds(&daemon).is_empty());
    assert_eq!(fake.close_count(), 0);
}
