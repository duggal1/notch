//! PERF agent — decider performance + correctness tests.
//!
//! All external CLIs are scripted through their injectable runners (no real
//! opencode / herdr subprocesses, no real sleeps except the explicit
//! parallel-timing probe). The fake `opencode` records every argv so the tests
//! can assert the exact decision path that fired:
//!   - fast path        -> ZERO `run` calls
//!   - persistent path  -> all `run` calls carry the SAME `--session <id>`
//!   - fallback path    -> persistent attempt + cold retry, logged as `perf`
//!   - fan-out          -> max concurrent judgments <= 4 and > 1
//!
//! Also covers the unit-level grammar via `notchd::decider` directly.

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;
use notchd::status::Status;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const T0: u64 = 1_763_520_000_000;
static TMP_N: AtomicU64 = AtomicU64::new(0);
/// `decider::decision_stats()` is process-global; serialize tests that inspect it.
static TEST_LOCK: Mutex<()> = Mutex::new(());
fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn tmp_root(tag: &str) -> PathBuf {
    let n = TMP_N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-perf-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn test_config(root: &Path) -> Config {
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 60,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

// ----------------------------- fake herdr ---------------------------------

struct FakeHerdr {
    panes: Mutex<Vec<String>>,
    prompts: Mutex<Vec<(String, String)>>,
    tail: Mutex<String>,
    idle: Mutex<bool>,
}

impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            panes: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            tail: Mutex::new("last line of pane output\n".to_string()),
            idle: Mutex::new(true),
        })
    }

    fn set_idle(&self, idle: bool) {
        *self.idle.lock().unwrap() = idle;
    }

    fn prompts(&self) -> Vec<(String, String)> {
        self.prompts.lock().unwrap().clone()
    }

    fn agent_entry(&self, pane: &str) -> Value {
        let status = if *self.idle.lock().unwrap() { "idle" } else { "working" };
        serde_json::json!({
            "agent": "opencode",
            "agent_status": status,
            "agent_session": {"agent": "opencode", "kind": "id", "source": "herdr:opencode", "value": format!("ses_{pane}")},
            "cwd": "/tmp",
            "focused": false,
            "pane_id": pane,
            "revision": 1,
            "terminal_title": format!("OC | {pane}"),
            "tab_id": "w1:t1",
            "workspace_id": "w1",
            "screen_detection_skipped": true
        })
    }

    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| {
            let sub = args.get(1).map(String::as_str).unwrap_or("");
            let sub2 = args.get(2).map(String::as_str).unwrap_or("");
            match (sub, sub2) {
                ("agent", "list") => {
                    let agents: Vec<Value> = fake
                        .panes
                        .lock()
                        .unwrap()
                        .clone()
                        .iter()
                        .map(|p| fake.agent_entry(p))
                        .collect();
                    Ok(serde_json::json!({
                        "id": "cli:agent:list",
                        "result": {"type": "agent_list", "agents": agents}
                    })
                    .to_string())
                }
                ("pane", "split") => {
                    let mut p = fake.panes.lock().unwrap();
                    let pane = format!("wT:p{}", p.len() + 1);
                    p.push(pane.clone());
                    Ok(serde_json::json!({
                        "id": "cli:pane:split",
                        "result": {"type": "pane_info", "pane": {
                            "pane_id": pane, "tab_id": "w1:t1", "workspace_id": "w1"
                        }}
                    })
                    .to_string())
                }
                ("agent", "start") => Ok(
                    r#"{"id":"cli:agent:start","result":{"type":"agent_started"}}"#.to_string()
                ),
                ("agent", "prompt") => {
                    let target = args.get(3).cloned().unwrap_or_default();
                    let text = args.get(4).cloned().unwrap_or_default();
                    fake.prompts.lock().unwrap().push((target, text));
                    Ok(r#"{"id":"cli:agent:prompt","result":{"type":"agent_prompted"}}"#.to_string())
                }
                ("agent", "read") => Ok(fake.tail.lock().unwrap().clone()),
                ("pane", "close") | ("agent", "focus") | ("agent", "wait") => Ok(
                    r#"{"id":"cli:ok","result":{"type":"ok"}}"#.to_string()
                ),
                _ => Err(HerdrError::Failed {
                    args: args.to_vec(),
                    code: Some(2),
                    stderr: format!("unexpected herdr call: {}", args.join(" ")),
                }),
            }
        })
    }
}

// --------------------------- fake opencode --------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Healthy,
    PersistentFails,
    Slow { ms: u64 },
}

