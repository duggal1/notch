//! In-process orchestrator engine — the same logic `notchd serve` runs,
//! implemented inside src-tauri because Agent A's crates/notchd has no public
//! API yet (see Cargo.toml → `notchd`, optional + `notchd-live`).
//!
//! MISSING notchd API (for the lead at integration):
//!   • a programmatic `serve` entry point / subscribe channel instead of our
//!     poll loop,
//!   • the snapshot builder (crates/notchd builds §1.3 snapshots from its tree),
//!   • the plan executor (`notchd plan <file>`),
//!   • the registry allocator (ID allocation rules, permanent ids),
//!   • the watchdog (stuck-session detection → kill_failed) — NOT implemented
//!     here on purpose; inventing watchdog policy inside the shell would
//!     duplicate Agent A's unfinished work. Vanished panes ARE marked done
//!     (re-adopt rule from bridge.md §6) and failures flow through spawn/kill.
//!
//! What IS implemented: state dir creation, ID registry (permanent, never
//! recycled), herdr client (agent list / pane split / agent start / agent
//! prompt / agent read / agent focus / pane close), vanished-pane detection,
//! eps estimation from events.jsonl, and the mpsc → emit → `app.emit` chain
//! with a 150 ms debounce.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::snapshot::{
    compare_ids, Attribution, Conflict, Orchestrator, Session, Snapshot, Status, Supervisor,
    SNAPSHOT_VERSION,
};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const HERDR_TIMEOUT: Duration = Duration::from_secs(5);
const EVENTS_ROTATE_BYTES: u64 = 4 * 1024 * 1024;
const EVENTS_EPS_WINDOW_MS: i64 = 10_000;
const EVENTS_EPS_READ_BYTES: u64 = 64 * 1024;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn single_line(s: &str, max: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        one.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

// ---------------------------------------------------------------------------
// Registry — the only ID allocator (bridge.md §1.2). Permanent ids, never reused.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub sup: String,
    pub pane_id: String,
    pub cwd: String,
    pub title: String,
    pub task: String,
    pub started_at: i64,
    pub status: Status,
    pub failure_reason: Option<String>,
    pub last_revision: Option<u64>,
    pub last_action: Option<String>,
    pub latency_ms: Option<f64>,
}

impl Entry {
    fn to_session(&self) -> Session {
        Session {
            id: self.id.clone(),
            sup: self.sup.clone(),
            status: self.status,
            title: self.title.clone(),
            task: self.task.clone(),
            cwd: self.cwd.clone(),
            pane_id: self.pane_id.clone(),
            started_at: self.started_at,
            last_action: self.last_action.clone().unwrap_or_default(),
            last_revision: self.last_revision.unwrap_or(0),
            latency_ms: self.latency_ms,
            failure_reason: self.failure_reason.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Registry {
    version: u32,
    supervisor_count: u32,
    session_seq_by_sup: BTreeMap<String, u32>,
    entries: Vec<Entry>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            version: 1,
            supervisor_count: 0,
            session_seq_by_sup: BTreeMap::new(),
            entries: Vec::new(),
        }
    }
}

impl Registry {
    fn load(state_dir: &Path) -> Registry {
        let path = state_dir.join("registry.json");
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                // Never destroy a corrupt registry: back it up, start clean.
                let _ = fs::write(state_dir.join("registry.json.bak"), &text);
                eprintln!("[notch] registry.json unreadable ({e}); backed up to registry.json.bak");
                Registry::default()
            }),
            Err(_) => Registry::default(),
        }
    }

    fn save(&self, state_dir: &Path) -> Result<(), String> {
        let path = state_dir.join("registry.json");
        let tmp = state_dir.join("registry.json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, &path).map_err(|e| format!("rename registry: {e}"))
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

// ---------------------------------------------------------------------------
// herdr client — the exact invocation surface from bridge.md §2 (read-only +
// the mutating verbs the commands expose). Nothing else; never `open`, never
// screencapture, never windows.
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize)]
struct HerdrAgent {
    pane_id: Option<String>,
    agent_status: Option<String>,
    revision: Option<u64>,
    cwd: Option<String>,
    terminal_title_stripped: Option<String>,
}

