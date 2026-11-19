//! Acceptance tests for notchd. Written before the implementation.
//! Every test here maps to a requirement in the build brief.

use notchd::attribution;
use notchd::bus::BusMessage;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::events::EventLog;
use notchd::herdr::{Herdr, HerdrError};
use notchd::registry::Registry;
use notchd::status::Status;
use notchd::clock::FakeClock;

use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

// ---------- helpers ----------

static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Unix socket paths are limited to SUN_LEN (104 on macOS). Use a guaranteed
/// short /tmp path instead of the (possibly deep) test tmp root.
fn short_sock(tag: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    PathBuf::from(format!("/tmp/nd-{tag}-{}-{n}.sock", std::process::id()))
}

fn tmp_root(tag: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-test-{tag}-{}-{n}", std::process::id()));
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

/// A scripted fake herdr. Records every invocation, returns canned JSON.
struct FakeHerdr {
    calls: Mutex<Vec<Vec<String>>>,
    agents: Mutex<Vec<Value>>,
    read_text: Mutex<String>,
    fail_subcommand: Mutex<Option<String>>,
    fail_panes: Mutex<Vec<String>>,
    split_seq: Mutex<u32>,
}

impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            agents: Mutex::new(Vec::new()),
            read_text: Mutex::new("last line of pane output\n".to_string()),
            fail_subcommand: Mutex::new(None),
            fail_panes: Mutex::new(Vec::new()),
            split_seq: Mutex::new(0),
        })
    }

    fn set_fail_panes(&self, panes: Vec<&str>) {
        *self.fail_panes.lock().unwrap() = panes.into_iter().map(str::to_string).collect();
    }

    fn set_agents(&self, agents: Vec<Value>) {
        *self.agents.lock().unwrap() = agents;
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| fake.run(args))
    }

    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        self.calls.lock().unwrap().push(args.to_vec());
        let sub = args.get(1).cloned().unwrap_or_default(); // e.g. "agent" / "pane"
        let sub2 = args.get(2).cloned().unwrap_or_default(); // e.g. "list" / "split"
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
            ("agent", "list") => {
                let agents = self.agents.lock().unwrap().clone();
                Ok(serde_json::json!({
                    "id": "cli:agent:list",
                    "result": { "type": "agent_list", "agents": agents }
                })
                .to_string())
            }
            ("pane", "list") => Ok(serde_json::json!({
                "id": "cli:pane:list",
                "result": { "type": "pane_list", "panes": self.agents.lock().unwrap().clone() }
            })
            .to_string()),
            ("pane", "split") => {
                let mut seq = self.split_seq.lock().unwrap();
                *seq += 1;
                let pane = format!("w9{x}:p{y}", x = *seq, y = *seq);
                Ok(serde_json::json!({
                    "id": "cli:pane:split",
                    "result": { "type": "pane_info", "pane": { "pane_id": pane, "tab_id": "w1:t1", "workspace_id": "w1" } }
                })
                .to_string())
            }
            ("agent", "start") => Ok(serde_json::json!({
                "id": "cli:agent:start",
                "result": { "type": "agent_started", "agent": { "pane_id": "w9:p1", "agent_status": "working", "revision": 1 }, "argv": ["opencode"] }
            })
            .to_string()),
            ("agent", "prompt") => Ok(serde_json::json!({
                "id": "cli:agent:prompt",
                "result": { "type": "agent_prompted", "agent": { "pane_id": "w9:p1", "agent_status": "working", "revision": 2 } }
            })
            .to_string()),
            ("agent", "read") => Ok(self.read_text.lock().unwrap().clone()),
            ("agent", "wait") => Ok(serde_json::json!({
                "id": "cli:agent:wait",
                "result": { "type": "ok" }
            })
            .to_string()),
            ("pane", "close") => {
                let pane = args.get(3).cloned().unwrap_or_default();
                if self.fail_panes.lock().unwrap().iter().any(|p| p == &pane) {
                    return Err(HerdrError::Failed {
                        args: args.to_vec(),
                        code: Some(1),
                        stderr: format!("pane {pane} already closed"),
                    });
                }
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
        "agent_session": {"agent": "opencode", "kind": "id", "source": "herdr:opencode", "value": pane_id},
        "cwd": "/tmp",
        "focused": false,
        "pane_id": pane_id,
        "revision": revision,
        "terminal_title": format!("OC | {pane_id}"),
        "terminal_id": format!("term_{pane_id}"),
        "tab_id": "w1:t1",
        "workspace_id": "w1",
        "screen_detection_skipped": true
    })
}

/// Scripted fake opencode. `decisions` is consumed in order; the fake
/// asserts deterministically via call accounting.
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
            models: Mutex::new(vec!["opencode/hy4-flash-free".to_string(), "opencode/inkling-2-small-free".to_string()]),
            decisions: Mutex::new(VecDeque::new()),
        })
    }

    fn set_api_idle(&self, session_ref: &str, idle: bool) {
        let entry = serde_json::json!({
            "id": session_ref,
            "model": {"id": "inkling-2-small-free", "providerID": "opencode", "variant": "max"},
            "time": if idle {
                serde_json::json!({"created": 1000, "updated": 2000, "busy": 1000, "idle": 2000})
            } else {
                serde_json::json!({"created": 1000, "updated": 2000, "busy": 3000})
            },
            "location": {"directory": "/tmp"}
        });
        *self.api_session_raw.lock().unwrap() =
            serde_json::json!({"cursor": null, "data": [entry]}).to_string();
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

fn spawn_daemon_with_opencode(root: &Path, fake: &Arc<FakeHerdr>, oc: &Arc<FakeOpencode>, clock: Arc<FakeClock>) -> Daemon {
    let config = test_config(root, 15);
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    let daemon = Daemon::open(config, fake.herdr(), clock.clone()).unwrap();
    daemon.set_opencode(oc.opencode());
    daemon
}

fn write_plan(root: &Path) -> PathBuf {
    for d in ["a", "b", "c", "d"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
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

fn spawn_daemon(root: &Path, fake: &Arc<FakeHerdr>, clock: Arc<FakeClock>) -> Daemon {
    let config = test_config(root, 15);
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    Daemon::open(config, fake.herdr(), clock).unwrap()
}

// ---------- 1. ID allocation ----------

#[test]
fn registry_allocates_unique_ids_never_reused_across_reload() {
    let root = tmp_root("registry");
    let path = root.join("registry.json");
    let clock = Arc::new(FakeClock::new(1_000_000));

    let mut reg = Registry::load(&path, clock.as_ref());
    let sv1 = reg.allocate_supervisor();
    let sv2 = reg.allocate_supervisor();
    assert_eq!(sv1, "SV-1");
    assert_eq!(sv2, "SV-2");
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..3 {
        seen.push(reg.allocate_session(1));
        seen.push(reg.allocate_session(2));
    }
    assert_eq!(seen, vec!["S1-OC01", "S2-OC01", "S1-OC02", "S2-OC02", "S1-OC03", "S2-OC03"]);
    reg.save(&path).unwrap();

    // "Kill" everything and reload: IDs must never be reused.
    let mut reg2 = Registry::load(&path, clock.as_ref());
    let sv3 = reg2.allocate_supervisor();
    assert_eq!(sv3, "SV-3");
    let next1 = reg2.allocate_session(1);
    assert_eq!(next1, "S1-OC04");
    let mut all: Vec<String> = reg2.ids().to_vec();
    all.sort();
    let before = all.len();
    all.dedup();
    assert_eq!(all.len(), before, "duplicate id allocated");
    assert_eq!(
        reg2.ids().len(),
        2 + 6 + 2,
        "every allocation is permanent"
    );
}

#[test]
fn ids_match_the_design_contract_format() {
    let root = tmp_root("idfmt");
    let path = root.join("registry.json");
    let clock = Arc::new(FakeClock::new(1_000_000));
    let mut reg = Registry::load(&path, clock.as_ref());
    let sv = reg.allocate_supervisor();
    let s = reg.allocate_session(1);
    assert!(sv.starts_with("SV-"));
    let parts: Vec<&str> = s.split("-OC").collect();
    assert_eq!(parts.len(), 2);
    assert!(parts[0].starts_with('S'));
    assert_eq!(parts[1].len(), 2);
}

// ---------- 2. Plan parsing + spawn order ----------

#[test]
fn two_by_two_plan_spawns_in_order_with_env_and_titles() {
    let root = tmp_root("plan");
    let fake = FakeHerdr::new();
    fake.set_agents(vec![]);
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let plan = write_plan(&root);

    let ack = daemon.run_plan(&plan);
    assert!(ack.ok, "run_plan failed: {:?}", ack.error);
    assert_eq!(ack.spawned, vec!["SV-1".to_string(), "SV-2".to_string()]);

    let calls = fake.calls();
    let splits: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("pane") && c.get(2).map(String::as_str) == Some("split"))
        .collect();
    let starts: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("start"))
        .collect();
    assert_eq!(splits.len(), 4, "expected 4 pane splits, calls: {calls:?}");
    assert_eq!(starts.len(), 4, "expected 4 agent starts");

    // Env var NOTCH_SESSION_ID set on every split; spawn order is plan order.
    let expected_ids = ["S1-OC01", "S1-OC02", "S2-OC01", "S2-OC02"];
    for (i, split) in splits.iter().enumerate() {
        let joined = split.join(" ");
        assert!(
            joined.contains(&format!("NOTCH_SESSION_ID={}", expected_ids[i])),
            "split {i} missing NOTCH_SESSION_ID={}: {joined}",
            expected_ids[i]
        );
        assert!(joined.contains("--direction"));
        assert!(joined.contains("right"));
        assert!(joined.contains("--cwd"));
    }
    for (i, start) in starts.iter().enumerate() {
        let joined = start.join(" ");
        // herdr agent names must be lowercase (herdr rejects uppercase).
        assert!(joined.contains(&expected_ids[i].to_lowercase()), "start {i} missing session name: {joined}");
        assert!(joined.contains("--kind"));
        assert!(joined.contains("opencode"));
    }

    // Order: all spawns for SV-1 happen before SV-2 in plan order.
    let prompts: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("prompt"))
        .collect();
    assert_eq!(prompts.len(), 4, "each session gets its prompt after start");

    // Snapshot reflects 4 sessions with the plan's titles.
    let snap = daemon.snapshot();
    let ids: Vec<&str> = snap.sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["S1-OC01", "S1-OC02", "S2-OC01", "S2-OC02"]);
    assert_eq!(snap.sessions[0].title, "Refactor auth middleware");
    assert_eq!(snap.sessions[1].title, "Fix login race");
    assert_eq!(snap.supervisors.len(), 2);
}