struct FakeOpencode {
    calls: Mutex<Vec<Vec<String>>>,
    mode: Mutex<RunMode>,
    concurrent: AtomicUsize,
    max_concurrent: AtomicUsize,
}

impl FakeOpencode {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            mode: Mutex::new(RunMode::Healthy),
            concurrent: AtomicUsize::new(0),
            max_concurrent: AtomicUsize::new(0),
        })
    }

    fn set_mode(&self, mode: RunMode) {
        *self.mode.lock().unwrap() = mode;
    }

    fn opencode(self: &Arc<Self>) -> Opencode {
        let fake = Arc::clone(self);
        Opencode::with_runner(move |args: &[String]| {
            fake.calls.lock().unwrap().push(args.to_vec());
            let joined = args[1..].join(" ");
            if joined == "api GET /api/session" {
                return Ok(r#"{"cursor":null,"data":[]}"#.to_string());
            }
            if joined == "models" {
                return Ok("opencode/hy4-flash-free".to_string());
            }
            if joined.starts_with("run --model") {
                let live = fake.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                fake.max_concurrent.fetch_max(live, Ordering::SeqCst);
                let mode = *fake.mode.lock().unwrap();
                if let RunMode::Slow { ms } = mode {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                fake.concurrent.fetch_sub(1, Ordering::SeqCst);
                if mode == RunMode::PersistentFails && joined.contains("--session") {
                    return Err(HerdrError::Failed {
                        args: args.to_vec(),
                        code: Some(1),
                        stderr: "simulated persistent-session failure".to_string(),
                    });
                }
                return Ok("CONTINUE: keep going and verify with a concrete command".to_string());
            }
            Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected opencode call: {joined}"),
            })
        })
    }

    fn run_calls(&self) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.get(1).map(String::as_str) == Some("run"))
            .cloned()
            .collect()
    }

    fn max_concurrent(&self) -> usize {
        self.max_concurrent.load(Ordering::SeqCst)
    }
}

// ------------------------------ helpers -----------------------------------

fn build(root: &Path) -> (Arc<FakeHerdr>, Arc<FakeOpencode>, Daemon, Arc<FakeClock>) {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/bin/true");
    let config = test_config(root);
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    let herdr = FakeHerdr::new();
    let oc = FakeOpencode::new();
    let clock = Arc::new(FakeClock::new(T0));
    let daemon = Daemon::open(config, herdr.herdr(), clock.clone()).unwrap();
    daemon.set_opencode(oc.opencode());
    (herdr, oc, daemon, clock)
}

fn submit(daemon: &Daemon, cwd: &Path, dod: &str) -> String {
    let ack = daemon.submit_task(
        format!("do work for {}", cwd.file_name().unwrap().to_string_lossy()),
        Some(cwd.to_string_lossy().to_string()),
        None,
        Some(dod.to_string()),
        None,
        None,
    );
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);
    ack.session_id.unwrap()
}

fn events(root: &Path) -> Vec<Value> {
    let s = std::fs::read_to_string(root.join("state").join("events.jsonl")).unwrap_or_default();
    s.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn session_status(daemon: &Daemon, id: &str) -> Status {
    daemon
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == id)
        .unwrap()
        .status
}

fn iterations(daemon: &Daemon, id: &str) -> u32 {
    daemon
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == id)
        .unwrap()
        .iterations
        .unwrap_or(0)
}

// -------------------------------- tests -----------------------------------

/// A locally checkable DoD is decided with ZERO decider calls.
#[test]
fn fast_path_satisfied_marks_done_with_zero_decider_calls() {
    let _guard = test_lock();
    notchd::decider::reset_decision_stats();
    let root = tmp_root("fast-done");
    let cwd = root.join("work");
    std::fs::create_dir_all(cwd.join("tests")).unwrap();
    std::fs::write(cwd.join("tests/a.rs"), "fn test_jsonify() {}").unwrap();
    let (_herdr, oc, daemon, _clock) = build(&root);
    let id = submit(
        &daemon,
        &cwd,
        "file:tests/a.rs exists\nfile:tests/a.rs contains:test_jsonify",
    );

    daemon.poll_once();

    assert_eq!(oc.run_calls().len(), 0, "fast path must make ZERO decider calls");
    assert_eq!(session_status(&daemon, &id), Status::Done, "local evidence satisfied");
    let ev = events(&root);
    assert!(
        ev.iter().any(|e| e["kind"] == "perf"
            && e["detail"].as_str().unwrap_or("").contains("path=fast")),
        "fast path must be logged: {ev:?}"
    );
    let report = notchd::decider::decision_stats_report();
    assert!(report.contains("fast=1"), "fast sample recorded: {report}");
    assert!(report.contains("persistent=0"), "no persistent sample: {report}");
}

