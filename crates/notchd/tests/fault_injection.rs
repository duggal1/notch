//! Fault-injection tests for the notchd daemon (fault_* suite).
//!
//! Every test spawns the real `notchd serve` binary with its herdr/opencode
//! seams pointed at the zsh shims in `tests/fixtures/shims/`. Each shim is a
//! scripted double driven by env vars (see the shim headers for the full
//! contract). Failure classes per shim:
//!
//!   healthy | exit1 | hang (sleep 45) | bad_json | http429 | http5xx | empty
//!   | wrong_schema
//!
//! Both shims additionally read `$SHIM_STATE_DIR/mode` (global) and
//! `$SHIM_STATE_DIR/<subkey>.mode` (per subcommand, e.g. `api_session.mode`,
//! `agent_list.mode`) on EVERY invocation — that is the "shim's own env file"
//! a test rewrites to swap behavior mid-run (healthy <-> fault).
//!
//! Assertions here are evidence-based: they observe the daemon only through
//! its NDJSON socket, its snapshot, and its events.jsonl — the same surfaces a
//! real client uses. No src/ code is edited by this suite.
//!
//! Known gaps (documented in the fault-injection report; each is a
//! finding, NOT a patch — src/ is off limits here):
//!
//!   * `opencode api GET /api/session` has NO timeout anywhere; a hung opencode
//!     wedges the poll thread for the child's full lifetime (test 2b, ignored).
//!   * A failed /api/session probe produces NO warning in snapshot.warnings;
//!     the fallback path is silent (test 1b, ignored).
//!   * A 429 emitted by the decider call itself is parsed as Unparseable and
//!     consumes an iteration; only the herdr pane tail triggers rate-limit
//!     backoff (test 4b documents this).
//!
//! Run: cargo test -p notchd --test fault_injection -- --test-threads=4
//! (each test spawns its own daemon on its own socket/state; all are
//! independent and parallel-safe).

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------- fixture plumbing ----------

static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn herdr_shim() -> String {
    fixtures_dir().join("shims").join("herdr").to_string_lossy().into_owned()
}

fn opencode_shim() -> String {
    fixtures_dir().join("shims").join("opencode").to_string_lossy().into_owned()
}

/// One running daemon under test. Killed on drop (SIGKILL) no matter how the
/// test ends. The per-test root is `$TMPDIR/notchd-fault-<pid>-<n>/`.
struct TestDaemon {
    child: Child,
    root: PathBuf,
    state_dir: PathBuf,
    shim_dir: PathBuf,
    sock: PathBuf,
    cwd: PathBuf,
    logs: Arc<Mutex<String>>,
}

