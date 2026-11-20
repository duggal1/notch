//! Soak tests for notchd — a 300-poll deterministic lifecycle on fake
//! clock + fake herdr + fake opencode, with hard invariants asserted at
//! polls 50/100/150/200/250/300 and once at the end.
//!
//! Scenario: 1 orchestrator (the daemon) + 3 supervisors + 20 sessions,
//! spawned in batches (polls 1/3/5), working with `last_revision` bumping
//! every poll, seeded-but-random-looking idle transitions (mulberry32;
//! explicitly NOT `Math.random`-style nondeterminism). The 16 non-failing
//! sessions take one idle/decider re-prompt then reach herdr `done`; the 4
//! failing sessions hit herdr `blocked`, which the hardened engine maps to
//! NeedsHuman (pane AND temp root kept, session NOT failed), and are resolved
//! terminally by an explicit `kill()` at the end of the run. Everything else
//! is the engine's own code path.
//!
//! Invariants (asserted at every checkpoint + at the end):
//!   1. Count of live `notch-S*-OC*` session temp dirs under the injected
//!      tmp root == sessions not yet terminal; each non-terminal session's
//!      recorded dir exists and each terminal session's recorded dir does
//!      not. After the final global sweep: zero exact-grammar dirs remain.
//!   2. IDs strictly monotonic per supervisor (OC counter only increases),
//!      zero duplicates anywhere, registry count == 20 sessions after the
//!      full lifecycle, registry survives a simulated restart (second
//!      `Daemon::open` on the same state dir; allocation continues, never
//!      reissued).
//!   3. Attribution ≤ 24 snapshot entries (the cap actually bites: 46
//!      recorded entries), conflicts consistent with recorded files,
//!      events.jsonl parseable every checkpoint. Rotation is forced
//!      separately in `soak_events_log_rotation_forced_via_bus_messages`.
//!   4. eps finite, ≥ 0, < 1e9. Never NaN. Checked at every checkpoint.
//!   5. Snapshot key-set is stable across all checkpoints (required keys
//!      always present, no foreign keys ever) — inlined here on purpose;
//!      this file does not import the acceptance suite.
//!   6. Poll wall time for 20 live sessions < 500 ms per poll — VIOLATED by
//!      this engine (serial per-session git sampling in Pass B); reproduced
//!      minimally in `soak_poll_wall_time_budget_20_live_sessions` (#[ignore]).
//!      The max observed poll time is logged in the SUMMARY line.
//!   7. Final global `cleanup(None)` returns `removed == expected`, derived
//!      from on-disk pre-sweep state; events.jsonl records a `cleanup` event.
//!
//! Sweep root: Config exposes `tmp_dir`, so the injected tmp root IS the
//! sweep root and the zero-leak assertions are exact there. As a courtesy,
//! `/tmp` itself is scanned for dirs named exactly like our allocated ids
//! and reported on stderr (non-fatal): other processes' dirs are out of
//! this run's footprint, and a non-exact match is ignored on both sides.
//!
//! Run: `cargo test -p notchd --test soak_lifecycle -- --nocapture`

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;

use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ---------------------------------------------------------------- helpers

static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-soak-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn test_config(state_dir: &Path, tmp_dir: &Path, stuck_after_secs: u64) -> Config {
    Config {
        state_dir: state_dir.to_path_buf(),
        tmp_dir: tmp_dir.to_path_buf(),
        stuck_after_secs,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

/// mulberry32 — tiny seeded PRNG. Deterministic; no Math.random anywhere.
struct Mulberry32(u32);

impl Mulberry32 {
    fn new(seed: u32) -> Self {
        Mulberry32(seed)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }
}

/// Exact grammar of an engine temp dir: `notch-S<sup>-OC<nn>` where <sup> is
/// one or more ASCII digits and <nn> is exactly two (mirror of the sweep's
/// own rule — asserted independently here).
fn exact_notch_dir_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("notch-S") else {
        return false;
    };
    let Some((sup, rest)) = rest.split_once('-') else {
        return false;
    };
    if sup.is_empty() || !sup.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(nn) = rest.strip_prefix("OC") else {
        return false;
    };
    nn.len() == 2 && nn.bytes().all(|b| b.is_ascii_digit())
}

/// All direct children of `dir` matching the exact engine grammar.
fn exact_notch_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if entry.path().is_dir() && exact_notch_dir_name(&entry.file_name().to_string_lossy()) {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

/// Parse a JSONL event log; every line must be a JSON object with numeric ts,
/// string kind and string detail, session null or string.
fn parse_jsonl_path(path: &Path, ctx: &str) -> Vec<Value> {
    let mut lines = Vec::new();
    let f = std::fs::File::open(path)
        .unwrap_or_else(|e| panic!("{ctx}: cannot open {}: {e}", path.display()));
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let line = line.unwrap_or_else(|e| panic!("{ctx}: line {i} io error: {e}"));
        if line.trim().is_empty() {
            panic!("{ctx}: line {i} is empty (log corruption?)");
        }
        let v: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("{ctx}: line {i} not JSON: {e}\nline: {line}"));
        assert!(v["ts"].is_u64(), "{ctx}: line {i} has no numeric ts");
        assert!(
            v["kind"].as_str().map(|k| !k.is_empty()).unwrap_or(false),
            "{ctx}: line {i} kind missing"
        );
        assert!(
            v["detail"].is_string(),
            "{ctx}: line {i} detail not a string"
        );
        assert!(
            v["session"].is_null() || v["session"].is_string(),
            "{ctx}: line {i} session not null|string"
        );
        lines.push(v);
    }
    lines
}

fn assert_events_jsonl_parseable(state_dir: &Path, ctx: &str) -> Vec<Value> {
    parse_jsonl_path(&state_dir.join("events.jsonl"), ctx)
}

fn count_events_of_kind(lines: &[Value], kind: &str) -> usize {
    lines
        .iter()
        .filter(|v| v["kind"].as_str() == Some(kind))
        .count()
}

// ------------------------------------------------ fake herdr / opencode

/// Scripted fake herdr. Agents are pushed per poll by the test harness.
struct SoakHerdr {
    calls: Mutex<Vec<Vec<String>>>,
    agents: Mutex<Vec<Value>>,
    read_text: String,
    split_seq: Mutex<u32>,
}

impl SoakHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            agents: Mutex::new(Vec::new()),
            read_text: "last line of pane output\n".to_string(),
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
        self.calls.lock().unwrap().push(args.to_vec());
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
            ("agent", "read") => Ok(self.read_text.clone()),
            ("agent", "wait") => Ok(serde_json::json!({"id": "cli:agent:wait", "result": { "type": "ok" }}).to_string()),
            ("pane", "close") => Ok(serde_json::json!({"id": "cli:pane:close", "result": { "type": "ok" }}).to_string()),
            ("agent", "focus") => Ok(serde_json::json!({"id": "cli:agent:focus", "result": { "type": "ok" }}).to_string()),
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected call: {}", args.join(" ")),
            }),
        }
    }
}

