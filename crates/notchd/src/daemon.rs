//! Daemon — the long-lived control plane. Single process, no model calls.
//!
//! Concurrency layout:
//! - `inner` — the mutable tree behind one mutex.
//! - herdr/git IO happens OUTSIDE the mutex in `poll_once`: a seed pass
//!   clones what's needed, a side-effect pass does IO, then one apply
//!   critical section commits results. Lock order everywhere is
//!   `inner` → `watchdog` → subscriber/pending locks. Never the reverse.
//! - `EventLog` — append-only `events.jsonl` with rotation + EPS window.
//! - `OrchestratorBus` — cross-supervisor routing; supervisors hold only
//!   `SupervisorSink`s; there is no path from one supervisor to another's
//!   mailbox except through the orchestrator.

use crate::attribution;
use crate::bus::{BusMessage, OrchestratorBus, SupervisorSink};
use crate::cleanup;
use crate::config::Config;
use crate::events::EventLog;
use crate::herdr::Herdr;
use crate::model::{
    single_line, truncate, AttributionEntry, ConflictEntry, SessionState, StateFile, SupervisorMeta,
};
use crate::plan;
use crate::registry::Registry;
use crate::snapshot::{OrchestratorSnapshot, SessionSnapshot, Snapshot, SupervisorSnapshot};
use crate::status::Status;
use crate::supervisor::{decide_on_failure, reprompt_text, FailureAction};
use crate::watchdog::Watchdog;
use crate::clock::Clock;

use serde::Serialize;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

const EVENTS_MAX_BYTES: u64 = crate::events::ROTATE_BYTES;
const MAX_WARNINGS: usize = 10;
const ATTRIBUTION_KEEP: usize = 512;
const DEBOUNCE_MS: u64 = 100;

pub struct Daemon {
    pub config: Config,
    herdr: Herdr,
    clock: Arc<dyn Clock>,
    pub event_log: EventLog,
    inner: Mutex<Inner>,
    watchdog: Mutex<Watchdog>,
    bus: OrchestratorBus,
    sinks: Mutex<std::collections::HashMap<String, SupervisorSink>>,
    mailboxes: Mutex<std::collections::HashMap<String, Receiver<BusMessage>>>,
    subscribers: Mutex<Subscribers>,
    pending_change: Mutex<Option<u64>>,
    start_ms: u64,
}

struct Inner {
    supervisors: Vec<SupervisorMeta>,
    sessions: Vec<SessionState>,
    attribution: Vec<AttributionEntry>,
    conflicts: Vec<ConflictEntry>,
    warnings: Vec<String>,
    attribution_seen: HashSet<(String, String)>,
    session_tracked: std::collections::HashMap<String, usize>,
    snapshot_version: u64,
}

#[derive(Default)]
struct Subscribers {
    senders: Vec<(u64, Sender<String>)>,
    next_id: u64,
}

fn lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn push_warning(inner: &mut Inner, warning: String) {
    if inner.warnings.last() == Some(&warning) {
        return; // dedupe consecutive duplicates
    }
    inner.warnings.push(warning);
    if inner.warnings.len() > MAX_WARNINGS {
        let drop = inner.warnings.len() - MAX_WARNINGS;
        inner.warnings.drain(0..drop);
    }
}

// ---------- acks (shared with the socket layer and the Tauri shell) ----------