#[test]
fn plan_parse_failure_is_a_clean_error_not_a_panic() {
    let root = tmp_root("badplan");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let bad = root.join("bad.json");
    std::fs::write(&bad, "{ not json").unwrap();
    let ack = daemon.run_plan(&bad);
    assert!(!ack.ok);
    assert!(ack.error.is_some());
    // A plan with zero supervisors is empty but legal.
    let empty = root.join("empty.json");
    std::fs::write(&empty, r#"{"stuck_after_secs": 5, "supervisors": []}"#).unwrap();
    let ack = daemon.run_plan(&empty);
    assert!(ack.ok);
    assert_eq!(ack.spawned.len(), 0);
}

// ---------- 3. Watchdog ----------

#[test]
fn watchdog_kills_only_stuck_working_session() {
    let root = tmp_root("watchdog");
    let fake = FakeHerdr::new();
    // pane assignment: S1-OC01 -> w91:p1, S1-OC02 -> w92:p2
    fake.set_agents(vec![
        agent_entry("w91:p1", "working", 7),
        agent_entry("w92:p2", "done", 3),
    ]);
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());

    // Assign pane ids as if spawn had happened (do a real spawn first for realism).
    let plan = write_plan(&root);
    // Rewrite fake agents to match the panes that spawn will allocate: the fake
    // allocator returns w{seq}:p{seq}. First two splits: w1:p1, w2:p2... instead
    // of guessing, query snapshot after spawn and map panes.
    let ack = daemon.run_plan(&plan);
    assert!(ack.ok);
    let snap = daemon.snapshot();
    let pane_of = |id: &str| snap.sessions.iter().find(|s| s.id == id).unwrap().pane_id.clone();
    let pane_a = pane_of("S1-OC01");
    let pane_b = pane_of("S1-OC02");
    // Reconfigure agents AFTER spawn, keyed to the real pane ids.
    fake.set_agents(vec![
        agent_entry(&pane_a, "working", 7),
        agent_entry(&pane_b, "done", 3),
    ]);

    // Pre-create the per-session temp dir for S1-OC01 to prove the hook runs.
    let tmp_a = root.join("tmp").join("notch-S1-OC01");
    std::fs::create_dir_all(&tmp_a).unwrap();

    // First poll: records revisions. Advance past stuck_after_secs (15s).
    daemon.poll_once();
    clock.advance_ms(16_000);
    daemon.poll_once();

    let calls = fake.calls();
    let closes: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("pane") && c.get(2).map(String::as_str) == Some("close"))
        .collect();
    assert_eq!(closes.len(), 1, "exactly one pane close expected: {calls:?}");
    assert!(closes[0].join(" ").contains(&pane_a));

    let snap = daemon.snapshot();
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    assert_eq!(s1.status, Status::Failed);
    assert_eq!(s1.failure_reason.as_deref(), Some("stuck"));
    let s2 = snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap();
    assert_eq!(s2.status, Status::Done);
    assert!(
        !root.join("tmp").join("notch-S1-OC01").exists(),
        "cleanup hook must remove the per-session temp dir"
    );
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("watchdog"), "watchdog event missing: {log}");
    assert!(
        snap.warnings.iter().any(|w| w.contains("S1-OC01")),
        "snapshot warning missing: {:?}",
        snap.warnings
    );
}

#[test]
fn non_working_session_is_never_killed_and_revision_advance_resets_the_timer() {
    let root = tmp_root("watchdog2");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());
    let plan = write_plan(&root);
    daemon.run_plan(&plan);
    let snap = daemon.snapshot();
    let pane_a = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap().pane_id.clone();
    let pane_b = snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap().pane_id.clone();

    // Both working; revisions static. One poll, then a poll where rev advances.
    fake.set_agents(vec![
        agent_entry(&pane_a, "working", 1),
        agent_entry(&pane_b, "working", 1),
    ]);
    daemon.poll_once();
    clock.advance_ms(10_000); // 10s < 15s — no kill yet even with static rev
    fake.set_agents(vec![
        agent_entry(&pane_a, "working", 2), // advanced -> timer resets
        agent_entry(&pane_b, "working", 1), // static
    ]);
    daemon.poll_once();
    clock.advance_ms(6_000); // now: A 6s since reset, B 16s since first sight
    daemon.poll_once();

    let calls = fake.calls();
    let closes: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("pane") && c.get(2).map(String::as_str) == Some("close"))
        .collect();
    assert_eq!(closes.len(), 1);
    assert!(closes[0].join(" ").contains(&pane_b), "B is the stuck one: {calls:?}");
}

// ---------- 4. Re-prompt on failure ----------

#[test]
fn failure_reprompts_once_then_marks_failed_with_escalation() {
    let root = tmp_root("reprompt");
    let fake = FakeHerdr::new();
    *fake.read_text.lock().unwrap() = "error: cannot compile src/auth.rs:12\n".to_string();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());
    let plan = write_plan(&root);
    daemon.run_plan(&plan);
    let snap = daemon.snapshot();
    let pane_a = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap().pane_id.clone();
    // Every session starts working; S1-OC01 goes blocked (-> failed blocked_waiting).
    fake.set_agents(vec![
        agent_entry(&pane_a, "blocked", 9),
        agent_entry(
            &snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap().pane_id.clone(),
            "done",
            9,
        ),
        agent_entry(
            &snap.sessions.iter().find(|s| s.id == "S2-OC01").unwrap().pane_id.clone(),
            "done",
            9,
        ),
        agent_entry(
            &snap.sessions.iter().find(|s| s.id == "S2-OC02").unwrap().pane_id.clone(),
            "done",
            9,
        ),
    ]);

    // Poll 1: blocked -> failed(blocked_waiting); policy re-prompts once with tail.
    daemon.poll_once();
    let calls = fake.calls();
    let prompts: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("prompt"))
        .collect();
    let reprompt = prompts
        .iter()
        .find(|p| p.join(" ").contains("cannot compile src/auth.rs:12"))
        .expect("expected a re-prompt for the blocked session");
    assert!(
        reprompt.join(" ").contains("cannot compile src/auth.rs:12"),
        "re-prompt must append the tail of pane output: {reprompt:?}"
    );
    assert!(
        reprompt.join(" ").contains("s1-oc01"),
        "re-prompt targets the lowercase herdr agent name: {reprompt:?}"
    );

    // Poll 2: still blocked -> no second re-prompt; mark failed + escalate.
    daemon.poll_once();
    let calls = fake.calls();
    let reprompt_count = calls
        .iter()
        .filter(|c| {
            c.get(1).map(String::as_str) == Some("agent")
                && c.get(2).map(String::as_str) == Some("prompt")
                && c.join(" ").contains("cannot compile")
        })
        .count();
    assert_eq!(reprompt_count, 1, "exactly one re-prompt allowed");

    let snap = daemon.snapshot();
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    assert_eq!(s1.status, Status::Failed);
    assert_eq!(s1.failure_reason.as_deref(), Some("blocked_waiting"));
    assert!(
        snap.warnings.iter().any(|w| w.contains("S1-OC01")),
        "escalation must surface a snapshot warning: {:?}",
        snap.warnings
    );
    let esc = &snap.escalations;
    assert_eq!(esc.len(), 1, "blocked failure must record an escalation: {esc:?}");
    assert_eq!(esc[0].kind, "blocked");
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("escalate"), "escalate event missing: {log}");
}

#[test]
fn session_that_recovers_after_reprompt_is_not_escalated() {
    let root = tmp_root("reprompt-ok");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());
    let plan = write_plan(&root);
    daemon.run_plan(&plan);
    let snap = daemon.snapshot();
    let pane_a = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap().pane_id.clone();
    let others: Vec<(String, String)> = snap
        .sessions
        .iter()
        .filter(|s| s.id != "S1-OC01")
        .map(|s| (s.id.clone(), s.pane_id.clone()))
        .collect();

    let mut agents_blocked = vec![agent_entry(&pane_a, "blocked", 5)];
    for (_, pane) in &others {
        agents_blocked.push(agent_entry(pane, "done", 5));
    }
    fake.set_agents(agents_blocked);
    daemon.poll_once(); // -> re-prompt

    // Agent recovers: done on next poll.
    let mut agents_done = vec![agent_entry(&pane_a, "done", 6)];
    for (_, pane) in &others {
        agents_done.push(agent_entry(pane, "done", 5));
    }
    fake.set_agents(agents_done);
    daemon.poll_once();

    let snap = daemon.snapshot();
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    assert_eq!(s1.status, Status::Done);
    assert!(
        !snap.warnings.iter().any(|w| w.contains("S1-OC01")),
        "recovered session must not escalate: {:?}",
        snap.warnings
    );
}

// ---------- 5. Bus ----------