/// A failed local check is a concrete CONTINUE, still with ZERO decider calls.
#[test]
fn fast_path_missing_reprompts_with_specific_evidence_and_zero_calls() {
    let _guard = test_lock();
    let root = tmp_root("fast-missing");
    let cwd = root.join("work");
    std::fs::create_dir_all(cwd.join("tests")).unwrap();
    // File exists but lacks the required marker -> concrete marker-mismatch.
    std::fs::write(cwd.join("tests/missing.rs"), "fn unrelated() {}").unwrap();
    let (herdr, oc, daemon, _clock) = build(&root);
    let id = submit(&daemon, &cwd, "file:tests/missing.rs contains:test_jsonify");

    daemon.poll_once();

    assert_eq!(oc.run_calls().len(), 0, "fast path must make ZERO decider calls");
    assert_eq!(session_status(&daemon, &id), Status::Working, "re-prompted");
    assert_eq!(iterations(&daemon, &id), 1, "one re-prompt counted");
    let prompts = herdr.prompts();
    let last = prompts.last().expect("a re-prompt was issued").1.clone();
    assert!(last.contains("tests/missing.rs"), "concrete missing file: {last}");
    assert!(last.contains("test_jsonify"), "concrete missing marker: {last}");
}

/// Every non-fast judgment reuses ONE decider session id, across polls.
#[test]
fn persistent_path_reuses_same_session_id_across_calls() {
    let _guard = test_lock();
    let root = tmp_root("persist");
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let (herdr, oc, daemon, clock) = build(&root);
    let _id = submit(&daemon, &cwd, "cargo test passes on main"); // not locally checkable

    for _ in 0..3 {
        herdr.set_idle(true);
        daemon.poll_once();
        clock.advance_ms(2_000);
    }

    let calls = oc.run_calls();
    assert_eq!(calls.len(), 3, "one judgment per poll: {calls:?}");
    let sids: Vec<String> = calls
        .iter()
        .map(|c| {
            c.windows(2)
                .find(|w| w[0] == "--session")
                .map(|w| w[1].clone())
                .unwrap_or_default()
        })
        .collect();
    assert!(sids.iter().all(|s| !s.is_empty()), "every call must carry --session: {calls:?}");
    assert!(sids.iter().all(|s| s == &sids[0]), "ONE session id reused: {sids:?}");
    assert!(sids[0].starts_with("ses"), "opencode-compatible id: {}", sids[0]);
    let ev = events(&root);
    assert!(
        ev.iter().any(|e| e["kind"] == "perf"
            && e["detail"].as_str().unwrap_or("").contains("path=persistent")),
        "persistent path logged: {ev:?}"
    );
}

/// If session reuse fails, the cold path fires and the fallback is logged.
#[test]
fn fallback_to_cold_run_is_logged() {
    let _guard = test_lock();
    notchd::decider::reset_decision_stats();
    let root = tmp_root("fallback");
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let (_herdr, oc, daemon, _clock) = build(&root);
    let _id = submit(&daemon, &cwd, "cargo test passes on main");
    oc.set_mode(RunMode::PersistentFails);

    daemon.poll_once();

    let calls = oc.run_calls();
    assert_eq!(calls.len(), 2, "persistent attempt + cold fallback: {calls:?}");
    assert!(calls[0].windows(2).any(|w| w[0] == "--session"), "first is persistent");
    assert!(!calls[1].windows(2).any(|w| w[0] == "--session"), "second is cold");
    let ev = events(&root);
    assert!(
        ev.iter().any(|e| e["kind"] == "perf"
            && e["detail"].as_str().unwrap_or("").contains("path=cold_fallback")),
        "fallback path must be logged: {ev:?}"
    );
    let report = notchd::decider::decision_stats_report();
    assert!(report.contains("cold_fallback=1"), "cold sample recorded: {report}");
}