#[derive(Deserialize)]
struct HerdrAgentList {
    result: HerdrAgentListResult,
}

#[derive(Deserialize)]
struct HerdrAgentListResult {
    agents: Vec<HerdrAgent>,
}

fn run_with_timeout(mut cmd: Command) -> Result<std::process::Output, String> {
    let program = format!("{:?}", cmd.get_program());
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {program}: {e}"))?;
    let deadline = Instant::now() + HERDR_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|e| e.to_string()),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait_with_output();
                    return Err(format!("{program} timed out after {}s", HERDR_TIMEOUT.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(format!("wait {program}: {e}")),
        }
    }
}

fn herdr_text(herdr: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(herdr);
    cmd.args(args);
    let out = run_with_timeout(cmd)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("herdr {} exited with {}", args.join(" "), out.status)
        } else {
            err
        })
    }
}

fn herdr_agent_list(herdr: &str) -> Result<Vec<HerdrAgent>, String> {
    let text = herdr_text(herdr, &["agent", "list"])?;
    let list: HerdrAgentList =
        serde_json::from_str(&text).map_err(|e| format!("herdr agent list: non-JSON output ({e})"))?;
    Ok(list.result.agents)
}

/// Map herdr `agent_status` onto our four states (bridge.md §1.1).
/// `blocked` becomes `failed` with `blocked_waiting`.
fn map_status(raw: Option<&str>) -> (Status, Option<&'static str>) {
    match raw {
        Some("working") => (Status::Working, None),
        Some("done") => (Status::Done, None),
        Some("blocked") => (Status::Failed, Some("blocked_waiting")),
        Some("unknown") => (Status::Idle, None),
        Some("idle") => (Status::Idle, None),
        Some(_) => (Status::Idle, None),
        None => (Status::Idle, None),
    }
}