#[test]
fn cross_supervisor_messages_route_through_orchestrator_in_order() {
    let root = tmp_root("bus");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());
    let plan = write_plan(&root);
    daemon.run_plan(&plan);

    daemon
        .bus_send("SV-1", "SV-2", Some("S1-OC01".to_string()), "status", "first")
        .unwrap();
    daemon
        .bus_send("SV-1", "SV-2", Some("S1-OC01".to_string()), "status", "second")
        .unwrap();
    daemon
        .bus_send("SV-2", "SV-1", None, "status", "reply")
        .unwrap();

    let routed = daemon.bus_drain();
    assert_eq!(routed, 3);

    let sv2: Vec<BusMessage> = daemon.mailbox_drain("SV-2");
    let sv1: Vec<BusMessage> = daemon.mailbox_drain("SV-1");
    assert_eq!(sv2.len(), 2);
    assert_eq!(sv2[0].detail, "first");
    assert_eq!(sv2[1].detail, "second");
    assert!(sv2[0].seq < sv2[1].seq, "ordering must be preserved");
    assert_eq!(sv2[0].from, "SV-1");
    assert_eq!(sv2[0].to, "SV-2");
    assert_eq!(sv1.len(), 1);
    assert_eq!(sv1[0].detail, "reply");

    // Stored in the event log.
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("bus SV-1->SV-2"), "event log missing bus entries: {log}");

    // Unknown supervisor is rejected.
    assert!(daemon.bus_send("SV-1", "SV-99", None, "status", "nope").is_err());
    assert!(daemon.bus_send("SV-99", "SV-1", None, "status", "nope").is_err());
}

// ---------- 6. Attribution + conflict ----------

#[test]
fn git_attribution_detects_changes_and_conflicts() {
    let root = tmp_root("attr");
    // Real git repo, real untracked file.
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(init.status.success());
    std::fs::write(repo.join("shared.rs"), "fn main() {}\n").unwrap();

    let files = attribution::sample_git(&repo);
    assert!(
        files.iter().any(|f| f.ends_with("shared.rs")),
        "expected shared.rs in {files:?}"
    );

    // Non-repo cwd is a non-signal, never an error.
    let files = attribution::sample_git(&root.join("not-a-repo-or-missing"));
    assert!(files.is_empty());

    // Two sessions touching the same file -> conflict, with an event.
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let plan = {
        let p = root.join("plan.json");
        std::fs::write(
            &p,
            serde_json::json!({
                "stuck_after_secs": 900,
                "supervisors": [
                    { "sessions": [
                        { "title": "A", "prompt": "task a", "cwd": repo },
                        { "title": "B", "prompt": "task b", "cwd": repo }
                    ]}
                ]
            })
            .to_string(),
        )
        .unwrap();
        p
    };
    daemon.run_plan(&plan);
    daemon.record_attribution("S1-OC01", &["src/shared.rs".to_string()]);
    assert!(daemon.snapshot().conflicts.is_empty());
    daemon.record_attribution("S1-OC02", &["src/shared.rs".to_string()]);

    let snap = daemon.snapshot();
    assert_eq!(snap.conflicts.len(), 1);
    assert_eq!(snap.conflicts[0].file, "src/shared.rs");
    let mut sessions = snap.conflicts[0].sessions.clone();
    sessions.sort();
    assert_eq!(sessions, vec!["S1-OC01".to_string(), "S1-OC02".to_string()]);
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("conflict"), "conflict event missing: {log}");
}

// ---------- 7. Cleanup ----------

#[test]
fn cleanup_sweep_removes_dead_session_dirs_and_rotates_events_log() {
    let root = tmp_root("cleanup");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);

    // Pre-create temp dirs and dead sessions via run_plan + kill.
    let plan = write_plan(&root);
    daemon.run_plan(&plan);
    let snap = daemon.snapshot();
    for s in &snap.sessions {
        std::fs::create_dir_all(root.join("tmp").join(format!("notch-{}", s.id))).unwrap();
    }
    // One done via fake herdr, the rest killed via the kill command.
    let mut agents = Vec::new();
    for s in &snap.sessions {
        agents.push(agent_entry(&s.pane_id, "done", 1));
    }
    fake.set_agents(agents);
    daemon.poll_once(); // -> all done -> cleanup already removes their dirs
    let remaining_without_sweep = std::fs::read_dir(root.join("tmp")).unwrap().count();
    assert_eq!(remaining_without_sweep, 0, "done sessions must be cleaned automatically");

    // Re-create a dead session dir and verify the explicit global sweep.
    let dead_dir = root.join("tmp").join("notch-S1-OC01");
    std::fs::create_dir_all(&dead_dir).unwrap();
    // Inflate events.jsonl past the 4 MB threshold.
    let events = root.join("state").join("events.jsonl");
    let line = serde_json::json!({"ts": 1, "session": null, "kind": "status", "detail": "x".repeat(200)});
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&events).unwrap();
    for _ in 0..22_000 {
        writeln!(f, "{}", line).unwrap();
    }
    drop(f);

    let ack = daemon.cleanup(None);
    assert!(ack.ok);
    assert_eq!(ack.removed, Some(1));
    assert!(!dead_dir.exists());
    assert!(
        root.join("state").join("events.jsonl.1").exists(),
        "stale events.jsonl must be rotated at 4 MB"
    );

    // Per-id cleanup of a live session is refused (or reports false removals=0).
    let ack = daemon.cleanup(Some("S2-OC02"));
    assert!(ack.ok);
    assert_eq!(ack.removed, Some(0));
}

// ---------- 8. Snapshot contract ----------

#[test]
fn snapshot_matches_bridge_v1_key_set_and_camelcase() {
    let root = tmp_root("snapshot");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let plan = write_plan(&root);
    daemon.run_plan(&plan);

    let snap = daemon.snapshot();
    let v = serde_json::to_value(&snap).unwrap();
    let obj = v.as_object().unwrap();
    let top_keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let expected_top: std::collections::BTreeSet<&str> =
        ["version", "generatedAt", "orchestrator", "supervisors", "sessions", "eps", "conflicts", "attribution", "warnings"]
            .into_iter()
            .collect();
    assert_eq!(top_keys, expected_top, "top-level key set drifted");
    assert_eq!(obj["version"], 1);

    let sup = &obj["supervisors"][0];
    let sup_keys: std::collections::BTreeSet<&str> = sup.as_object().unwrap().keys().map(String::as_str).collect();
    let expected_sup: std::collections::BTreeSet<&str> =
        ["id", "status", "working", "idle", "failed", "done", "title"].into_iter().collect();
    assert_eq!(sup_keys, expected_sup);

    let ses = &obj["sessions"][0];
    let ses_keys: std::collections::BTreeSet<&str> = ses.as_object().unwrap().keys().map(String::as_str).collect();
    let expected_ses: std::collections::BTreeSet<&str> = [
        "id", "sup", "status", "title", "task", "cwd", "paneId", "startedAt", "lastAction",
        "lastRevision", "latencyMs", "failureReason",
        // v1.3 additive fields, always present on sessions spawned by this engine
        "goal", "iterations", "maxIterations", "wallClockDeadline",
    ]
    .into_iter()
    .collect();
    assert_eq!(ses_keys, expected_ses);

    // Status vocabulary is exactly the four allowed values.
    assert!(matches!(ses["status"].as_str().unwrap(), "working" | "idle" | "failed" | "done"));

    // supervisors ordered by id, sessions ordered by id
    let sup_ids: Vec<&str> = obj["supervisors"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(sup_ids, vec!["SV-1", "SV-2"]);
    let ses_ids: Vec<&str> = obj["sessions"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ses_ids, vec!["S1-OC01", "S1-OC02", "S2-OC01", "S2-OC02"]);

    // eps is a float
    assert!(obj["eps"].is_f64());
}

#[test]
fn empty_tree_is_legal_and_serializes_cleanly() {
    let root = tmp_root("emptytree");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let snap = daemon.snapshot();
    let v = serde_json::to_value(&snap).unwrap();
    assert_eq!(v["supervisors"].as_array().unwrap().len(), 0);
    assert_eq!(v["sessions"].as_array().unwrap().len(), 0);
    assert_eq!(v["eps"], 0.0);
    assert_eq!(v["warnings"].as_array().unwrap().len(), 0);
}

// ---------- 9. Socket / NDJSON roundtrip ----------

#[test]
fn ndjson_socket_hello_ping_subscribe_kill_unknown_cleanup() {
    use std::os::unix::net::UnixListener;
    let root = tmp_root("socket");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = Arc::new(spawn_daemon(&root, &fake, clock.clone()));

    let sock = short_sock("sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let server = {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || notchd::socket::serve_with_shutdown(listener, daemon, shutdown))
    };

    let stream = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let hello: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(hello["event"], "hello");
    assert_eq!(hello["version"], 1);

    let send = |writer: &mut std::os::unix::net::UnixStream, obj: Value| {
        writeln!(writer, "{}", obj).unwrap();
        writer.flush().unwrap();
    };
    let read = |reader: &mut BufReader<std::os::unix::net::UnixStream>| -> Value {
        let mut l = String::new();
        reader.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap()
    };

    send(&mut writer, serde_json::json!({"cmd": "ping"}));
    let pong = read(&mut reader);
    assert_eq!(pong["event"], "ack");
    assert_eq!(pong["cmd"], "ping");
    assert_eq!(pong["ok"], true);
    assert!(pong["pong"].is_u64());

    send(&mut writer, serde_json::json!({"cmd": "subscribe"}));
    let ack = read(&mut reader);
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "subscribe");
    assert_eq!(ack["ok"], true);
    let snap = read(&mut reader);
    assert_eq!(snap["event"], "snapshot");
    assert_eq!(snap["snapshot"]["version"], 1);

    // kill unknown id -> ok:false, machine-readable error
    send(&mut writer, serde_json::json!({"cmd": "kill", "id": "S9-OC99"}));
    let ack = read(&mut reader);
    assert_eq!(ack["cmd"], "kill");
    assert_eq!(ack["ok"], false);
    assert!(ack["error"].is_string());

    // cleanup roundtrip
    send(&mut writer, serde_json::json!({"cmd": "cleanup"}));
    let ack = read(&mut reader);
    assert_eq!(ack["cmd"], "cleanup");
    assert_eq!(ack["ok"], true);
    assert!(ack["removed"].is_u64());

    // status roundtrip
    send(&mut writer, serde_json::json!({"cmd": "status"}));
    let status = read(&mut reader);
    assert_eq!(status["event"], "status");
    assert_eq!(status["pid"], std::process::id());

    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(writer);
    let _ = server.join();
}

