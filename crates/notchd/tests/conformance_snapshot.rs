//! conformance_snapshot.rs — Layer A of the contract-drift guard (bridge.md v1.3).
//!
//! Two layers prove the implementation can never silently drift from the
//! frozen wire contract:
//!
//! - Layer A (this file): builds a snapshot through the real `Daemon` (fake
//!   herdr/opencode runners) and asserts the v1.3 §1.3 shape — exact key sets
//!   per object level, camelCase keys everywhere, status vocabulary, f64 eps,
//!   integer-ms timestamps, version == 1 — plus a v1.2-consumer forward-compat
//!   test (additive policy) and §2 socket ack envelopes.
//! - Layer B (tools/contract-check.mjs): spawns `notchd serve` with
//!   NOTCH_* env against a fake HERDR shim and validates the same rules from
//!   JavaScript plus a hardcoded command manifest.
//!
//! Run:  cargo test -p notchd --test conformance_snapshot
//!
//! Canonical key lists below are checked in and mirror docs/bridge.md §1.3
//! (v1.3). Any key not in the allowed-optional lists fails the build:
//! additive fields are legal ONLY if they are in the allowed-optional list.
//!   - sessions[].item: goal / definitionOfDone / iterations / maxIterations /
//!     wallClockDeadline / lastSupervisorAction / model
//!   - supervisors[].item: paused
//!   - top: decisions / escalations / busMessages
//!
//! If this file and docs/bridge.md disagree, bridge.md wins — file a lead
//! sign-off, do not edit this list to make a failure go away.

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;
use notchd::status::Status;

use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------- canonical key lists (checked in; mirror bridge.md §1.3 v1.3) ----------

/// Top level, always present.
const TOP_REQUIRED: &[&str] = &[
    "version",
    "generatedAt",
    "orchestrator",
    "supervisors",
    "sessions",
    "eps",
    "conflicts",
    "attribution",
    "warnings",
];
/// Top level, v1.3 allowed-optional (present only when data exists).
const TOP_OPTIONAL: &[&str] = &["decisions", "escalations", "busMessages"];

const ORCHESTRATOR_KEYS: &[&str] = &["id", "status"];

const SUPERVISOR_REQUIRED: &[&str] = &["id", "status", "working", "idle", "failed", "done"];
/// v1.3 allowed-optional on supervisors.
const SUPERVISOR_OPTIONAL: &[&str] = &["paused", "title"]; // lead sign-off 2026-11-20: additive display title, documented in bridge.md §1.3

const SESSION_REQUIRED: &[&str] = &[
    "id",
    "sup",
    "status",
    "title",
    "task",
    "cwd",
    "paneId",
    "startedAt",
    "lastAction",
    "lastRevision",
    "latencyMs",
    "failureReason",
];
/// v1.3 allowed-optional on sessions.
const SESSION_OPTIONAL: &[&str] = &[
    "goal",
    "definitionOfDone",
    "iterations",
    "maxIterations",
    "wallClockDeadline",
    "lastSupervisorAction",
    "model",
];

const CONFLICT_KEYS: &[&str] = &["file", "sessions"];
const ATTRIBUTION_KEYS: &[&str] = &["session", "file"];
const DECISION_KEYS: &[&str] = &["at", "supervisor", "session", "action", "reason"];
const ESCALATION_KEYS: &[&str] = &["id", "session", "supervisor", "kind", "reason", "at"];
const BUS_MESSAGE_KEYS: &[&str] = &["id", "from", "to", "type", "preview", "at"];

const STATUS_ENUM: &[&str] = &["working", "idle", "failed", "done"];

const T0: u64 = 1_763_520_000_000;
const RESTART_MS: u64 = 3_600_000;

// ---------- shared key/type helpers ----------