/// Extract a pane id (`w80:p2` form) from `herdr pane split` output. Tolerant:
/// JSON first (checked at depth ≤ 4), then a token scan for the id pattern.
pub fn parse_pane_id(stdout: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) {
        fn walk(v: &Value, depth: u8) -> Option<String> {
            if depth == 0 {
                return None;
            }
            match v {
                Value::Object(map) => {
                    for key in ["pane_id", "paneId", "id"] {
                        if let Some(Value::String(s)) = map.get(key) {
                            if s.contains(':') {
                                return Some(s.clone());
                            }
                        }
                    }
                    map.values().find_map(|v| walk(v, depth - 1))
                }
                Value::Array(items) => items.iter().find_map(|v| walk(v, depth - 1)),
                _ => None,
            }
        }
        if let Some(id) = walk(&v, 4) {
            return Some(id);
        }
    }
    let is_pane_token = |tok: &str| {
        let mut parts = tok.split(':');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(a), Some(b), None) => {
                !a.is_empty()
                    && !b.is_empty()
                    && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    && b.starts_with('p')
                    && b[1..].chars().all(|c| c.is_ascii_digit())
            }
            _ => false,
        }
    };
    // Token scan: split on whitespace and JSON-ish punctuation, but NOT on
    // ':' — the pane id itself contains one ("w80:p2").
    stdout
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '{' | '}' | '[' | ']' | ','))
        .map(str::trim)
        .find(|tok| is_pane_token(tok))
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsPayload {
    pub id: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPlanResult {
    pub ok: bool,
    pub spawned: Vec<String>,
    pub sessions: Vec<String>,
    pub warnings: Vec<String>,
}

pub struct Engine {
    state_dir: PathBuf,
    herdr: String,
    registry: Mutex<Registry>,
    last_fingerprint: Mutex<Option<String>>,
    tx: Sender<Snapshot>,
}

impl Engine {
    /// Create the state dir (~/Library/Application Support/notch/, honoring
    /// NOTCH_STATE_DIR), load the registry, and spawn the poll thread.
    /// Returns the engine (for commands) and the snapshot receiver.
    pub fn start() -> Result<(Arc<Engine>, Receiver<Snapshot>), String> {
        let state_dir = state_dir();
        fs::create_dir_all(&state_dir)
            .map_err(|e| format!("create state dir {}: {e}", state_dir.display()))?;
        let registry = Registry::load(&state_dir);
        // Materialize registry.json on first run — it is the only ID allocator
        // and consumers (and tests) should not have to distinguish "missing"
        // from "empty".
        if let Err(e) = registry.save(&state_dir) {
            eprintln!("[notch] could not write registry.json: {e}");
        }

        let (tx, rx) = mpsc::channel::<Snapshot>();
        let engine = Arc::new(Engine {
            state_dir,
            herdr: std::env::var("NOTCH_HERDR").unwrap_or_else(|_| "herdr".into()),
            registry: Mutex::new(registry),
            last_fingerprint: Mutex::new(None),
            tx,
        });
        engine.log_event("status", None, "notch shell engine started");

        let poll_engine = Arc::clone(&engine);
        std::thread::Builder::new()
            .name("notch-engine-poll".into())
            .spawn(move || poll_loop(poll_engine))
            .map_err(|e| format!("spawn engine poll thread: {e}"))?;

        Ok((engine, rx))
    }

    fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn log_event(&self, kind: &str, session: Option<&str>, detail: &str) {
        let line = json!({
            "ts": now_ms(),
            "session": session,
            "kind": kind,
            "detail": detail,
        });
        let path = self.state_dir.join("events.jsonl");
        // Rotate at 4 MB (bridge.md §5). Best effort — never fail a command.
        if let Ok(meta) = fs::metadata(&path) {
            if meta.len() > EVENTS_ROTATE_BYTES {
                let _ = fs::rename(&path, self.state_dir.join("events.jsonl.1"));
            }
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }

    fn eps_estimate(&self) -> f64 {
        let path = self.state_dir.join("events.jsonl");
        let mut f = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return 0.0,
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let start = len.saturating_sub(EVENTS_EPS_READ_BYTES);
        if f.seek(SeekFrom::Start(start)).is_err() {
            return 0.0;
        }
        let mut buf = String::new();
        if f.read_to_string(&mut buf).is_err() {
            return 0.0;
        }
        let cutoff = now_ms() - EVENTS_EPS_WINDOW_MS;
        let hits = buf
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v.get("ts").and_then(Value::as_i64).map(|t| t >= cutoff).unwrap_or(false))
            .count();
        (hits as f64) / (EVENTS_EPS_WINDOW_MS as f64 / 1000.0)
    }

    /// Build a snapshot from the registry merged with live herdr state.
    /// herdr failure is not fatal: sessions reflect the registry and a
    /// `warnings` entry says so.
    pub fn build_snapshot(&self) -> Snapshot {
        let now = now_ms();
        let mut warnings: Vec<String> = Vec::new();
        let agents = match herdr_agent_list(&self.herdr) {
            Ok(a) => Some(a),
            Err(e) => {
                warnings.push(format!("herdr unreachable ({e}); showing last-known registry state"));
                None
            }
        };

        let mut reg = lock(&self.registry);
        let mut mutated = false;
        if let Some(agents) = &agents {
            let by_pane: BTreeMap<&str, &HerdrAgent> = agents
                .iter()
                .filter_map(|a| a.pane_id.as_deref().map(|p| (p, a)))
                .collect();
            for entry in &mut reg.entries {
                if entry.pane_id.is_empty() {
                    continue;
                }
                match by_pane.get(entry.pane_id.as_str()) {
                    Some(agent) => {
                        let (status, reason) = map_status(agent.agent_status.as_deref());
                        if entry.status != status {
                            mutated = true;
                        }
                        match (status, reason) {
                            (Status::Failed, Some(r)) => entry.failure_reason = Some(r.to_string()),
                            (Status::Failed, None) => {
                                // Agency only clears its own blocked_waiting when work resumes.
                                if entry.failure_reason.as_deref() == Some("blocked_waiting") {
                                    entry.failure_reason = None;
                                }
                            }
                            (Status::Working | Status::Idle | Status::Done, _) => {
                                entry.failure_reason = None;
                            }
                        }
                        entry.status = status;
                        entry.last_revision = agent.revision.or(entry.last_revision);
                        if let Some(t) = &agent.terminal_title_stripped {
                            entry.last_action = Some(single_line(t, 80));
                        }
                        if entry.cwd.is_empty() {
                            if let Some(cwd) = &agent.cwd {
                                entry.cwd = cwd.clone();
                            }
                        }
                    }
                    None => {
                        // Re-adopt rule: a known pane that vanished from herdr
                        // is done (bridge.md §6). Already-failed stays failed.
                        if matches!(entry.status, Status::Working | Status::Idle) {
                            entry.status = Status::Done;
                            entry.failure_reason = None;
                            mutated = true;
                            self.log_event("status", Some(&entry.id), "pane vanished; marked done");
                        }
                    }
                }
            }
        }
        if mutated {
            let _ = reg.save(&self.state_dir);
        }

        // Supervisors: every allocated supervisor id (1..=count), plus any
        // sv referenced by entries above the counter (hand-edited registry).
        let mut sup_ids: Vec<String> = (1..=reg.supervisor_count)
            .map(|i| format!("SV-{i}"))
            .collect();
        for e in &reg.entries {
            if !sup_ids.contains(&e.sup) {
                sup_ids.push(e.sup.clone());
            }
        }
        sup_ids.sort_by(|a, b| compare_ids(a, b));

        let supervisors = sup_ids
            .into_iter()
            .map(|id| {
                let mut sv = Supervisor {
                    id: id.clone(),
                    status: Status::Idle,
                    working: 0,
                    idle: 0,
                    failed: 0,
                    done: 0,
                };
                for e in reg.entries.iter().filter(|e| e.sup == id) {
                    match e.status {
                        Status::Working => sv.working += 1,
                        Status::Idle => sv.idle += 1,
                        Status::Failed => sv.failed += 1,
                        Status::Done => sv.done += 1,
                    }
                }
                sv.status = aggregate_status(sv.working, sv.idle, sv.failed, sv.done);
                sv
            })
            .collect::<Vec<_>>();

        let mut sessions: Vec<Session> = reg.entries.iter().map(Entry::to_session).collect();
        sessions.sort_by(|a, b| compare_ids(&a.id, &b.id));

        let (w, i, f, d) = sessions.iter().fold((0, 0, 0, 0), |acc, s| match s.status {
            Status::Working => (acc.0 + 1, acc.1, acc.2, acc.3),
            Status::Idle => (acc.0, acc.1 + 1, acc.2, acc.3),
            Status::Failed => (acc.0, acc.1, acc.2 + 1, acc.3),
            Status::Done => (acc.0, acc.1, acc.2, acc.3 + 1),
        });
        let orchestrator = Orchestrator {
            id: "ORCH".into(),
            status: aggregate_status(w, i, f, d),
        };

        Snapshot {
            version: SNAPSHOT_VERSION,
            generated_at: now,
            orchestrator,
            supervisors,
            sessions,
            eps: self.eps_estimate(),
            conflicts: Vec::<Conflict>::new(), // Agent A's conflict rules; shelled out here at integration
            attribution: Vec::<Attribution>::new(), // ditto
            warnings,
        }
    }

    /// Snapshot + ship it down the mpsc channel if it changed (generatedAt and
    /// sub-0.5 eps jitter are ignored for change detection).
    pub fn snapshot_and_send(&self) -> Snapshot {
        let snap = self.build_snapshot();
        let fingerprint = fingerprint(&snap);
        let mut last = lock(&self.last_fingerprint);
        if last.as_deref() != Some(fingerprint.as_str()) {
            *last = Some(fingerprint);
            // Receiver may be gone during shutdown; dropping is fine.
            let _ = self.tx.send(snap.clone());
        }
        snap
    }

    // -- ID allocation (bridge.md §1.2) --------------------------------------

    fn resolve_supervisor_locked(&self, reg: &mut Registry, requested: &str) -> Result<String, String> {
        let trimmed = requested.trim();
        let alloc = |reg: &mut Registry| -> String {
            reg.supervisor_count += 1;
            format!("SV-{}", reg.supervisor_count)
        };
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("new") {
            return Ok(alloc(reg));
        }
        if let Some(n) = parse_sv_id(trimmed) {
            if n == 0 {
                return Err(format!("supervisor id {trimmed} is invalid (ids are 1-based)"));
            }
            if (n as u32) > reg.supervisor_count {
                reg.supervisor_count = n as u32;
            }
            return Ok(format!("SV-{n}"));
        }
        // Unknown non-SV input: never guess silently — allocate fresh and log.
        let id = alloc(reg);
        self.log_event("spawn", None, &format!("supervisor '{trimmed}' is not an SV id; allocated {id}"));
        Ok(id)
    }

    fn next_session_id_locked(reg: &mut Registry, sup_id: &str) -> String {
        let seq = reg.session_seq_by_sup.entry(sup_id.to_string()).or_insert(0);
        *seq += 1;
        let n: u64 = sup_id
            .trim_start_matches("SV-")
            .parse()
            .unwrap_or(1);
        format!("S{n}-OC{:02}", *seq)
    }

    fn find_entry(&self, id: &str) -> Option<Entry> {
        lock(&self.registry).entry(id).cloned()
    }

    fn update_entry<F: FnOnce(&mut Entry)>(&self, id: &str, f: F) -> Result<(), String> {
        let mut reg = lock(&self.registry);
        let entry = reg
            .entry_mut(id)
            .ok_or_else(|| format!("no session {id}"))?;
        f(entry);
        reg.save(&self.state_dir)
    }

    // -- herdr operations used by commands ------------------------------------

    /// Spawn a session: pane split → `herdr agent start --kind opencode`.
    /// Returns the allocated session id. On failure the (permanent) id is
    /// kept and marked failed with `spawn_failed` (bridge.md §1.1).
    pub fn spawn_session(
        &self,
        supervisor: &str,
        prompt: &str,
        cwd: &str,
        title: &str,
    ) -> Result<String, String> {
        let prompt = single_line(prompt.trim(), 400);
        if prompt.is_empty() {
            return Err("prompt is empty".into());
        }
        if !Path::new(cwd).is_dir() {
            return Err(format!("cwd does not exist or is not a directory: {cwd}"));
        }
        let title = {
            let t = title.trim();
            if t.is_empty() {
                single_line(&prompt, 60)
            } else {
                single_line(t, 80)
            }
        };

        let (id, sup_id) = {
            let mut reg = lock(&self.registry);
            let sup_id = self.resolve_supervisor_locked(&mut reg, supervisor)?;
            let id = Self::next_session_id_locked(&mut reg, &sup_id);
            // Persist allocation immediately: ids are permanent even if herdr fails.
            reg.entries.push(Entry {
                id: id.clone(),
                sup: sup_id.clone(),
                pane_id: String::new(),
                cwd: cwd.to_string(),
                title: title.clone(),
                task: single_line(&prompt, 140),
                started_at: now_ms(),
                status: Status::Working,
                failure_reason: None,
                last_revision: None,
                last_action: None,
                latency_ms: None,
            });
            let _ = reg.save(&self.state_dir);
            (id, sup_id)
        };

        // Split a new pane to the right, in the target cwd. Never steals focus.
        let split_out = match herdr_text(&self.herdr, &["pane", "split", "--direction", "right", "--cwd", cwd]) {
            Ok(out) => out,
            Err(e) => return self.spawn_failed(&id, &format!("herdr pane split failed: {e}")),
        };
        let pane_id = match parse_pane_id(&split_out) {
            Some(p) => p,
            None => {
                return self.spawn_failed(
                    &id,
                    &format!("could not parse pane id from herdr pane split output: {:?}", split_out.trim()),
                )
            }
        };
        if let Err(e) = self.update_entry(&id, |e| e.pane_id = pane_id.clone()) {
            // Registry unreachable/corrupt — the session would be untracked. Fail loudly.
            return self.spawn_failed(&id, &format!("failed to record pane {pane_id}: {e}"));
        }

        if let Err(e) = herdr_text(&self.herdr, &["agent", "start", &id, "--kind", "opencode", "--pane", &pane_id]) {
            return self.spawn_failed(&id, &format!("herdr agent start failed: {e}"));
        }

        self.log_event("spawn", Some(&id), &format!("started under {sup_id} in pane {pane_id}"));
        Ok(id)
    }

    fn spawn_failed<T>(&self, id: &str, detail: &str) -> Result<T, String> {
        let _ = self.update_entry(id, |e| {
            e.status = Status::Failed;
            e.failure_reason = Some("spawn_failed".to_string());
        });
        self.log_event("spawn", Some(id), detail);
        Err(detail.to_string())
    }

    pub fn run_plan(&self, path: &str) -> Result<RunPlanResult, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("read plan {path}: {e}"))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("parse plan {path}: {e}"))?;

        let mut warnings: Vec<String> = Vec::new();
        let mut spawned_sv: Vec<String> = Vec::new();
        let mut spawned_sessions: Vec<String> = Vec::new();

        let plan_cwd = std::env::current_dir().ok();

        // Shape A: { "supervisors": [ { "supervisor"?, "cwd"?, "prompt"?, "title"?,
        //                            "sessions"?: [ { prompt, cwd?, title? } ] } ] }
        // Shape B: { "sessions": [ { prompt, cwd?, title? } ] } → one fresh supervisor.
        let mut sup_specs: Vec<&Value> = v
            .get("supervisors")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        let flat_sessions: Vec<&Value> = v
            .get("sessions")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        if sup_specs.is_empty() && !flat_sessions.is_empty() {
            sup_specs = vec![&Value::Null]; // sentinel: single fresh supervisor for all flat sessions
        }
        if sup_specs.is_empty() && flat_sessions.is_empty() {
            return Err("plan parsed but contained no supervisors or sessions".into());
        }

        for sup_spec in sup_specs {
            // For the flat-sessions shape, sup_spec is Value::Null.
            let requested_sup = sup_spec.get("supervisor").and_then(Value::as_str).unwrap_or("new").to_string();
            let sup_cwd = sup_spec
                .get("cwd")
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| plan_cwd.as_ref().map(|p| p.display().to_string()))
                .unwrap_or_else(|| "/".into());

            // Gather session specs for this supervisor.
            let session_specs: Vec<&Value> = if sup_spec.is_null() {
                flat_sessions.clone()
            } else if let Some(arr) = sup_spec.get("sessions").and_then(Value::as_array) {
                arr.iter().collect()
            } else if sup_spec.get("prompt").is_some() {
                vec![sup_spec]
            } else {
                warnings.push("supervisor entry without sessions or prompt; skipped".into());
                continue;
            };

            for sess in session_specs {
                let prompt = sess
                    .get("prompt")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(String::from);
                let Some(prompt) = prompt else {
                    warnings.push("session entry without a prompt; skipped".into());
                    continue;
                };
                let cwd = sess
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .unwrap_or_else(|| sup_cwd.clone());
                let title = sess.get("title").and_then(Value::as_str).unwrap_or("").to_string();
                match self.spawn_session(&requested_sup, &prompt, &cwd, &title) {
                    Ok(id) => {
                        if let Some(e) = self.find_entry(&id) {
                            if !spawned_sv.contains(&e.sup) {
                                spawned_sv.push(e.sup.clone());
                            }
                        }
                        spawned_sessions.push(id);
                    }
                    Err(e) => warnings.push(format!("spawn failed: {e}")),
                }
            }
        }

        if spawned_sessions.is_empty() {
            return Err(if warnings.is_empty() {
                "plan parsed but nothing could be spawned".into()
            } else {
                format!("plan failed: {}", warnings.join("; "))
            });
        }
        self.log_event(
            "spawn",
            None,
            &format!("run_plan {path}: {} session(s) under {} supervisor(s)", spawned_sessions.len(), spawned_sv.len()),
        );
        Ok(RunPlanResult {
            ok: true,
            spawned: spawned_sv,
            sessions: spawned_sessions,
            warnings,
        })
    }

    pub fn kill(&self, id: &str, reason: Option<&str>) -> Result<(), String> {
        let entry = self.find_entry(id).ok_or_else(|| format!("no session {id}"))?;
        let target = if entry.pane_id.is_empty() { id.to_string() } else { entry.pane_id.clone() };
        // herdr pane close takes the pane id positionally (verified against herdr 0.9.5).
        match herdr_text(&self.herdr, &["pane", "close", &target]) {
            Ok(_) => {}
            Err(e) => {
                // Idempotent kill: the pane is already gone — that's success.
                let gone = e.to_lowercase().contains("not found")
                    || e.to_lowercase().contains("no such")
                    || e.to_lowercase().contains("unknown pane");
                if !gone {
                    return Err(format!("herdr pane close {target}: {e}"));
                }
            }
        }
        let reason = reason.unwrap_or("killed").to_string();
        self.update_entry(id, |e| {
            e.status = Status::Failed;
            e.failure_reason = Some(reason.clone());
        })?;
        self.log_event("kill", Some(id), &format!("pane {target} closed; marked failed ({reason})"));
        Ok(())
    }

    pub fn reprompt(&self, id: &str, text: &str) -> Result<(), String> {
        let text = single_line(text.trim(), 2000);
        if text.is_empty() {
            return Err("reprompt text is empty".into());
        }
        let entry = self.find_entry(id).ok_or_else(|| format!("no session {id}"))?;
        let target = if entry.pane_id.is_empty() { id.to_string() } else { entry.pane_id.clone() };
        herdr_text(&self.herdr, &["agent", "prompt", &target, &text])
            .map_err(|e| format!("herdr agent prompt {target}: {e}"))?;
        self.log_event("reprompt", Some(id), &single_line(&text, 120));
        Ok(())
    }

    pub fn logs(&self, id: &str, tail: Option<u32>) -> Result<LogsPayload, String> {
        let tail = tail.unwrap_or(200).clamp(1, 2000) as usize;
        let entry = self.find_entry(id).ok_or_else(|| format!("no session {id}"))?;
        let target = if entry.pane_id.is_empty() { id.to_string() } else { entry.pane_id.clone() };
        let lines_arg = tail.to_string();
        let out = herdr_text(&self.herdr, &["agent", "read", &target, "--lines", &lines_arg, "--format", "text"])
            .map_err(|e| format!("herdr agent read {target}: {e}"))?;
        let mut lines: Vec<String> = out.lines().map(String::from).collect();
        let lines = if lines.len() > tail { lines.split_off(lines.len() - tail) } else { lines };
        Ok(LogsPayload { id: id.to_string(), lines })
    }

    pub fn focus(&self, id: &str) -> Result<(), String> {
        let entry = self.find_entry(id).ok_or_else(|| format!("no session {id}"))?;
        let target = if entry.pane_id.is_empty() { id.to_string() } else { entry.pane_id.clone() };
        // Focus moves herdr's own focus to the pane. It never opens a window.
        herdr_text(&self.herdr, &["agent", "focus", &target])
            .map_err(|e| format!("herdr agent focus {target}: {e}"))?;
        self.log_event("status", Some(id), &format!("focused pane {target}"));
        Ok(())
    }

    /// Remove `$TMPDIR/notch-<id>` scratch dirs. Global sweep when `id` is
    /// None: removes every dir whose id maps to a done/failed entry (or no
    /// registry entry). Never touches dirs of live (working/idle) sessions.
    pub fn cleanup_temp(&self, id: Option<&str>) -> Result<u32, String> {
        let tmp = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let live_ids: Vec<String> = {
            let reg = lock(&self.registry);
            reg.entries
                .iter()
                .filter(|e| matches!(e.status, Status::Working | Status::Idle))
                .map(|e| e.id.clone())
                .collect()
        };

        let mut removable = Vec::<(String, PathBuf)>::new();
        match id {
            Some(id) => {
                if live_ids.iter().any(|l| l == id) {
                    return Err(format!("{id} is still working; refusing to sweep its temp dir"));
                }
                let dir = tmp.join(format!("notch-{id}"));
                if dir.is_dir() {
                    removable.push((id.to_string(), dir));
                }
            }
            None => {
                if let Ok(rd) = fs::read_dir(&tmp) {
                    for dent in rd.flatten() {
                        let name = dent.file_name().to_string_lossy().into_owned();
                        let Some(sid) = name.strip_prefix("notch-") else { continue };
                        if live_ids.iter().any(|l| l == sid) {
                            continue; // live session — never touch
                        }
                        let dir = dent.path();
                        if dir.is_dir() {
                            removable.push((sid.to_string(), dir));
                        }
                    }
                }
            }
        }

        let mut removed = 0u32;
        for (sid, dir) in removable {
            if fs::remove_dir_all(&dir).is_ok() {
                removed += 1;
                self.log_event("cleanup", Some(&sid), &format!("removed {}", dir.display()));
            }
        }
        Ok(removed)
    }

    pub fn state_dir_path(&self) -> PathBuf {
        self.state_dir().to_path_buf()
    }
}