// ---------- 10. Live herdr fixture ----------

#[test]
fn herdr_agent_list_fixture_parses() {
    let fixture = include_str!("fixtures/herdr-agent-list.json");
    let agents = notchd::herdr::parse_agent_list_output(fixture).expect("fixture must parse");
    assert!(!agents.is_empty(), "fixture must contain at least one agent");
    for a in &agents {
        assert!(!a.pane_id.is_empty(), "every entry must carry a pane_id");
    }
}

// ---------- 11. Status mapping ----------

#[test]
fn herdr_status_mapping_matches_bridge_md() {
    assert_eq!(Status::from_herdr("working").0, Status::Working);
    assert_eq!(Status::from_herdr("idle").0, Status::Idle);
    assert_eq!(Status::from_herdr("done").0, Status::Done);
    let (s, reason) = Status::from_herdr("blocked");
    assert_eq!(s, Status::Failed);
    assert_eq!(reason.as_deref(), Some("blocked_waiting"));
    assert_eq!(Status::from_herdr("unknown").0, Status::Idle);
    assert_eq!(Status::from_herdr("something-else").0, Status::Idle);
}

// ---------- 12. EPS rolling window ----------

#[test]
fn eps_is_events_per_second_over_rolling_10s_window() {
    let root = tmp_root("eps");
    let clock = Arc::new(FakeClock::new(1_000_000));
    let log = EventLog::new(root.join("events.jsonl"), 1_000_000, clock.clone());
    for _ in 0..20 {
        log.append(None, "status", "x").unwrap();
    }
    assert_eq!(log.eps(), 2.0);
    clock.advance_ms(11_000);
    assert_eq!(log.eps(), 0.0);
    clock.advance_ms(0);
    for _ in 0..5 {
        log.append(None, "status", "y").unwrap();
    }
    assert_eq!(log.eps(), 0.5);
}

// ---------- 13. Kill command ----------

#[test]
fn kill_marks_failed_runs_cleanup_and_events() {
    let root = tmp_root("kill");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let plan = write_plan(&root);
    daemon.run_plan(&plan);
    std::fs::create_dir_all(root.join("tmp").join("notch-S1-OC01")).unwrap();

    let ack = daemon.kill("S1-OC01", Some("operator stop".to_string())).unwrap();
    assert!(ack.ok);
    let snap = daemon.snapshot();
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    assert_eq!(s1.status, Status::Failed);
    assert_eq!(s1.failure_reason.as_deref(), Some("operator stop"));
    assert!(!root.join("tmp").join("notch-S1-OC01").exists());
    let calls = fake.calls();
    let closes: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("pane") && c.get(2).map(String::as_str) == Some("close"))
        .collect();
    assert_eq!(closes.len(), 1);

    // Unknown id is a clean error.
    assert!(daemon.kill("S9-OC99", None).is_err());
}

// ---------- 14. Re-adoption on start ----------

#[test]
fn start_marks_vanished_panes_done_and_keeps_live_sessions() {
    let root = tmp_root("readopt");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let state_dir = root.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    // Pre-write a state.json with two live-looking sessions.
    let panes = ["w1:p1", "w1:p2"];
    let mut sessions = Vec::new();
    for (id, sup, pane) in [("S1-OC01", "SV-1", panes[0]), ("S1-OC02", "SV-1", panes[1])] {
        sessions.push(serde_json::json!({
            "id": id, "sup": sup, "status": "working",
            "title": "t", "task": "t", "cwd": root.join("a"),
            "paneId": pane, "startedAt": 1_763_520_000_000u64,
            "lastAction": null, "lastRevision": 3, "latencyMs": null,
            "failureReason": null
        }));
    }
    std::fs::write(
        state_dir.join("state.json"),
        serde_json::json!({
            "version": 1,
            "supervisors": [ { "id": "SV-1", "number": 1 } ],
            "sessions": sessions
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        state_dir.join("registry.json"),
        serde_json::json!({
            "version": 1,
            "supervisorCount": 1,
            "sessionCounts": { "1": 2 },
            "ids": ["SV-1", "S1-OC01", "S1-OC02"]
        })
        .to_string(),
    )
    .unwrap();
    // Only p1 still exists.
    fake.set_agents(vec![agent_entry("w1:p1", "working", 9)]);

    let daemon = spawn_daemon(&root, &fake, clock);
    daemon.reconcile();

    let snap = daemon.snapshot();
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    let s2 = snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap();
    assert_eq!(s1.status, Status::Working);
    assert_eq!(s2.status, Status::Done);
    let log = std::fs::read_to_string(state_dir.join("events.jsonl")).unwrap();
    assert!(log.contains("readopt") || log.contains("done"), "expected a re-adoption/done event: {log}");
}

#[test]
fn start_survives_herdr_failure_with_warning_and_no_crash() {
    let root = tmp_root("readopt-fail");
    let fake = FakeHerdr::new();
    *fake.fail_subcommand.lock().unwrap() = Some("agent list".to_string());
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    daemon.reconcile(); // must not panic
    let snap = daemon.snapshot();
    assert!(
        snap.warnings.iter().any(|w| w.contains("herdr")),
        "expected a herdr warning: {:?}",
        snap.warnings
    );
}

// ---------- 15. Attribution cap ----------

#[test]
fn attribution_is_newest_first_and_capped_at_24() {
    let root = tmp_root("attrcap");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let files: Vec<String> = (0..30).map(|i| format!("src/file_{i}.rs")).collect();
    for f in &files {
        daemon.record_attribution("S1-OC01", std::slice::from_ref(f));
    }
    let snap = daemon.snapshot();
    assert_eq!(snap.attribution.len(), 24);
    assert_eq!(snap.attribution[0].file, "src/file_29.rs", "newest first");
}

// Keep a reference so unused warnings don't bite when the API settles.
#[allow(dead_code)]
fn _take(v: VecDeque<u64>) -> VecDeque<u64> {
    v
}

// ---------- 16. Debounced subscriber push over the socket ----------

#[test]
fn subscribe_pushes_snapshot_on_change_after_100ms_debounce() {
    use std::os::unix::net::UnixListener;
    let root = tmp_root("push");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = Arc::new(spawn_daemon(&root, &fake, clock.clone()));
    let plan = write_plan(&root);
    daemon.run_plan(&plan);

    let sock = short_sock("sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let server = {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || notchd::socket::serve_with_shutdown(listener, daemon, shutdown))
    };

    let stream = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap(); // hello
    writeln!(writer, "{}", serde_json::json!({"cmd": "subscribe"})).unwrap();
    writer.flush().unwrap();
    reader.read_line(&mut line).unwrap(); // ack — hmm, reuse `line`; read fresh
    let mut l2 = String::new();
    reader.read_line(&mut l2).unwrap(); // immediate snapshot
    let snap_line: Value = serde_json::from_str(&l2).unwrap();
    assert_eq!(snap_line["event"], "snapshot");

    // Trigger a change: kill S1-OC01.
    daemon.kill("S1-OC01", None).unwrap();
    // Not yet due — debounce window is 100 ms; fake clock has not advanced.
    // (The 25 ms flusher in the server also uses the never-advancing fake
    // clock, so no push can leak early.)
    clock.advance_ms(150);
    daemon.flush_subscriber_snapshots();

    let mut pushed = String::new();
    reader.read_line(&mut pushed).unwrap();
    let pushed_json: Value = serde_json::from_str(&pushed).unwrap();
    assert_eq!(pushed_json["event"], "snapshot");
    let sessions = pushed_json["snapshot"]["sessions"].as_array().unwrap();
    let s1 = sessions.iter().find(|s| s["id"] == "S1-OC01").unwrap();
    assert_eq!(s1["status"], "failed");

    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(writer);
    let _ = server.join();
}

// ---------- 17. Corrupt state files never panic ----------

#[test]
fn corrupt_state_and_registry_files_load_as_defaults() {
    let root = tmp_root("corrupt");
    let state_dir = root.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("state.json"), "{{{{ not json !!!").unwrap();
    std::fs::write(state_dir.join("registry.json"), "also not json").unwrap();
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let snap = daemon.snapshot();
    assert_eq!(snap.sessions.len(), 0);
    assert_eq!(snap.supervisors.len(), 0);
}

// ---------- 18. pane-split output parser tolerance ----------

#[test]
fn pane_split_parser_accepts_envelope_bare_and_rejects_garbage() {
    // Envelope with result.pane.pane_id (the schema-plausible shape)
    let h = Herdr::with_runner(|_args: &[String]| {
        Ok(serde_json::json!({
            "id": "cli:pane:split",
            "result": { "type": "pane_info", "pane": { "pane_id": "w7:p9" } }
        })
        .to_string())
    });
    assert_eq!(h.pane_split("/tmp", &[]).unwrap(), "w7:p9");

    // Bare pane id on stdout
    let h = Herdr::with_runner(|_args: &[String]| Ok("w7:p10\n".to_string()));
    assert_eq!(h.pane_split("/tmp", &[]).unwrap(), "w7:p10");

    // Garbage -> clean error, never a panic
    let h = Herdr::with_runner(|_args: &[String]| Ok("total garbage output".to_string()));
    assert!(h.pane_split("/tmp", &[]).is_err());
}

// ---------- 19. Attribution baseline: pre-dirty files are not attributable ----------

#[test]
fn first_git_sample_sets_a_baseline_and_only_new_files_attribute() {
    let root = tmp_root("baseline");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(init.status.success());
    // Dirty at spawn time: another agent's in-flight work. Must NOT attribute.
    std::fs::write(repo.join("other-agents-work.rs"), "fn dirty() {}\n").unwrap();

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let plan_path = root.join("plan.json");
    std::fs::write(
        &plan_path,
        serde_json::json!({
            "stuck_after_secs": 900,
            "supervisors": [
                { "sessions": [ { "title": "A", "prompt": "task a", "cwd": repo } ] }
            ]
        })
        .to_string(),
    )
    .unwrap();
    daemon.run_plan(&plan_path);
    let snap = daemon.snapshot();
    let pane = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap().pane_id.clone();
    fake.set_agents(vec![agent_entry(&pane, "working", 1)]);

    // Poll 1: sets the baseline; the pre-dirty file must NOT be attributed.
    daemon.poll_once();
    let snap = daemon.snapshot();
    assert!(
        !snap.attribution.iter().any(|a| a.file.contains("other-agents-work.rs")),
        "baseline files must not be attributed to the session: {:?}",
        snap.attribution
    );

    // Session's own new work appears: must be attributed on the next poll.
    std::fs::write(repo.join("session-own-work.rs"), "fn mine() {}\n").unwrap();
    fake.set_agents(vec![agent_entry(&pane, "working", 2)]);
    daemon.poll_once();
    let snap = daemon.snapshot();
    assert!(
        snap.attribution.iter().any(|a| a.file.contains("session-own-work.rs")),
        "new files after the baseline must be attributed: {:?}",
        snap.attribution
    );
}

// ---------- 20. submit_task (bridge.md v1.2) ----------

fn submit_plan_repo(root: &Path) -> PathBuf {
    let repo = root.join("submit-repo");
    std::fs::create_dir_all(&repo).unwrap();
    repo
}

#[test]
fn submit_task_allocates_titles_and_sends_full_task_to_agent() {
    let root = tmp_root("submit");
    let repo = submit_plan_repo(&root);
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);

    let task = "Refactor the auth middleware to remove the shared session cache from the login path and the token refresh path";
    let ack = daemon.submit_task(task.to_string(), Some(repo.to_str().unwrap().to_string()), None, None, None, None);
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);
    assert_eq!(ack.supervisor_id.as_deref(), Some("SV-1"));
    assert_eq!(ack.session_id.as_deref(), Some("S1-OC01"));

    // The FULL task text reaches the agent untruncated.
    let calls = fake.calls();
    let prompts: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("prompt"))
        .collect();
    assert_eq!(prompts.len(), 1, "exactly one prompt expected: {calls:?}");
    assert_eq!(prompts[0][4], task, "full task text must reach the agent untruncated");

    // Titles: 48-char word-boundary truncation, single line.
    let snap = daemon.snapshot();
    assert_eq!(snap.supervisors.len(), 1);
    assert_eq!(snap.supervisors[0].id, "SV-1");
    assert_eq!(
        snap.supervisors[0].title, "Refactor the auth middleware to remove the",
        "supervisor title must be the task head on a word boundary"
    );
    assert_eq!(snap.sessions.len(), 1);
    assert_eq!(snap.sessions[0].title, "Refactor the auth middleware to remove the");
    assert_eq!(snap.sessions[0].sup, "SV-1");
    assert!(snap.sessions[0].task.starts_with("Refactor the auth middleware"));
    assert_eq!(snap.sessions[0].status, Status::Working);

    // Per-session temp dir created like run-plan.
    assert!(root.join("tmp").join("notch-S1-OC01").exists());
}