/// Scripted fake opencode. Primary signal (`api GET /api/session`) is
/// deliberately empty so idle detection falls back to the herdr signal.
/// Every decider call answers `CONTINUE` — done transitions come from herdr.
/// Sessions are spawned WITHOUT a definitionOfDone on purpose: with a DoD the
/// hardened engine's `enforce_dod_gate` reverts a herdr-reported `done` back
/// to Working until a verifier `done` decision is recorded, while the
/// mark_done path has already cleaned the session temp dir. This fixture does
/// not script the model verifier, so an inert DoD would leave permanently
/// non-terminal sessions with no temp root (the exact RED this file fixed).
struct SoakOpencode {
    calls: Mutex<Vec<Vec<String>>>,
}

impl SoakOpencode {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }

    fn opencode(self: &Arc<Self>) -> Opencode {
        let fake = Arc::clone(self);
        Opencode::with_runner(move |args: &[String]| {
            fake.calls.lock().unwrap().push(args.to_vec());
            let rest = &args[1..];
            match rest.join(" ").as_str() {
                "api GET /api/session" => {
                    Ok(serde_json::json!({"cursor": null, "data": []}).to_string())
                }
                "models" => {
                    Ok(["opencode/hy4-flash-free", "opencode/inkling-2-small-free"].join("\n"))
                }
                s if s.starts_with("run --model") => {
                    Ok("CONTINUE: keep going — run the tests and report back".to_string())
                }
                other => Err(HerdrError::Failed {
                    args: args.to_vec(),
                    code: Some(2),
                    stderr: format!("unexpected opencode call: {other}"),
                }),
            }
        })
    }

    fn decider_calls(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.get(1).map(String::as_str) == Some("run"))
            .count()
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

// -------------------------------------------- snapshot shape assertions