impl TestDaemon {
    /// Spawn `notchd serve`. `env_extra` is layered over the baseline env;
    /// `files` are written into SHIM_STATE_DIR before the daemon starts
    /// (e.g. dynamic-mode files, `api_state`).
    fn spawn(tag: &str, env_extra: &[(&str, &str)], files: &[(&str, &str)]) -> TestDaemon {
        let n = FIXTURE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("notchd-fault-{tag}-{}-{n}", std::process::id()));
        let state_dir = root.join("state");
        let shim_dir = root.join("shim");
        let cwd = root.join("cwd");
        std::fs::create_dir_all(&state_dir).expect("create state dir");
        std::fs::create_dir_all(&shim_dir).expect("create shim dir");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        for (name, body) in files {
            std::fs::write(shim_dir.join(name), body).expect("write shim file");
        }
        let sock = PathBuf::from(format!("/tmp/nd-fi-{}-{n}.sock", std::process::id()));
        let logs = Arc::new(Mutex::new(String::new()));

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_notchd"));
        cmd.arg("serve")
            .env("NOTCH_STATE_DIR", &state_dir)
            .env("NOTCH_SOCK", &sock)
            .env("NOTCH_HERDR_CMD", herdr_shim())
            .env("NOTCH_OPENCODE_CMD", opencode_shim())
            .env("SHIM_STATE_DIR", &shim_dir)
            .env("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true") // never touch the real opencode config
            .env("NOTCH_POLL_INTERVAL_MS", "250")
            .env("NOTCH_STUCK_AFTER_SECS", "3600")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env_extra {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap_or_else(|e| panic!("spawn notchd serve: {e}"));
        // Drain stdout/stderr so the child never blocks on a full pipe; keep
        // the tail in memory for debugging.
        macro_rules! drain {
            ($stream:expr) => {{
                let logs = Arc::clone(&logs);
                std::thread::spawn(move || {
                    let mut buf = String::new();
                    let mut reader = std::io::BufReader::new($stream);
                    loop {
                        use std::io::Read;
                        let mut chunk = [0u8; 1024];
                        match reader.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                                if buf.len() > 16 * 1024 {
                                    let excess = buf.len() - 16 * 1024;
                                    buf = buf[excess..].to_string();
                                }
                            }
                        }
                    }
                    *logs.lock().unwrap() = buf;
                });
            }};
        }
        if let Some(s) = child.stdout.take() {
            drain!(s);
        }
        if let Some(s) = child.stderr.take() {
            drain!(s);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if sock.exists() && UnixStream::connect(&sock).is_ok() {
                break;
            }
            if Instant::now() > deadline {
                panic!("daemon did not bind {} within 10s; logs: {}", sock.display(), logs.lock().unwrap());
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        TestDaemon { child, root, state_dir, shim_dir, sock, cwd, logs }
    }

    fn logs(&self) -> String {
        self.logs.lock().unwrap().clone()
    }

    /// Write/overwrite a file in the shim state dir (dynamic mode swap).
    fn shim_write(&self, name: &str, contents: &str) {
        std::fs::write(self.shim_dir.join(name), contents).expect("shim_write");
    }

    fn shim_read(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.shim_dir.join(name))
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn shim_count_lines(&self, name: &str) -> usize {
        self.shim_read(name).len()
    }

    /// One socket round-trip: hello, then `cmd`, read its reply line.
    /// Returns (hello, reply).
    fn request(&self, cmd: Value) -> (Value, Value) {
        socket_request(&self.sock, cmd).unwrap_or_else(|e| {
            panic!("socket request failed against {}: {e}\ndaemon logs: {}", self.sock.display(), self.logs())
        })
    }

    fn ping(&self) -> (bool, u128) {
        let t = Instant::now();
        match socket_request(&self.sock, json!({"cmd": "ping"})) {
            Ok((_hello, ack)) => {
                let ok = ack.get("ok").and_then(Value::as_bool).unwrap_or(false);
                (ok, t.elapsed().as_millis())
            }
            Err(_) => (false, t.elapsed().as_millis()),
        }
    }

    fn snapshot(&self) -> Value {
        let (_hello, reply) = self.request(json!({"cmd": "snapshot"}));
        reply.get("snapshot")
            .cloned()
            .unwrap_or_else(|| panic!("snapshot reply had no snapshot object: {reply}"))
    }

    fn submit_task(&self, task: &str, supervisor_id: Option<&str>) -> Value {
        let mut cmd = json!({"cmd": "submit_task", "task": task, "cwd": self.cwd});
        if let Some(s) = supervisor_id {
            cmd["supervisorId"] = json!(s);
        }
        let (_hello, ack) = self.request(cmd);
        ack
    }

    fn logs_cmd(&self, id: &str, tail: u64) -> Value {
        let (_hello, reply) = self.request(json!({"cmd": "logs", "id": id, "tail": tail}));
        reply
    }

    fn events(&self) -> Vec<Value> {
        read_events(&self.state_dir)
    }

    /// Kill the daemon and reap it (consuming). Idempotent for Drop.
    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock);
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock);
    }
}

/// Connect, read the hello line, write one command, read one reply line.
fn socket_request(sock: &Path, cmd: Value) -> std::io::Result<(Value, Value)> {
    let stream = UnixStream::connect(sock)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let hello = read_json_line(&mut reader)?;
    let mut line = cmd.to_string();
    line.push('\n');
    stream.try_clone()?.write_all(line.as_bytes())?;
    let reply = read_json_line(&mut reader)?;
    Ok((hello, reply))
}

fn read_json_line(reader: &mut BufReader<UnixStream>) -> std::io::Result<Value> {
    let mut buf = String::new();
    match reader.read_line(&mut buf) {
        Ok(0) => Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "daemon closed the socket")),
        Ok(_) => serde_json::from_str(buf.trim())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bad JSON line: {e}: {buf}"))),
        Err(e) => Err(e),
    }
}

fn read_events(state_dir: &Path) -> Vec<Value> {
    let path = state_dir.join("events.jsonl");
    std::fs::read_to_string(&path)
        .map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

fn wait_until<F: FnMut() -> bool>(timeout: Duration, mut pred: F) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn session<'a>(snap: &'a Value, id: &str) -> &'a Value {
    snap["sessions"]
        .as_array()
        .unwrap_or_else(|| panic!("snapshot.sessions missing: {snap}"))
        .iter()
        .find(|s| s["id"] == json!(id))
        .unwrap_or_else(|| panic!("session {id} not in snapshot: {snap}"))
}

fn sessions(snap: &Value) -> &Vec<Value> {
    snap["sessions"].as_array().unwrap_or_else(|| panic!("snapshot.sessions missing: {snap}"))
}

fn decisions(snap: &Value) -> Vec<Value> {
    snap["decisions"].as_array().cloned().unwrap_or_default()
}

fn escalations(snap: &Value) -> Vec<Value> {
    snap["escalations"].as_array().cloned().unwrap_or_default()
}

fn warnings(snap: &Value) -> Vec<String> {
    snap["warnings"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Ping the daemon `count` times at `step` intervals; returns one
/// (ok, latency_ms) pair per ping. Runs the whole loop before returning so it
/// is safe to call from a dedicated thread.
fn ping_loop(sock: &Path, count: usize, step: Duration) -> Vec<(bool, u128)> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let t = Instant::now();
        match socket_request(sock, json!({"cmd": "ping"})) {
            Ok((_h, ack)) => {
                out.push((ack.get("ok").and_then(Value::as_bool).unwrap_or(false), t.elapsed().as_millis()));
            }
            Err(_) => out.push((false, t.elapsed().as_millis())),
        }
        std::thread::sleep(step);
    }
    out
}