#[test]
fn submit_task_title_override_and_single_line_normalization() {
    let root = tmp_root("submit-title");
    let repo = submit_plan_repo(&root);
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);

    // Multi-line title with extra whitespace is normalized to one line.
    let ack = daemon.submit_task("do the thing\nwith two lines".to_string(), Some(repo.to_str().unwrap().to_string()), Some("My\n  Custom   Title".to_string()), None, None, None);
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);
    let snap = daemon.snapshot();
    assert_eq!(snap.supervisors[0].title, "My Custom Title");
    // Task head is also normalized to one line for the title fallback path.
    assert_eq!(snap.sessions[0].title, "do the thing with two lines");
}

#[test]
fn submit_task_validation_allocates_nothing() {
    let root = tmp_root("submit-validate");
    let repo = submit_plan_repo(&root);
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);

    // Nonexistent cwd
    let ack = daemon.submit_task("task one".to_string(), Some(root.join("nope").to_str().unwrap().to_string()), None, None, None, None);
    assert!(!ack.ok);
    assert!(ack.error.is_some());
    // No ids on a validation failure
    assert_eq!(ack.supervisor_id, None);
    assert_eq!(ack.session_id, None);

    // File, not a directory
    let file = root.join("a-file.txt");
    std::fs::write(&file, "x").unwrap();
    let ack = daemon.submit_task("task two".to_string(), Some(file.to_str().unwrap().to_string()), None, None, None, None);
    assert!(!ack.ok);
    assert!(ack.error.is_some());

    // Empty and whitespace-only task
    let ack = daemon.submit_task("".to_string(), Some(repo.to_str().unwrap().to_string()), None, None, None, None);
    assert!(!ack.ok);
    let ack = daemon.submit_task("   \n\t  ".to_string(), Some(repo.to_str().unwrap().to_string()), None, None, None, None);
    assert!(!ack.ok);

    // Nothing was allocated: the next valid submit gets SV-1 / S1-OC01.
    let ack = daemon.submit_task("real task".to_string(), Some(repo.to_str().unwrap().to_string()), None, None, None, None);
    assert!(ack.ok);
    assert_eq!(ack.supervisor_id.as_deref(), Some("SV-1"));
    assert_eq!(ack.session_id.as_deref(), Some("S1-OC01"));
}

#[test]
fn submit_task_spawn_failure_marks_session_failed_and_returns_error() {
    let root = tmp_root("submit-fail");
    let repo = submit_plan_repo(&root);
    let fake = FakeHerdr::new();
    *fake.fail_subcommand.lock().unwrap() = Some("pane split".to_string());
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);

    let ack = daemon.submit_task("task that cannot spawn".to_string(), Some(repo.to_str().unwrap().to_string()), None, None, None, None);
    assert!(!ack.ok);
    assert!(ack.error.is_some());
    // The allocated ids are reported and the session is marked failed.
    assert_eq!(ack.supervisor_id.as_deref(), Some("SV-1"));
    assert_eq!(ack.session_id.as_deref(), Some("S1-OC01"));
    let snap = daemon.snapshot();
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    assert_eq!(s1.status, Status::Failed);
    assert_eq!(s1.failure_reason.as_deref(), Some("spawn_failed"));
    assert!(
        snap.warnings.iter().any(|w| w.contains("S1-OC01")),
        "spawn failure must surface a snapshot warning"
    );
}

#[test]
fn submit_task_second_submit_allocates_incrementing_ids() {
    let root = tmp_root("submit-again");
    let repo = submit_plan_repo(&root);
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);

    let ack1 = daemon.submit_task("first task".to_string(), Some(repo.to_str().unwrap().to_string()), None, None, None, None);
    let ack2 = daemon.submit_task("second task".to_string(), Some(repo.to_str().unwrap().to_string()), None, None, None, None);
    assert!(ack1.ok && ack2.ok);
    assert_eq!(ack1.supervisor_id.as_deref(), Some("SV-1"));
    assert_eq!(ack2.supervisor_id.as_deref(), Some("SV-2"));
    assert_eq!(ack1.session_id.as_deref(), Some("S1-OC01"));
    assert_eq!(ack2.session_id.as_deref(), Some("S2-OC01"));
    let snap = daemon.snapshot();
    let sup_ids: Vec<&str> = snap.supervisors.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(sup_ids, vec!["SV-1", "SV-2"]);
    assert_eq!(snap.supervisors[0].title, "first task");
    assert_eq!(snap.supervisors[1].title, "second task");
}

#[test]
fn task_head_truncation_is_word_boundary_single_line() {
    use notchd::model::{single_line, truncate_words};
    assert_eq!(truncate_words("short task", 48), "short task");
    assert_eq!(truncate_words("a\n\nmulti   line\ttask", 48), "a multi line task");
    // Exactly 48 chars fits; the same line with one more word is cut at the boundary.
    let t = truncate_words("one two three four five six seven eight nine ten eleven twelve thirteen", 48);
    assert_eq!(t, "one two three four five six seven eight nine ten");
    assert_eq!(t.chars().count(), 48);
    // A short line that fits is returned whole.
    assert_eq!(truncate_words("alpha beta gammadelta epsilon zeta eta theta", 48), "alpha beta gammadelta epsilon zeta eta theta");
    // Single word longer than 48 is hard-cut at 48 (documented fallback)
    let long = "x".repeat(100);
    assert_eq!(truncate_words(&long, 48), "x".repeat(48));
    // No trailing space, never a mid-word cut
    for s in ["Refactor the auth middleware to remove the shared session cache from the login path", "a b c", "   spaced   out   "] {
        let cut = truncate_words(s, 48);
        assert!(cut.chars().count() <= 48);
        assert!(!cut.ends_with(' '), "no trailing space: `{cut}`");
    }
    // task head keeps the 78-char single-line contract
    assert_eq!(single_line("x\ny"), "x");
}

