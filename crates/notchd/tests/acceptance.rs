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
    split_seq: Mutex<u32>,
}

impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            agents: Mutex::new(Vec::new()),
            read_text: Mutex::new("last line of pane output\n".to_string()),
            fail_subcommand: Mutex::new(None),
            split_seq: Mutex::new(0),
        })
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
            ("pane", "close") => Ok(serde_json::json!({
                "id": "cli:pane:close",
                "result": { "type": "ok" }
            })
            .to_string()),
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
        assert!(joined.contains(expected_ids[i]), "start {i} missing session id: {joined}");
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
        reprompt.join(" ").contains("S1-OC01"),
        "re-prompt targets the session: {reprompt:?}"
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
        ["id", "status", "working", "idle", "failed", "done"].into_iter().collect();
    assert_eq!(sup_keys, expected_sup);

    let ses = &obj["sessions"][0];
    let ses_keys: std::collections::BTreeSet<&str> = ses.as_object().unwrap().keys().map(String::as_str).collect();
    let expected_ses: std::collections::BTreeSet<&str> = [
        "id", "sup", "status", "title", "task", "cwd", "paneId", "startedAt", "lastAction",
        "lastRevision", "latencyMs", "failureReason",
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

    let sock = root.join("notch.sock");
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

    let sock = root.join("notch.sock");
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