/// Judgments for several idle sessions fan out to <= 4 bounded threads.
#[test]
fn parallel_judgments_are_bounded_and_concurrent() {
    let _guard = test_lock();
    let root = tmp_root("parallel");
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let (_herdr, oc, daemon, _clock) = build(&root);
    for _ in 0..6 {
        submit(&daemon, &cwd, "cargo test passes on main");
    }
    oc.set_mode(RunMode::Slow { ms: 120 });

    let start = std::time::Instant::now();
    daemon.poll_once();
    let elapsed = start.elapsed();

    let calls = oc.run_calls();
    assert_eq!(calls.len(), 6, "one judgment per idle session: {calls:?}");
    let peak = oc.max_concurrent();
    assert!(peak <= 4, "bounded at 4, saw {peak}");
    assert!(peak > 1, "actually parallel, saw {peak}");
    // 6 * 120ms serial = 720ms; 4-wide should be ~240ms plus poll overhead.
    println!(
        "perf fan-out: 6 judgments, peak_concurrency={peak}, wall={}ms (serial would be ~720ms)",
        elapsed.as_millis()
    );
    assert!(elapsed < Duration::from_millis(650), "fan-out too slow: {elapsed:?}");
    // All six reused the same session id.
    let sids: std::collections::HashSet<String> = calls
        .iter()
        .map(|c| {
            c.windows(2)
                .find(|w| w[0] == "--session")
                .map(|w| w[1].clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(sids.len(), 1, "one shared decider session: {sids:?}");
}

/// Pause / budget / working-session rules still gate before any model call.
#[test]
fn gates_still_precede_decider_calls() {
    let _guard = test_lock();
    let root = tmp_root("gates");
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let (herdr, oc, daemon, _clock) = build(&root);
    let id = submit(&daemon, &cwd, "cargo test passes on main");

    // A working session must never be re-prompted or judged.
    herdr.set_idle(false);
    daemon.poll_once();
    assert_eq!(oc.run_calls().len(), 0, "working session: no judgment");
    assert_eq!(session_status(&daemon, &id), Status::Working);

    // Paused supervisor: no judgment either.
    herdr.set_idle(true);
    let sup = daemon.snapshot().sessions[0].sup.clone();
    assert!(daemon.pause_supervisor(&sup).is_ok());
    daemon.poll_once();
    assert_eq!(oc.run_calls().len(), 0, "paused supervisor: no judgment");
}

/// Ignored benchmark: exercises all three real decision paths and prints the
/// p50/p95 recorded by the production instrumentation. Run with
/// `cargo test -p notchd --test perf_decider -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark: prints per-path p50/p95"]
fn bench_after_per_path_p50_p95() {
    let _guard = test_lock();
    notchd::decider::reset_decision_stats();

    // Fast path: 10 sessions with locally checkable DoDs, zero calls.
    {
        let root = tmp_root("bench-fast");
        let cwd = root.join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("marker.txt"), "READY").unwrap();
        let (_h, oc, daemon, _c) = build(&root);
        for _ in 0..10 {
            submit(&daemon, &cwd, "file:marker.txt contains:READY");
        }
        daemon.poll_once();
        assert_eq!(oc.run_calls().len(), 0);
    }

    // Persistent path: 10 judgments on one reused session, 120 ms simulated
    // inference each (the shim stands in for the real model call).
    {
        let root = tmp_root("bench-persist");
        let cwd = root.join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let (_h, oc, daemon, _clock) = build(&root);
        for _ in 0..10 {
            submit(&daemon, &cwd, "cargo test passes on main");
        }
        oc.set_mode(RunMode::Slow { ms: 120 });
        daemon.poll_once();
    }

    // Fallback path: persistent fails, cold retry fires.
    {
        let root = tmp_root("bench-fallback");
        let cwd = root.join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let (_h, oc, daemon, _c) = build(&root);
        submit(&daemon, &cwd, "cargo test passes on main");
        oc.set_mode(RunMode::PersistentFails);
        daemon.poll_once();
    }

    println!("AFTER {}", notchd::decider::decision_stats_report());
}

/// Grammar smoke via the public API (mirrors decider.rs unit tests).
#[test]
fn local_check_grammar_public_api() {
    let _guard = test_lock();
    use notchd::decider::{evaluate_local_dod, LocalCheckOutcome};
    let root = tmp_root("grammar");
    std::fs::write(root.join("f.txt"), "hello").unwrap();
    let cwd = root.to_string_lossy();
    assert!(matches!(
        evaluate_local_dod(Some("file:f.txt exists"), &cwd, ""),
        LocalCheckOutcome::Satisfied { .. }
    ));
    assert!(matches!(
        evaluate_local_dod(Some("output contains:READY"), &cwd, "status READY"),
        LocalCheckOutcome::Satisfied { .. }
    ));
    assert!(matches!(
        evaluate_local_dod(Some("just trust me"), &cwd, ""),
        LocalCheckOutcome::NotRunnable
    ));
}