#[test]
fn socket_submit_task_roundtrip() {
    use std::os::unix::net::UnixListener;
    let root = tmp_root("submit-socket");
    let repo = submit_plan_repo(&root);
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = Arc::new(spawn_daemon(&root, &fake, clock.clone()));

    let sock = short_sock("sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let server = {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || notchd::socket::serve_with_shutdown(listener, daemon, shutdown))
    };

    let stream = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap(); // hello

    let send = |writer: &mut std::os::unix::net::UnixStream, obj: Value| {
        writeln!(writer, "{}", obj).unwrap();
        writer.flush().unwrap();
    };
    let read = |reader: &mut BufReader<std::os::unix::net::UnixStream>| -> Value {
        let mut l = String::new();
        reader.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap()
    };

    // Validation failure over the wire: no ids.
    send(&mut writer, serde_json::json!({"cmd": "submit_task", "task": "", "cwd": repo.to_str().unwrap()}));
    let bad = read(&mut reader);
    assert_eq!(bad["event"], "ack");
    assert_eq!(bad["cmd"], "submit_task");
    assert_eq!(bad["ok"], false);
    assert!(bad["error"].is_string());
    assert!(bad.get("supervisorId").is_none());

    // Happy path over the wire.
    send(&mut writer, serde_json::json!({"cmd": "submit_task", "task": "Wire task head that is long enough to prove the task field carries the full text of the submitted job", "cwd": repo.to_str().unwrap(), "title": "Wire Title"}));
    let good = read(&mut reader);
    assert_eq!(good["cmd"], "submit_task");
    assert_eq!(good["ok"], true);
    assert_eq!(good["supervisorId"], "SV-1");
    assert_eq!(good["sessionId"], "S1-OC01");

    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(writer);
    let _ = server.join();
}

// ---------- 21. submit_task v1.3 (dod / model / supervisorId) ----------

#[test]
fn submit_task_v1_3_stores_goal_dod_model_and_budgets() {
    let root = tmp_root("submit13");
    let repo = submit_plan_repo(&root);
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());

    let ack = daemon.submit_task(
        "Write a failing regression test for the registry reuse bug".to_string(),
        Some(repo.to_str().unwrap().to_string()),
        Some("Reg trip".to_string()),
        Some("cargo test registry_rejects_duplicate passes".to_string()),
        Some("opencode/hy4-flash-free".to_string()),
        None,
    );
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);
    assert_eq!(ack.supervisor_id.as_deref(), Some("SV-1"));
    assert_eq!(ack.session_id.as_deref(), Some("S1-OC01"));

    let snap = daemon.snapshot();
    assert_eq!(snap.sessions.len(), 1);
    let s = &snap.sessions[0];
    assert_eq!(s.goal.as_deref(), Some("Write a failing regression test for the registry reuse bug"));
    assert_eq!(s.definition_of_done.as_deref(), Some("cargo test registry_rejects_duplicate passes"));
    assert_eq!(s.model.as_deref(), Some("opencode/hy4-flash-free"));
    assert_eq!(s.iterations, Some(0));
    assert_eq!(s.max_iterations, Some(8));
    assert_eq!(s.wall_clock_deadline, Some(1_763_520_000_000 + 3_600_000));

    // The interactive opencode harness takes no --model; the model field is
    // the decider model only. Agent start must NEVER carry --model.
    let calls = fake.calls();
    let start = calls
        .iter()
        .find(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("start"))
        .expect("agent start call");
    assert!(!start.iter().any(|a| a == "--model"), "no model passthrough may reach agent start: {start:?}");

    // Unset model: no passthrough tokens anywhere in argv.
    let ack = daemon.submit_task(
        "plain task".to_string(),
        Some(repo.to_str().unwrap().to_string()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok);
    let calls = fake.calls();
    let last_start = calls
        .iter()
        .rfind(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("start"))
        .unwrap();
    assert!(!last_start.iter().any(|a| a == "--model"));
}

#[test]
fn submit_task_supervisor_id_reroutes_into_existing_supervisor() {
    let root = tmp_root("submit-reroute");
    let repo = submit_plan_repo(&root);
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());

    let ack1 = daemon.submit_task("first".to_string(), Some(repo.to_str().unwrap().to_string()), None, None, None, None);
    assert!(ack1.ok);
    let ack2 = daemon.submit_task(
        "second under the same supervisor".to_string(),
        Some(repo.to_str().unwrap().to_string()),
        None,
        None,
        None,
        Some("SV-1".to_string()),
    );
    assert!(ack2.ok, "reroute failed: {:?}", ack2.error);
    assert_eq!(ack2.supervisor_id.as_deref(), Some("SV-1"));
    assert_eq!(ack2.session_id.as_deref(), Some("S1-OC02"));
    let snap = daemon.snapshot();
    assert_eq!(snap.supervisors.len(), 1, "no new supervisor allocated");
    assert_eq!(snap.sessions.len(), 2);

    // Unknown supervisor id: nothing allocated.
    let ack3 = daemon.submit_task(
        "third".to_string(),
        Some(repo.to_str().unwrap().to_string()),
        None,
        None,
        None,
        Some("SV-99".to_string()),
    );
    assert!(!ack3.ok);
    assert_eq!(ack3.supervisor_id, None);
    assert_eq!(ack3.session_id, None);
    let snap = daemon.snapshot();
    assert_eq!(snap.supervisors.len(), 1);
    assert_eq!(snap.sessions.len(), 2);
}

// ---------- 22. activity probe (opencode /api/session) ----------

#[test]
fn activity_probe_parses_live_fixture() {
    let fixture = include_str!("fixtures/opencode-api-session.json");
    let sessions = notchd::opencode::parse_api_session_list(fixture).expect("fixture parses");
    assert!(!sessions.is_empty());
    for s in &sessions {
        assert!(!s.id.is_empty());
        // idle iff time.idle >= time.busy
        let idle = match s.idle {
            Some(i) => s.busy.map(|b| i >= b).unwrap_or(true),
            None => false,
        };
        let _ = idle; // shape check only; classification logic lives in the decider module
    }
    // At least one entry has an idle timestamp in this capture.
    assert!(sessions.iter().any(|s| s.idle.is_some()));
    assert!(sessions.iter().any(|s| s.idle.is_none()) || sessions.iter().all(|s| s.idle.is_some()));
}

#[test]
fn activity_classification_idle_busy_and_missing() {
    use notchd::opencode::ApiSession;
    let idle_s = ApiSession { id: "a".into(), busy: Some(1000), idle: Some(2000), model: None, directory: None };
    let busy_s = ApiSession { id: "b".into(), busy: Some(3000), idle: Some(2000), model: None, directory: None };
    let missing_s = ApiSession { id: "c".into(), busy: Some(3000), idle: None, model: None, directory: None };
    let missing_busy = ApiSession { id: "d".into(), busy: None, idle: Some(2000), model: None, directory: None };
    assert!(notchd::opencode::is_idle(&idle_s));
    assert!(!notchd::opencode::is_idle(&busy_s));
    assert!(!notchd::opencode::is_idle(&missing_s));
    assert!(notchd::opencode::is_idle(&missing_busy), "no busy marker yet means idle");
}

// ---------- 23. Supervisor decision loop (v1.3) ----------

fn submit_simple_goal(daemon: &Daemon, root: &Path, title: &str) -> String {
    let repo = root.join("goal-repo");
    std::fs::create_dir_all(&repo).unwrap();
    let ack = daemon.submit_task(
        format!("Goal: {title}"),
        Some(repo.to_str().unwrap().to_string()),
        Some(title.to_string()),
        Some("proof.txt exists and contains notch-idle-proof".to_string()),
        None,
        None,
    );
    assert!(ack.ok, "submit failed: {:?}", ack.error);
    ack.session_id.unwrap()
}

fn pane_of(daemon: &Daemon, id: &str) -> String {
    daemon.snapshot().sessions.iter().find(|s| s.id == id).unwrap().pane_id.clone()
}

fn mark_idle(fake: &Arc<FakeHerdr>, oc: &Arc<FakeOpencode>, pane_id: &str, revision: u64) {
    let entry = agent_entry(pane_id, "idle", revision);
    fake.set_agents(vec![entry]);
    oc.set_api_idle(pane_id, true);
}

#[test]
fn idle_triggers_one_decider_and_one_reprompt_continue_path() {
    let root = tmp_root("sup-continue");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);
    fake.set_agents(vec![agent_entry(&pane, "working", 1)]);

    // Working: no decider call, no re-prompt.
    daemon.poll_once();
    assert_eq!(oc.decider_calls(), 0);

    // Idle: exactly one decider call, one re-prompt with the instruction.
    mark_idle(&fake, &oc, &pane, 2);
    oc.push_decision("CONTINUE: create proof.txt containing exactly notch-idle-proof and cat it");
    daemon.poll_once();

    assert_eq!(oc.decider_calls(), 1, "one decider call expected");
    let calls = fake.calls();
    let prompts: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("prompt"))
        .collect();
    let reprompt = prompts
        .iter()
        .find(|p| p.join(" ").contains("create proof.txt containing exactly notch-idle-proof"))
        .expect("re-prompt with decider instruction");
    assert_eq!(reprompt[4], "create proof.txt containing exactly notch-idle-proof and cat it");

    let snap = daemon.snapshot();
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(s.iterations, Some(1));
    assert!(s.last_supervisor_action.as_deref().unwrap().starts_with("reprompt:"));
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("\"kind\":\"decision\""), "decision event missing: {log}");
    assert!(log.contains("\"kind\":\"idle_signal\""), "idle_signal event missing: {log}");
}

#[test]
fn done_verifier_yes_marks_done_and_cleans_up_no_iterations() {
    let root = tmp_root("sup-done");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);
    mark_idle(&fake, &oc, &pane, 7);
    assert!(root.join("tmp").join(format!("notch-{sid}")).exists());

    oc.push_decision("DONE: proof.txt created and cat shows notch-idle-proof");
    oc.push_decision("YES: output proves the DoD is satisfied");
    daemon.poll_once();

    let snap = daemon.snapshot();
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(s.status, Status::Done);
    assert_eq!(s.iterations, Some(0));
    assert!(s.last_supervisor_action.as_deref().unwrap().starts_with("done:"));
    assert!(!root.join("tmp").join(format!("notch-{sid}")).exists(), "verifier-accepted done must clean up");
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("done (verifier accepted evidence)"));
}