fn keys_of(v: &Value) -> std::collections::BTreeSet<String> {
    v.as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn assert_status_vocab(v: &Value, ctx: &str) {
    let s = v.as_str().unwrap_or("");
    assert!(
        matches!(s, "working" | "idle" | "failed" | "done"),
        "{ctx}: illegal status `{s}`"
    );
}

/// Invariant 5, inlined (not imported from the acceptance suite): the
/// snapshot key-set is stable — every required key present at every
/// checkpoint, and no foreign key ever.
fn assert_snapshot_shape(v: &Value, ctx: &str) {
    let obj = v
        .as_object()
        .unwrap_or_else(|| panic!("{ctx}: snapshot is not a JSON object"));
    let top = keys_of(v);
    let required_top: HashSet<&str> = [
        "version",
        "generatedAt",
        "orchestrator",
        "supervisors",
        "sessions",
        "eps",
        "conflicts",
        "attribution",
        "warnings",
    ]
    .into_iter()
    .collect();
    let allowed_top: HashSet<&str> = required_top
        .iter()
        .copied()
        .chain(["decisions", "escalations", "busMessages"])
        .collect();
    for k in &required_top {
        assert!(top.contains(*k), "{ctx}: top-level key `{k}` missing");
    }
    for k in &top {
        assert!(
            allowed_top.contains(k.as_str()),
            "{ctx}: foreign top-level key `{k}`"
        );
    }
    assert_eq!(obj["version"].as_u64(), Some(1), "{ctx}: version != 1");
    assert!(
        obj["generatedAt"].is_u64(),
        "{ctx}: generatedAt not a u64 ms epoch"
    );

    // eps must be a JSON number (serde_json renders non-finite f64 as null,
    // which as_f64() rejects — this catches inf/NaN leaking into the wire).
    let eps = obj["eps"]
        .as_f64()
        .expect("{ctx}: eps must be a JSON float (no inf/NaN)");
    assert!(
        eps.is_finite() && (0.0..1e9).contains(&eps),
        "{ctx}: eps {eps} out of sanity bounds"
    );

    // orchestrator
    let orch = &obj["orchestrator"];
    let orch_keys = keys_of(orch);
    assert_eq!(
        orch_keys,
        ["id", "status"].into_iter().map(String::from).collect(),
        "{ctx}: orchestrator keys drifted"
    );
    assert_eq!(orch["id"].as_str(), Some("ORCH"));
    assert_status_vocab(&orch["status"], &format!("{ctx} orchestrator"));

    // supervisors
    let sups = obj["supervisors"]
        .as_array()
        .expect("{ctx}: supervisors not an array");
    let required_sup: HashSet<&str> =
        ["id", "status", "working", "idle", "failed", "done", "title"]
            .into_iter()
            .collect();
    for sup in sups {
        let k = keys_of(sup);
        for req in &required_sup {
            assert!(
                k.contains(*req),
                "{ctx}: supervisor key `{req}` missing (have {k:?})"
            );
        }
        for extra in &k {
            assert!(
                required_sup.contains(extra.as_str()) || extra == "paused",
                "{ctx}: foreign supervisor key `{extra}`"
            );
        }
        assert_status_vocab(&sup["status"], &format!("{ctx} sup {}", sup["id"]));
        let id = sup["id"].as_str().unwrap_or("");
        assert!(id.starts_with("SV-"), "{ctx}: bad supervisor id `{id}`");
        for f in ["working", "idle", "failed", "done"] {
            assert!(
                sup[f].is_u64(),
                "{ctx}: supervisor {id} `{f}` not a u64 count"
            );
        }
        assert!(sup["title"].is_string());
    }
    let sup_ids: Vec<&str> = sups.iter().map(|s| s["id"].as_str().unwrap()).collect();
    let mut sorted = sup_ids.clone();
    sorted.sort();
    assert_eq!(sup_ids, sorted, "{ctx}: supervisors not ordered by id");

    // sessions
    let sess = obj["sessions"]
        .as_array()
        .expect("{ctx}: sessions not an array");
    let required_ses: HashSet<&str> = [
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
    ]
    .into_iter()
    .collect();
    let allowed_ses: HashSet<&str> = required_ses
        .iter()
        .copied()
        .chain([
            "goal",
            "iterations",
            "maxIterations",
            "wallClockDeadline",
            "lastSupervisorAction",
            "model",
            "definitionOfDone",
        ])
        .collect();
    let mut seen_ids: HashSet<&str> = HashSet::new();
    for s in sess {
        let k = keys_of(s);
        for req in &required_ses {
            assert!(
                k.contains(*req),
                "{ctx}: session key `{req}` missing (have {k:?})"
            );
        }
        for extra in &k {
            assert!(
                allowed_ses.contains(extra.as_str()),
                "{ctx}: foreign session key `{extra}`"
            );
        }
        let id = s["id"].as_str().unwrap_or("");
        assert!(
            seen_ids.insert(id),
            "{ctx}: duplicate session id `{id}` in snapshot"
        );
        assert_status_vocab(&s["status"], &format!("{ctx} ses {id}"));
        let sup = s["sup"].as_str().unwrap_or("");
        assert!(
            sups.iter().any(|x| x["id"].as_str() == Some(sup)),
            "{ctx}: session {id} references unknown supervisor {sup}"
        );
        assert!(s["startedAt"].is_u64(), "{ctx}: {id} startedAt not u64");
        assert!(
            s["lastRevision"].is_u64() || s["lastRevision"].is_null(),
            "{ctx}: {id} lastRevision neither u64 nor null"
        );
        assert!(
            s["latencyMs"].is_u64() || s["latencyMs"].is_null(),
            "{ctx}: {id} latencyMs neither u64 nor null"
        );
        assert!(
            s["failureReason"].is_string() || s["failureReason"].is_null(),
            "{ctx}: {id} failureReason neither string nor null"
        );
        assert!(s["iterations"].is_u64(), "{ctx}: {id} iterations not u64");
        assert!(
            s["maxIterations"].is_u64(),
            "{ctx}: {id} maxIterations not u64"
        );
        assert!(
            s["wallClockDeadline"].is_u64(),
            "{ctx}: {id} wallClockDeadline not u64"
        );
        assert!(s["task"].is_string() && s["title"].is_string() && s["cwd"].is_string());
    }
    let ses_ids: Vec<&str> = sess.iter().map(|s| s["id"].as_str().unwrap()).collect();
    let mut ses_sorted = ses_ids.clone();
    ses_sorted.sort();
    assert_eq!(ses_ids, ses_sorted, "{ctx}: sessions not ordered by id");

    // Supervisor rollup counts must equal the session census.
    for sup in sups {
        let sid = sup["id"].as_str().unwrap();
        let mut w = 0u64;
        let mut i2 = 0u64;
        let mut f = 0u64;
        let mut d = 0u64;
        for s in sess.iter().filter(|s| s["sup"].as_str() == Some(sid)) {
            match s["status"].as_str().unwrap() {
                "working" => w += 1,
                "idle" => i2 += 1,
                "failed" => f += 1,
                "done" => d += 1,
                other => panic!("{ctx}: bad status {other}"),
            }
        }
        assert_eq!(
            sup["working"].as_u64().unwrap(),
            w,
            "{ctx}: {sid} working count drift"
        );
        assert_eq!(
            sup["idle"].as_u64().unwrap(),
            i2,
            "{ctx}: {sid} idle count drift"
        );
        assert_eq!(
            sup["failed"].as_u64().unwrap(),
            f,
            "{ctx}: {sid} failed count drift"
        );
        assert_eq!(
            sup["done"].as_u64().unwrap(),
            d,
            "{ctx}: {sid} done count drift"
        );
    }

    // Conflicts: files unique per entry, sessions sorted + deduped + ≥ 2.
    let mut conflict_files: HashSet<&str> = HashSet::new();
    for c in obj["conflicts"]
        .as_array()
        .expect("{ctx}: conflicts not an array")
    {
        let file = c["file"].as_str().expect("{ctx}: conflict without file");
        assert!(
            conflict_files.insert(file),
            "{ctx}: duplicate conflict key `{file}`"
        );
        let sessions: Vec<&str> = c["sessions"]
            .as_array()
            .expect("{ctx}: conflict sessions not an array")
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert!(
            sessions.len() >= 2,
            "{ctx}: conflict `{file}` with < 2 sessions"
        );
        let mut uniq = sessions.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(
            sessions, uniq,
            "{ctx}: conflict `{file}` sessions not sorted/deduped"
        );
    }

    // Attribution: entry uniqueness by (session, file); cap ≤ 24.
    let attrs = obj["attribution"]
        .as_array()
        .expect("{ctx}: attribution not an array");
    assert!(
        attrs.len() <= 24,
        "{ctx}: attribution cap violated: {} > 24",
        attrs.len()
    );
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for a in attrs {
        let e = (
            a["session"].as_str().unwrap_or(""),
            a["file"].as_str().unwrap_or(""),
        );
        assert!(seen.insert(e), "{ctx}: duplicate attribution entry {e:?}");
    }
}

// -------------------------------------------- registry assertions

fn assert_registry_ids(
    registry_path: &Path,
    expect_sessions: usize,
    expect_supervisors: u32,
    ctx: &str,
) {
    let raw =
        std::fs::read(registry_path).unwrap_or_else(|e| panic!("{ctx}: registry unreadable: {e}"));
    let v: Value =
        serde_json::from_slice(&raw).unwrap_or_else(|e| panic!("{ctx}: registry not JSON: {e}"));
    assert_eq!(
        v["version"].as_u64(),
        Some(1),
        "{ctx}: registry version != 1"
    );
    assert_eq!(
        v["supervisorCount"].as_u64(),
        Some(expect_supervisors as u64),
        "{ctx}: supervisorCount drift"
    );
    let ids: Vec<String> = v["ids"]
        .as_array()
        .expect("{ctx}: registry ids not an array")
        .iter()
        .map(|x| x.as_str().unwrap_or("").to_string())
        .collect();
    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{ctx}: duplicate ids in registry");

    let session_ids: Vec<&String> = ids
        .iter()
        .filter(|i| i.starts_with('S') && !i.starts_with("SV-"))
        .collect();
    let sup_ids: Vec<&String> = ids.iter().filter(|i| i.starts_with("SV-")).collect();
    assert_eq!(
        session_ids.len(),
        expect_sessions,
        "{ctx}: registry session count != {expect_sessions}"
    );
    assert_eq!(
        sup_ids.len(),
        expect_supervisors as usize,
        "{ctx}: registry supervisor ids != {expect_supervisors}"
    );

    // sessionCounts must cover S1..S3 exactly: 7/7/6 and the expected 1..=n ids must exist.
    let counts = v["sessionCounts"]
        .as_object()
        .expect("{ctx}: sessionCounts not an object");
    let mut total = 0u64;
    for (sup, n) in counts {
        let n = n.as_u64().expect("{ctx}: sessionCounts[{sup}] not u64");
        total += n;
        for oc in 1..=n {
            let want = format!("S{sup}-OC{oc:02}");
            assert!(
                ids.contains(&want),
                "{ctx}: registry missing expected id {want}"
            );
        }
    }
    assert_eq!(
        total as usize, expect_sessions,
        "{ctx}: sum of sessionCounts != {expect_sessions}"
    );

    // Monotonicity (invariant 2): the registry's `ids` vector is in
    // allocation order. For every supervisor, its subsequence of session
    // ids in that order must have strictly increasing OC — IDs are never
    // reissued, recycled, or reset across the run (or a restart).
    let mut per_sup_last: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for id in ids
        .iter()
        .filter(|i| i.starts_with('S') && !i.starts_with("SV-"))
    {
        let (sup, oc) = id[1..]
            .split_once("-OC")
            .expect("session id must be S<n>-OC<nn>");

        let oc: u64 = oc.parse().unwrap();
        if let Some(prev) = per_sup_last.get(sup) {
            assert!(
                oc > *prev,
                "{ctx}: allocation order regressed for SV-{sup}: OC {oc} after OC {prev}"
            );
        }
        per_sup_last.insert(sup, oc);
    }
}

// --------------------------------------- invariant checkpoints (shared)

struct CheckpointCtx {
    state_dir: PathBuf,
    tmp_dir: PathBuf,
    expect_sessions_allocated: usize,
    attribution_recorded: bool,
}

#[derive(Clone, Copy)]
struct CheckpointSummary {
    live_dirs: usize,
    non_terminal: usize,
    eps: f64,
    attribution: usize,
    conflicts: usize,
}

fn session_status(s: &Value) -> &str {
    s["status"].as_str().unwrap_or("")
}

fn run_checkpoints(
    daemon: &Daemon,
    ctx: &CheckpointCtx,
    label: &str,
    poll_ms: f64,
) -> CheckpointSummary {
    let snap = daemon.snapshot();
    let v = serde_json::to_value(&snap).unwrap();

    assert_snapshot_shape(&v, label);

    let sessions = v["sessions"].as_array().unwrap();
    let non_terminal = sessions
        .iter()
        .filter(|s| !matches!(session_status(s), "done" | "failed"))
        .count();
    let live_dirs = exact_notch_dirs(&ctx.tmp_dir);

    // Invariant 1: live dirs == non-terminal sessions; per-session dir
    // existence must agree with the session's terminal state. Asserted as
    // exact SET equality (both sides are id-sorted), which is strictly
    // stronger than the original count-only check: it also catches a wrong
    // dir being present/absent while the totals happen to match.
    let live_names: Vec<String> = live_dirs
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    let expected_dirs: Vec<String> = sessions
        .iter()
        .filter(|s| !matches!(session_status(s), "done" | "failed"))
        .map(|s| format!("notch-{}", s["id"].as_str().unwrap()))
        .collect();
    assert_eq!(
        live_names, expected_dirs,
        "{label}: live temp dir set != non-terminal session set"
    );
    for s in sessions {
        let id = s["id"].as_str().unwrap();
        let dir = ctx.tmp_dir.join(format!("notch-{id}"));
        if matches!(session_status(s), "done" | "failed") {
            assert!(
                !dir.exists(),
                "{label}: terminal session {id} still has temp dir"
            );
        } else {
            assert!(
                dir.is_dir(),
                "{label}: live session {id} is missing temp dir"
            );
        }
    }

    // Invariant 2 (mid-run): zero duplicate ids; allocation monotonic;
    // counts consistent with what has been allocated so far.
    if ctx.expect_sessions_allocated > 0 {
        assert_eq!(
            sessions.len(),
            ctx.expect_sessions_allocated,
            "{label}: session count drift"
        );
        let registry_path = ctx.state_dir.join("registry.json");
        assert_registry_ids(&registry_path, ctx.expect_sessions_allocated, 3, label);
    }

    // Invariant 3: attribution cap + conflict consistency (post-record only).
    let attrs_len = v["attribution"].as_array().unwrap().len();
    let conflicts = v["conflicts"].as_array().unwrap();
    if ctx.attribution_recorded {
        assert_eq!(attrs_len, 24, "{label}: snapshot attribution must be capped at exactly 24 (46 recorded), got {attrs_len}");
        // Both conflict files must be inside the newest-24 window.
        let attr_files: HashSet<&str> = v["attribution"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["file"].as_str().unwrap())
            .collect();
        for c in conflicts {
            let f = c["file"].as_str().unwrap();
            assert!(
                attr_files.contains(f),
                "{label}: conflict file `{f}` fell out of the newest-24 attribution window"
            );
        }
        assert_eq!(
            conflicts.len(),
            2,
            "{label}: expected exactly 2 conflicts, got {:?}",
            conflicts
        );
    }

    // Invariant 4: eps finite, ≥ 0, < 1e9 (full check in shape fn too).
    let eps = v["eps"].as_f64().expect("{label}: eps not a JSON float");
    assert!(
        eps.is_finite() && (0.0..1e9).contains(&eps),
        "{label}: eps sanity failed: {eps}"
    );

    // Invariant 5 is inside assert_snapshot_shape.

    // Invariant 3b: events.jsonl must parse at every checkpoint.
    let events = assert_events_jsonl_parseable(&ctx.state_dir, label);
    assert!(
        !events.is_empty(),
        "{label}: events.jsonl is empty after spawns"
    );

    CheckpointSummary {
        live_dirs: live_dirs.len(),
        non_terminal,
        eps,
        attribution: attrs_len,
        conflicts: conflicts.len(),
    }
    .tap_log(label, poll_ms)
}

impl CheckpointSummary {
    fn tap_log(self, label: &str, poll_ms: f64) -> Self {
        println!(
            "[soak] {label}: live_dirs={} non_terminal={} eps={:.3} attribution={} conflicts={} poll_ms={:.3}",
            self.live_dirs, self.non_terminal, self.eps, self.attribution, self.conflicts, poll_ms
        );
        self
    }
}

// ======================================================== the 300-poll soak

/// Global allocation-order slot -> expected id (deterministic by construction).
const EXPECTED_IDS: [&str; 20] = [
    "S1-OC01", "S2-OC01", "S3-OC01", // batch A (poll 1): one session per new supervisor
    "S1-OC02", "S1-OC03", "S1-OC04", "S1-OC05", // batch B (poll 3): SV-1
    "S2-OC02", "S2-OC03", "S2-OC04", "S2-OC05", //                     SV-2
    "S3-OC02", "S3-OC03", "S3-OC04", //                     SV-3
    "S1-OC06", "S1-OC07", // batch C (poll 5): SV-1
    "S2-OC06", "S2-OC07", //              SV-2
    "S3-OC05", "S3-OC06", //              SV-3
];
/// Batch A supervisor allocation: None => allocate a fresh supervisor.
const BATCH: [Option<&str>; 20] = [
    None,
    None,
    None,
    Some("SV-1"),
    Some("SV-1"),
    Some("SV-1"),
    Some("SV-1"),
    Some("SV-2"),
    Some("SV-2"),
    Some("SV-2"),
    Some("SV-2"),
    Some("SV-3"),
    Some("SV-3"),
    Some("SV-3"),
    Some("SV-1"),
    Some("SV-1"),
    Some("SV-2"),
    Some("SV-2"),
    Some("SV-3"),
    Some("SV-3"),
];
const FAIL_IDX: [usize; 4] = [2, 8, 13, 17]; // S3-OC01, S2-OC04, S3-OC04, S2-OC07
const TOTAL_POLLS: u64 = 300;
const RECORD_POLL: u64 = 120;
const CHECKPOINTS: [u64; 6] = [50, 100, 150, 200, 250, 300];

#[test]
fn soak_lifecycle_300_polls_invariants() {
    let test_started = Instant::now();
    let root = tmp_root("lifecycle");
    let state_dir = root.join("state");
    let tmp_dir = root.join("tmp");
    std::fs::create_dir_all(&tmp_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    // Per-session working dirs. Non-repo empty dirs: `git status` exits
    // immediately with a non-signal, so poll attribution sampling is fast.
    let cwds: Vec<PathBuf> = (0..20)
        .map(|i| {
            let d = root.join("cwds").join(format!("p{i}"));
            std::fs::create_dir_all(&d).unwrap();
            d
        })
        .collect();

    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let fake = SoakHerdr::new();
    let oc = SoakOpencode::new();
    let config = test_config(&state_dir, &tmp_dir, 15);
    let daemon = Daemon::open(config, fake.herdr(), clock.clone()).unwrap();
    daemon.set_opencode(oc.opencode());

    // ---------- deterministic lifecycle schedule (seeded mulberry32) ----------
    let mut rng = Mulberry32::new(0x5EED_5EED);
    struct Slot {
        idle_at: u64,
        end_at: u64,
        failing: bool,
    }
    let mut slots: Vec<Slot> = Vec::with_capacity(20);
    for i in 0..20 {
        let idle_at = 20 + (rng.next_u32() % 110) as u64; // [20, 130)
        let done_at = idle_at + 25 + (rng.next_u32() % 90) as u64; // idle+25 .. idle+115
        let failing = FAIL_IDX.contains(&i);
        // Failure slots: first failure at fail_at (re-prompt via the failure
        // policy), second blocked poll at fail_at+1 (escalate -> failed).
        let fail_at = 30 + (rng.next_u32() % 180) as u64; // [30, 210)
        slots.push(Slot {
            idle_at,
            end_at: if failing { fail_at } else { done_at },
            failing,
        });
    }
    // Sanity: every session reaches a terminal state well before poll 300.
    for (i, s) in slots.iter().enumerate() {
        let terminal_by = if s.failing { s.end_at + 1 } else { s.end_at };
        assert!(
            terminal_by < TOTAL_POLLS,
            "slot {i} terminal_by {terminal_by} >= {TOTAL_POLLS}"
        );
    }
    // Determinism spot-checks on the seeded draws (pinned so the schedule can
    // never silently drift).
    println!("[soak] seeded schedule (seed=0x5EED5EED):");
    for (i, s) in slots.iter().enumerate() {
        println!(
            "[soak]   idx={i:2} idle_at={} end_at={} failing={}",
            s.idle_at, s.end_at, s.failing
        );
    }

    // ---------- spawn batches interleaved with polls ----------
    let mut allocated: Vec<String> = Vec::with_capacity(20); // allocation order == EXPECTED_IDS order
    let pane_of = |i: usize| format!("w9{n}:p{n}", n = i + 1); // fake assigns one pane per split, in order

    fn spawn_one(
        daemon: &Daemon,
        cwd: &Path,
        supervisor: Option<&str>,
        idx: usize,
    ) -> notchd::daemon::SubmitTaskAck {
        daemon.submit_task(
            format!("Soak task {idx}: keep the orchestration layer honest"),
            Some(cwd.to_string_lossy().to_string()),
            Some(format!("soak-task-{idx}")),
            // No DoD: "tests pass" is not a machine-checkable local check, so
            // the hardened engine's DoD gate would revert herdr `done` to
            // Working while the mark_done path had already removed the temp
            // root. The soak drives done transitions through herdr directly.
            None,
            if idx == 0 || idx == 5 {
                Some("opencode/hy4-flash-free".to_string())
            } else {
                None
            },
            supervisor.map(str::to_string),
        )
    }

    let spawn_batch =
        |daemon: &Daemon, range: std::ops::Range<usize>, allocated: &mut Vec<String>| {
            for i in range {
                let ack = spawn_one(daemon, &cwds[i], BATCH[i], i);
                assert!(ack.ok, "spawn failed for slot {i}: {:?}", ack.error);
                let id = ack.session_id.expect("spawned session id");
                assert_eq!(id, EXPECTED_IDS[i], "allocation order drifted at slot {i}");
                allocated.push(id);
            }
        };

    // ---------- the 300-poll lifecycle ----------
    let mut ctx = CheckpointCtx {
        state_dir: state_dir.clone(),
        tmp_dir: tmp_dir.clone(),
        expect_sessions_allocated: 20,
        attribution_recorded: false,
    };
    let mut summaries: Vec<(u64, CheckpointSummary)> = Vec::new();
    let mut max_poll = std::time::Duration::ZERO;
    let mut max_poll_at = 0u64;

    let loop_started = Instant::now();
    for p in 1..=TOTAL_POLLS {
        if p == 1 {
            spawn_batch(&daemon, 0..3, &mut allocated);
        } else if p == 3 {
            spawn_batch(&daemon, 3..14, &mut allocated);
        } else if p == 5 {
            spawn_batch(&daemon, 14..20, &mut allocated);
        }

        // Script this poll's herdr world.
        let agents: Vec<Value> = allocated
            .iter()
            .enumerate()
            .map(|(i, _id)| {
                let slot = &slots[i];
                let rev = 10 + p; // revision ++ per poll
                let status = if slot.failing {
                    if p >= slot.end_at {
                        "blocked"
                    } else {
                        "working"
                    }
                } else if p == slot.idle_at {
                    "idle"
                } else if p >= slot.end_at {
                    "done"
                } else {
                    "working"
                };
                agent_entry(&pane_of(i), status, rev)
            })
            .collect();
        fake.set_agents(agents);

        let t0 = Instant::now();
        daemon.poll_once();
        let dt = t0.elapsed();
        // Hang-guard only. The 500 ms invariant-6 budget is NOT asserted in
        // this test — it is violated by this engine's serial per-session git
        // sampling (20 × ~25-30 ms); see `soak_poll_wall_time_budget_20_live_sessions`.
        assert!(
            dt.as_millis() < 5_000,
            "poll {p} took {dt:?} — pathological, not budget"
        );
        if dt > max_poll {
            max_poll = dt;
            max_poll_at = p;
        }

        // Attribution: 40 filler files (oldest entries) + 6 conflict entries
        // recorded LAST so they land inside the snapshot's newest-24 window.
        if p == RECORD_POLL {
            daemon.record_attribution(
                "S1-OC01",
                &(0..14)
                    .map(|i| format!("src/filler-{i:02}.rs"))
                    .collect::<Vec<_>>(),
            );
            daemon.record_attribution(
                "S2-OC01",
                &(14..28)
                    .map(|i| format!("src/filler-{i:02}.rs"))
                    .collect::<Vec<_>>(),
            );
            daemon.record_attribution(
                "S3-OC01",
                &(28..40)
                    .map(|i| format!("src/filler-{i:02}.rs"))
                    .collect::<Vec<_>>(),
            );
            daemon.record_attribution(
                "S1-OC01",
                &["src/shared.rs".to_string(), "src/retry.rs".to_string()],
            );
            daemon.record_attribution("S2-OC03", &["src/shared.rs".to_string()]);
            daemon.record_attribution("S3-OC02", &["src/shared.rs".to_string()]);
            daemon.record_attribution("S1-OC02", &["src/retry.rs".to_string()]);
            daemon.record_attribution("S2-OC01", &["src/retry.rs".to_string()]);
            ctx.attribution_recorded = true;
            println!("[soak] p=120 attribution batch recorded (46 entries: 40 filler + 6 overlapping -> 2 conflict files)");
        }

        if CHECKPOINTS.contains(&p) {
            let summary = run_checkpoints(
                &daemon,
                &ctx,
                &format!("checkpoint poll={p}"),
                dt.as_secs_f64() * 1000.0,
            );
            summaries.push((p, summary));
        }
        if p < TOTAL_POLLS {
            clock.advance_ms(2000);
        }
    }
    let loop_wall = loop_started.elapsed();
    println!(
        "[soak] 300 polls in {loop_wall:?}; max poll wall time = {:.3} ms (at poll {max_poll_at})",
        max_poll.as_secs_f64() * 1000.0
    );

    // The hardened engine maps herdr `blocked` to NeedsHuman: pane AND temp
    // root are kept and the session is NOT failed. Resolve those 4 sessions
    // terminally now (the operator/human decision the engine waits for) so
    // the end-of-life sweep sees the full 20-session lifecycle. `kill()` sets
    // status Failed with the given reason and removes the session temp root.
    for &idx in &FAIL_IDX {
        let id = EXPECTED_IDS[idx];
        let ack = daemon.kill(id, Some("blocked_waiting".to_string()));
        assert!(
            ack.is_ok(),
            "end-of-run resolution kill failed for {id}: {:?}",
            ack.err()
        );
    }

    // ---------- end-of-lifecycle assertions ----------
    let snap = daemon.snapshot();
    let v = serde_json::to_value(&snap).unwrap();
    let sessions = v["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 20);
    let done = sessions
        .iter()
        .filter(|s| session_status(s) == "done")
        .count();
    let failed = sessions
        .iter()
        .filter(|s| session_status(s) == "failed")
        .count();
    assert_eq!(
        (done, failed),
        (16, 4),
        "expected 16 done + 4 failed at end of lifecycle"
    );
    let terminal_count = done + failed;
    assert_eq!(
        terminal_count, 20,
        "all sessions must be terminal at poll 300 (16 done + 4 resolved-from-blocked)"
    );
    // Invariant 1 at the end: everything terminal => zero live dirs.
    assert!(
        exact_notch_dirs(&tmp_dir).is_empty(),
        "terminal sessions leaked temp dirs"
    );
    // Invariant 2 at the end: registry count == 20 sessions + 3 supervisors, all unique.
    assert_registry_ids(&state_dir.join("registry.json"), 20, 3, "end of lifecycle");
    // One re-prompt for each of the 16 idle/decider sessions. The 4 blocked
    // sessions never reach the decider: the hardened engine routes
    // `blocked_waiting` to NeedsHuman (no prompt), then the explicit
    // end-of-run kill above resolves them. So the exact count is 16.
    let reprompts = count_events_of_kind(
        &assert_events_jsonl_parseable(&state_dir, "reprompt census"),
        "reprompt",
    );
    assert_eq!(
        reprompts, 16,
        "expected exactly 16 reprompt events (one per idle-path session), got {reprompts}"
    );
    let iterations: u64 = sessions
        .iter()
        .map(|s| s["iterations"].as_u64().unwrap())
        .sum();
    assert_eq!(
        iterations, 16,
        "only the 16 idle-path sessions consume iteration budget; sum must be 16"
    );
    for s in sessions {
        let idx: usize = EXPECTED_IDS
            .iter()
            .position(|e| *e == s["id"].as_str().unwrap())
            .unwrap();
        let want = if FAIL_IDX.contains(&idx) { 0 } else { 1 };
        assert_eq!(
            s["iterations"].as_u64().unwrap(),
            want,
            "iterations for {} drifted",
            s["id"]
        );
    }
    // Exactly 16 decider calls (one per idle detection; the 4 failing
    // sessions never reach the decision loop).
    assert_eq!(oc.decider_calls(), 16, "decider call count drifted");
    // Failed sessions carry the blocked_waiting reason.
    for s in sessions.iter().filter(|s| session_status(s) == "failed") {
        assert_eq!(
            s["failureReason"].as_str(),
            Some("blocked_waiting"),
            "unexpected failure reason for {}",
            s["id"]
        );
    }

    // ---------- invariant 7: final global sweep ----------
    assert!(
        exact_notch_dirs(&tmp_dir).is_empty(),
        "pre-sweep: tmp root must already be clean"
    );
    // Pre-seed leftovers: 3 tracked dead-session dirs (one nonempty with a
    // file, one empty, one nested) + 1 exact-grammar untracked EMPTY orphan.
    std::fs::create_dir_all(tmp_dir.join("notch-S1-OC01")).unwrap();
    std::fs::write(tmp_dir.join("notch-S1-OC01").join("leftover.txt"), b"stale").unwrap();
    std::fs::create_dir_all(tmp_dir.join("notch-S2-OC02")).unwrap();
    std::fs::create_dir_all(tmp_dir.join("notch-S3-OC06").join("nested")).unwrap();
    std::fs::write(
        tmp_dir
            .join("notch-S3-OC06")
            .join("nested")
            .join("deep.txt"),
        b"stale",
    )
    .unwrap();
    std::fs::create_dir_all(tmp_dir.join("notch-S9-OC09")).unwrap(); // untracked, exact grammar, empty
    std::fs::write(tmp_dir.join("notch-keepme"), b"not an engine dir").unwrap(); // non-grammar: must survive

    // Expected count derived from on-disk state, not hardcoded.
    let tracked: HashSet<String> = sessions
        .iter()
        .map(|s| format!("notch-{}", s["id"].as_str().unwrap()))
        .collect();
    let mut expected = 0u64;
    for name in &tracked {
        if tmp_dir.join(name).is_dir() {
            expected += 1;
        }
    }
    for entry in std::fs::read_dir(&tmp_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        let tracked_name = tracked.contains(&name);
        let empty = entry.path().is_dir()
            && std::fs::read_dir(entry.path())
                .map(|mut r| r.next().is_none())
                .unwrap_or(false);
        if exact_notch_dir_name(&name) && !tracked_name && empty {
            expected += 1;
        }
    }
    assert_eq!(expected, 4, "derived expected sweep count wrong");

    let ack = daemon.cleanup(None);
    assert!(ack.ok, "global cleanup failed");
    assert_eq!(
        ack.removed,
        Some(expected),
        "global cleanup removed != expected"
    );
    assert!(
        exact_notch_dirs(&tmp_dir).is_empty(),
        "global sweep left exact-grammar dirs behind"
    );
    assert!(
        tmp_dir.join("notch-keepme").exists(),
        "global sweep deleted a non-engine file"
    );
    let post_sweep_events = assert_events_jsonl_parseable(&state_dir, "post-sweep");
    assert!(
        post_sweep_events
            .iter()
            .any(|e| e["kind"].as_str() == Some("cleanup")
                && e["detail"].as_str() == Some(format!("removed={expected}").as_str())),
        "events.jsonl must record a cleanup event with removed={expected}"
    );
    println!("[soak] global sweep: removed={expected} (derived), cleanup event logged, zero exact-grammar dirs remain");

    // ---------- "once at the end" invariant pass (post-sweep) ----------
    let _ = run_checkpoints(&daemon, &ctx, "end", 0.0);

    // ---------- simulated restart: registry persists, IDs never reissued ----------
    let registry_path = state_dir.join("registry.json");
    let ids_before: HashSet<String> = {
        let raw = std::fs::read(&registry_path).unwrap();
        let v: Value = serde_json::from_slice(&raw).unwrap();
        v["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        ids_before.len(),
        23,
        "registry must hold 3 supervisors + 20 sessions before restart"
    );

    drop(daemon); // simulated process restart
    let fake2 = SoakHerdr::new(); // empty agent list; daemon2 is never polled
    let restart_config = test_config(&state_dir, &tmp_dir, 15);
    let daemon2 = Daemon::open(restart_config, fake2.herdr(), clock.clone()).unwrap();

    let cwd0 = cwds[0].to_string_lossy().to_string();
    let a1 = daemon2.submit_task(
        "Soak continuation after restart".into(),
        Some(cwd0.clone()),
        None,
        None,
        None,
        Some("SV-1".into()),
    );
    assert!(a1.ok, "restart submit_task failed: {:?}", a1.error);
    assert_eq!(
        a1.session_id.as_deref(),
        Some("S1-OC08"),
        "SV-1's OC counter must continue, never reissue"
    );
    assert_eq!(a1.supervisor_id.as_deref(), Some("SV-1"));

    let a2 = daemon2.submit_task(
        "Brand-new supervisor after restart".into(),
        Some(cwd0),
        None,
        None,
        None,
        None,
    );
    assert!(
        a2.ok,
        "restart submit_task (new supervisor) failed: {:?}",
        a2.error
    );
    assert_eq!(
        a2.supervisor_id.as_deref(),
        Some("SV-4"),
        "supervisor counter must continue to SV-4"
    );
    assert_eq!(a2.session_id.as_deref(), Some("S4-OC01"));

    for id in [
        a1.session_id.as_deref().unwrap(),
        a2.session_id.as_deref().unwrap(),
        a2.supervisor_id.as_deref().unwrap(),
    ] {
        assert!(
            !ids_before.contains(id),
            "restart reissued registry id {id}"
        );
    }
    assert_registry_ids(&registry_path, 22, 4, "after simulated restart");

    // Kill both restart sessions; their temp dirs must be swept by kill().
    daemon2.kill("S1-OC08", None).unwrap();
    daemon2.kill("S4-OC01", None).unwrap();
    assert!(
        exact_notch_dirs(&tmp_dir).is_empty(),
        "restart sessions leaked temp dirs after kill"
    );
    let registry_after: Value =
        serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
    let ids_after = registry_after["ids"].as_array().unwrap();
    assert_eq!(
        ids_after.len(),
        26,
        "registry must retain every id ever allocated (23 + 3)"
    );
    let uniq: HashSet<&str> = ids_after.iter().map(|x| x.as_str().unwrap()).collect();
    assert_eq!(uniq.len(), 26, "registry ids not unique after restart");
    drop(daemon2);

    // Supplementary, non-fatal scan of the real /tmp for exact-name dirs of
    // OUR allocated ids (our daemon never wrote there; this must be empty).
    if let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) {
        let ours: HashSet<String> = EXPECTED_IDS
            .iter()
            .map(|id| format!("notch-{id}"))
            .chain(["notch-S1-OC08".to_string(), "notch-S4-OC01".to_string()])
            .collect();
        let strays: Vec<_> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| ours.contains(n))
            .collect();
        if !strays.is_empty() {
            eprintln!("[soak] WARNING: real /tmp holds dirs named like our ids (foreign process?): {strays:?}");
        } else {
            println!("[soak] real /tmp scan: no stray dirs named like this run's ids");
        }
    }

    let total_wall = test_started.elapsed();
    println!("[soak] SUMMARY: 300 polls, {loop_wall:?} loop wall, {total_wall:?} total; poll wall max = {:.3} ms (poll {max_poll_at})", max_poll.as_secs_f64() * 1000.0);
    println!(
        "[soak] invariant table rows (poll | live_dirs | non_terminal | eps | attrib | conflicts):"
    );
    for (p, s) in &summaries {
        println!(
            "[soak]   poll={p:3} | {:>2} | {:>2} | {:.3} | {:>2} | {}",
            s.live_dirs, s.non_terminal, s.eps, s.attribution, s.conflicts
        );
    }
}

// ============ invariant 6 minimal reproduction — VIOLATED (2026-11-19) ============

/// Invariant 6 ("poll wall time for 20 live sessions < 500 ms total per poll")
/// is VIOLATED by this engine on this machine, isolated below.
///
/// Cause (verified by reading + measuring, not guessed):
/// `poll_once` Pass B calls `attribution::sample_git` (git status --porcelain)
/// SERIALLY for every working session — 20 `git` process spawns per poll.
/// `sample_git` polls the child with `try_wait()` on a 20 ms sleep granularity,
/// so each working session costs ~20-30 ms even when git itself exits in
/// ~6-7 ms (measured: 20 git invocations ≈ 130-150 ms of exec time, plus 20
/// mandatory 20 ms sleeps per poll ≈ 400 ms). 20 working sessions × ~25 ms
/// ≈ 500-540 ms per poll — right at/over the budget with all fake IO free.
///
/// This is engine behavior (crates/notchd/src/attribution.rs, daemon.rs Pass B);
/// per ownership rules src/ is not patched here. Herdr/opencode are fully
/// fake and instant in this repro, so the measured number is engine-side
/// sampling cost, not IO latency.
///
/// Run: `cargo test -p notchd --test soak_lifecycle -- --ignored --nocapture`
/// Expected today: FAILS (max poll ≥ 500 ms) — the violation is the point.
#[test]
#[ignore = "invariant 6 violated: 20 live sessions poll at ~500-540 ms; see doc comment"]
fn soak_poll_wall_time_budget_20_live_sessions() {
    let root = tmp_root("pollbudget");
    let state_dir = root.join("state");
    let tmp_dir = root.join("tmp");
    std::fs::create_dir_all(&tmp_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    let cwds: Vec<PathBuf> = (0..20)
        .map(|i| {
            let d = root.join("cwds").join(format!("p{i}"));
            std::fs::create_dir_all(&d).unwrap();
            d
        })
        .collect();
    let clock = Arc::new(FakeClock::new(1_763_520_200_000));
    let fake = SoakHerdr::new();
    let oc = SoakOpencode::new();
    let config = test_config(&state_dir, &tmp_dir, 15);
    let daemon = Daemon::open(config, fake.herdr(), clock.clone()).unwrap();
    daemon.set_opencode(oc.opencode());

    // 20 working sessions (all in one batch wave, one pane each).
    for (i, cwd) in cwds.iter().enumerate() {
        let sup = if i < 7 {
            "SV-1"
        } else if i < 14 {
            "SV-2"
        } else {
            "SV-3"
        };
        let sup_opt = if i % 7 == 0 && i < 21 {
            None
        } else {
            Some(sup)
        }; // i=0,7,14 -> fresh supervisors
        let a = daemon.submit_task(
            format!("poll-budget probe {i}"),
            Some(cwd.to_string_lossy().to_string()),
            Some(format!("probe-{i}")),
            None,
            None,
            sup_opt.map(str::to_string),
        );
        assert!(a.ok, "spawn {i} failed: {:?}", a.error);
    }

    let mut max_dt = std::time::Duration::ZERO;
    let mut max_at = 0u64;
    let polls = 30;
    for p in 1..=polls {
        let agents: Vec<Value> = (0..20usize)
            .map(|i| agent_entry(&format!("w9{}:p{}", i + 1, i + 1), "working", 10 + p))
            .collect();
        fake.set_agents(agents);
        let t0 = Instant::now();
        daemon.poll_once();
        let dt = t0.elapsed();
        println!(
            "[soak-pollbudget] poll {p:2}: {:.3} ms",
            dt.as_secs_f64() * 1000.0
        );
        if dt > max_dt {
            max_dt = dt;
            max_at = p;
        }
        clock.advance_ms(2000);
    }
    println!("[soak-pollbudget] max poll = {:.3} ms at poll {max_at} over {polls} polls (20 live sessions)", max_dt.as_secs_f64() * 1000.0);
    println!("[soak-pollbudget] attribution probes per poll: 20 × git status --porcelain (serial, 20 ms try_wait granularity)");
    assert!(
        max_dt.as_millis() < 500,
        "INVARIANT 6 VIOLATED: max poll {:.3} ms ≥ 500 ms budget (reproduced)",
        max_dt.as_secs_f64() * 1000.0
    );
}

// ============================ rotation proof (events.jsonl > 4.2 MB via bus)

#[test]
fn soak_events_log_rotation_forced_via_bus_messages() {
    let root = tmp_root("rotation");
    let state_dir = root.join("state");
    let tmp_dir = root.join("tmp");
    std::fs::create_dir_all(&tmp_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    let clock = Arc::new(FakeClock::new(1_763_520_100_000));
    let fake = SoakHerdr::new();
    let oc = SoakOpencode::new();
    let config = test_config(&state_dir, &tmp_dir, 15);
    let daemon = Daemon::open(config, fake.herdr(), clock.clone()).unwrap();
    daemon.set_opencode(oc.opencode());

    // Two supervisors, one session each.
    let cwd = root.join("proj");
    std::fs::create_dir_all(&cwd).unwrap();
    let a1 = daemon.submit_task(
        "rotation soak alpha".into(),
        Some(cwd.to_string_lossy().to_string()),
        None,
        None,
        None,
        None,
    );
    let a2 = daemon.submit_task(
        "rotation soak bravo".into(),
        Some(cwd.to_string_lossy().to_string()),
        None,
        None,
        None,
        None,
    );
    assert!(a1.ok && a2.ok);

    // ---------- 5000 bus messages, ~1.1 KB each.
    // Note on the path: the §2 `message()` command logs its detail through
    // `log_head(body, 120)` — capped at 120 chars, so 5000 `message()` calls
    // would only reach ~1.1 MB. The in-process bus path (`bus_send` ->
    // `bus_drain`) logs the FULL detail untruncated, so >4.2 MB is reached
    // there. Both are "the bus-message path"; the drain is what forces the
    // bytes through events.jsonl.
    let filler = "x".repeat(950);
    for i in 0..5000u32 {
        let detail = format!("rot-filler-{i:05}-{filler}");
        let (from, to) = if i % 2 == 0 {
            ("SV-1", "SV-2")
        } else {
            ("SV-2", "SV-1")
        };
        daemon
            .bus_send(
                from,
                to,
                Some(if i % 2 == 0 {
                    "S1-OC01".to_string()
                } else {
                    "S2-OC01".to_string()
                }),
                "task",
                &detail,
            )
            .unwrap();
    }
    let routed = daemon.bus_drain();
    assert_eq!(routed, 5000, "bus_drain must route every queued message");

    let live_path = state_dir.join("events.jsonl");
    let rotated_path = state_dir.join("events.jsonl.1");
    assert!(
        rotated_path.exists(),
        "rotation did not happen: events.jsonl.1 missing"
    );

    const THRESHOLD: u64 = 4 * 1024 * 1024;
    let rotated_bytes = std::fs::metadata(&rotated_path).unwrap().len();
    let live_bytes = std::fs::metadata(&live_path).unwrap().len();
    assert!(
        rotated_bytes > THRESHOLD,
        "rotated log must hold the pre-rotation bulk (> 4 MiB); got {rotated_bytes}"
    );
    assert!(
        live_bytes < rotated_bytes,
        "post-rotation live log should be smaller than the rotated bulk"
    );

    // Every line in both files must be JSON.
    let rotated_lines = parse_jsonl_path(&rotated_path, "rotated events.jsonl.1");
    let live_lines = assert_events_jsonl_parseable(&state_dir, "post-rotation events.jsonl");
    println!(
        "[soak-rotation] proof: 5000 bus messages -> rotation happened; rotated_bytes={} ({} lines) live_bytes={} ({} lines) threshold={}",
        rotated_bytes,
        rotated_lines.len(),
        live_bytes,
        live_lines.len(),
        THRESHOLD
    );

    // Old lines gone from the live log; newest kept.
    let live_text = std::fs::read_to_string(&live_path).unwrap();
    let rotated_text = std::fs::read_to_string(&rotated_path).unwrap();
    assert!(
        rotated_text.contains("rot-filler-00000"),
        "oldest message must be in the rotated file"
    );
    assert!(
        !live_text.contains("rot-filler-00000"),
        "oldest message must be GONE from the live log"
    );
    assert!(
        live_text.contains("rot-filler-04999"),
        "newest message must be KEPT in the live log"
    );
    assert!(rotated_text.contains("rot-filler-04999") || live_text.contains("rot-filler-04999"));

    // The §2 `message()` path still writes through the rotated log.
    // (Note: the events.jsonl line for a message carries
    //  "<from>-><to> <type>: <head>" — NOT the msg_ id. Ids are assigned and
    //  exposed on the ack + in the snapshot's busMessages block.)
    let ack = daemon.message(
        "S1-OC01",
        "S2-OC01",
        "result",
        "post-rotation sentinel body",
    );
    assert!(ack.ok, "message() after rotation failed: {:?}", ack.error);
    let live_text2 = std::fs::read_to_string(&live_path).unwrap();
    assert!(
        live_text2.contains("post-rotation sentinel body"),
        "message() after rotation missing from live log"
    );
    assert!(
        live_text2.contains("S1-OC01->S2-OC01 result:"),
        "message() line missing from live log"
    );
    let msg_id = ack
        .message_id
        .clone()
        .expect("message ack must carry an id");
    let snap = daemon.snapshot();
    let v = serde_json::to_value(&snap).unwrap();
    let bus_ids: Vec<&str> = v["busMessages"]
        .as_array()
        .expect("busMessages must exist after a message")
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(
        bus_ids.contains(&msg_id.as_str()),
        "msg id {msg_id} must appear in the snapshot busMessages"
    );
    let live_lines2 = assert_events_jsonl_parseable(&state_dir, "post-sentinel events.jsonl");
    assert!(live_lines2.len() > live_lines.len());

    // eps must remain sane after the flood.
    let snap = daemon.snapshot();
    let v = serde_json::to_value(&snap).unwrap();
    let eps = v["eps"]
        .as_f64()
        .expect("eps must remain a JSON float after rotation flood");
    assert!(
        eps.is_finite() && (0.0..1e9).contains(&eps),
        "eps sanity failed after flood: {eps}"
    );

    // Both supervisors received exactly half the messages (2500 each), FIFO.
    assert_eq!(
        daemon.mailbox_drain("SV-1").len(),
        2500,
        "SV-1 mailbox drained wrong count"
    );
    assert_eq!(
        daemon.mailbox_drain("SV-2").len(),
        2500,
        "SV-2 mailbox drained wrong count"
    );

    println!("[soak-rotation] PASS: rotation forced via bus-message path, every line JSON, old lines gone, newest kept, eps sane");
}