#[derive(Serialize)]
pub struct RunPlanAck {
    pub ok: bool,
    pub spawned: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct KillAck {
    pub ok: bool,
    pub id: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct OkAck {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct StatusInfo {
    pub event: &'static str,
    pub pid: u32,
    pub sessions: u64,
    #[serde(rename = "uptimeS")]
    pub uptime_s: u64,
}

#[derive(Serialize)]
pub struct SpawnAck {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Daemon {
    /// Open the daemon: load registry + state (missing/corrupt -> defaults),
    /// register bus endpoints for known supervisors. NO herdr calls here —
    /// herdr failure must never prevent startup.
    pub fn open(config: Config, herdr: Herdr, clock: Arc<dyn Clock>) -> io::Result<Daemon> {
        std::fs::create_dir_all(&config.state_dir)?;
        std::fs::create_dir_all(&config.tmp_dir)?;

        let state_path = config.state_dir.join("state.json");
        let state = load_state(&state_path);
        let stuck_after = state.stuck_after_secs.unwrap_or(config.stuck_after_secs);
        let start_ms = clock.now_ms();
        let watchdog = Watchdog::new(stuck_after);

        let attribution_seen: HashSet<(String, String)> = state
            .attribution
            .iter()
            .map(|a| (a.session.clone(), a.file.clone()))
            .collect();
        let session_tracked: std::collections::HashMap<String, usize> = state
            .attribution
            .iter()
            .fold(std::collections::HashMap::new(), |mut acc, a| {
                *acc.entry(a.session.clone()).or_insert(0) += 1;
                acc
            });

        let inner = Inner {
            supervisors: state.supervisors.into_iter().collect(),
            sessions: state.sessions.into_iter().collect(),
            attribution: state.attribution.into_iter().collect(),
            conflicts: state.conflicts.into_iter().collect(),
            warnings: state.warnings.into_iter().collect(),
            attribution_seen,
            session_tracked,
            snapshot_version: 0,
        };

        let bus = OrchestratorBus::new();
        let mut sinks = std::collections::HashMap::new();
        let mut mailboxes = std::collections::HashMap::new();
        for sup in &inner.supervisors {
            let (sink, rx) = bus.register(&sup.id);
            sinks.insert(sup.id.clone(), sink);
            mailboxes.insert(sup.id.clone(), rx);
        }

        let event_log = EventLog::new(
            config.state_dir.join("events.jsonl"),
            EVENTS_MAX_BYTES,
            clock.clone(),
        );

        Ok(Daemon {
            config,
            herdr,
            clock,
            event_log,
            inner: Mutex::new(inner),
            watchdog: Mutex::new(watchdog),
            bus,
            sinks: Mutex::new(sinks),
            mailboxes: Mutex::new(mailboxes),
            subscribers: Mutex::new(Subscribers::default()),
            pending_change: Mutex::new(None),
            start_ms,
        })
    }

    fn state_path(&self) -> PathBuf {
        self.config.state_dir.join("state.json")
    }

    fn registry(&self) -> Registry {
        Registry::load(&registry_path(&self.config.state_dir), self.clock.as_ref())
    }

    fn save_registry(&self, reg: &Registry) {
        let _ = reg.save(&registry_path(&self.config.state_dir));
    }

    /// Persist the tree atomically (write-to-temp + rename).
    fn save_state(&self, inner: &Inner) {
        let file = StateFile {
            version: 1,
            supervisors: inner.supervisors.clone(),
            sessions: inner.sessions.clone(),
            attribution: inner.attribution.clone(),
            conflicts: inner.conflicts.clone(),
            warnings: inner.warnings.clone(),
            stuck_after_secs: Some(lock(&self.watchdog).stuck_after_ms() / 1000),
        };
        let bytes = match serde_json::to_vec_pretty(&file) {
            Ok(b) => b,
            Err(_) => return,
        };
        let _ = crate::registry::write_atomic(&self.state_path(), bytes);
    }

    fn now(&self) -> u64 {
        self.clock.now_ms()
    }

    fn register_supervisor_locked(&self, inner: &mut Inner, sup: &SupervisorMeta) {
        let (sink, rx) = self.bus.register(&sup.id);
        lock(&self.sinks).insert(sup.id.clone(), sink);
        lock(&self.mailboxes).insert(sup.id.clone(), rx);
        inner.supervisors.push(sup.clone());
    }

    /// Bump the snapshot version and mark a change pending; subscribers are
    /// pushed by `flush_subscriber_snapshots` (100 ms debounce).
    fn notify_change(&self) {
        {
            let mut inner = lock(&self.inner);
            inner.snapshot_version = inner.snapshot_version.saturating_add(1);
        }
        *lock(&self.pending_change) = Some(self.now());
    }

    pub fn snapshot_version(&self) -> u64 {
        lock(&self.inner).snapshot_version
    }

    // ---------------- snapshot ----------------

    pub fn snapshot(&self) -> Snapshot {
        let inner = lock(&self.inner);
        let now = self.now();
        let mut supervisors = inner.supervisors.clone();
        supervisors.sort_by_key(|s| s.number);
        let mut sessions = inner.sessions.clone();
        sessions.sort_by(|a, b| a.id.cmp(&b.id));

        let supervisor_snaps: Vec<SupervisorSnapshot> = supervisors
            .iter()
            .map(|sup| {
                let mut working = 0u32;
                let mut idle = 0u32;
                let mut failed = 0u32;
                let mut done = 0u32;
                for s in sessions.iter().filter(|s| s.sup == sup.id) {
                    match s.status {
                        Status::Working => working += 1,
                        Status::Idle => idle += 1,
                        Status::Failed => failed += 1,
                        Status::Done => done += 1,
                    }
                }
                SupervisorSnapshot {
                    id: sup.id.clone(),
                    status: crate::snapshot::rollup(
                        sessions.iter().filter(|s| s.sup == sup.id).map(|s| s.status),
                    ),
                    working,
                    idle,
                    failed,
                    done,
                }
            })
            .collect();

        let session_snaps: Vec<SessionSnapshot> = sessions
            .into_iter()
            .map(|s| SessionSnapshot {
                id: s.id,
                sup: s.sup,
                status: s.status,
                title: s.title,
                task: s.task,
                cwd: s.cwd,
                pane_id: s.pane_id,
                started_at: s.started_at,
                last_action: s.last_action,
                last_revision: s.last_revision,
                latency_ms: s.latency_ms,
                failure_reason: s.failure_reason,
            })
            .collect();

        let mut conflicts = inner.conflicts.clone();
        conflicts.sort_by(|a, b| a.file.cmp(&b.file));
        let attribution: Vec<AttributionEntry> = inner
            .attribution
            .iter()
            .take(crate::attribution::snapshot_cap())
            .cloned()
            .collect();

        Snapshot {
            version: 1,
            generated_at: now,
            orchestrator: OrchestratorSnapshot {
                id: "ORCH".to_string(),
                status: crate::snapshot::rollup(session_snaps.iter().map(|s| s.status)),
            },
            supervisors: supervisor_snaps,
            sessions: session_snaps,
            eps: self.event_log.eps(),
            conflicts,
            attribution,
            warnings: inner.warnings.clone(),
        }
    }

    // ---------------- reconcile on start ----------------

    /// Re-adopt panes that still reference known session IDs. Sessions
    /// whose panes vanished are marked `done` (cleanup runs). On herdr
    /// failure: warning, sessions unchanged, never a crash.
    pub fn reconcile(&self) {
        let agents = match self.herdr.agent_list() {
            Ok(a) => a,
            Err(e) => {
                let mut inner = lock(&self.inner);
                push_warning(&mut inner, format!("herdr unavailable on start; re-adoption skipped: {e}"));
                self.save_state(&inner);
                drop(inner);
                self.notify_change();
                return;
            }
        };
        let by_pane: std::collections::HashMap<&str, &crate::herdr::AgentEntry> =
            agents.iter().map(|a| (a.pane_id.as_str(), a)).collect();
        let now = self.now();
        let mut cleaned: Vec<String> = Vec::new();
        let mut marked_done = Vec::new();
        {
            let mut inner = lock(&self.inner);
            for s in inner.sessions.iter_mut() {
                if s.status.is_terminal() {
                    continue;
                }
                let Some(entry) = by_pane.get(s.pane_id.as_str()) else {
                    // Pane vanished -> done (never crash, never keep lying).
                    s.status = Status::Done;
                    s.failure_reason = None;
                    cleaned.push(s.id.clone());
                    marked_done.push(s.id.clone());
                    continue;
                };
                let (mapped, _reason) = Status::from_herdr(&entry.agent_status);
                match mapped {
                    Status::Done => {
                        s.status = Status::Done;
                        s.failure_reason = None;
                        cleaned.push(s.id.clone());
                        marked_done.push(s.id.clone());
                    }
                    Status::Failed => {
                        s.status = Status::Failed;
                        cleaned.push(s.id.clone());
                    }
                    other => {
                        s.status = other;
                    }
                }
                if s.last_revision != Some(entry.revision) {
                    s.last_revision = Some(entry.revision);
                    s.revision_changed_at_ms = now;
                }
            }
            for id in &cleaned {
                let _ = cleanup::cleanup_session(&self.config.tmp_dir, id);
            }
            for id in &marked_done {
                let _ = self.event_log.append(
                    Some(id.clone()),
                    "status",
                    "re-adopt on start: pane vanished or done; marked done",
                );
            }
            self.save_state(&inner);
        }
        self.notify_change();
    }

    // ---------------- watchdog / poll ----------------

    /// One watchdog + supervision pass. `run_poll_loop` drives it every
    /// `poll_interval_ms`; tests drive it manually with a fake clock.
    pub fn poll_once(&self) {
        let now = self.now();
        let agents = match self.herdr.agent_list() {
            Ok(a) => a,
            Err(e) => {
                let mut inner = lock(&self.inner);
                push_warning(&mut inner, format!("herdr unavailable: {e}"));
                self.save_state(&inner);
                drop(inner);
                self.notify_change();
                return;
            }
        };
        let by_pane: std::collections::HashMap<String, crate::herdr::AgentEntry> = agents
            .into_iter()
            .map(|a| (a.pane_id.clone(), a))
            .collect();

        // Pass A — seeds: cloned session state needed to make decisions.
        struct Seed {
            idx: usize,
            id: String,
            pane_id: String,
            status: Status,
            cwd: String,
            task_head: String,
            failure_count: u32,
            last_revision: Option<u64>,
            revision_changed_at_ms: u64,
            baseline: Option<HashSet<String>>,
        }
        let seeds: Vec<Seed> = {
            let inner = lock(&self.inner);
            inner
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.status.is_terminal())
                .map(|(idx, s)| Seed {
                    idx,
                    id: s.id.clone(),
                    pane_id: s.pane_id.clone(),
                    status: s.status,
                    cwd: s.cwd.clone(),
                    task_head: s.task.clone(),
                    failure_count: s.failure_count,
                    last_revision: s.last_revision,
                    revision_changed_at_ms: s.revision_changed_at_ms,
                    baseline: s.attribution_baseline.clone(),
                })
                .collect()
        };

        // Pass B — herdr/git IO. No locks held.
        enum Effect {
            Vanished { idx: usize },
            Update {
                idx: usize,
                id: String,
                revision: Option<u64>,
                revision_changed: bool,
                status: Status,
                last_action: Option<String>,
                files: Vec<String>,
                reprompt_text: Option<String>,
                reprompt_failed: Option<String>,
                escalate: Option<String>,
                mark_done: bool,
                watchdog_stuck: bool,
                pane_id: String,
                baseline: Option<HashSet<String>>,
            },
        }
        let mut effects: Vec<Effect> = Vec::new();
        for seed in seeds {
            let Some(entry) = by_pane.get(&seed.pane_id) else {
                if !seed.pane_id.is_empty() {
                    effects.push(Effect::Vanished { idx: seed.idx });
                }
                continue;
            };
            let revision_changed = seed.last_revision != Some(entry.revision);
            let (mapped, reason) = Status::from_herdr(&entry.agent_status);
            let stuck = seed.status == Status::Working
                && mapped == Status::Working
                && lock(&self.watchdog).is_stuck(Status::Working, seed.revision_changed_at_ms, now);
            match mapped {
                Status::Failed => {
                    let tail = self.herdr.agent_read(&seed.id, 20).ok();
                    match decide_on_failure(seed.failure_count, tail) {
                        FailureAction::Reprompt { tail } => {
                            let text = reprompt_text(&seed.task_head, &tail);
                            match self.herdr.agent_prompt(&seed.id, &text) {
                                Ok(()) => effects.push(Effect::Update {
                                    idx: seed.idx,
                                    id: seed.id,
                                    revision: Some(entry.revision),
                                    revision_changed: true,
                                    status: Status::Working,
                                    last_action: None,
                                    files: Vec::new(),
                                    reprompt_text: Some(text),
                                    reprompt_failed: None,
                                    escalate: None,
                                    mark_done: false,
                                    watchdog_stuck: false,
                                    pane_id: seed.pane_id,
                                    baseline: None,
                                }),
                                Err(e) => effects.push(Effect::Update {
                                    idx: seed.idx,
                                    id: seed.id,
                                    revision: Some(entry.revision),
                                    revision_changed,
                                    status: Status::Failed,
                                    last_action: None,
                                    files: Vec::new(),
                                    reprompt_text: None,
                                    reprompt_failed: Some(format!("re-prompt failed: {e}")),
                                    escalate: Some(reason.unwrap_or_else(|| "blocked_waiting".into())),
                                    mark_done: false,
                                    watchdog_stuck: false,
                                    pane_id: seed.pane_id,
                                    baseline: None,
                                }),
                            }
                        }
                        FailureAction::Escalate => effects.push(Effect::Update {
                            idx: seed.idx,
                            id: seed.id,
                            revision: Some(entry.revision),
                            revision_changed,
                            status: Status::Failed,
                            last_action: None,
                            files: Vec::new(),
                            reprompt_text: None,
                            reprompt_failed: None,
                            escalate: Some(reason.unwrap_or_else(|| "blocked_waiting".into())),
                            mark_done: false,
                            watchdog_stuck: false,
                            pane_id: seed.pane_id,
                            baseline: None,
                        }),
                    }
                }
                Status::Done => {
                    effects.push(Effect::Update {
                        idx: seed.idx,
                        id: seed.id,
                        revision: Some(entry.revision),
                        revision_changed,
                        status: Status::Done,
                        last_action: None,
                        files: Vec::new(),
                        reprompt_text: None,
                        reprompt_failed: None,
                        escalate: None,
                        mark_done: true,
                        watchdog_stuck: false,
                        pane_id: seed.pane_id,
                        baseline: None,
                    });
                }
                Status::Idle | Status::Working => {
                    let (baseline_new, files) = if mapped == Status::Working {
                        let sampled = attribution::sample_git(Path::new(&seed.cwd));
                        match &seed.baseline {
                            None => (Some(sampled.into_iter().collect::<HashSet<_>>()), Vec::new()),
                            Some(b) => (
                                None,
                                sampled.into_iter().filter(|f| !b.contains(f)).collect(),
                            ),
                        }
                    } else {
                        (None, Vec::new())
                    };
                    let last_action = if mapped == Status::Working {
                        self.herdr.agent_read(&seed.id, 1).ok().and_then(|t| {
                            t.lines()
                                .rfind(|l| !l.trim().is_empty())
                                .map(|l| truncate(l.trim(), 80))
                        })
                    } else {
                        None
                    };
                    effects.push(Effect::Update {
                        idx: seed.idx,
                        id: seed.id,
                        revision: Some(entry.revision),
                        revision_changed,
                        status: mapped,
                        last_action,
                        files,
                        reprompt_text: None,
                        reprompt_failed: None,
                        escalate: None,
                        mark_done: false,
                        watchdog_stuck: stuck,
                        pane_id: seed.pane_id,
                        baseline: baseline_new,
                    });
                }
            }
        }

        // Pass C — apply. One short critical section.
        let mut cleaned: Vec<String> = Vec::new();
        let mut changed = false;
        let mut attr_jobs: Vec<(String, Vec<String>)> = Vec::new();
        let mut pending_warnings: Vec<String> = Vec::new();
        {
            let mut inner = lock(&self.inner);
            for effect in effects {
                match effect {
                    Effect::Vanished { idx } => {
                        if let Some(s) = inner.sessions.get_mut(idx) {
                            if !s.status.is_terminal() {
                                s.status = Status::Done;
                                s.failure_reason = None;
                                let id = s.id.clone();
                                let _ = self.event_log.append(
                                    Some(id.clone()),
                                    "status",
                                    "pane vanished; marked done (re-adoption)",
                                );
                                cleaned.push(id);
                            }
                        }
                        changed = true;
                    }
                    Effect::Update {
                        idx,
                        id,
                        revision,
                        revision_changed,
                        status,
                        last_action,
                        files,
                        reprompt_text,
                        reprompt_failed,
                        escalate,
                        mark_done,
                        watchdog_stuck,
                        pane_id,
                        baseline,
                    } => {
                        let Some(s) = inner.sessions.get_mut(idx) else {
                            continue;
                        };
                if let Some(b) = baseline {
                    s.attribution_baseline = Some(b);
                }
                if let Some(rev) = revision {
                    if revision_changed {
                        s.last_revision = Some(rev);
                        s.revision_changed_at_ms = now;
                    }
                }
                if let Some(a) = last_action {
                    if !a.is_empty() {
                        s.last_action = Some(a);
                    }
                }
                if let Some(text) = reprompt_text {
                    s.failure_count = s.failure_count.saturating_add(1);
                    s.status = Status::Working;
                    s.revision_changed_at_ms = now;
                    let detail = truncate(text.lines().next().unwrap_or(""), 80);
                    let _ = self.event_log.append(Some(id.clone()), "reprompt", &detail);
                } else if let Some(reason) = escalate {
                    s.failure_count = s.failure_count.saturating_add(1);
                    s.status = Status::Failed;
                    s.failure_reason = Some(reason.clone());
                    let detail = match &reprompt_failed {
                        Some(e) => format!("{e}; marked failed ({reason})"),
                        None => format!("failed after re-prompt ({reason})"),
                    };
                    pending_warnings.push(format!("{id} {detail}"));
                    let _ = self.event_log.append(Some(id.clone()), "escalate", &detail);
                    cleaned.push(id.clone());
                } else if mark_done {
                    s.status = Status::Done;
                    s.failure_reason = None;
                    let _ = self.event_log.append(Some(id.clone()), "status", "done");
                    cleaned.push(id.clone());
                } else if watchdog_stuck {
                    match self.herdr.pane_close(&pane_id) {
                        Ok(()) => pending_warnings.push(format!(
                            "{id} stuck (no revision change); pane {pane_id} closed"
                        )),
                        Err(e) => pending_warnings.push(format!(
                            "{id} stuck (no revision change); pane close failed: {e}"
                        )),
                    }
                    s.status = Status::Failed;
                    s.failure_reason = Some("stuck".to_string());
                    let _ = self.event_log.append(
                        Some(id.clone()),
                        "watchdog",
                        "stuck: no revision change",
                    );
                    cleaned.push(id.clone());
                } else if s.status != status {
                    let from = s.status.as_str();
                    s.status = status;
                    let _ = self.event_log.append(
                        Some(id.clone()),
                        "status",
                        &format!("{from} -> {}", status.as_str()),
                    );
                }
                changed = true;
                if !files.is_empty() {
                    attr_jobs.push((id, files));
                }
                    }
                }
            }
            for w in pending_warnings {
                push_warning(&mut inner, w);
            }
            for (id, files) in attr_jobs {
                record_attribution_locked(&mut inner, &self.event_log, &id, &files);
            }
            for id in &cleaned {
                let _ = cleanup::cleanup_session(&self.config.tmp_dir, id);
            }
            if changed || !cleaned.is_empty() {
                self.save_state(&inner);
            }
        }
        self.notify_change();
    }

    /// The 2-second loop used by `notchd serve`. Returns when `shutdown` is set.
    pub fn run_poll_loop(&self, shutdown: Arc<AtomicBool>) {
        while !shutdown.load(Ordering::SeqCst) {
            self.poll_once();
            let deadline = self.config.poll_interval_ms;
            let mut waited = 0u64;
            while waited < deadline && !shutdown.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(50));
                waited += 50;
            }
        }
    }

    // ---------------- plan / spawn ----------------

    pub fn run_plan(&self, path: &Path) -> RunPlanAck {
        let parsed = match plan::load_plan(path) {
            Ok(p) => p,
            Err(e) => {
                return RunPlanAck {
                    ok: false,
                    spawned: Vec::new(),
                    error: Some(e),
                }
            }
        };
        if let Some(secs) = parsed.stuck_after_secs {
            lock(&self.watchdog).set_stuck_after_secs(secs);
        }
        let mut reg = self.registry();
        let started_at = self.now();
        let mut new_supervisor_ids: Vec<String> = Vec::new();
        let mut new_sessions: Vec<SessionState> = Vec::new();
        let mut prompts: Vec<(String, String)> = Vec::new(); // (session id, prompt)

        for sessions in &parsed.supervisors {
            let sup_id = reg.allocate_supervisor();
            let sup_number: u32 = sup_id
                .strip_prefix("SV-")
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            new_supervisor_ids.push(sup_id.clone());
            for ses in sessions {
                let id = reg.allocate_session(sup_number);
                let session = SessionState {
                    id: id.clone(),
                    sup: sup_id.clone(),
                    status: Status::Working,
                    title: ses.title.clone(),
                    task: ses.task_head(),
                    cwd: ses.cwd.clone(),
                    pane_id: String::new(),
                    started_at,
                    last_action: None,
                    last_revision: None,
                    latency_ms: None,
                    failure_reason: None,
                    failure_count: 0,
                    revision_changed_at_ms: started_at,
                    attribution_baseline: None,
                };
                prompts.push((id.clone(), ses.prompt.clone()));
                new_sessions.push(session);
            }
        }
        self.save_registry(&reg);

        // Spawn in plan order. No locks held across herdr IO.
        for session in new_sessions.iter_mut() {
            let prompt = prompts
                .iter()
                .find(|(id, _)| id == &session.id)
                .map(|(_, p)| p.clone())
                .unwrap_or_default();
            if let Err(e) = self.spawn_one(session, &prompt) {
                session.status = Status::Failed;
                session.failure_reason = Some("spawn_failed".to_string());
                {
                    let mut inner = lock(&self.inner);
                    push_warning(&mut inner, format!("{} spawn failed: {e}", session.id));
                }
                let _ = self.event_log.append(
                    Some(session.id.clone()),
                    "escalate",
                    &format!("spawn_failed: {e}"),
                );
            }
        }

        {
            let mut inner = lock(&self.inner);
            for id in &new_supervisor_ids {
                let number: u32 = id
                    .strip_prefix("SV-")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                self.register_supervisor_locked(&mut inner, &SupervisorMeta { id: id.clone(), number });
            }
            for session in new_sessions {
                inner.sessions.push(session);
            }
            inner.sessions.sort_by(|a, b| a.id.cmp(&b.id));
            inner.supervisors.sort_by_key(|s| s.number);
            self.save_state(&inner);
        }
        for id in &new_supervisor_ids {
            let _ = self.event_log.append(None, "spawn", &format!("supervisor {id}"));
        }
        let spawned = new_supervisor_ids;
        self.notify_change();
        RunPlanAck {
            ok: true,
            spawned,
            error: None,
        }
    }

    /// Split a pane, start opencode inside it, prompt it. Creates the
    /// per-session temp dir. Returns Err on failure.
    fn spawn_one(&self, session: &mut SessionState, prompt: &str) -> Result<(), String> {
        let tmp = cleanup::session_tmp_dir(&self.config.tmp_dir, &session.id);
        std::fs::create_dir_all(&tmp).map_err(|e| format!("temp dir {}: {e}", tmp.display()))?;

        let env = vec![("NOTCH_SESSION_ID".to_string(), session.id.clone())];
        let pane = self
            .herdr
            .pane_split(&session.cwd, &env)
            .map_err(|e| format!("pane split: {e}"))?;
        session.pane_id = pane.clone();

        if let Err(e) = self.herdr.agent_start(&session.id, &pane) {
            // We own the pane we just made — close it rather than leak it.
            let _ = self.herdr.pane_close(&pane);
            return Err(format!("agent start: {e}"));
        }
        if let Err(e) = self.herdr.agent_prompt(&session.id, prompt) {
            // The agent exists; a lost prompt is degraded, not fatal.
            // The watchdog will catch a session that never makes progress.
            let mut inner = lock(&self.inner);
            push_warning(&mut inner, format!("{} prompt failed: {e}", session.id));
        }
        let _ = self.event_log.append(
            Some(session.id.clone()),
            "spawn",
            &format!("pane={pane} cwd={}", session.cwd),
        );
        Ok(())
    }

    pub fn spawn(&self, supervisor: &str, prompt: &str, cwd: &str, title: &str) -> SpawnAck {
        if !Path::new(cwd).is_dir() {
            return SpawnAck {
                ok: false,
                id: None,
                error: Some(format!("cwd `{cwd}` is not a directory")),
            };
        }
        let sup_number = {
            let inner = lock(&self.inner);
            inner.supervisors.iter().find(|s| s.id == supervisor).map(|s| s.number)
        };
        let Some(sup_number) = sup_number else {
            return SpawnAck {
                ok: false,
                id: None,
                error: Some(format!("unknown supervisor `{supervisor}`")),
            };
        };
        let mut reg = self.registry();
        let id = reg.allocate_session(sup_number);
        let now = self.now();
        let mut session = SessionState {
            id: id.clone(),
            sup: supervisor.to_string(),
            status: Status::Working,
            title: title.to_string(),
            task: single_line(prompt),
            cwd: cwd.to_string(),
            pane_id: String::new(),
            started_at: now,
            last_action: None,
            last_revision: None,
            latency_ms: None,
            failure_reason: None,
            failure_count: 0,
            revision_changed_at_ms: now,
            attribution_baseline: None,
        };
        match self.spawn_one(&mut session, prompt) {
            Ok(()) => {}
            Err(e) => {
                session.status = Status::Failed;
                session.failure_reason = Some("spawn_failed".to_string());
                let mut inner = lock(&self.inner);
                push_warning(&mut inner, format!("{id} spawn failed: {e}"));
            }
        }
        self.save_registry(&reg);
        {
            let mut inner = lock(&self.inner);
            inner.sessions.push(session);
            inner.sessions.sort_by(|a, b| a.id.cmp(&b.id));
            self.save_state(&inner);
        }
        self.notify_change();
        SpawnAck {
            ok: true,
            id: Some(id),
            error: None,
        }
    }

    // ---------------- commands ----------------

    pub fn kill(&self, id: &str, reason: Option<String>) -> Result<KillAck, String> {
        let reason = reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "killed".to_string());
        let pane = {
            let inner = lock(&self.inner);
            inner.sessions.iter().find(|s| s.id == id).map(|s| s.pane_id.clone())
        };
        let Some(pane) = pane else {
            return Err("unknown_session".to_string());
        };
        // Close the pane. Failure is degraded: mark failed + warn, never panic.
        if !pane.is_empty() {
            if let Err(e) = self.herdr.pane_close(&pane) {
                let mut inner = lock(&self.inner);
                push_warning(&mut inner, format!("kill {id}: pane close failed: {e}"));
            }
        }
        {
            let mut inner = lock(&self.inner);
            if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == id) {
                s.status = Status::Failed;
                s.failure_reason = Some(reason.clone());
            }
            let _ = self.event_log.append(Some(id.to_string()), "kill", &reason);
            self.save_state(&inner);
        }
        let _ = cleanup::cleanup_session(&self.config.tmp_dir, id);
        self.notify_change();
        Ok(KillAck {
            ok: true,
            id: id.to_string(),
            reason,
            error: None,
        })
    }

    pub fn reprompt(&self, id: &str, text: &str) -> Result<OkAck, String> {
        let exists = {
            let inner = lock(&self.inner);
            inner.sessions.iter().any(|s| s.id == id)
        };
        if !exists {
            return Err("unknown_session".to_string());
        }
        self.herdr
            .agent_prompt(id, text)
            .map_err(|e| format!("prompt: {e}"))?;
        let _ = self
            .event_log
            .append(Some(id.to_string()), "reprompt", &truncate(text, 80));
        self.notify_change();
        Ok(OkAck {
            ok: true,
            id: Some(id.to_string()),
            removed: None,
            error: None,
        })
    }

    /// `id: Some(x)` removes that session's temp dir — but only when the
    /// session is dead (done/failed) or unknown; a live session's temp dir
    /// is never deleted. `id: None` sweeps dead sessions, orphan dirs and
    /// the events log.
    pub fn cleanup(&self, id: Option<&str>) -> OkAck {
        let removed = match id {
            Some(id) => {
                let deletable = {
                    let inner = lock(&self.inner);
                    inner
                        .sessions
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.status.is_terminal())
                        .unwrap_or(true) // unknown id: explicit request, sweep what's there
                };
                if !deletable {
                    0
                } else {
                    match cleanup::cleanup_session(&self.config.tmp_dir, id) {
                        Ok(true) => 1,
                        _ => 0,
                    }
                }
            }
            None => {
                let sessions = {
                    let inner = lock(&self.inner);
                    inner.sessions.clone()
                };
                cleanup::sweep(
                    &self.config.tmp_dir,
                    &sessions,
                    &self.config.state_dir.join("events.jsonl"),
                    EVENTS_MAX_BYTES,
                )
                .unwrap_or(0)
            }
        };
        let _ = self
            .event_log
            .append(id.map(str::to_string), "cleanup", &format!("removed={removed}"));
        self.notify_change();
        OkAck {
            ok: true,
            id: id.map(str::to_string),
            removed: Some(removed),
            error: None,
        }
    }

    pub fn logs(&self, id: &str, tail: Option<usize>) -> Result<Vec<String>, String> {
        let exists = {
            let inner = lock(&self.inner);
            inner.sessions.iter().any(|s| s.id == id)
        };
        if !exists {
            return Err("unknown_session".to_string());
        }
        let tail = tail.unwrap_or(200).clamp(1, 10_000);
        let text = self
            .herdr
            .agent_read(id, tail)
            .map_err(|e| format!("read: {e}"))?;
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.pop();
        }
        Ok(lines)
    }

    pub fn focus(&self, id: &str) -> Result<OkAck, String> {
        let exists = {
            let inner = lock(&self.inner);
            inner.sessions.iter().any(|s| s.id == id)
        };
        if !exists {
            return Err("unknown_session".to_string());
        }
        self.herdr
            .agent_focus(id)
            .map_err(|e| format!("focus: {e}"))?;
        Ok(OkAck {
            ok: true,
            id: Some(id.to_string()),
            removed: None,
            error: None,
        })
    }

    pub fn status(&self) -> StatusInfo {
        let sessions = {
            let inner = lock(&self.inner);
            inner.sessions.iter().filter(|s| !s.status.is_terminal()).count() as u64
        };
        StatusInfo {
            event: "status",
            pid: std::process::id(),
            sessions,
            uptime_s: self.now().saturating_sub(self.start_ms) / 1000,
        }
    }

    // ---------------- attribution / conflicts ----------------

    /// Record that `session` touched `files`. Dedupes, caps per-session
    /// tracking at 64, prepends (newest first), detects cross-session file
    /// conflicts. Attribution never mutates the filesystem.
    pub fn record_attribution(&self, session: &str, files: &[String]) {
        let mut inner = lock(&self.inner);
        record_attribution_locked(&mut inner, &self.event_log, session, files);
        self.save_state(&inner);
    }

    // ---------------- bus ----------------

    pub fn bus_send(
        &self,
        from: &str,
        to: &str,
        session: Option<String>,
        kind: &str,
        detail: &str,
    ) -> Result<(), String> {
        if !self.bus.is_registered(from) {
            return Err(format!("unknown supervisor `{from}`"));
        }
        if !self.bus.is_registered(to) {
            return Err(format!("unknown supervisor `{to}`"));
        }
        let sinks = lock(&self.sinks);
        let Some(sink) = sinks.get(from) else {
            return Err(format!("supervisor `{from}` has no bus endpoint"));
        };
        sink.send_to(to, session, kind, detail, self.now())
    }

    /// Route everything pending in the orchestrator inbox. Returns how many
    /// were delivered. Ordering is FIFO.
    pub fn bus_drain(&self) -> usize {
        let log = &self.event_log;
        self.bus.drain(&mut |msg: &BusMessage| {
            let _ = log.append(
                msg.session.clone(),
                "status",
                &format!("bus {}->{}: {} {}", msg.from, msg.to, msg.kind, msg.detail),
            );
        })
    }

    pub fn mailbox_drain(&self, supervisor: &str) -> Vec<BusMessage> {
        let mailboxes = lock(&self.mailboxes);
        let Some(rx) = mailboxes.get(supervisor) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    // ---------------- subscribers (socket layer) ----------------

    pub fn register_subscriber(&self) -> (u64, Receiver<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut subs = lock(&self.subscribers);
        subs.next_id = subs.next_id.saturating_add(1);
        let id = subs.next_id;
        subs.senders.push((id, tx));
        (id, rx)
    }

    pub fn unregister_subscriber(&self, id: u64) {
        let mut subs = lock(&self.subscribers);
        subs.senders.retain(|(sid, _)| *sid != id);
    }

    /// Push the current snapshot to all subscribers if a change is pending
    /// and at least 100 ms elapsed since it was marked pending. Called by
    /// the socket loop (and manually in tests).
    pub fn flush_subscriber_snapshots(&self) {
        let due = {
            let pending = lock(&self.pending_change);
            pending
                .map(|t| self.now().saturating_sub(t) >= DEBOUNCE_MS)
                .unwrap_or(false)
        };
        if !due {
            return;
        }
        *lock(&self.pending_change) = None;
        let line = self.snapshot_line();
        let mut subs = lock(&self.subscribers);
        subs.senders.retain(|(_, tx)| tx.send(line.clone()).is_ok());
    }

    /// Push regardless of the debounce window.
    pub fn flush_subscribers_immediate(&self) {
        *lock(&self.pending_change) = None;
        let line = self.snapshot_line();
        let mut subs = lock(&self.subscribers);
        subs.senders.retain(|(_, tx)| tx.send(line.clone()).is_ok());
    }

    pub fn has_pending_change(&self) -> bool {
        lock(&self.pending_change).is_some()
    }

    pub fn snapshot_line(&self) -> String {
        let snap = self.snapshot();
        serde_json::to_string(&serde_json::json!({ "event": "snapshot", "snapshot": snap }))
            .unwrap_or_else(|_| "{\"event\":\"snapshot\",\"snapshot\":null}".to_string())
    }
}