// ========== test 1 — opencode exit1 during activity poll ==========

/// opencode exit1 at the poll's activity probe: the daemon must stay alive,
/// keep serving ping, and recover cleanly (primary idle signal restored) once
/// the shim is swapped back to healthy via its env files.
#[test]
fn fault_1_opencode_exit1_poll_survives_and_recovers() {
    let d = TestDaemon::spawn("t1", &[], &[]);
    let ack = d.submit_task("fault one: activity probe fails", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");

    // Break only the /api/session probe (herdr stays healthy).
    d.shim_write("api_session.mode", "exit1");
    std::thread::sleep(Duration::from_millis(1100)); // >= 4 poll cycles

    let (ok, latency) = d.ping();
    assert!(ok, "ping must succeed during opencode exit1");
    assert!(latency < 2000, "ping latency {latency}ms exceeds 2000ms during fault");

    // The session is untouched: still a live, working session. (The probe
    // failure has no session-level effect by design.)
    let snap = d.snapshot();
    assert_eq!(sessions(&snap).len(), 1);
    assert_eq!(session(&snap, "S1-OC01")["status"], json!("working"));

    // Recovery: swap the opencode probe back to healthy and report the
    // session idle. The very next polls must pick the primary signal up.
    d.shim_write("api_session.mode", "healthy");
    d.shim_write("api_state", "idle");

    let recovered = wait_until(Duration::from_secs(10), || {
        d.events()
            .iter()
            .any(|e| e["kind"] == json!("idle_signal") && e["detail"] == json!("primary"))
    });
    assert!(
        recovered,
        "after healthy swap, events.jsonl should show idle_signal=primary; events: {}",
        serde_json::Value::Array(d.events())
    );

    // The decider then runs (healthy, default CONTINUE) — proof the full
    // supervisor path recovered, not just the probe.
    let decider_ran = wait_until(Duration::from_secs(10), || {
        decisions(&d.snapshot())
            .iter()
            .any(|dec| dec["action"] == json!("reprompt"))
    });
    assert!(decider_ran, "after recovery, the decider path should act (reprompt decision)");
    let snap = d.snapshot();
    let iter = session(&snap, "S1-OC01")["iterations"].as_u64().unwrap_or(0);
    assert!(iter >= 1, "recovery re-prompts must consume an iteration, got {iter}");
}

/// KNOWN GAP (ignored): a failed /api/session probe produces NO entry in
/// snapshot.warnings — daemon.rs's opencode_api_sessions() returns None
/// silently ("the signal degrades to herdr agent_status", but only inside the
/// supervisor; nothing is logged anywhere). Expected by the task brief:
/// warnings should mention the probe failure.
///
/// Reproduce:
///   NOTCH_HERDR_CMD=tests/fixtures/shims/herdr \
///   NOTCH_OPENCODE_CMD=tests/fixtures/shims/opencode \
///     with $SHIM_STATE_DIR/api_session.mode = "exit1", then poll
///   -> snapshot.warnings == []  (nothing about opencode / the probe)
#[test]
#[ignore = "opencode /api/session probe failure is silent: no warning pushed in snapshot.warnings (daemon.rs:opencode_api_sessions). See report."]
fn fault_1b_opencode_exit1_probe_failure_warns() {
    let d = TestDaemon::spawn("t1b", &[], &[]);
    let _ = d.submit_task("fault 1b: probe failure must warn", None);
    d.shim_write("api_session.mode", "exit1");
    std::thread::sleep(Duration::from_millis(1500));

    let snap = d.snapshot();
    let warns = warnings(&snap);
    let mentioned = warns.iter().any(|w| {
        let w = w.to_lowercase();
        w.contains("opencode") || w.contains("probe") || w.contains("/api/session")
    });
    assert!(
        mentioned,
        "expected a warning mentioning the opencode probe failure; snapshot.warnings were: {warns:?}; daemon logs: {}",
        d.logs()
    );
}

// ========== test 2 — opencode hang (sleep 45) ==========

/// The opencode shim hangs for 45s past every caller's patience. The poll
/// cycle is wedged until the child exits, but the daemon as a service stays
/// responsive (ping/snapshot are independent of the poll thread), and the
/// watchdog fires as soon as the poll loop is unblocked.
#[test]
fn fault_2a_opencode_hang_ping_responsive_and_watchdog_fires() {
    // stuck_after_secs = 10: long enough that nothing fires pre-hang, short
    // enough that the watchdog verdict lands immediately after unblock.
    let d = TestDaemon::spawn("t2a", &[("NOTCH_STUCK_AFTER_SECS", "10")], &[]);
    let ack = d.submit_task("fault two: opencode hangs", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");
    std::thread::sleep(Duration::from_millis(700)); // let a couple of clean polls happen

    d.shim_write("api_session.mode", "hang");
    let t_hang = Instant::now();

    // While the poll thread is wedged inside the opencode probe, prove the
    // socket path is fully responsive (concurrent ping thread).
    let sock = d.sock.clone();
    let pinger = std::thread::spawn(move || ping_loop(&sock, 24, Duration::from_millis(150)));
    let pings = pinger.join().expect("ping thread panicked");
    let max_latency = pings.iter().map(|(_, ms)| *ms).max().unwrap_or(0);
    let ok_count = pings.iter().filter(|(ok, _)| *ok).count();
    println!("fault_2a: {ok_count}/{} pings ok during hang, max latency {max_latency}ms", pings.len());
    assert_eq!(ok_count, pings.len(), "every ping must succeed during the opencode hang");
    assert!(max_latency < 2000, "ping latency {max_latency}ms exceeds 2000ms during hang");

    // The watchdog verdict lands once the blocked poll returns (~45s in).
    let recovered = wait_until(Duration::from_secs(80), || {
        let snap = d.snapshot();
        snap["sessions"].as_array().map(|_| session(&snap, "S1-OC01")["failureReason"] == json!("stuck")).unwrap_or(false)
    });
    let blocked_ms = t_hang.elapsed().as_millis();
    println!("fault_2a: poll path unblocked and watchdog fired after {blocked_ms}ms");
    assert!(
        recovered,
        "watchdog should mark S1-OC01 failed/stuck after the hang clears (blocked ~{blocked_ms}ms); daemon logs: {}",
        d.logs()
    );

    let (ok, latency) = d.ping();
    assert!(ok && latency < 2000, "daemon must be pingable after recovery (ok={ok}, {latency}ms)");
}

/// KNOWN GAP (ignored): the task brief demands the poll cycle complete
/// within the caller's designed budget (< 2x the 1s watchdog attribution
/// timeout). It does not — `opencode::invoke_argv` uses
/// `std::process::Command::output()` with no deadline, so a hung opencode
/// blocks `poll_once` for the child's full lifetime (~45_000ms with
/// SHIM_HANG_SECS=45). Not a deadlock — the daemon recovers on its own — but
/// there is no designed timeout to honor; this finding stands until the src/
/// seam gains one.
///
/// Reproduce: NOTCH_HERDR_CMD/NOTCH_OPENCODE_CMD=fixture shims; spawn a
/// session with a goal; set $SHIM_STATE_DIR/api_session.mode=hang; measure
/// time from the hang file write to the next observable poll effect.
#[test]
#[ignore = "no timeout on the opencode subprocess seam (opencode.rs: Command::output blocks indefinitely); poll cycle wedges ~45s. See report."]
fn fault_2b_opencode_hang_poll_budget() {
    let d = TestDaemon::spawn("t2b", &[("NOTCH_STUCK_AFTER_SECS", "10")], &[]);
    let _ = d.submit_task("fault 2b: poll budget under hang", None);
    std::thread::sleep(Duration::from_millis(700));
    d.shim_write("api_session.mode", "hang");
    let t_hang = Instant::now();
    wait_until(Duration::from_secs(90), || {
        session(&d.snapshot(), "S1-OC01")["failureReason"] == json!("stuck")
    });
    let blocked_ms = t_hang.elapsed().as_millis();
    println!("fault_2b: poll cycle(s) blocked for {blocked_ms}ms (designed budget: < 2000ms)");
    assert!(blocked_ms < 2000, "design budget missed by {blocked_ms}ms");
}

// ========== test 3 — bad_json at decider time ==========

/// Decider answers "not json {{{" on every call. Expected (and asserted):
/// each bad answer is logged as "decider unparseable", the session is NOT
/// marked failed by the decider path, a re-prompt with fresh framing is
/// issued, and after 3 consecutive strikes the session is escalated
/// (loop_detected) — the actual code path for this input class.
#[test]
fn fault_3_opencode_bad_json_decider_unparseable_then_escalation() {
    let d = TestDaemon::spawn("t3", &[], &[]);
    let ack = d.submit_task("fault three: decider garbage", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");

    d.shim_write("api_state", "idle"); // primary idle signal: decider path armed
    d.shim_write("run.mode", "bad_json");

    let escalated = wait_until(Duration::from_secs(12), || {
        escalations(&d.snapshot())
            .iter()
            .any(|e| e["kind"] == json!("loop_detected") && e["reason"].as_str().unwrap_or("").contains("unparseable"))
    });
    assert!(
        escalated,
        "three consecutive unparseable decider answers must escalate (loop_detected); snapshot: {}",
        d.snapshot()
    );

    let snap = d.snapshot();
    let s = session(&snap, "S1-OC01");
    assert_ne!(s["status"], json!("failed"), "decider path never marks the session failed");
    assert_eq!(s["iterations"], json!(3), "each unparseable answer consumes exactly one iteration");

    let decs = decisions(&snap);
    let unparseable: Vec<&Value> = decs
        .iter()
        .filter(|dec| dec["reason"].as_str().unwrap_or("").contains("decider unparseable"))
        .collect();
    assert!(
        unparseable.len() >= 3,
        "expected >= 3 'decider unparseable' decision entries, got {}: {decs:?}",
        unparseable.len()
    );
    assert!(
        decs.iter().all(|dec| dec["action"] != json!("escalate") || dec["reason"].as_str().unwrap_or("").contains("unparseable")),
        "decisions should only mention unparseable causes: {decs:?}"
    );

    // Events log carries the same truth (kind=decision, detail contains the marker).
    let events = d.events();
    assert!(
        events.iter().any(|e| e["kind"] == json!("decision")
            && e["detail"].as_str().unwrap_or("").contains("decider unparseable")),
        "events.jsonl must contain the decider unparseable decisions: {events:?}"
    );

    // Prompt accounting: 1 spawn prompt + exactly 3 re-prompts (the escalated
    // session is excluded from the decision loop forever after).
    assert_eq!(
        d.shim_count_lines("prompts.tsv"),
        4,
        "spawn prompt + 3 unparseable re-prompts; got: {:?}",
        d.shim_read("prompts.tsv")
    );

    let (ok, _) = d.ping();
    assert!(ok, "daemon must stay pingable after the escalation");
}

// ========== test 4 — 429 at decider time ==========

/// 429 text in the pane tail at decider time: jittered backoff is logged
/// (decision action "backoff", reason carries the 429/quota marker), and — per
/// code semantics — the rate-limit backoff does NOT consume an iteration.
#[test]
fn fault_4a_triage_429_tail_triggers_jittered_backoff_without_iterating() {
    let d = TestDaemon::spawn("t4a", &[], &[]);
    let ack = d.submit_task("fault four A: rate limit in pane output", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");

    d.shim_write("api_state", "idle");
    d.shim_write("agent_read.mode", "http429"); // herdr read prints "429 too many requests"

    let backoff_logged = wait_until(Duration::from_secs(10), || {
        decisions(&d.snapshot()).iter().any(|dec| {
            // decision action is exactly "backoff"; the reason carries the
            // 429/quota marker and the jittered duration.
            dec["action"] == json!("backoff") && dec["reason"].as_str().unwrap_or("").contains("429")
        })
    });
    assert!(
        backoff_logged,
        "a rate-limit marker in the pane tail must log a 'backoff' decision; snapshot: {}",
        d.snapshot()
    );

    // events.jsonl agrees (kind=decision, detail carries the 429 marker).
    let events = d.events();
    assert!(
        events.iter().any(|e| e["kind"] == json!("decision")
            && e["detail"].as_str().unwrap_or("").contains("429")
            && e["detail"].as_str().unwrap_or("").contains("backoff")),
        "events.jsonl must carry the 429 backoff decision: {events:?}"
    );

    let snap = d.snapshot();
    assert_eq!(
        session(&snap, "S1-OC01")["iterations"],
        json!(0),
        "rate-limit backoff must NOT increment iterations"
    );
    assert_eq!(
        d.shim_count_lines("prompts.tsv"),
        1,
        "rate-limit backoff must NOT re-prompt the session (spawn prompt only)"
    );
    assert_eq!(escalations(&snap).len(), 0, "a single rate-limit hit must not escalate yet");
    let decs = decisions(&snap);
    assert!(
        decs.iter().all(|dec| dec["action"] != json!("reprompt")),
        "no re-prompt may be issued under rate-limit backoff: {decs:?}"
    );
    let (ok, _) = d.ping();
    assert!(ok, "daemon must stay pingable under rate-limit backoff");
}

/// FINDING, documented with real behavior (not ignored — it is green, it just
/// is NOT what the task brief hopes for):
/// when the 429 text comes from the decider model call itself
/// (SHIM_RUN_MODE=http429), the daemon does NOT take a rate-limit backoff.
/// `contains_rate_limit` is only applied to the herdr pane tail. The decider
/// output "429 too many requests" is parsed as an unparseable verdict:
/// consecutive_decider_failures += 1, stuck_strikes += 1, and — per the
/// Unparseable branch in supervise_once — `iterations` IS incremented, a fresh
/// re-prompt IS issued, and three strikes escalate with loop_detected. The
/// 429 is also not recognized as a rate limit anywhere downstream of the
/// tail check.
#[test]
fn fault_4b_opencode_decider_429_is_unparseable_not_rate_limit_backoff() {
    let d = TestDaemon::spawn("t4b", &[], &[]);
    let ack = d.submit_task("fault four B: decider itself is rate limited", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");

    d.shim_write("api_state", "idle");
    d.shim_write("run.mode", "http429"); // the decider's own stdout is the 429 text

    let consumed = wait_until(Duration::from_secs(10), || {
        session(&d.snapshot(), "S1-OC01")["iterations"].as_u64().unwrap_or(0) >= 1
    });
    assert!(consumed, "decider 429 currently consumes an iteration; daemon: {}", d.logs());

    let snap = d.snapshot();
    let decs = decisions(&snap);
    assert_eq!(
        session(&snap, "S1-OC01")["iterations"],
        json!(1),
        "documented finding: decider-side 429 increments iterations"
    );
    assert!(
        decs.iter().all(|dec| dec["action"] != json!("backoff")),
        "documented finding: decider-side 429 never routes to the 'backoff' decision path: {decs:?}"
    );
    assert!(
        decs.iter().any(|dec| dec["reason"].as_str().unwrap_or("").contains("decider unparseable")),
        "decider-side 429 is logged as an unparseable verdict: {decs:?}"
    );
    let (ok, _) = d.ping();
    assert!(ok);
}

// ========== test 5 — herdr exit1 mid-poll ==========

/// agent list fails mid-poll (pane list stays healthy): reconcile and the
/// poll loop are safe, the daemon logs a warning, nothing crashes or wedges,
/// and once the shim swaps back to healthy, polling recovers.
#[test]
fn fault_5_herdr_exit1_mid_poll_reconcile_safe() {
    let d = TestDaemon::spawn("t5", &[], &[]);
    let ack = d.submit_task("fault five: agent list dies mid-run", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");
    std::thread::sleep(Duration::from_millis(600)); // a few clean polls

    // Mid-run fault: only `agent list` fails; `pane list` (nothing in the
    // daemon's poll path calls it) and everything else stay healthy.
    d.shim_write("agent_list.mode", "exit1");
    std::thread::sleep(Duration::from_millis(1200)); // >= 4 poll cycles

    let snap = d.snapshot();
    let warns = warnings(&snap);
    assert!(
        warns.iter().any(|w| w.contains("herdr unavailable")),
        "mid-poll agent-list failure must push an 'herdr unavailable' warning; got: {warns:?}"
    );
    assert_eq!(sessions(&snap).len(), 1, "session must survive an agent-list fault: {snap}");
    let (ok, _) = d.ping();
    assert!(ok, "daemon must respond to ping during agent-list fault");

    // Swap back to healthy; polling recovers without any state surgery.
    d.shim_write("agent_list.mode", "healthy");
    std::thread::sleep(Duration::from_millis(900));
    let snap = d.snapshot();
    assert_eq!(sessions(&snap).len(), 1);
    assert_eq!(session(&snap, "S1-OC01")["status"], json!("working"));
    let logs = d.logs_cmd("S1-OC01", 5);
    assert_eq!(logs["event"], json!("logs"));
    assert!(
        logs["lines"].as_array().map(|l| l.len() >= 2).unwrap_or(false),
        "herdr agent read must work again after recovery: {logs}"
    );
    let (ok, _) = d.ping();
    assert!(ok);

    // Reconcile path, same fault, fresh daemon over the same state dir:
    // the warning is logged on startup, sessions are left alone, no crash.
    d.shim_write("agent_list.mode", "exit1");
    let state_dir = d.state_dir.clone();
    let shim_dir = d.shim_dir.clone();
    let root = d.root.clone();
    let cwd = d.cwd.clone();
    d.stop();

    // Re-open the first daemon's state dir through a fresh daemon with the
    // same SHIM_STATE_DIR so the fault persists across the restart.
    let n = FIXTURE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let sock = PathBuf::from(format!("/tmp/nd-fi-{}-{n}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_notchd"));
    cmd.arg("serve")
        .env("NOTCH_STATE_DIR", &state_dir)
        .env("NOTCH_SOCK", &sock)
        .env("NOTCH_HERDR_CMD", herdr_shim())
        .env("NOTCH_OPENCODE_CMD", opencode_shim())
        .env("SHIM_STATE_DIR", &shim_dir)
        .env("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true")
        .env("NOTCH_POLL_INTERVAL_MS", "250")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let child = cmd.spawn().expect("respawn notchd serve");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if sock.exists() && UnixStream::connect(&sock).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "respawned daemon did not bind socket");
        std::thread::sleep(Duration::from_millis(40));
    }
    let d2 = TestDaemon { child, root, state_dir: state_dir.clone(), shim_dir, sock: sock.clone(), cwd, logs: Arc::new(Mutex::new(String::new())) };

    let snap = d2.snapshot();
    let warns = warnings(&snap);
    assert!(
        warns.iter().any(|w| w.contains("re-adoption skipped")),
        "reconcile with a failing agent list must warn 're-adoption skipped'; got: {warns:?}"
    );
    assert_eq!(sessions(&snap).len(), 1, "reconcile must not drop or duplicate sessions: {snap}");
    assert_eq!(session(&snap, "S1-OC01")["status"], json!("working"), "reconcile must not touch session status on herdr failure");
    let (ok, _) = d2.ping();
    assert!(ok, "respawned daemon must respond to ping");
}

// ========== test 6 — herdr wrong_schema on pane split ==========

/// `herdr pane split` answers with valid JSON of the wrong shape. Spawn must
/// fail cleanly: session marked failed/spawn_failed, ack ok:false, other
/// sessions unaffected, registry ids retired and never reused.
#[test]
fn fault_6_herdr_wrong_schema_pane_split_spawn_failed_clean() {
    let d = TestDaemon::spawn("t6", &[], &[]);

    // Phase 1: split emits {"result": 42} — no pane id in any shape we accept.
    d.shim_write("pane_split.mode", "wrong_schema");
    let ack = d.submit_task("fault six: split says nonsense", None);
    assert_eq!(ack["ok"], json!(false), "spawn must report failure: {ack}");
    assert_eq!(ack["sessionId"], json!("S1-OC01"));
    assert_eq!(ack["supervisorId"], json!("SV-1"));
    assert!(
        ack["error"].as_str().unwrap_or("").contains("pane split"),
        "ack error must name the failing stage: {ack}"
    );

    let snap = d.snapshot();
    let s = session(&snap, "S1-OC01");
    assert_eq!(s["status"], json!("failed"), "failed spawn must mark the session failed");
    assert_eq!(s["failureReason"], json!("spawn_failed"));
    assert!(warnings(&snap).iter().any(|w| w.contains("spawn failed")));

    // Phase 2: split healthy again. Two more spawns under the same supervisor.
    d.shim_write("pane_split.mode", "healthy");
    let ack2 = d.submit_task("fault six: good spawn two", Some("SV-1"));
    let ack3 = d.submit_task("fault six: good spawn three", Some("SV-1"));
    assert_eq!(ack2["ok"], json!(true), "spawn two should succeed: {ack2}");
    assert_eq!(ack3["ok"], json!(true), "spawn three should succeed: {ack3}");
    assert_eq!(ack2["sessionId"], json!("S1-OC02"), "ids are never reused, even for failed spawns");
    assert_eq!(ack3["sessionId"], json!("S1-OC03"), "ids are never reused, even for failed spawns");

    let snap = d.snapshot();
    assert_eq!(session(&snap, "S1-OC01")["status"], json!("failed"));
    assert_eq!(session(&snap, "S1-OC02")["status"], json!("working"));
    assert_eq!(session(&snap, "S1-OC03")["status"], json!("working"));
    assert_eq!(sessions(&snap).len(), 3, "S1-OC01 failed-but-present; no corruption: {snap}");

    // Registry: every allocated id (including the failed spawn's) is retired,
    // monotonic, and appears exactly once.
    let raw = std::fs::read_to_string(d.state_dir.join("registry.json")).expect("registry.json");
    let reg: Value = serde_json::from_str(&raw).expect("registry.json must parse");
    let ids: Vec<&str> = reg["ids"].as_array().expect("ids array").iter().filter_map(Value::as_str).collect();
    assert_eq!(
        ids,
        vec!["SV-1", "S1-OC01", "S1-OC02", "S1-OC03"],
        "registry ids must be retired in allocation order and never reused"
    );
    assert_eq!(
        reg["sessionCounts"]["1"], json!(3),
        "three S1-OCNN allocations were made; none recycled"
    );

    // state.json must still parse and agree.
    let raw = std::fs::read_to_string(d.state_dir.join("state.json")).expect("state.json");
    let state: Value = serde_json::from_str(&raw).expect("state.json must parse");
    assert_eq!(state["sessions"].as_array().unwrap().len(), 3);

    // events.jsonl carries the spawn_failed escalation for S1-OC01.
    let events = d.events();
    assert!(
        events.iter().any(|e| e["kind"] == json!("escalate")
            && e["session"] == json!("S1-OC01")
            && e["detail"].as_str().unwrap_or("").contains("spawn_failed")),
        "events.jsonl must record spawn_failed for S1-OC01: {events:?}"
    );

    let (ok, _) = d.ping();
    assert!(ok, "daemon must stay pingable after a failed spawn");
}

// ========== test 7 — empty output from the opencode decider ==========

/// The decider exits 0 printing nothing. Every call is unparseable; the
/// session is never marked failed, no panic, and no re-prompt is issued
/// beyond the decision budget. The observed code path: the unparseable
/// branch re-prompts with fresh framing, increments iterations + strikes,
/// and on the 3rd strike escalates loop_detected ("3 unparseable decider
/// answers in a row") — which fires BEFORE the default 8-iteration budget is
/// reached. After escalation the session is excluded from the decision loop,
/// so prompt traffic stops entirely.
#[test]
fn fault_7_opencode_empty_decider_unparseable_no_panic_no_overbudget() {
    let d = TestDaemon::spawn("t7", &[], &[]);
    let ack = d.submit_task("fault seven: decider says nothing", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");

    d.shim_write("api_state", "idle");
    d.shim_write("run.mode", "empty");

    let escalated = wait_until(Duration::from_secs(12), || {
        escalations(&d.snapshot())
            .iter()
            .any(|e| e["kind"] == json!("loop_detected") && e["reason"].as_str().unwrap_or("").contains("unparseable"))
    });
    assert!(escalated, "3 empty decider answers must escalate; snapshot: {}", d.snapshot());

    let snap = d.snapshot();
    assert_ne!(session(&snap, "S1-OC01")["status"], json!("failed"), "empty decider output never fails the session");
    assert_eq!(session(&snap, "S1-OC01")["iterations"], json!(3), "one iteration per empty answer");

    // events: kind=decision, detail contains "decider unparseable" with an empty head.
    let events = d.events();
    assert!(
        events.iter().any(|e| e["kind"] == json!("decision") && e["detail"].as_str().unwrap_or("").contains("decider unparseable")),
        "events.jsonl must record the unparseable decisions: {events:?}"
    );

    // No re-prompts beyond the escalation: the decision loop is closed for
    // this session. Count must be stable.
    let prompts_at_escalation = d.shim_count_lines("prompts.tsv");
    assert_eq!(prompts_at_escalation, 4, "spawn + 3 re-prompts; got {:?}", d.shim_read("prompts.tsv"));
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        d.shim_count_lines("prompts.tsv"),
        prompts_at_escalation,
        "no re-prompt may be issued after escalation: {:?}",
        d.shim_read("prompts.tsv")
    );

    // No decider calls either, despite the session being idle every poll.
    let decider_calls = d.shim_count_lines("decider_calls");
    assert_eq!(decider_calls, 3, "decider must not be called after escalation: {decider_calls}");

    let (ok, _) = d.ping();
    assert!(ok, "daemon must stay pingable after empty-decider escalation");
}

/// Sibling fact for the fault matrix: a decider that fails by EXIT CODE
/// (exit1) never re-prompts at all — it only increments
/// consecutive_decider_failures and logs a 'backoff' decision, and on the
/// 3rd consecutive failure escalates budget_exceeded ("decider failed 3
/// times in a row"). Verified from code semantics (daemon.rs Err arm) and
/// asserted here end-to-end.
#[test]
fn fault_7b_decider_exit1_never_reprompts_budget_escalation() {
    let d = TestDaemon::spawn("t7b", &[], &[]);
    let ack = d.submit_task("fault 7b: decider exits 1", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");

    d.shim_write("api_state", "idle");
    d.shim_write("run.mode", "exit1");

    let escalated = wait_until(Duration::from_secs(12), || {
        escalations(&d.snapshot())
            .iter()
            .any(|e| e["kind"] == json!("budget_exceeded") && e["reason"].as_str().unwrap_or("").contains("decider failed"))
    });
    assert!(
        escalated,
        "3 consecutive decider call failures must escalate budget_exceeded; snapshot: {}",
        d.snapshot()
    );

    assert_eq!(
        d.shim_count_lines("prompts.tsv"),
        1,
        "a failing decider call must never trigger a re-prompt: {:?}",
        d.shim_read("prompts.tsv")
    );
    assert_eq!(session(&d.snapshot(), "S1-OC01")["iterations"], json!(0), "a failed call must not consume iterations");
    let snap = d.snapshot();
    assert_ne!(session(&snap, "S1-OC01")["status"], json!("failed"));
    let (ok, _) = d.ping();
    assert!(ok);
}

// ========== test 8 — every shim × ping responsiveness during the fault ==========

/// Global fault mode rotated across every shim class while a session exists;
/// during each class a concurrent ping thread proves the daemon stays fully
/// responsive. Runs on both shims at once (SHIM_MODE global file).
#[test]
fn fault_8_every_shim_ping_responsive_during_fault() {
    let d = TestDaemon::spawn("t8", &[("NOTCH_STUCK_AFTER_SECS", "3600")], &[]);
    let ack = d.submit_task("fault eight: ping keeps working", None);
    assert_eq!(ack["ok"], json!(true), "spawn should succeed: {ack}");
    std::thread::sleep(Duration::from_millis(600));

    for mode in ["healthy", "exit1", "bad_json", "http429", "http5xx", "empty", "wrong_schema", "hang"] {
        d.shim_write("mode", mode);
        let sock = d.sock.clone();
        let pinger = std::thread::spawn(move || ping_loop(&sock, 12, Duration::from_millis(120)));
        let pings = pinger.join().expect("ping thread panicked");
        let all_ok = pings.iter().all(|(ok, _)| *ok);
        let max_latency = pings.iter().map(|(_, ms)| *ms).max().unwrap_or(0);
        println!("fault_8: mode={mode}: {}/{} pings ok, max latency {max_latency}ms", pings.iter().filter(|(o, _)| *o).count(), pings.len());
        assert!(all_ok, "mode={mode}: every ping must succeed; got {pings:?}");
        assert!(max_latency < 2000, "mode={mode}: ping latency {max_latency}ms >= 2000ms");
        d.shim_write("mode", "healthy");
    }

    // The daemon survived the whole matrix: session still present and pingable.
    let snap = d.snapshot();
    assert_eq!(sessions(&snap).len(), 1);
    let (ok, _) = d.ping();
    assert!(ok, "daemon must answer ping after the full fault matrix");
}