#[test]
fn done_verifier_no_reprompts_for_missing_evidence_counts_iteration() {
    let root = tmp_root("sup-missing");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);
    mark_idle(&fake, &oc, &pane, 7);

    oc.push_decision("DONE: proof.txt created");
    oc.push_decision("NO: the file content was never shown; only its existence");
    daemon.poll_once();

    let calls = fake.calls();
    let reprompt = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("prompt"))
        .find(|p| p.join(" ").contains("definition of done is not yet satisfied") || p.join(" ").contains("never shown"));
    assert!(reprompt.is_some(), "expected a missing-evidence re-prompt: {calls:?}");
    let snap = daemon.snapshot();
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(s.iterations, Some(1));
    assert_eq!(s.status, Status::Working);
    assert!(s.last_supervisor_action.as_deref().unwrap().starts_with("verify:"));
}

#[test]
fn three_consecutive_stucks_trigger_strategy_change_then_escalate() {
    let root = tmp_root("sup-stuck");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);

    // Idle rounds with the same blocker three times -> strategy change; fourth -> escalate.
    for round in 1..=4u32 {
        mark_idle(&fake, &oc, &pane, 10 + round as u64);
        oc.push_decision("STUCK: the same compile error persists at src/auth.rs:12");
        daemon.poll_once();
    }
    let snap = daemon.snapshot();
    let esc = &snap.escalations;
    assert_eq!(esc.len(), 1, "one escalation expected: {esc:?}");
    assert_eq!(esc[0].kind, "loop_detected");
    // Escalated sessions stay as-is (not killed, not re-prompted) per the contract.
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(s.status, Status::Idle);
    let decisions: Vec<&str> = snap.decisions.iter().map(|d| d.action.as_str()).collect();
    assert!(decisions.contains(&"escalate"), "escalate decision recorded: {decisions:?}");
    // No fifth decider call — the escalated session is excluded from the loop.
    let calls_before = oc.decider_calls();
    daemon.poll_once();
    assert_eq!(oc.decider_calls(), calls_before, "escalated session must not call the decider");
}

#[test]
fn budget_exhaustion_escalates_at_max_iterations() {
    let root = tmp_root("sup-budget");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);

    // Eight CONTINUE rounds (the default max). Advance the fake pane output
    // every round so the loop detector does not (correctly) fire.
    for round in 1..=8u32 {
        mark_idle(&fake, &oc, &pane, 20 + round as u64);
        *fake.read_text.lock().unwrap() = format!("round {round}: cargo test output line {round}
");
        oc.push_decision(&format!("CONTINUE: do the next thing (round {round})"));
        daemon.poll_once();
        let s = daemon.snapshot().sessions.into_iter().find(|s| s.id == sid).unwrap();
        eprintln!("[loop] round={round} status={} reason={:?} iterations={:?}", s.status.as_str(), s.failure_reason, s.iterations);
    }
    let snap = daemon.snapshot();
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(s.iterations, Some(8));
    // Ninth idle poll must NOT call the decider; it must escalate budget_exceeded.
    let calls_before = oc.decider_calls();
    mark_idle(&fake, &oc, &pane, 40);
    daemon.poll_once();
    assert_eq!(oc.decider_calls(), calls_before, "decider must not be called past budget");
    let snap = daemon.snapshot();
    let esc = &snap.escalations;
    assert!(esc.iter().any(|e| e.kind == "budget_exceeded"), "budget escalation expected: {esc:?}");
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(s.failure_reason.as_deref(), None::<&str>, "budget escalation does not kill the session");
}

#[test]
fn paused_supervisor_gets_no_decisions_until_resumed() {
    let root = tmp_root("sup-pause");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);

    daemon.pause_supervisor("SV-1").unwrap();
    mark_idle(&fake, &oc, &pane, 30);
    daemon.poll_once();
    assert_eq!(oc.decider_calls(), 0, "paused supervisor must not decide");
    let snap = daemon.snapshot();
    assert!(snap.supervisors[0].paused);

    daemon.resume_supervisor("SV-1").unwrap();
    mark_idle(&fake, &oc, &pane, 31);
    oc.push_decision("CONTINUE: create proof.txt and cat it");
    daemon.poll_once();
    assert_eq!(oc.decider_calls(), 1);
    let snap = daemon.snapshot();
    assert!(!snap.supervisors[0].paused);
}

#[test]
fn kill_switch_closes_everything_and_marks_failed() {
    let root = tmp_root("sup-kill");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid1 = submit_simple_goal(&daemon, &root, "Task one");
    let sid2 = submit_simple_goal(&daemon, &root, "Task two");
    let _ = (sid1, sid2);
    std::fs::create_dir_all(root.join("tmp").join("notch-S1-OC01")).unwrap();
    std::fs::create_dir_all(root.join("tmp").join("notch-S2-OC01")).unwrap();

    let ack = daemon.kill_switch();
    assert_eq!(ack.closed, 2, "both panes must close");
    let snap = daemon.snapshot();
    for s in &snap.sessions {
        assert_eq!(s.status, Status::Failed);
        assert_eq!(s.failure_reason.as_deref(), Some("kill_switch"));
    }
    assert!(!root.join("tmp").join("notch-S1-OC01").exists());
    assert!(!root.join("tmp").join("notch-S2-OC01").exists());
    let calls = fake.calls();
    let closes: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("pane") && c.get(2).map(String::as_str) == Some("close"))
        .collect();
    assert_eq!(closes.len(), 2);
}

#[test]
fn message_delivers_verbatim_and_is_logged_and_capped() {
    let root = tmp_root("sup-msg");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid1 = submit_simple_goal(&daemon, &root, "Task one");
    let sid2 = submit_simple_goal(&daemon, &root, "Task two");

    let ack = daemon.message(&sid1, &sid2, "handoff", "handoff: the tests fail at auth.rs:12");
    assert!(ack.ok, "message failed: {:?}", ack.error);
    assert_eq!(ack.message_id.as_deref(), Some("msg_1"));

    let calls = fake.calls();
    let prompts: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("agent") && c.get(2).map(String::as_str) == Some("prompt"))
        .collect();
    let delivery = prompts.iter().find(|p| p.join(" ").contains("the tests fail at auth.rs:12")).unwrap();
    assert_eq!(delivery[3], sid2.to_lowercase());
    assert_eq!(delivery[4], "handoff: the tests fail at auth.rs:12", "body is verbatim");

    // Unknown session and oversized body are rejected without delivery.
    let ack = daemon.message(&sid1, "S9-OC99", "handoff", "hello");
    assert!(!ack.ok);
    let big = "x".repeat(4001);
    let ack = daemon.message(&sid1, &sid2, "handoff", &big);
    assert!(!ack.ok);
    let ack = daemon.message(&sid1, &sid2, "bogus-type", "hello");
    assert!(!ack.ok);

    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("\"kind\":\"message\""));
    assert!(log.contains("S2-OC01"));

    // busMessages cap: 30 entries, newest first.
    for i in 0..31 {
        let msg = format!("fill message number {i}");
        let ack = daemon.message(&sid1, &sid2, "result", &msg);
        assert!(ack.ok, "fill message {i} failed");
    }
    let snap = daemon.snapshot();
    assert_eq!(snap.bus_messages.len(), 30);
    assert_eq!(snap.bus_messages[0].preview, "fill message number 30");
}

#[test]
fn list_models_parses_and_caches() {
    let root = tmp_root("sup-models");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock);
    let m1 = daemon.list_models();
    assert_eq!(m1.models, vec!["opencode/hy4-flash-free", "opencode/inkling-2-small-free"]);
    let m2 = daemon.list_models();
    assert_eq!(m2.models, m1.models);
    let runs = oc.calls.lock().unwrap().iter().filter(|c| c.get(1).map(String::as_str) == Some("models")).count();
    assert_eq!(runs, 1, "second call must hit the cache");
}

#[test]
fn snapshot_v1_3_blocks_carry_decisions_escalations_and_bus_messages() {
    let root = tmp_root("sup-blocks");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock.clone());
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);

    // 13 CONTINUE rounds — but the default iteration budget (8) is reached
    // first: 8 continue decisions + 1 budget escalation, then the decider
    // stops calling (per the contract, escalation halts the loop).
    for round in 1..=13u32 {
        mark_idle(&fake, &oc, &pane, 50 + round as u64);
        *fake.read_text.lock().unwrap() = format!("round {round}: cargo test output line {round}");
        oc.push_decision(&format!("CONTINUE: do round {round}"));
        daemon.poll_once();
    }
    let snap = daemon.snapshot();
    assert_eq!(snap.decisions.len(), 9);
    assert_eq!(snap.decisions[0].action, "escalate");
    assert_eq!(snap.decisions[1].reason, "continue: do round 8");
    // bus message visible
    let ack = daemon.message(&sid, &sid, "result", "self-check note");
    assert!(ack.ok);
    let snap = daemon.snapshot();
    assert_eq!(snap.bus_messages.len(), 1);
    assert_eq!(snap.bus_messages[0].from, sid);
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("\"kind\":\"idle_signal\"") && log.contains("\"kind\":\"decision\""));
}

#[test]
fn rate_limit_tail_triggers_backoff_without_calling_decider() {
    let root = tmp_root("sup-429");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock);
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);
    mark_idle(&fake, &oc, &pane, 60);
    *fake.read_text.lock().unwrap() = "Error: 429 Too Many Requests: rate limit exceeded\n".to_string();

    daemon.poll_once();
    assert_eq!(oc.decider_calls(), 0, "decider must not fire on a rate-limited tail");
    let snap = daemon.snapshot();
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert!(s.last_supervisor_action.as_deref().unwrap().starts_with("backoff:"));
    assert!(snap.warnings.iter().any(|w| w.contains("rate limit")));
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("backoff"), "backoff decision missing: {log}");
}