fn is_camel_case_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// Recursive camelCase audit of every object key at every level.
fn walk_keys(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, sub) in map {
                if !is_camel_case_key(k) {
                    out.push(format!(
                        "{path}.{k}: key does not match ^[a-z][a-zA-Z0-9]*$"
                    ));
                }
                let sub_path = format!("{path}.{k}");
                walk_keys(sub, &sub_path, out);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_keys(item, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn check_object_keys(
    obj: &Value,
    required: &[&str],
    optional: &[&str],
    path: &str,
    out: &mut Vec<String>,
) {
    let Some(map) = obj.as_object() else {
        out.push(format!("{path}: expected an object, got {obj}"));
        return;
    };
    let mut allowed: Vec<&str> = required.to_vec();
    allowed.extend_from_slice(optional);
    for r in required {
        if !map.contains_key(*r) {
            out.push(format!("{path}: missing required key `{r}`"));
        }
    }
    for k in map.keys() {
        if !allowed.contains(&k.as_str()) {
            out.push(format!(
                "{path}: unexpected key `{k}` (allowed here: {allowed:?})"
            ));
        }
    }
}

fn check_items(
    v: &Value,
    field: &str,
    required: &[&str],
    optional: &[&str],
    item_name: &str,
    out: &mut Vec<String>,
) {
    match v.get(field) {
        None => out.push(format!("top: missing required array `{field}`")),
        Some(Value::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                check_object_keys(item, required, optional, &format!("{item_name}[{i}]"), out);
            }
        }
        Some(other) => out.push(format!("top: `{field}` must be an array, got {other}")),
    }
}

/// Every key-set rule of §1.3, applied to one serialized snapshot.
/// Returns the list of violations (empty == conformant).
fn snapshot_key_violations(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    check_object_keys(v, TOP_REQUIRED, TOP_OPTIONAL, "<top>", &mut out);
    match v.get("orchestrator") {
        Some(orch) => check_object_keys(orch, ORCHESTRATOR_KEYS, &[], "orchestrator", &mut out),
        None => out.push("<top>: missing required key `orchestrator`".to_string()),
    }
    check_items(
        v,
        "supervisors",
        SUPERVISOR_REQUIRED,
        SUPERVISOR_OPTIONAL,
        "supervisors",
        &mut out,
    );
    check_items(
        v,
        "sessions",
        SESSION_REQUIRED,
        SESSION_OPTIONAL,
        "sessions",
        &mut out,
    );
    check_items(v, "conflicts", CONFLICT_KEYS, &[], "conflicts", &mut out);
    check_items(
        v,
        "attribution",
        ATTRIBUTION_KEYS,
        &[],
        "attribution",
        &mut out,
    );
    // v1.3 optional top-level blocks: only when present (they are skipped
    // when empty). When present they must exist as arrays of exact items.
    if v.get("decisions").is_some() {
        check_items(v, "decisions", DECISION_KEYS, &[], "decisions", &mut out);
    }
    if v.get("escalations").is_some() {
        check_items(
            v,
            "escalations",
            ESCALATION_KEYS,
            &[],
            "escalations",
            &mut out,
        );
    }
    if v.get("busMessages").is_some() {
        check_items(
            v,
            "busMessages",
            BUS_MESSAGE_KEYS,
            &[],
            "busMessages",
            &mut out,
        );
    }
    out
}

fn status_enum_violations(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    if let Some(s) = v.pointer("/orchestrator/status").and_then(Value::as_str) {
        seen.push(("orchestrator.status".to_string(), s));
    }
    if let Some(arr) = v.get("supervisors").and_then(Value::as_array) {
        for (i, s) in arr.iter().enumerate() {
            if let Some(x) = s.get("status").and_then(Value::as_str) {
                seen.push((format!("supervisors[{i}].status"), x));
            }
        }
    }
    if let Some(arr) = v.get("sessions").and_then(Value::as_array) {
        for (i, s) in arr.iter().enumerate() {
            if let Some(x) = s.get("status").and_then(Value::as_str) {
                seen.push((format!("sessions[{i}].status"), x));
            }
        }
    }
    if seen.len() < 6 {
        out.push(format!(
            "expected at least 6 status fields (orchestrator + supervisors + sessions), saw {}",
            seen.len()
        ));
    }
    for (path, s) in seen {
        if !STATUS_ENUM.contains(&s) {
            out.push(format!("{path}: `{s}` is not one of {STATUS_ENUM:?}"));
        }
    }
    out
}

fn assert_integer_ms(v: Option<&Value>, path: &str, out: &mut Vec<String>) {
    match v {
        Some(Value::Number(n)) if n.is_u64() => {}
        other => out.push(format!("{path}: expected integer ms, got {other:?}")),
    }
}

// ---------- fixture: real Daemon, fake runners ----------

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!(
        "notchd-conformance-{tag}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn short_sock(tag: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    PathBuf::from(format!(
        "/tmp/nd-conf-{tag}-{}-{n}.sock",
        std::process::id()
    ))
}

/// Scripted fake herdr. Records nothing we need; returns canned JSON.
struct FixtureHerdr {
    agents: Mutex<Vec<Value>>,
    read_text: Mutex<String>,
    split_seq: Mutex<u32>,
}

impl FixtureHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            agents: Mutex::new(Vec::new()),
            read_text: Mutex::new("last line of pane output\n".to_string()),
            split_seq: Mutex::new(0),
        })
    }

    fn set_agents(&self, agents: Vec<Value>) {
        *self.agents.lock().unwrap() = agents;
    }

    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| fake.run(args))
    }

    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        let sub = args.get(1).cloned().unwrap_or_default();
        let sub2 = args.get(2).cloned().unwrap_or_default();
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

