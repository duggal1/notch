//! The supervisor decision loop (bridge.md v1.3 §8). Runs after each
//! poll pass; for every non-terminal, non-escalated session with a goal it
//! classifies idle (primary: opencode /api/session; fallbacks: herdr status,
//! output-quiet) and applies the deterministic budget/backoff policy, then
//! judges the session. The orchestrator itself makes zero model calls;
//! failures here never wedge the loop.
//!
//! Perf design (2026-11-19):
//! 1. Deterministic FAST PATH first — a locally checkable DoD (`file:… exists`,
//!    `file:… contains:…`, `output contains:…`) is decided with zero model
//!    calls and honest, concrete evidence (see `decider::evaluate_local_dod`).
//! 2. One persistent, long-lived decider session via `opencode run --session
//!    <id> --auto --archived`, with automatic fallback to the original cold
//!    `opencode run --model …` if session reuse fails. The path that fired is
//!    logged as a `perf` event; p50/p95 per path are recorded in
//!    `decider::decision_stats()`.
//! 3. Judgments for different idle sessions run on bounded std threads (cap 4).
//!    Budgets, pause rules, loop detection and "never re-prompt a working
//!    session" are all evaluated before the parallel fan-out; verdicts are
//!    applied serially in seed order so state transitions stay deterministic.
use super::types::{
    agent_target, contains_rate_limit, lock, push_warning, tail_key, Daemon, Inner, DECISIONS_CAP,
};
use crate::cleanup;
use crate::decider::{self, DecisionPath};
use crate::model::DecisionEntry;
use crate::opencode::Opencode;
use crate::status::Status;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Number of long-lived judgment workers per daemon. They are the ONLY
/// threads that call `opencode run`; the poll thread never does.
const JUDGE_WORKERS: usize = 4;
/// Bounded hand-off queue. `try_send` never blocks the poll thread.
const JUDGE_QUEUE_CAP: usize = 32;

/// Two kinds of model work. `Verify` is a distinct job so evidence checking
/// is bounded by the same deadline + queue as a normal judgment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobKind {
    Judge,
    Verify,
}

/// The context a worker carries back so the poll thread can apply the result
/// deterministically, in the same critical-section order as before.
struct JobCtx {
    seed: Seed,
    tail: String,
    model: String,
    remaining: u64,
}

struct Job {
    session: String,
    kind: JobKind,
    model: String,
    prompt: String,
    ctx: JobCtx,
}

struct Finished {
    session: String,
    kind: JobKind,
    ctx: JobCtx,
    raw: Result<String, String>,
    path: DecisionPath,
    ms: u64,
}

/// A DONE verdict awaiting its verifier. Held on the pool so the poll thread
/// can apply the verifier answer without blocking on the model call.
struct PendingVerify {
    seed: Seed,
    tail: String,
}

/// Long-lived, bounded judgment pool. Workers own the blocking model calls;
/// `poll_once` only submits and drains.
struct JudgePool {
    tx: Mutex<Option<SyncSender<Job>>>,
    done_rx: Mutex<mpsc::Receiver<Finished>>,
    /// At most one outstanding job per session (`docs/HARDENING.md §3.2`).
    in_flight: Mutex<HashSet<String>>,
    pending: Mutex<HashMap<String, PendingVerify>>,
    client: Arc<Mutex<Option<Opencode>>>,
}

