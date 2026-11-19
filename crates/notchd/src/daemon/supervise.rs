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
use std::time::Instant;

/// Upper bound on simultaneous decider judgments.
const MAX_PARALLEL_JUDGMENTS: usize = 4;

/// A session that passed all gates and needs a model judgment.
struct Prepared {
    idx: usize,
    seed: Seed,
    tail: String,
    model: String,
    remaining: u64,
}

/// A completed model judgment, with the prepare context carried back so
/// verdicts can be applied serially in seed order.
struct Judgment {
    idx: usize,
    seed: Seed,
    tail: String,
    model: String,
    raw: Result<String, String>,
    path: DecisionPath,
    ms: u64,
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
) -> (Result<String, String>, DecisionPath, u64) {
    let start = Instant::now();
    let Some(oc) = client else {
        let ms = start.elapsed().as_millis() as u64;
        let path = DecisionPath::Cold;
        decider::record_decision(path, ms);
        return (Err("decider client not configured".to_string()), path, ms);
    };

    let sid = decider::decider_session_id();
    let (result, path) = match oc.run_decider_session(model, &sid, prompt) {
        Ok(raw) => (Ok(raw), DecisionPath::Persistent),
        Err(persist_err) => {
            // Session reuse failed (bad id, corrupt context, quota on that
            // session, …). Fall back to the original cold path — never lose a
            // judgment because the optimization broke.
            match oc.run_decider(model, prompt) {
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

    /// Clone the injectable client so judgment threads never hold the
    /// `opencode` mutex across a (slow) process call. `Opencode` is `Clone`
    /// (cmd string + shared runner Arc), so cloned clients share test seams.
    fn opencode_client(&self) -> Option<Opencode> {
        lock(&self.opencode).clone()
    }

    /// Serial variant used for the verifier call inside the apply phase.
    fn judge_serial(&self, model: &str, prompt: &str) -> (Result<String, String>, DecisionPath, u64) {
        let client = self.opencode_client();
        judge(client.as_ref(), model, prompt)
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

    /// Fan judgments out to at most `MAX_PARALLEL_JUDGMENTS` std threads.
    /// Results are returned in seed order so state application stays
    /// deterministic (loop detection reads `last_tails` in that order).
    fn parallel_judge(&self, jobs: Vec<Prepared>) -> Vec<Judgment> {
        if jobs.is_empty() {
            return Vec::new();
        }
        let make = |j: Prepared, client: Option<&Opencode>| {
            let prompt = j.seed.prompt(j.remaining, &j.tail);
            let (raw, path, ms) = judge(client, &j.model, &prompt);
            Judgment {
                idx: j.idx,
                seed: j.seed,
                tail: j.tail,
                model: j.model,
                raw,
                path,
                ms,
            }
        };
        if jobs.len() == 1 {
            let client = self.opencode_client();
            return jobs.into_iter().map(|j| make(j, client.as_ref())).collect();
        }

        let workers = jobs.len().min(MAX_PARALLEL_JUDGMENTS);
        let mut chunks: Vec<Vec<Prepared>> = (0..workers).map(|_| Vec::new()).collect();
        for (i, job) in jobs.into_iter().enumerate() {
            chunks[i % workers].push(job);
        }
        let client = self.opencode_client();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let client = client.clone();
                std::thread::spawn(move || {
                    chunk
                        .into_iter()
                        .map(|j| make(j, client.as_ref()))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut results = Vec::new();
        for h in handles {
            if let Ok(v) = h.join() {
                results.extend(v);
            }
        }
        results.sort_by_key(|r| r.idx);
        results
    }

    pub(crate) fn supervise_once(&self, api_sessions: Option<&[crate::opencode::ApiSession]>) {
        let now = self.now();
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
        for (idx, seed) in seeds.into_iter().enumerate() {
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
            prepared.push(Prepared { idx, seed, tail, model, remaining });
        }

        // ---- Phase B: bounded-parallel model judgments ----
        let judgments = self.parallel_judge(prepared);

        // ---- Phase C: apply verdicts serially, in seed order ----
        for judgment in judgments {
            self.apply_judgment(judgment, now);
        }
        let _ = now;
    }

    /// Apply one completed judgment. Preserves the original decision-loop
    /// semantics: loop detection, stuck strikes, budget accounting, verifier
    /// evidence checks, and "never re-prompt a working session".
    fn apply_judgment(&self, j: Judgment, now: u64) {
        let Judgment { seed, tail, model, raw, path, ms, .. } = j;
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
                        // Verifier pass — same model, evidence-checking prompt.
                        let vprompt = decider::verifier_prompt(
                            seed.goal.as_deref().unwrap_or(""),
                            seed.dod.as_deref(),
                            &tail,
                            &evidence,
                        );
                        let (vraw, vpath, vms) = self.judge_serial(&model, &vprompt);
                        self.log_path(&seed.id, vpath, vms, "verifier");
                        let verdict = vraw.ok().map(|v| decider::parse_verifier_output(&v));
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
                                let _ = cleanup::cleanup_session(&self.config.tmp_dir, &seed.id);
                            }
                            Some(decider::VerifierVerdict::No { missing }) => {
                                self.reprompt_session(
                                    &seed.id,
                                    &decider::reprompt_text_missing_evidence(&missing),
                                );
                                let mut inner = lock(&self.inner);
                                if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                                    s.iterations = s.iterations.saturating_add(1);
                                    s.status = Status::Working;
                                    s.revision_changed_at_ms = self.now();
                                }
                                self.log_decision(
                                    &mut inner,
                                    &seed.sup,
                                    &seed.id,
                                    "verify",
                                    &format!("verifier NO: {missing}; re-prompting for evidence"),
                                );
                            }
                            Some(decider::VerifierVerdict::Unparseable { head }) => {
                                self.reprompt_session(
                                    &seed.id,
                                    &decider::reprompt_text_missing_evidence(
                                        "the verifier could not parse its own answer",
                                    ),
                                );
                                let mut inner = lock(&self.inner);
                                if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                                    s.iterations = s.iterations.saturating_add(1);
                                    s.status = Status::Working;
                                    s.revision_changed_at_ms = self.now();
                                }
                                self.log_decision(
                                    &mut inner,
                                    &seed.sup,
                                    &seed.id,
                                    "verify",
                                    &format!("verifier unparseable: {head}; re-prompting"),
                                );
                            }
                            None => {
                                self.reprompt_session(
                                    &seed.id,
                                    &decider::reprompt_text_missing_evidence(
                                        "the verifier call failed",
                                    ),
                                );
                                let mut inner = lock(&self.inner);
                                if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == seed.id) {
                                    s.iterations = s.iterations.saturating_add(1);
                                    s.status = Status::Working;
                                    s.revision_changed_at_ms = self.now();
                                }
                                self.log_decision(
                                    &mut inner,
                                    &seed.sup,
                                    &seed.id,
                                    "verify",
                                    "verifier call failed; re-prompting",
                                );
                            }
                        }
                    }
                    decider::DeciderVerdict::Continue { instruction } => {
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
}
