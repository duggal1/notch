//! W2-4 — daemon-level headless tests for the fleet guard wiring.
//!
//! Proves the reasons reach the real decision log, that a burst is deferred,
//! that a storm preempts without closing a pane, and that nothing re-prompts
//! forever. Fake herdr + fake clock; no sleeps, no GUI, no `#[ignore]`.

use notchd::clock::{Clock, FakeClock};
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;
use notchd::status::Status;
use notchd::supervisor::{fleet_guard_for, BURST_STARTS, STORM_STARTS};

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const T0: u64 = 8_000_000_000;

static N: AtomicU64 = AtomicU64::new(0);

fn tmp_root() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-w2-storm-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

struct FakeHerdr {
    calls: Mutex<Vec<Vec<String>>>,
    agents: Mutex<Vec<Value>>,
    split_seq: Mutex<u32>,
}

impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            agents: Mutex::new(Vec::new()),
            split_seq: Mutex::new(0),
        })
    }
    fn set_agents(&self, agents: Vec<Value>) {
        *self.agents.lock().unwrap() = agents;
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
    fn prompt_count(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.get(1).map(String::as_str) == Some("agent")
                && c.get(2).map(String::as_str) == Some("prompt"))
            .count()
    }
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args| fake.run(args))
    }
    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        self.calls.lock().unwrap().push(args.to_vec());
        match (
            args.get(1).map(String::as_str).unwrap_or(""),
            args.get(2).map(String::as_str).unwrap_or(""),
        ) {
            ("agent", "list") => Ok(serde_json::json!({
                "id": "cli:agent:list",
                "result": { "agents": self.agents.lock().unwrap().clone() }
            })
            .to_string()),
            ("pane", "split") => {
                let mut s = self.split_seq.lock().unwrap();
                *s += 1;
                Ok(serde_json::json!({
                    "id": "cli:pane:split",
                    "result": { "pane": { "pane_id": format!("wT:p{s}") } }
                })
                .to_string())
            }
            ("agent", "start") | ("agent", "prompt") => {
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            }
            ("pane", "close") => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
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
    let sessions: Vec<Value> = (0..n)
        .map(|i| {
            serde_json::json!({
                "title": format!("Task {i}"),
                "prompt": format!("Do task {i}"),
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

fn spawn(root: &Path, fake: &Arc<FakeHerdr>, clock: Arc<FakeClock>, n: usize) -> (Daemon, Config) {
    let cfg = config(root);
    let daemon = build(root, fake, clock);
    assert!(daemon.run_plan(&plan(root, n)).ok);
    // Prime: apply the async spawn outcomes and give every pane a working
    // revision, the same way the liveness harness does.
    let agents: Vec<Value> = (1..=n).map(|i| agent(&format!("wT:p{i}"), "working", 1)).collect();
    fake.set_agents(agents);
    daemon.poll_once();
    (daemon, cfg)
}

fn decisions(daemon: &Daemon) -> Vec<(String, String)> {
    daemon
        .snapshot()
        .decisions
        .into_iter()
        .map(|d| (d.action, d.reason))
        .collect()
}

#[test]
fn burst_of_failing_sessions_is_held_with_reasons_then_released() {
    let root = tmp_root();
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, _cfg) = spawn(&root, &fake, clock.clone(), 20);
    assert_eq!(daemon.snapshot().sessions.len(), 20);

    // All 20 fail in one poll. The guard must hold the burst, not stampede.
    let agents: Vec<Value> = (1..=20)
        .map(|i| agent(&format!("wT:p{i}"), "failed", 2))
        .collect();
    fake.set_agents(agents);
    fake.clear_calls();
    daemon.poll_once();

    let expected = 20 - BURST_STARTS + 1;
    assert_eq!(
        fake.prompt_count(),
        expected,
        "only the starts before the burst threshold are admitted; the rest are held"
    );
    let d = decisions(&daemon);
    assert!(
        d.iter().any(|(a, r)| a == "start_hold" && r.contains("burst")),
        "a hold must be logged with a reason in the decision log: {d:?}"
    );
    assert_eq!(fake.close_count(), 0, "a start hold must never close a pane");

    // Advance past the brief hold and poll again: the deferred starts go out
    // and the release itself is logged with a reason.
    clock.advance_ms(2_001);
    fake.clear_calls();
    daemon.poll_once();
    let d2 = decisions(&daemon);
    assert!(
        d2.iter().any(|(a, r)| a == "start_hold_released" && r.contains("released")),
        "release must be logged with a reason: {d2:?}"
    );
    assert!(
        fake.prompt_count() >= 20 - expected,
        "held starts are released after the brief window"
    );
    assert_eq!(fake.close_count(), 0, "still zero false kills after release");
}

#[test]
fn storm_is_logged_with_a_reason_and_preempts_without_a_kill() {
    let root = tmp_root();
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(T0));
    let (daemon, cfg) = spawn(&root, &fake, clock.clone(), 1);
    let id = daemon.snapshot().sessions[0].id.clone();

    // Feed the daemon's own guard a 6-fast-start storm on this session.
    {
        let guard = fleet_guard_for(&cfg.state_dir);
        let mut g = guard.lock().unwrap_or_else(|e| e.into_inner());
        for i in 0..STORM_STARTS {
            let _ = g.note_start(&id, clock.mono_ms() + (i as u64) * 1_000);
        }
        assert!(g.stormed_ids().contains(&id));
    }

    fake.set_agents(vec![agent("wT:p1", "failed", 2)]);
    fake.clear_calls();
    daemon.poll_once();

    let d = decisions(&daemon);
    assert!(
        d.iter().any(|(a, r)| a == "storm" && r.contains("storm")),
        "storm detection must carry a reason in the decision log: {d:?}"
    );
    assert!(
        d.iter().any(|(a, r)| a == "preempt" && r.contains("preempt")),
        "preemption must carry a reason in the decision log: {d:?}"
    );
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions[0].status, Status::Idle, "preempted worker is held, not failed");
    assert!(
        snap.escalations.iter().any(|e| e.kind == "fleet_storm"),
        "preemption escalates with an honest reason"
    );
    assert_eq!(fake.prompt_count(), 0, "a preempted worker is not re-prompted");
    assert_eq!(fake.close_count(), 0, "zero false kills");

    // No infinite re-prompts: further polls while herdr still says failed are
    // all absorbed by the guard's bounded preemption state.
    for _ in 0..5 {
        clock.advance_ms(2_000);
        daemon.poll_once();
    }
    assert_eq!(fake.prompt_count(), 0, "no unbounded re-prompt loop");
    assert_eq!(fake.close_count(), 0);
    assert_eq!(
        daemon.snapshot().escalations.iter().filter(|e| e.kind == "fleet_storm").count(),
        1,
        "one preemption per storm episode"
    );
}