impl JudgePool {
    fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::sync_channel::<Job>(JUDGE_QUEUE_CAP);
        let (done_tx, done_rx) = mpsc::channel::<Finished>();
        let pool = Arc::new(Self {
            tx: Mutex::new(Some(tx)),
            done_rx: Mutex::new(done_rx),
            in_flight: Mutex::new(HashSet::new()),
            pending: Mutex::new(HashMap::new()),
            client: Arc::new(Mutex::new(None)),
        });
        let rx = Arc::new(Mutex::new(rx));
        for _ in 0..JUDGE_WORKERS {
            let rx = Arc::clone(&rx);
            let done_tx = done_tx.clone();
            let client = Arc::clone(&pool.client);
            std::thread::spawn(move || loop {
                // The lock is held only across `recv`; once a job is taken the
                // next worker can receive the following one.
                let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                let Ok(job) = job else {
                    break; // sender dropped
                };
                let oc = {
                    let guard = client.lock().unwrap_or_else(|e| e.into_inner());
                    guard.clone()
                };
                let session_id = job.ctx.seed.id.clone();
                let session_cwd = job.ctx.seed.cwd.clone();
                let cwd = if session_cwd.is_empty() {
                    None
                } else {
                    Some(Path::new(&session_cwd))
                };
                let (raw, path, ms) =
                    judge(oc.as_ref(), &job.model, &job.prompt, &session_id, cwd);
                let finished = Finished {
                    session: job.session,
                    kind: job.kind,
                    ctx: job.ctx,
                    raw,
                    path,
                    ms,
                };
                if done_tx.send(finished).is_err() {
                    break; // poll side gone
                }
            });
        }
        pool
    }

    /// Non-blocking submit. Returns false when the queue is full or this
    /// session already has a job in flight — never waits on a worker.
    fn try_submit(&self, job: Job) -> bool {
        let mut in_flight = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        if in_flight.contains(&job.session) {
            return false;
        }
        let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        let Some(tx) = guard.as_ref() else {
            return false;
        };
        let session = job.session.clone();
        match tx.try_send(job) {
            Ok(()) => {
                in_flight.insert(session);
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Take every completed job. The session slot is freed here, so the next
    /// poll may submit a fresh job for it.
    fn drain_done(&self) -> Vec<Finished> {
        let mut out = Vec::new();
        loop {
            let guard = self.done_rx.lock().unwrap_or_else(|e| e.into_inner());
            match guard.try_recv() {
                Ok(finished) => out.push(finished),
                Err(_) => break,
            }
        }
        if !out.is_empty() {
            let mut in_flight = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
            for finished in &out {
                in_flight.remove(&finished.session);
            }
        }
        out
    }
}

/// Pools are keyed by state dir so test daemons never share a client seam.
/// This lives entirely in A2's owned files (no `daemon/types.rs` change).
static JUDGE_POOLS: OnceLock<Mutex<HashMap<PathBuf, Arc<JudgePool>>>> = OnceLock::new();

/// A session that passed all gates and needs a model judgment.
struct Prepared {
    seed: Seed,
    tail: String,
    model: String,
    remaining: u64,
}

#[derive(Clone)]
struct Seed {
    id: String,
    sup: String,
    cwd: String,
    status: Status,
    goal: Option<String>,
    dod: Option<String>,
    model: Option<String>,
    iterations: u32,
    max_iterations: u32,
    wall_clock_deadline: Option<u64>,
    backoff_until_ms: u64,
    consecutive_decider_failures: u32,
    stuck_strikes: u32,
    session_ref: Option<String>,
    revision_changed_at_ms: u64,
}

impl Seed {
    fn prompt(&self, remaining: u64, tail: &str) -> String {
        decider::decider_prompt(
            self.goal.as_deref().unwrap_or(""),
            self.dod.as_deref(),
            self.iterations.saturating_add(1),
            self.max_iterations,
            remaining,
            tail,
        )
    }
}

/// One model call. Tries the persistent session first; on any failure falls
/// back to the original cold invocation and reports which path fired. The
/// `run --model` argv prefix is preserved so injected test runners keep working.
fn judge(
    client: Option<&Opencode>,
    model: &str,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
) -> (Result<String, String>, DecisionPath, u64) {
    let start = Instant::now();
    let Some(oc) = client else {
        let ms = start.elapsed().as_millis() as u64;
        let path = DecisionPath::Cold;
        decider::record_decision(path, ms);
        return (Err("decider client not configured".to_string()), path, ms);
    };

    // A1's per-notch-session id + real cwd: the judge must read this
    // session's worktree, not a shared context in the daemon's cwd.
    let sid = decider::decider_session_id_for(session_id);
    let (result, path) = match oc.run_decider_session_in(model, &sid, prompt, cwd) {
        Ok(raw) => (Ok(raw), DecisionPath::Persistent),
        Err(persist_err) => {
            // Session reuse failed (bad id, corrupt context, quota on that
            // session, …). Fall back to the original cold path — never lose a
            // judgment because the optimization broke.
            match oc.run_decider_in(model, prompt, cwd) {
                Ok(raw) => (Ok(raw), DecisionPath::Cold),
                Err(cold_err) => (
                    Err(format!(
                        "persistent decider failed ({persist_err}); cold fallback failed ({cold_err})"
                    )),
                    DecisionPath::Cold,
                ),
            }
        }
    };
    let ms = start.elapsed().as_millis() as u64;
    decider::record_decision(path, ms);
    (result, path, ms)
}

impl Daemon {
    pub(crate) fn is_supervisor_paused(&self, sup: &str) -> bool {
        lock(&self.inner).paused_supervisors.contains(sup)
    }

    /// This daemon's long-lived judgment pool, keyed by state dir so each
    /// test daemon keeps its own injectable client seam.
    fn judge_pool(&self) -> Arc<JudgePool> {
        let pools = JUDGE_POOLS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut map = pools.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            map.entry(self.config.state_dir.clone())
                .or_insert_with(JudgePool::new),
        )
    }

    /// Keep the pool's cloned client in sync with `Daemon::set_opencode`.
    /// Called from `lifecycle.rs` (A2-owned) on every client swap.
    pub(crate) fn set_judge_client(&self, client: Option<Opencode>) {
        let pool = self.judge_pool();
        let mut guard = pool.client.lock().unwrap_or_else(|e| e.into_inner());
        *guard = client;
    }

    pub(crate) fn reprompt_session(&self, id: &str, text: &str) {
        let _ = self.herdr.agent_prompt(&agent_target(id), text);
        let _ = self.event_log.append(
            Some(id.to_string()),
            "reprompt",
            &decider::log_head(text, 120),
        );
    }

    pub(crate) fn log_decision(
        &self,
        inner: &mut Inner,
        supervisor: &str,
        session: &str,
        action: &str,
        reason: &str,
    ) {
        let _ = self.event_log.append(
            Some(session.to_string()),
            "decision",
            &format!("{action}: {}", decider::log_head(reason, 140)),
        );
        inner.decisions.insert(
            0,
            DecisionEntry {
                at: self.now(),
                supervisor: supervisor.to_string(),
                session: session.to_string(),
                action: action.to_string(),
                reason: decider::log_head(reason, 160),
            },
        );
        if inner.decisions.len() > DECISIONS_CAP {
            inner.decisions.truncate(DECISIONS_CAP);
        }
        if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == session) {
            s.last_supervisor_action =
                Some(format!("{}: {}", action, decider::log_head(reason, 110)));
        }
    }

    fn log_path(&self, session: &str, path: DecisionPath, ms: u64, note: &str) {
        let _ = self.event_log.append(
            Some(session.to_string()),
            "perf",
            &format!("path={} ms={} {note}", path.as_str(), ms),
        );
    }

    pub(crate) fn supervise_once(&self, api_sessions: Option<&[crate::opencode::ApiSession]>) {
        let now = self.now();
        let pool = self.judge_pool();
        // P2: apply spawns that continued after a fast ack.
        for outcome in self.drain_spawn_outcomes() {
            self.apply_spawn_outcome(outcome);
        }
        // 3.2: drain finished jobs first. Applying a result is pure state work
        // on this thread; no model call ever happens inside `poll_once`.
        let finished = pool.drain_done();
        for f in finished {
            self.apply_finished(f, now, &pool);
        }
        // P1: herdr `done` on a DoD-bearing session is NOT a verdict. Revert
        // it and let the verifier decide before any `DONE` status is visible.
        self.enforce_dod_gate(now, &pool);

        let seeds: Vec<Seed> = {
            let inner = lock(&self.inner);
            inner
                .sessions
                .iter()
                .filter(|s| !s.status.is_terminal() && !s.escalated && s.goal.is_some())
                .map(|s| Seed {
                    id: s.id.clone(),
                    sup: s.sup.clone(),
                    cwd: s.cwd.clone(),
                    status: s.status,
                    goal: s.goal.clone(),
                    dod: s.definition_of_done.clone(),
                    model: s.model.clone(),
                    iterations: s.iterations,
                    max_iterations: s.max_iterations,
                    wall_clock_deadline: s.wall_clock_deadline,
                    backoff_until_ms: s.backoff_until_ms,
                    consecutive_decider_failures: s.consecutive_decider_failures,
                    stuck_strikes: s.stuck_strikes,
                    session_ref: s.session_ref.clone(),
                    revision_changed_at_ms: s.revision_changed_at_ms,
                })
                .collect()
        };

        // ---- Phase A: gates + tail read + deterministic fast path (serial) ----
        let mut prepared: Vec<Prepared> = Vec::new();
        for seed in seeds {
            // Paused supervisors: sessions keep running, nothing is decided.
            // Pause gates ONLY the decision loop — the watchdog (stuck-kill)
            // still fires on paused sessions.
            if self.is_supervisor_paused(&seed.sup) {
                continue;
            }
            // Never re-prompt a session that is working; only act on idle.
            let (signal, idle) = match seed
                .session_ref
                .as_ref()
                .and_then(|r| api_sessions.and_then(|api| api.iter().find(|s| &s.id == r)))
                .map(crate::opencode::is_idle)
            {
                Some(false) => ("primary", false),
                Some(true) => ("primary", true),
                None => {
                    let quiet = seed.revision_changed_at_ms != 0
                        && now >= seed.revision_changed_at_ms.saturating_add(90_000);
                    if seed.status == Status::Idle {
                        ("herdr", true)
                    } else if quiet && seed.status != Status::Working {
                        ("quiet", true)
                    } else {
                        ("herdr", false)
                    }
                }
            };
            if !idle {
                continue;
            }
            let _ = self
                .event_log
                .append(Some(seed.id.clone()), "idle_signal", signal);

            // Budgets first — before any model call.
            if seed.iterations >= seed.max_iterations {
                self.escalate_session(
                    &seed.id,
                    "budget_exceeded",
                    &format!("iterations {}/{}", seed.iterations, seed.max_iterations),
                );
                continue;
            }
            if let Some(deadline) = seed.wall_clock_deadline {
                if now >= deadline {
                    self.escalate_session(
                        &seed.id,
                        "budget_exceeded",
                        "wall clock deadline exceeded",
                    );
                    continue;
                }
            }
            if seed.backoff_until_ms > now {
                continue; // in backoff; the backoff was logged when it started
            }

            // Read the tail (~200 lines). Tolerate failure as empty output.
            let tail = self
                .herdr
                .agent_read(&agent_target(&seed.id), 200)
                .unwrap_or_default();

            // 429 / quota: back off with jitter; never hammer.
            if contains_rate_limit(&tail) {
                let backoff_ms = 30_000 + (now % 61_000);
                let failures = seed.consecutive_decider_failures.saturating_add(1);
                {
                    let mut inner = lock(&self.inner);
                    if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                        s.backoff_until_ms = now.saturating_add(backoff_ms);
                        s.consecutive_decider_failures = failures;
                    }
                    push_warning(
                        &mut inner,
                        format!(
                            "{}: rate limit in pane output; backing off {}s",
                            seed.id,
                            backoff_ms / 1000
                        ),
                    );
                    self.log_decision(
                        &mut inner,
                        &seed.sup,
                        &seed.id,
                        "backoff",
                        &format!("rate limit (429/quota); backing off {}s", backoff_ms / 1000),
                    );
                }
                self.save_state(&lock(&self.inner));
                if failures >= 3 {
                    self.escalate_session(
                        &seed.id,
                        "budget_exceeded",
                        "rate limited 3 times without progress",
                    );
                }
                continue;
            }

            // FAST PATH — zero model calls for locally checkable DoDs.
            match decider::evaluate_local_dod(seed.dod.as_deref(), &seed.cwd, &tail) {
                decider::LocalCheckOutcome::Satisfied { evidence } => {
                    decider::record_decision(DecisionPath::Fast, 0);
                    self.log_path(&seed.id, DecisionPath::Fast, 0, &format!("evidence={}", decider::log_head(&evidence, 120)));
                    let mut inner = lock(&self.inner);
                    if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                        s.status = Status::Done;
                        s.failure_reason = None;
                        s.consecutive_decider_failures = 0;
                    }
                    self.log_decision(
                        &mut inner,
                        &seed.sup,
                        &seed.id,
                        "done",
                        &format!("fast-path evidence: {evidence}"),
                    );
                    let _ = self.event_log.append(
                        Some(seed.id.clone()),
                        "status",
                        "done (fast path: local DoD evidence verified)",
                    );
                    drop(inner);
                    let _ = cleanup::cleanup_session(&self.config.tmp_dir, &seed.id);
                    continue;
                }
                decider::LocalCheckOutcome::Missing { reason } => {
                    decider::record_decision(DecisionPath::Fast, 0);
                    self.log_path(&seed.id, DecisionPath::Fast, 0, &format!("missing={}", decider::log_head(&reason, 120)));
                    self.reprompt_session(
                        &seed.id,
                        &decider::reprompt_text_missing_evidence(&reason),
                    );
                    let mut inner = lock(&self.inner);
                    if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                        s.iterations = s.iterations.saturating_add(1);
                        s.status = Status::Working;
                        s.revision_changed_at_ms = self.now();
                        s.consecutive_decider_failures = 0;
                    }
                    self.log_decision(
                        &mut inner,
                        &seed.sup,
                        &seed.id,
                        "verify",
                        &format!("fast-path missing evidence: {reason}; re-prompting"),
                    );
                    continue;
                }
                decider::LocalCheckOutcome::NotRunnable => {}
            }

            let model = seed
                .model
                .clone()
                .unwrap_or_else(|| decider::DECIDER_MODEL.to_string());
            let remaining = seed
                .wall_clock_deadline
                .map(|d| d.saturating_sub(now) / 1000)
                .unwrap_or(3600);
            prepared.push(Prepared { seed, tail, model, remaining });
        }

        // ---- Phase B: non-blocking submit to the long-lived workers ----
        // `try_send` on a bounded channel: a full queue or a session already
        // in flight is skipped until a later poll, never waited on.
        for p in prepared {
            let prompt = p.seed.prompt(p.remaining, &p.tail);
            let job = Job {
                session: p.seed.id.clone(),
                kind: JobKind::Judge,
                model: p.model.clone(),
                prompt,
                ctx: JobCtx {
                    seed: p.seed,
                    tail: p.tail,
                    model: p.model,
                    remaining: p.remaining,
                },
            };
            let _ = pool.try_submit(job);
        }
    }

    /// Commit a background spawn's result to the session tree. The ack was
    /// already returned; failures land here as warnings + failed status.
    fn apply_spawn_outcome(&self, outcome: super::lifecycle::SpawnOutcome) {
        let super::lifecycle::SpawnOutcome { session, warnings, error, pane } = outcome;
        let cwd = session.cwd.clone();
        {
            let mut inner = lock(&self.inner);
            if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == session.id) {
                s.pane_id = session.pane_id;
                s.cwd = session.cwd;
                s.worktree = session.worktree;
                s.branch = session.branch;
                s.base_rev = session.base_rev;
                s.temp_dir = session.temp_dir;
                if error.is_some() {
                    s.status = Status::Failed;
                    s.failure_reason = Some("spawn_failed".to_string());
                }
            }
            for warning in warnings {
                push_warning(&mut inner, warning);
            }
            self.save_state(&inner);
        }
        if let Some(e) = error {
            let _ = self.event_log.append(None, "escalate", &format!("spawn_failed: {e}"));
        } else if let Some(pane) = pane {
            let _ = self.event_log.append(
                Some(session.id.clone()),
                "spawn",
                &format!("pane={pane} cwd={cwd}"),
            );
        }
    }

    /// Apply one completed worker result. Preserves the original decision-loop
    /// semantics: loop detection, stuck strikes, budget accounting, verifier
    /// evidence checks, and "never re-prompt a working session".
    fn apply_judgment(&self, f: Finished, now: u64, pool: &Arc<JudgePool>) {
        let Finished { ctx, raw, path, ms, .. } = f;
        let JobCtx { seed, tail, model, remaining } = ctx;
        self.log_path(&seed.id, path, ms, "judgment");

        // Persist the new tail for loop detection across calls.
        let tkey = tail_key(&tail);

        match raw {
            Err(e) => {
                let head = decider::log_head(&e, 120);
                let failures = seed.consecutive_decider_failures.saturating_add(1);
                {
                    let mut inner = lock(&self.inner);
                    if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                        s.consecutive_decider_failures = failures;
                    }
                    self.log_decision(
                        &mut inner,
                        &seed.sup,
                        &seed.id,
                        "backoff",
                        &format!("decider failed: {head}"),
                    );
                }
                if failures >= 3 {
                    self.escalate_session(
                        &seed.id,
                        "budget_exceeded",
                        "decider failed 3 times in a row",
                    );
                }
            }
            Ok(raw) => {
                let raw_head = decider::log_head(&raw, 200);
                {
                    let mut inner = lock(&self.inner);
                    if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                        s.consecutive_decider_failures = 0;
                        s.last_tails.push_back(tkey);
                        while s.last_tails.len() > 3 {
                            s.last_tails.pop_front();
                        }
                    }
                }
                match decider::parse_decider_output(&raw) {
                    decider::DeciderVerdict::Done { evidence } => {
                        // P1 + 3.2: DONE is never applied on a bare judgment.
                        // The evidence-checking call is a second Job kind on
                        // the same bounded pool, so the poll thread stays free
                        // and no status flips before a verifier decision.
                        let vprompt = decider::verifier_prompt(
                            seed.goal.as_deref().unwrap_or(""),
                            seed.dod.as_deref(),
                            &tail,
                            &evidence,
                        );
                        pool.pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(
                                seed.id.clone(),
                                PendingVerify {
                                    seed: seed.clone(),
                                    tail: tail.clone(),
                                },
                            );
                        let job = Job {
                            session: seed.id.clone(),
                            kind: JobKind::Verify,
                            model: model.clone(),
                            prompt: vprompt,
                            ctx: JobCtx {
                                seed: seed.clone(),
                                tail: tail.clone(),
                                model: model.clone(),
                                remaining,
                            },
                        };
                        if !pool.try_submit(job) {
                            // A job for this session is already in flight:
                            // drop the stale pending entry, retry next poll.
                            pool.pending
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .remove(&seed.id);
                            self.log_decision(
                                &mut lock(&self.inner),
                                &seed.sup,
                                &seed.id,
                                "verify_pending",
                                "verifier slot busy; DONE deferred",
                            );
                        }
                    }
                    decider::DeciderVerdict::Continue { instruction } => {
                        if !matches!(self.admit_start(&seed.id, self.clock.mono_ms(), &seed.sup), crate::supervisor::StartGate::Admit) {
                            return;
                        }
                        let mut inner = lock(&self.inner);
                        let three_identical = inner
                            .sessions
                            .iter()
                            .find(|s| s.id == seed.id)
                            .map(|s| {
                                s.last_tails.len() >= 3
                                    && s.last_tails
                                        .iter()
                                        .collect::<std::collections::HashSet<_>>()
                                        .len()
                                        == 1
                            })
                            .unwrap_or(false);
                        if three_identical {
                            drop(inner);
                            self.escalate_session(
                                &seed.id,
                                "loop_detected",
                                "3 near-identical output tails after CONTINUE re-prompts",
                            );
                            return;
                        }
                        self.reprompt_session(&seed.id, &instruction);
                        if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                            s.iterations = s.iterations.saturating_add(1);
                            s.status = Status::Working;
                            s.revision_changed_at_ms = self.now();
                            s.stuck_strikes = 0;
                        }
                        self.log_decision(
                            &mut inner,
                            &seed.sup,
                            &seed.id,
                            "reprompt",
                            &format!("continue: {instruction}"),
                        );
                    }
                    decider::DeciderVerdict::Stuck { blocker } => {
                        if !matches!(self.admit_start(&seed.id, self.clock.mono_ms(), &seed.sup), crate::supervisor::StartGate::Admit) {
                            return;
                        }
                        let mut inner = lock(&self.inner);
                        let prev_strikes = inner
                            .sessions
                            .iter()
                            .find(|s| s.id == seed.id)
                            .map(|s| (s.stuck_strikes, s.strategy_changed))
                            .unwrap_or((0, false));
                        let (strikes, strategy_changed) =
                            (prev_strikes.0.saturating_add(1), prev_strikes.1);
                        if strategy_changed && strikes >= 3 {
                            drop(inner);
                            self.escalate_session(
                                &seed.id,
                                "loop_detected",
                                &format!("stuck persists after strategy change: {blocker}"),
                            );
                            return;
                        }
                        if strikes >= 3 && !strategy_changed {
                            // One strategy change first.
                            self.reprompt_session(
                                &seed.id,
                                &decider::reprompt_text_stuck(&blocker),
                            );
                            if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                                s.iterations = s.iterations.saturating_add(1);
                                s.status = Status::Working;
                                s.revision_changed_at_ms = self.now();
                                s.stuck_strikes = strikes;
                                s.strategy_changed = true;
                            }
                            self.log_decision(
                                &mut inner,
                                &seed.sup,
                                &seed.id,
                                "reprompt",
                                &format!("stuck ×3; strategy change: {blocker}"),
                            );
                            return;
                        }
                        self.reprompt_session(
                            &seed.id,
                            &decider::reprompt_text_stuck(&blocker),
                        );
                        if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                            s.iterations = s.iterations.saturating_add(1);
                            s.status = Status::Working;
                            s.revision_changed_at_ms = self.now();
                            s.stuck_strikes = strikes;
                        }
                        self.log_decision(
                            &mut inner,
                            &seed.sup,
                            &seed.id,
                            "reprompt",
                            &format!("stuck; fresh framing: {blocker}"),
                        );
                    }
                    decider::DeciderVerdict::Unparseable { head } => {
                        if !matches!(self.admit_start(&seed.id, self.clock.mono_ms(), &seed.sup), crate::supervisor::StartGate::Admit) {
                            return;
                        }
                        let strikes = seed.stuck_strikes.saturating_add(1);
                        self.reprompt_session(&seed.id, &decider::reprompt_text_stuck(
                            "the supervisor's judgment could not be parsed; start from the simplest verifiable step",
                        ));
                        let mut inner = lock(&self.inner);
                        if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                            s.iterations = s.iterations.saturating_add(1);
                            s.status = Status::Working;
                            s.revision_changed_at_ms = self.now();
                            s.stuck_strikes = strikes;
                            s.consecutive_decider_failures =
                                s.consecutive_decider_failures.saturating_add(1);
                        }
                        self.log_decision(
                            &mut inner,
                            &seed.sup,
                            &seed.id,
                            "reprompt",
                            &format!("decider unparseable: {head}"),
                        );
                        if strikes >= 3 {
                            drop(inner);
                            self.escalate_session(
                                &seed.id,
                                "loop_detected",
                                "3 unparseable decider answers in a row",
                            );
                        }
                    }
                }
                let _ = raw_head; // raw answer head logged via decision reasons; full text stays out of events
            }
        }
        let _ = now;
    }

    /// Route a finished worker result by job kind. Both kinds are applied
    /// here, on the poll thread, so state transitions stay serialized.
    fn apply_finished(&self, f: Finished, now: u64, pool: &Arc<JudgePool>) {
        match f.kind {
            JobKind::Judge => self.apply_judgment(f, now, pool),
            JobKind::Verify => self.apply_verify(f, now),
        }
    }

    /// Apply a verifier answer. This is the ONLY path (besides the local-DoD
    /// fast path) allowed to log `decision{action:"done"}` and set Done.
    fn apply_verify(&self, f: Finished, now: u64) {
        let Finished { session, raw, path, ms, .. } = f;
        let Some(pending) = self
            .judge_pool()
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session)
        else {
            return; // stale result (session escalated/removed meanwhile)
        };
        let PendingVerify { seed, tail, .. } = pending;
        self.log_path(&seed.id, path, ms, "verifier");
        let verdict = raw.ok().map(|v| decider::parse_verifier_output(&v));
        match verdict {
            Some(decider::VerifierVerdict::Yes { why }) => {
                let mut inner = lock(&self.inner);
                if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                    s.status = Status::Done;
                    s.failure_reason = None;
                }
                self.log_decision(
                    &mut inner,
                    &seed.sup,
                    &seed.id,
                    "done",
                    &format!("verifier YES: {why}"),
                );
                let _ = self.event_log.append(
                    Some(seed.id.clone()),
                    "status",
                    "done (verifier accepted evidence)",
                );
                drop(inner);
                let _ = cleanup::cleanup_session(&self.config.tmp_dir, &seed.id);
            }
            Some(decider::VerifierVerdict::No { missing }) => {
                if !matches!(self.admit_start(&seed.id, self.clock.mono_ms(), &seed.sup), crate::supervisor::StartGate::Admit) {
                    return;
                }
                self.reprompt_session(
                    &seed.id,
                    &decider::reprompt_text_missing_evidence(&missing),
                );
                self.advance_after_failed_verify(&seed, now);
                self.log_decision(
                    &mut lock(&self.inner),
                    &seed.sup,
                    &seed.id,
                    "verify",
                    &format!("verifier NO: {missing}; re-prompting for evidence"),
                );
            }
            Some(decider::VerifierVerdict::Unparseable { head }) => {
                if !matches!(self.admit_start(&seed.id, self.clock.mono_ms(), &seed.sup), crate::supervisor::StartGate::Admit) {
                    return;
                }
                self.reprompt_session(
                    &seed.id,
                    &decider::reprompt_text_missing_evidence(
                        "the verifier could not parse its own answer",
                    ),
                );
                self.advance_after_failed_verify(&seed, now);
                self.log_decision(
                    &mut lock(&self.inner),
                    &seed.sup,
                    &seed.id,
                    "verify",
                    &format!("verifier unparseable: {head}; re-prompting"),
                );
            }
            None => {
                if !matches!(self.admit_start(&seed.id, self.clock.mono_ms(), &seed.sup), crate::supervisor::StartGate::Admit) {
                    return;
                }
                self.reprompt_session(
                    &seed.id,
                    &decider::reprompt_text_missing_evidence("the verifier call failed"),
                );
                self.advance_after_failed_verify(&seed, now);
                self.log_decision(
                    &mut lock(&self.inner),
                    &seed.sup,
                    &seed.id,
                    "verify",
                    "verifier call failed; re-prompting",
                );
            }
        }
        let _ = tail; // carried for parity with the judgment context
    }

    fn advance_after_failed_verify(&self, seed: &Seed, now: u64) {
        let mut inner = lock(&self.inner);
        if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
            s.iterations = s.iterations.saturating_add(1);
            s.status = Status::Working;
            s.revision_changed_at_ms = now;
        }
    }

    /// P1: herdr can report `done` long before the evidence verifier has
    /// spoken. A DoD-bearing session may only become Done after a
    /// `decision{action:"done"}` has been logged (fast path or verifier YES).
    /// Every poll, any premature Done is reverted and a judgment is submitted
    /// on the bounded pool; the in-flight set keeps this to one call/session.
    fn enforce_dod_gate(&self, now: u64, pool: &Arc<JudgePool>) {
        let premature: Vec<(String, String)> = {
            let inner = lock(&self.inner);
            inner
                .sessions
                .iter()
                .filter(|s| s.status == Status::Done && s.definition_of_done.is_some())
                .filter(|s| {
                    !inner
                        .decisions
                        .iter()
                        .any(|d| d.session == s.id && d.action == "done")
                })
                .map(|s| (s.id.clone(), s.sup.clone()))
                .collect()
        };
        for (id, sup) in premature {
            {
                let mut inner = lock(&self.inner);
                if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == id) {
                    s.status = Status::Working;
                    s.failure_reason = None;
                }
                self.log_decision(
                    &mut inner,
                    &sup,
                    &id,
                    "verify_pending",
                    "herdr reported done before evidence verification; reverted",
                );
            }
            self.save_state(&lock(&self.inner));
            if pool.in_flight.lock().unwrap_or_else(|e| e.into_inner()).contains(&id) {
                continue; // one judgment per session, no spin
            }
            let (goal, dod, model, iterations, max_iterations, session_cwd) = {
                let inner = lock(&self.inner);
                let Some(s) = inner.sessions.iter().find(|s| s.id == id) else {
                    continue;
                };
                (
                    s.goal.clone(),
                    s.definition_of_done.clone(),
                    s.model.clone().unwrap_or_else(|| decider::DECIDER_MODEL.to_string()),
                    s.iterations,
                    s.max_iterations,
                    s.cwd.clone(),
                )
            };
            // herdr IO happens with no locks held.
            let tail = self.herdr.agent_read(&agent_target(&id), 200).unwrap_or_default();
            let remaining = 3600;
            let prompt = decider::decider_prompt(
                goal.as_deref().unwrap_or(""),
                dod.as_deref(),
                iterations.saturating_add(1),
                max_iterations,
                remaining,
                &tail,
            );
            let job = Job {
                session: id.clone(),
                kind: JobKind::Judge,
                model: model.clone(),
                prompt,
                ctx: JobCtx {
                    seed: Seed {
                        id,
                        sup,
                        cwd: session_cwd,
                        status: Status::Working,
                        goal,
                        dod,
                        model: Some(model.clone()),
                        iterations,
                        max_iterations,
                        wall_clock_deadline: None,
                        backoff_until_ms: 0,
                        consecutive_decider_failures: 0,
                        stuck_strikes: 0,
                        session_ref: None,
                        revision_changed_at_ms: now,
                    },
                    tail,
                    model,
                    remaining,
                },
            };
            let _ = pool.try_submit(job);
        }
    }
}