#[test]
fn unparseable_decider_output_is_logged_honestly_as_stuck() {
    let root = tmp_root("sup-unparseable");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock);
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);
    mark_idle(&fake, &oc, &pane, 70);
    oc.push_decision("hmm, {invalid json, not a verdict}");
    daemon.poll_once();
    let snap = daemon.snapshot();
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert!(s.last_supervisor_action.as_deref().unwrap().contains("decider unparseable"));
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("decider unparseable"));
}

#[test]
fn idle_signal_event_uses_primary_source() {
    let root = tmp_root("sup-signal");
    let fake = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon_with_opencode(&root, &fake, &oc, clock);
    let sid = submit_simple_goal(&daemon, &root, "Write the proof file");
    let pane = pane_of(&daemon, &sid);
    mark_idle(&fake, &oc, &pane, 80);
    oc.push_decision("DONE: proof.txt contains notch-idle-proof per cat output");
    oc.push_decision("YES: cat output proves it");
    daemon.poll_once();
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    let signal_line = log.lines().find(|l| l.contains("\"kind\":\"idle_signal\"")).expect("idle_signal logged");
    assert!(signal_line.contains("primary"), "primary signal expected: {signal_line}");
}


// ---------- 33. Watchdog stuck-kill records a `stuck` escalation (v1.3) ----------

#[test]
fn stuck_kill_records_stuck_escalation_in_snapshot() {
    let root = tmp_root("stuck-esc");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());

    let plan = write_plan(&root);
    daemon.run_plan(&plan);
    let snap = daemon.snapshot();
    let pane_a = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap().pane_id.clone();
    let pane_b = snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap().pane_id.clone();

    // A working forever, one done — only A is a stuck candidate.
    fake.set_agents(vec![
        agent_entry(&pane_a, "working", 7),
        agent_entry(&pane_b, "done", 3),
    ]);
    daemon.poll_once();
    clock.advance_ms(16_000);
    daemon.poll_once();

    let snap = daemon.snapshot();
    let esc = &snap.escalations;
    assert_eq!(esc.len(), 1, "stuck kill must record exactly one escalation: {esc:?}");
    assert_eq!(esc[0].kind, "stuck");
    assert_eq!(esc[0].session, "S1-OC01");
    assert_eq!(esc[0].supervisor, "SV-1");
    assert!(esc[0].reason.contains("no revision change"), "reason: {}", esc[0].reason);
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    assert_eq!(s1.status, Status::Failed);
    assert_eq!(s1.failure_reason.as_deref(), Some("stuck"));
}

// ---------- 34. kill_switch: close EVERY managed pane regardless of status (lead ruling) ----------

#[test]
fn kill_switch_closes_done_session_panes_too() {
    let root = tmp_root("killswitch-done");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());

    let plan = write_plan(&root);
    daemon.run_plan(&plan);
    let snap = daemon.snapshot();
    let pane_a = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap().pane_id.clone();
    let pane_b = snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap().pane_id.clone();

    // Mark S1-OC01 done (idle TUIs persist after done), S1-OC02 stays working.
    fake.set_agents(vec![
        agent_entry(&pane_a, "done", 1),
        agent_entry(&pane_b, "working", 1),
    ]);
    daemon.poll_once();
    // Recreate a temp dir for the done session to prove cleanup runs for it too.
    std::fs::create_dir_all(root.join("tmp").join("notch-S1-OC01")).unwrap();

    let ack = daemon.kill_switch();
    assert_eq!(ack.closed, 4, "every managed pane closes, including the done session's TUI");
    let calls = fake.calls();
    let closes: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("pane") && c.get(2).map(String::as_str) == Some("close"))
        .collect();
    assert_eq!(closes.len(), 4);
    assert!(closes.iter().any(|c| c.join(" ").contains(&pane_a)));

    let snap = daemon.snapshot();
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    assert_eq!(s1.status, Status::Done, "done sessions stay done");
    let s2 = snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap();
    assert_eq!(s2.status, Status::Failed);
    assert_eq!(s2.failure_reason.as_deref(), Some("kill_switch"));
    assert!(!root.join("tmp").join("notch-S1-OC01").exists());
}

#[test]
fn kill_switch_one_failed_close_does_not_abort_the_batch() {
    let root = tmp_root("killswitch-one-fail");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());

    let plan = write_plan(&root);
    daemon.run_plan(&plan);
    let snap = daemon.snapshot();
    let pane_a = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap().pane_id.clone();
    let pane_b = snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap().pane_id.clone();

    // The done session's pane is already gone in herdr.
    fake.set_agents(vec![
        agent_entry(&pane_a, "working", 1),
        agent_entry(&pane_b, "working", 1),
    ]);
    daemon.poll_once();
    fake.set_fail_panes(vec![&pane_a]);

    let ack = daemon.kill_switch();
    assert_eq!(ack.closed, 3, "three panes close; the already-gone one doesn't count");
    let snap = daemon.snapshot();
    let s1 = snap.sessions.iter().find(|s| s.id == "S1-OC01").unwrap();
    let s2 = snap.sessions.iter().find(|s| s.id == "S1-OC02").unwrap();
    assert_eq!(s1.status, Status::Failed);
    assert_eq!(s2.status, Status::Failed);
    assert_eq!(s1.failure_reason.as_deref(), Some("kill_switch"));
    assert_eq!(s2.failure_reason.as_deref(), Some("kill_switch"));
    let log = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap();
    assert!(log.contains("pane_already_closed"), "failure must be logged, got: {log}");
}

#[test]
fn kill_switch_skips_sessions_with_empty_pane_id() {
    let root = tmp_root("killswitch-empty-pane");
    let fake = FakeHerdr::new();
    *fake.fail_subcommand.lock().unwrap() = Some("pane split".to_string());
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock.clone());

    let plan = write_plan(&root);
    daemon.run_plan(&plan); // spawn fails: pane split fails -> pane_id empty, session failed
    let snap = daemon.snapshot();
    assert!(snap.sessions.iter().all(|s| s.pane_id.is_empty()));

    let ack = daemon.kill_switch();
    assert_eq!(ack.closed, 0);
    let calls = fake.calls();
    let closes: Vec<&Vec<String>> = calls
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("pane") && c.get(2).map(String::as_str) == Some("close"))
        .collect();
    assert_eq!(closes.len(), 0, "empty paneId must be skipped, not closed");
}

// ---------- 35. Global sweep must never delete non-engine dirs (lead ruling) ----------

#[test]
fn sweep_never_deletes_non_engine_dirs() {
    let root = tmp_root("sweep-safety");
    let state_dir = root.join("state");
    let tmp_dir = root.join("tmp");
    std::fs::create_dir_all(&tmp_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    // Dirs that look like notch's but are not the engine's — must survive.
    let notch_accept = tmp_dir.join("notch-accept-123");
    std::fs::create_dir_all(notch_accept.join("nested")).unwrap();
    std::fs::write(notch_accept.join("nested").join("keep.txt"), "mine").unwrap();
    let notch_foo = tmp_dir.join("notch-foo-bar");
    std::fs::create_dir_all(&notch_foo).unwrap();
    std::fs::write(notch_foo.join("keep.txt"), "mine").unwrap();

    // Nonempty exact-name orphan we never spawned — must survive + warn.
    let impostor = tmp_dir.join("notch-S1-OC09");
    std::fs::create_dir_all(&impostor).unwrap();
    std::fs::write(impostor.join("evidence.txt"), "acceptance evidence").unwrap();

    // Empty exact-name orphan — eligible for the sweep.
    let empty_orphan = tmp_dir.join("notch-S9-OC09");
    std::fs::create_dir_all(&empty_orphan).unwrap();

    // A TRACKED dead session with its tempDir recorded — must be removed.
    let tracked_dir = tmp_dir.join("notch-S1-OC01");
    std::fs::create_dir_all(&tracked_dir).unwrap();
    std::fs::write(tracked_dir.join("work.txt"), "session output").unwrap();
    std::fs::write(
        state_dir.join("state.json"),
        serde_json::json!({
            "version": 1,
            "supervisors": [{ "id": "SV-1", "number": 1, "title": "t" }],
            "sessions": [{
                "id": "S1-OC01", "sup": "SV-1", "status": "failed",
                "title": "t", "task": "t", "cwd": "/tmp",
                "paneId": "w1:p1", "startedAt": 1_763_520_000_000u64,
                "lastAction": null, "lastRevision": null,
                "latencyMs": null, "failureReason": "spawn_failed",
                "tempDir": tracked_dir.to_string_lossy()
            }],
            "attribution": [], "conflicts": [], "warnings": [],
            "stuck_after_secs": 900, "paused_supervisors": [],
            "decisions": [], "escalations": [], "bus_messages": [],
            "esc_seq": 0, "msg_seq": 0
        })
        .to_string(),
    )
    .unwrap();

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let ack = daemon.cleanup(None);
    assert!(ack.ok);
    assert!(notch_accept.exists(), "unrelated notch-accept-123 must survive sweep");
    assert!(notch_foo.exists(), "unrelated notch-foo-bar must survive sweep");
    assert!(impostor.exists(), "nonempty untracked exact-name dir must survive sweep");
    assert!(!empty_orphan.exists(), "exact-name empty orphan must be swept");
    assert!(!tracked_dir.exists(), "tracked dead session dir must be removed");
    assert_eq!(ack.removed, Some(2), "one tracked dead dir + one empty orphan, got {:?}", ack.removed);
    let snap = daemon.snapshot();
    assert!(
        snap.warnings.iter().any(|w| w.contains("notch-S1-OC09")),
        "untracked nonempty exact-name dir must produce a warning: {:?}",
        snap.warnings
    );
}