/// Scripted fake opencode: canned /api/session, two models, scripted decisions.
struct FixtureOpencode {
    api_session_raw: Mutex<String>,
    decisions: Mutex<VecDeque<String>>,
}

impl FixtureOpencode {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            api_session_raw: Mutex::new(
                serde_json::json!({"cursor": null, "data": []}).to_string(),
            ),
            decisions: Mutex::new(VecDeque::new()),
        })
    }

    fn set_api_idle(&self, pane_id: &str, idle: bool) {
        let entry = serde_json::json!({
            "id": pane_id,
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

    fn opencode(self: &Arc<Self>) -> Opencode {
        let fake = Arc::clone(self);
        Opencode::with_runner(move |args: &[String]| {
            let rest = &args[1..];
            match rest.join(" ").as_str() {
                "api GET /api/session" => Ok(fake.api_session_raw.lock().unwrap().clone()),
                "models" => {
                    Ok("opencode/hy4-flash-free\nopencode/inkling-2-small-free\n".to_string())
                }
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

struct Fixture {
    daemon: Daemon,
    fake: Arc<FixtureHerdr>,
    oc: Arc<FixtureOpencode>,
}

/// Build a daemon with:
/// - 2 supervisors (SV-1 paused at the end), 3 sessions
/// - S1-OC01: done, full v1.3 control block (goal/DoD/iterations/budget/model)
/// - S1-OC02: failed, reason `blocked_waiting`
/// - S2-OC01: working
/// - decisions (reprompt + escalate), one escalation (`blocked`), one bus
///   message, one attribution conflict, at least one warning
fn build_fixture(tag: &str) -> Fixture {
    let root = tmp_root(tag);
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let fake = FixtureHerdr::new();
    fake.set_agents(vec![]);
    let oc = FixtureOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let config = Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 15,
        poll_interval_ms: 2000,
        opencode_decider: false,
    };
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    let daemon = Daemon::open(config, fake.herdr(), clock).unwrap();
    daemon.set_opencode(oc.opencode());

    let repo_str = repo.to_string_lossy().to_string();

    let ack1 = daemon.submit_task(
        "Ship the audit-signoff release checklist".to_string(),
        Some(repo_str.clone()),
        Some("Audit sign-off".to_string()),
        Some("cargo test passes on main".to_string()),
        Some("opencode/hy4-flash-free".to_string()),
        None,
    );
    assert!(ack1.ok, "submit_task 1 failed: {:?}", ack1.error);
    let sid1 = ack1.session_id.clone().unwrap();
    assert_eq!(sid1, "S1-OC01");

    let ack2 = daemon.submit_task(
        "Cross-check the invoice totals".to_string(),
        Some(repo_str.clone()),
        None,
        None,
        None,
        Some("SV-1".to_string()),
    );
    assert!(ack2.ok, "submit_task 2 failed: {:?}", ack2.error);
    let sid2 = ack2.session_id.clone().unwrap();
    assert_eq!(sid2, "S1-OC02");

    let ack3 = daemon.submit_task(
        "Migrate the metrics exporter".to_string(),
        Some(repo_str),
        None,
        None,
        None,
        None,
    );
    assert!(ack3.ok, "submit_task 3 failed: {:?}", ack3.error);
    assert_eq!(ack3.supervisor_id.as_deref(), Some("SV-2"));
    let sid3 = ack3.session_id.clone().unwrap();
    assert_eq!(sid3, "S2-OC01");

    let pane_of = |id: &str| -> String {
        daemon
            .snapshot()
            .sessions
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .pane_id
            .clone()
    };
    let pane1 = pane_of(&sid1);
    let pane2 = pane_of(&sid2);
    let pane3 = pane_of(&sid3);
    assert_ne!(pane1, pane2);
    assert_ne!(pane2, pane3);

    // Poll 1: S1-OC01 idle via the primary signal; decider says CONTINUE ->
    // one re-prompt, iterations = 1, a `reprompt` decision is logged.
    fake.set_agents(vec![
        agent_entry(&pane1, "idle", 2),
        agent_entry(&pane2, "working", 2),
        agent_entry(&pane3, "working", 2),
    ]);
    oc.set_api_idle(&pane1, true);
    oc.push_decision("CONTINUE: finish the remaining checklist items and re-run cargo test");
    daemon.poll_once();
    // Stop the primary idle signal for the rest of the fixture so no stray
    // decider call fires in later polls.
    oc.set_api_idle(&pane1, false);

    // P1 DoD gate: herdr `done` is NOT a verdict. When herdr reports done for
    // a DoD-bearing session, supervise.rs reverts it and submits a Judge job;
    // a DONE verdict creates a PendingVerify + Verify job; apply_verify on a
    // later drain tick logs `decision{action:"done"}` and finally sets Done.
    // The fake runner pops one string per `run --model` call, in this order.
    oc.push_decision("DONE: the audit-signoff file exists and matches the checklist");
    oc.push_decision("YES: verified the real sign-off file");

    // Poll 2: S1-OC01 herdr-done -> gate reverts + Judge job; S1-OC02 blocked
    // strike 1 -> one re-prompt.
    fake.set_agents(vec![
        agent_entry(&pane1, "done", 3),
        agent_entry(&pane2, "blocked", 3),
        agent_entry(&pane3, "working", 3),
    ]);
    daemon.poll_once();

    // Poll 3: S1-OC02 stays idle (the human owns the permission decision).
    // A second `blocked` sample would log a second needs_you escalation, so
    // from here on herdr reports idle for that pane.
    fake.set_agents(vec![
        agent_entry(&pane1, "done", 3),
        agent_entry(&pane2, "idle", 4),
        agent_entry(&pane3, "working", 4),
    ]);
    daemon.poll_once();

    // Drain the bounded JudgePool: Judge result -> Verify job -> DONE. The
    // fake clock must NOT advance here (that would trip the stall guard); the
    // wait is only for real worker threads to finish.
    let drain_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        daemon.poll_once();
        let done = daemon
            .snapshot()
            .sessions
            .iter()
            .any(|s| s.id == "S1-OC01" && s.status == Status::Done);
        if done {
            break;
        }
        assert!(
            Instant::now() < drain_deadline,
            "verifier never produced a DONE decision"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // One bus message (handoff S1-OC01 -> S2-OC01).
    let bus = daemon.message(
        "S1-OC01",
        "S2-OC01",
        "handoff",
        "Exporter cutover notes attached",
    );
    assert!(bus.ok, "bus message failed: {:?}", bus.error);

    // Attribution + a conflict on one shared file.
    daemon.record_attribution("S1-OC01", &["src/shared.rs".to_string()]);
    daemon.record_attribution("S2-OC01", &["src/shared.rs".to_string()]);

    // Pause SV-1 so the snapshot exercises the allowed-optional `paused`.
    let paused = daemon.pause_supervisor("SV-1");
    assert!(
        paused.is_ok(),
        "pause_supervisor failed: {}",
        paused.err().unwrap_or_default()
    );

    Fixture { daemon, fake, oc }
}

fn snapshot_value(f: &Fixture) -> Value {
    serde_json::to_value(f.daemon.snapshot()).unwrap()
}

/// Assert the fixture actually has the shape the contract test exercises —
/// otherwise the key-set assertions would pass vacuously on empty arrays.
fn assert_fixture_prereqs(v: &Value) {
    let supervisors = v["supervisors"].as_array().unwrap();
    assert_eq!(supervisors.len(), 2, "expected 2 supervisors: {v}");
    assert_eq!(supervisors[0]["id"], "SV-1");
    assert_eq!(supervisors[1]["id"], "SV-2");
    assert_eq!(
        supervisors[0]["paused"],
        Value::Bool(true),
        "SV-1 paused flag: {v}"
    );
    assert!(
        supervisors[1].get("paused").is_none(),
        "SV-2 must not carry `paused`"
    );

    let sessions = v["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 3, "expected 3 sessions: {v}");
    let ids: Vec<&str> = sessions.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["S1-OC01", "S1-OC02", "S2-OC01"]);

    let s1 = &sessions[0];
    assert_eq!(s1["status"], "done");
    assert_eq!(s1["failureReason"], Value::Null);
    assert_eq!(s1["goal"], "Ship the audit-signoff release checklist");
    assert_eq!(s1["definitionOfDone"], "cargo test passes on main");
    assert_eq!(s1["model"], "opencode/hy4-flash-free");
    assert_eq!(s1["iterations"], 1);
    assert_eq!(s1["maxIterations"], 8);
    assert_eq!(s1["wallClockDeadline"], T0 + RESTART_MS);
    // DONE now arrives through the verifier, so the last supervisor action is
    // the evidence-backed `done:` decision, not the earlier re-prompt.
    assert!(s1["lastSupervisorAction"]
        .as_str()
        .unwrap()
        .starts_with("done:"));

    let s2 = &sessions[1];
    // P3.8: `blocked` means waiting on a human, not a failed session. The
    // pane/temp dir are kept and the session stays idle with a needs_you
    // escalation — never a re-prompt into the permission dialog.
    assert_eq!(s2["status"], "idle");
    assert_eq!(s2["failureReason"], "blocked_waiting");

    let s3 = &sessions[2];
    assert_eq!(s3["status"], "working");

    assert!(
        !v["decisions"].as_array().unwrap().is_empty(),
        "decisions must be non-empty"
    );
    let actions: Vec<&str> = v["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["action"].as_str().unwrap())
        .collect();
    assert!(
        actions.contains(&"reprompt"),
        "expected a reprompt decision: {actions:?}"
    );
    assert!(
        actions.contains(&"escalate"),
        "expected an escalate decision: {actions:?}"
    );

    let escalations = v["escalations"].as_array().unwrap();
    assert_eq!(escalations.len(), 1, "escalations: {escalations:?}");
    assert_eq!(escalations[0]["kind"], "needs_you");
    assert_eq!(escalations[0]["session"], "S1-OC02");

    let bus = v["busMessages"].as_array().unwrap();
    assert_eq!(bus.len(), 1);
    assert_eq!(bus[0]["type"], "handoff");
    assert_eq!(bus[0]["from"], "S1-OC01");
    assert_eq!(bus[0]["to"], "S2-OC01");

    let conflicts = v["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["file"], "src/shared.rs");
    assert_eq!(
        conflicts[0]["sessions"],
        serde_json::json!(["S1-OC01", "S2-OC01"])
    );

    assert!(
        !v["attribution"].as_array().unwrap().is_empty(),
        "attribution empty"
    );
    assert!(
        !v["warnings"].as_array().unwrap().is_empty(),
        "warnings empty"
    );
    assert_eq!(v["orchestrator"]["status"], "working");
}

// ---------- Layer A tests ----------

#[test]
fn snapshot_object_key_sets_match_canonical_contract() {
    let fixture = build_fixture("keys");
    let v = snapshot_value(&fixture);
    assert_fixture_prereqs(&v);
    let violations = snapshot_key_violations(&v);
    assert!(
        violations.is_empty(),
        "snapshot violates the §1.3 key contract:\n{}",
        violations.join("\n")
    );
}

#[test]
fn snapshot_keys_are_camel_case_everywhere_no_snake_case() {
    let fixture = build_fixture("camel");
    let v = snapshot_value(&fixture);
    assert_fixture_prereqs(&v);
    let mut violations = Vec::new();
    walk_keys(&v, "$", &mut violations);
    assert!(
        violations.is_empty(),
        "snapshot contains non-camelCase keys:\n{}",
        violations.join("\n")
    );
}

#[test]
fn snapshot_status_enum_eps_f64_integer_ms_and_version() {
    let fixture = build_fixture("types");
    let v = snapshot_value(&fixture);
    assert_fixture_prereqs(&v);

    let violations = status_enum_violations(&v);
    assert!(
        violations.is_empty(),
        "status vocabulary violations:\n{}",
        violations.join("\n")
    );

    match v.get("eps") {
        Some(Value::Number(n)) => assert!(
            n.is_f64(),
            "eps must serialize as an f64 (got integer lexical form {n}); bridge.md §1.3 says `eps` is a float"
        ),
        other => panic!("eps missing or not a number: {other:?}"),
    }

    assert_eq!(
        v.get("version").and_then(Value::as_u64),
        Some(1),
        "version must be exactly 1"
    );

    let mut ms_violations = Vec::new();
    assert_integer_ms(v.get("generatedAt"), "generatedAt", &mut ms_violations);
    for (i, s) in v["sessions"].as_array().unwrap().iter().enumerate() {
        assert_integer_ms(
            s.get("startedAt"),
            &format!("sessions[{i}].startedAt"),
            &mut ms_violations,
        );
        if let Some(wcd) = s.get("wallClockDeadline") {
            assert_integer_ms(
                Some(wcd),
                &format!("sessions[{i}].wallClockDeadline"),
                &mut ms_violations,
            );
        }
    }
    for (i, d) in v["decisions"].as_array().unwrap().iter().enumerate() {
        assert_integer_ms(
            d.get("at"),
            &format!("decisions[{i}].at"),
            &mut ms_violations,
        );
    }
    for (i, e) in v["escalations"].as_array().unwrap().iter().enumerate() {
        assert_integer_ms(
            e.get("at"),
            &format!("escalations[{i}].at"),
            &mut ms_violations,
        );
    }
    for (i, m) in v["busMessages"].as_array().unwrap().iter().enumerate() {
        assert_integer_ms(
            m.get("at"),
            &format!("busMessages[{i}].at"),
            &mut ms_violations,
        );
    }
    assert!(
        ms_violations.is_empty(),
        "timestamp fields must be integer ms since epoch:\n{}",
        ms_violations.join("\n")
    );
}

/// A consumer written against v1.2 (bridge.md before the v1.3 bump) reading
/// ONLY v1.2 keys must parse a v1.3 snapshot without help. This is the
/// byte-compat forward test for the additive policy: any v1.3 field that was
/// removed, renamed or re-typed would fail here.
#[test]
fn v12_consumer_reads_only_v12_keys_on_v13_snapshot() {
    let fixture = build_fixture("v12");
    let v = snapshot_value(&fixture);
    assert_fixture_prereqs(&v);

    struct V12Session {
        id: String,
        sup: String,
        status: String,
        title: String,
        task: String,
        cwd: String,
        pane_id: String,
        started_at: u64,
        last_action: Option<String>,
        last_revision: Option<u64>,
        latency_ms: Option<u64>,
        failure_reason: Option<String>,
    }

    struct V12Summary {
        version: u64,
        orchestrator_id: String,
        orchestrator_status: String,
        supervisors: Vec<(String, String, u64, u64, u64, u64)>,
        sessions: Vec<V12Session>,
        eps: f64,
        warnings: Vec<String>,
    }

    fn opt_str(v: &Value) -> Option<String> {
        v.as_str().map(str::to_string)
    }
    fn opt_u64(v: &Value) -> Option<u64> {
        v.as_u64()
    }

    // The v1.2 consumer: touches exactly the keys bridge.md §1.3 froze at
    // v1.2. v1.3-only keys (goal, definitionOfDone, iterations,
    // maxIterations, wallClockDeadline, lastSupervisorAction, model,
    // decisions, escalations, busMessages, paused) are never read.
    fn parse_v12(v: &Value) -> Result<V12Summary, String> {
        let version = v["version"].as_u64().ok_or("version: missing/not u64")?;
        let _generated_at = v["generatedAt"]
            .as_u64()
            .ok_or("generatedAt: missing/not u64")?;
        let orchestrator_id = v["orchestrator"]["id"]
            .as_str()
            .ok_or("orchestrator.id")?
            .to_string();
        let orchestrator_status = v["orchestrator"]["status"]
            .as_str()
            .ok_or("orchestrator.status")?
            .to_string();

        let mut supervisors = Vec::new();
        for (i, sup) in v["supervisors"]
            .as_array()
            .ok_or("supervisors: missing/not array")?
            .iter()
            .enumerate()
        {
            supervisors.push((
                sup["id"]
                    .as_str()
                    .ok_or(format!("supervisors[{i}].id"))?
                    .to_string(),
                sup["status"]
                    .as_str()
                    .ok_or(format!("supervisors[{i}].status"))?
                    .to_string(),
                sup["working"]
                    .as_u64()
                    .ok_or(format!("supervisors[{i}].working"))?,
                sup["idle"]
                    .as_u64()
                    .ok_or(format!("supervisors[{i}].idle"))?,
                sup["failed"]
                    .as_u64()
                    .ok_or(format!("supervisors[{i}].failed"))?,
                sup["done"]
                    .as_u64()
                    .ok_or(format!("supervisors[{i}].done"))?,
            ));
        }

        let mut sessions = Vec::new();
        for (i, s) in v["sessions"]
            .as_array()
            .ok_or("sessions: missing/not array")?
            .iter()
            .enumerate()
        {
            sessions.push(V12Session {
                id: s["id"]
                    .as_str()
                    .ok_or(format!("sessions[{i}].id"))?
                    .to_string(),
                sup: s["sup"]
                    .as_str()
                    .ok_or(format!("sessions[{i}].sup"))?
                    .to_string(),
                status: s["status"]
                    .as_str()
                    .ok_or(format!("sessions[{i}].status"))?
                    .to_string(),
                title: s["title"]
                    .as_str()
                    .ok_or(format!("sessions[{i}].title"))?
                    .to_string(),
                task: s["task"]
                    .as_str()
                    .ok_or(format!("sessions[{i}].task"))?
                    .to_string(),
                cwd: s["cwd"]
                    .as_str()
                    .ok_or(format!("sessions[{i}].cwd"))?
                    .to_string(),
                pane_id: s["paneId"]
                    .as_str()
                    .ok_or(format!("sessions[{i}].paneId"))?
                    .to_string(),
                started_at: s["startedAt"]
                    .as_u64()
                    .ok_or(format!("sessions[{i}].startedAt"))?,
                last_action: opt_str(&s["lastAction"]),
                last_revision: opt_u64(&s["lastRevision"]),
                latency_ms: opt_u64(&s["latencyMs"]),
                failure_reason: opt_str(&s["failureReason"]),
            });
        }

        let eps = v["eps"].as_f64().ok_or("eps: missing/not f64")?;

        for (i, c) in v["conflicts"]
            .as_array()
            .ok_or("conflicts: missing/not array")?
            .iter()
            .enumerate()
        {
            let _file = c["file"].as_str().ok_or(format!("conflicts[{i}].file"))?;
            for (j, sid) in c["sessions"]
                .as_array()
                .ok_or(format!("conflicts[{i}].sessions"))?
                .iter()
                .enumerate()
            {
                let _ = sid
                    .as_str()
                    .ok_or(format!("conflicts[{i}].sessions[{j}]"))?;
            }
        }
        for (i, a) in v["attribution"]
            .as_array()
            .ok_or("attribution: missing/not array")?
            .iter()
            .enumerate()
        {
            let _ = a["session"]
                .as_str()
                .ok_or(format!("attribution[{i}].session"))?;
            let _ = a["file"].as_str().ok_or(format!("attribution[{i}].file"))?;
        }

        let mut warnings = Vec::new();
        for (i, w) in v["warnings"]
            .as_array()
            .ok_or("warnings: missing/not array")?
            .iter()
            .enumerate()
        {
            warnings.push(w.as_str().ok_or(format!("warnings[{i}]"))?.to_string());
        }

        Ok(V12Summary {
            version,
            orchestrator_id,
            orchestrator_status,
            supervisors,
            sessions,
            eps,
            warnings,
        })
    }

    let summary =
        parse_v12(&v).unwrap_or_else(|e| panic!("v1.2 consumer failed on v1.3 snapshot: {e}"));
    assert_eq!(summary.version, 1);
    assert_eq!(summary.orchestrator_id, "ORCH");
    assert!(STATUS_ENUM.contains(&summary.orchestrator_status.as_str()));
    assert_eq!(summary.supervisors.len(), 2);
    assert_eq!(summary.supervisors[0].0, "SV-1");
    assert_eq!(summary.supervisors[1].0, "SV-2");
    assert_eq!(summary.sessions.len(), 3);
    assert_eq!(summary.sessions[0].id, "S1-OC01");
    assert_eq!(summary.sessions[0].status, "done");
    assert_eq!(summary.sessions[0].sup, "SV-1");
    assert!(
        summary.sessions[0].cwd.ends_with("/repo"),
        "cwd: {}",
        summary.sessions[0].cwd
    );
    assert!(summary.sessions[0].pane_id.starts_with('w'));
    assert_eq!(summary.sessions[0].started_at, T0);
    assert!(summary.sessions[0].last_revision.is_some());
    assert_eq!(
        summary.sessions[0].title,
        "Ship the audit-signoff release checklist"
    );
    assert!(
        !summary.sessions[0].task.contains('\n'),
        "task must be single line"
    );
    assert!(summary.sessions[0].task.contains("audit-signoff"));
    let _ = summary.sessions[0].last_action; // presence/type enforced by the parser (string-or-null)
    let _ = summary.sessions[0].latency_ms; // presence/type enforced by the parser (u64-or-null)
    assert_eq!(summary.sessions[1].id, "S1-OC02");
    assert_eq!(
        summary.sessions[1].failure_reason.as_deref(),
        Some("blocked_waiting")
    );
    assert_eq!(summary.sessions[2].id, "S2-OC01");
    assert!(summary.eps.is_finite() && summary.eps >= 0.0);
    assert!(!summary.warnings.is_empty());

    // The additive policy is what makes this work: v1.3 fields are present
    // (data exists) but the v1.2 consumer never needed them.
    assert!(v.get("decisions").is_some());
    assert!(v.get("escalations").is_some());
    assert!(v.get("busMessages").is_some());
}

#[test]
fn socket_ack_envelopes_match_contract_v13() {
    let fixture = build_fixture("socket");
    let daemon = Arc::new(fixture.daemon);
    let _ = fixture.fake; // keep the fake alive for the duration of the test
    let _ = fixture.oc;

    let sock = short_sock("ack");
    let _ = std::fs::remove_file(&sock);
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let server = {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || notchd::socket::serve_with_shutdown(listener, daemon, shutdown))
    };

    let stream = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    let send = |writer: &mut std::os::unix::net::UnixStream, obj: Value| {
        writeln!(writer, "{obj}").unwrap();
        writer.flush().unwrap();
    };
    let next_frame = |reader: &mut BufReader<std::os::unix::net::UnixStream>| -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).unwrap()
    };

    // §2: the server speaks first — { "event": "hello", "version": 1, "pid": n }.
    let hello = next_frame(&mut reader);
    assert_eq!(hello["event"], "hello", "hello frame: {hello}");
    assert_eq!(hello["version"], 1, "hello version: {hello}");
    assert!(
        hello["pid"].as_u64().is_some(),
        "hello pid must be an integer: {hello}"
    );

    // ping → { "event": "ack", "cmd": "ping", "ok": true, "pong": <ms> }
    send(&mut writer, serde_json::json!({"cmd": "ping"}));
    let pong = next_frame(&mut reader);
    assert_eq!(pong["event"], "ack", "ping ack: {pong}");
    assert_eq!(pong["cmd"], "ping", "ping cmd: {pong}");
    assert!(pong["ok"].is_boolean(), "ping ok must be a bool: {pong}");
    assert_eq!(pong["ok"], Value::Bool(true), "ping ok: {pong}");
    assert!(
        pong["pong"].as_u64().is_some(),
        "ping pong must be integer ms: {pong}"
    );

    // snapshot → { "event": "snapshot", "snapshot": {…} } — not an ack; the
    // embedded object must satisfy the full §1.3 key contract.
    send(&mut writer, serde_json::json!({"cmd": "snapshot"}));
    let snap_frame = next_frame(&mut reader);
    assert_eq!(
        snap_frame["event"], "snapshot",
        "snapshot frame: {snap_frame}"
    );
    assert!(
        snap_frame.get("cmd").is_none(),
        "snapshot frame must not be an ack"
    );
    assert!(
        snap_frame.get("ok").is_none(),
        "snapshot frame must not carry ok"
    );
    let wire_snap = &snap_frame["snapshot"];
    assert_eq!(
        wire_snap["version"], 1,
        "wire snapshot version: {wire_snap}"
    );
    let wire_violations = snapshot_key_violations(wire_snap);
    assert!(
        wire_violations.is_empty(),
        "wire snapshot violates the §1.3 key contract:\n{}",
        wire_violations.join("\n")
    );

    // kill with an id the daemon does not know → error ack.
    send(
        &mut writer,
        serde_json::json!({"cmd": "kill", "id": "S9-OC99", "reason": "conformance probe"}),
    );
    let kill = next_frame(&mut reader);
    assert_eq!(kill["event"], "ack", "kill ack: {kill}");
    assert_eq!(kill["cmd"], "kill", "kill cmd: {kill}");
    assert!(kill["ok"].is_boolean(), "kill ok must be a bool: {kill}");
    assert_eq!(
        kill["ok"],
        Value::Bool(false),
        "kill with bad id must fail: {kill}"
    );
    assert!(
        kill["error"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "kill error ack must carry a non-empty error string: {kill}"
    );

    // message with an illegal `type` → error ack (validated before delivery).
    send(
        &mut writer,
        serde_json::json!({
            "cmd": "message",
            "from": "S1-OC01",
            "to": "S2-OC01",
            "type": "bogus",
            "body": "conformance probe"
        }),
    );
    let msg = next_frame(&mut reader);
    assert_eq!(msg["event"], "ack", "message ack: {msg}");
    assert_eq!(msg["cmd"], "message", "message cmd: {msg}");
    assert!(msg["ok"].is_boolean(), "message ok must be a bool: {msg}");
    assert_eq!(
        msg["ok"],
        Value::Bool(false),
        "message with bad type must fail: {msg}"
    );
    assert!(
        msg["error"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "message error ack must carry a non-empty error string: {msg}"
    );
    assert!(
        msg.get("messageId").is_none(),
        "failed message ack must not carry messageId: {msg}"
    );

    // Unknown command → error ack echoing the command name.
    send(&mut writer, serde_json::json!({"cmd": "frobnicate"}));
    let unknown = next_frame(&mut reader);
    assert_eq!(unknown["event"], "ack", "unknown ack: {unknown}");
    assert_eq!(unknown["cmd"], "frobnicate", "unknown cmd echo: {unknown}");
    assert!(
        unknown["ok"].is_boolean(),
        "unknown ok must be a bool: {unknown}"
    );
    assert_eq!(
        unknown["ok"],
        Value::Bool(false),
        "unknown command must fail: {unknown}"
    );
    assert!(
        unknown["error"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "unknown command ack must carry a non-empty error string: {unknown}"
    );

    shutdown.store(true, Ordering::SeqCst);
    drop(writer);
    let _ = server.join();
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn internal_sanity_status_enum_matches_design_md() {
    // Guards the STATUS_ENUM constant against drift from the DESIGN.md
    // vocabulary: exactly working|idle|failed|done, nothing else.
    assert_eq!(STATUS_ENUM, &["working", "idle", "failed", "done"]);
    assert_eq!(Status::Working.as_str(), "working");
    assert_eq!(Status::Idle.as_str(), "idle");
    assert_eq!(Status::Failed.as_str(), "failed");
    assert_eq!(Status::Done.as_str(), "done");
}