fn aggregate_status(working: u32, idle: u32, failed: u32, done: u32) -> Status {
    if working > 0 {
        Status::Working
    } else if failed > 0 {
        Status::Failed
    } else if idle > 0 {
        Status::Idle
    } else if done > 0 {
        Status::Done
    } else {
        Status::Idle
    }
}

fn parse_sv_id(s: &str) -> Option<u64> {
    s.strip_prefix("SV-").or_else(|| s.strip_prefix("sv-"))?.parse().ok()
}

fn fingerprint(snap: &Snapshot) -> String {
    let mut v = serde_json::to_value(snap).unwrap_or_else(|_| json!({}));
    if let Value::Object(map) = &mut v {
        map.insert("generatedAt".into(), Value::from(0));
        // 0.5/s granularity is enough for change detection and prevents
        // eps sub-quantization from hiding real changes.
        let eps = ((snap.eps * 2.0).round() / 2.0).clamp(0.0, 1.0e9);
        map.insert("eps".into(), Value::from(eps));
    }
    v.to_string()
}

fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NOTCH_STATE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Library").join("Application Support").join("notch")
}

fn poll_loop(engine: Arc<Engine>) {
    loop {
        // First iteration runs immediately (re-adopt on startup), then every 2 s.
        let _ = engine.snapshot_and_send();
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Drain the engine's mpsc channel onto `app.emit("snapshot", …)`, debounced to
/// 150 ms. Also refreshes the tray plate on every emit.
pub fn spawn_emit_thread(app: tauri::AppHandle, rx: Receiver<Snapshot>) {
    std::thread::Builder::new()
        .name("notch-emit".into())
        .spawn(move || {
            use tauri::Emitter;
            let mut last_emit = Instant::now() - Duration::from_millis(150);
            let mut pending: Option<Snapshot> = None;
            loop {
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(s) => pending = Some(s),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                while let Ok(s) = rx.try_recv() {
                    pending = Some(s); // keep the newest only
                }
                if let Some(s) = pending.take() {
                    if last_emit.elapsed() >= Duration::from_millis(150) {
                        let _ = app.emit("snapshot", &s);
                        crate::tray::update_status_from_snapshot(&app, &s);
                        last_emit = Instant::now();
                    } else {
                        pending = Some(s); // rate-limited; flush on the next tick
                    }
                }
            }
            if let Some(s) = pending {
                let _ = app.emit("snapshot", &s);
            }
        })
        .ok();
}