fn record_attribution_locked(
    inner: &mut Inner,
    event_log: &EventLog,
    session: &str,
    files: &[String],
) {
    if session.is_empty() {
        return;
    }
    let mut tracked = inner.session_tracked.get(session).copied().unwrap_or(0);
    for file in files {
        if tracked >= attribution::per_session_cap() {
            break;
        }
        let key = (session.to_string(), file.clone());
        if !inner.attribution_seen.insert(key) {
            continue; // already tracked
        }
        inner.attribution.insert(
            0,
            AttributionEntry {
                session: session.to_string(),
                file: file.clone(),
            },
        );
        tracked += 1;
        // Conflict: same file touched by >= 2 sessions.
        let others: HashSet<String> = inner
            .attribution
            .iter()
            .filter(|a| &a.file == file && a.session != session)
            .map(|a| a.session.clone())
            .collect();
        if !others.is_empty() {
            let mut sessions: Vec<String> = others.into_iter().collect();
            sessions.push(session.to_string());
            sessions.sort();
            sessions.dedup();
            match inner.conflicts.iter_mut().find(|c| &c.file == file) {
                Some(entry) => {
                    if entry.sessions != sessions {
                        entry.sessions = sessions.clone();
                        let _ = event_log.append(
                            Some(session.to_string()),
                            "conflict",
                            &format!("{file}: {}", sessions.join(",")),
                        );
                    }
                }
                None => {
                    inner.conflicts.push(ConflictEntry {
                        file: file.clone(),
                        sessions: sessions.clone(),
                    });
                    let _ = event_log.append(
                        Some(session.to_string()),
                        "conflict",
                        &format!("{file}: {}", sessions.join(",")),
                    );
                }
            }
        }
    }
    inner.session_tracked.insert(session.to_string(), tracked);
    while inner.attribution.len() > ATTRIBUTION_KEEP {
        if let Some(dropped) = inner.attribution.pop() {
            inner.attribution_seen.remove(&(dropped.session.clone(), dropped.file.clone()));
            if let Some(n) = inner.session_tracked.get_mut(&dropped.session) {
                *n = n.saturating_sub(1);
            }
        }
    }
}

fn registry_path(state_dir: &Path) -> PathBuf {
    state_dir.join("registry.json")
}

fn load_state(path: &Path) -> StateFile {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<StateFile>(&bytes).unwrap_or_else(|_| StateFile::empty()),
        Err(_) => StateFile::empty(),
    }
}
