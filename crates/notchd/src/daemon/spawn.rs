//! Spawning: plans, single spawns, `submit_task`, and the pre-spawn hook.
//! Every path allocates IDs through the registry, splits a herdr pane,
//! starts the opencode harness in it, and prompts it with the full text.
use super::acks::{RunPlanAck, SpawnAck, SubmitTaskAck};
use super::lifecycle::SpawnOutcome;
use super::types::{agent_target, lock, push_warning, Daemon};
use crate::cleanup;
use crate::herdr::{Herdr, HerdrError};
use crate::model::{single_line, truncate_words, SessionState, SupervisorMeta};
use crate::plan;
use crate::status::Status;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

/// Backoffs before retries 1/2/3 of a transient herdr busy response. Total
/// worst-case wait is 1.5 s; the attempt count is bounded (never a spin loop).
const BUSY_RETRY_DELAYS_MS: &[u64] = &[250, 500, 750];

/// Generous bound for the worker's early "real pane id" event before the
/// submit ack. Event-driven (channel recv, no sleep-as-sync): under a loaded
/// host the detached thread can take hundreds of ms to reach herdr, but a
/// genuinely stalled/gated herdr must not hold the fast ack past the P2
/// <1 s contract. On timeout the pane id stays honestly empty until the
/// full SpawnOutcome is applied on poll — never a fabricated id.
const PANE_ID_ACK_WAIT: Duration = Duration::from_millis(750);

/// Early signal from the detached worker, before the full SpawnOutcome.
pub(crate) enum PaneEvent {
    /// herdr returned a real pane id — publish it before the ack/snapshots.
    Ready(String),
    /// herdr pane split failed for good — release the ack immediately; the
    /// failure still lands honestly via the SpawnOutcome on the next poll.
    Failed,
}

/// Does this herdr failure look like a transient busy state?
/// `agent_pane_busy` is the observed live string; "busy" also covers other
/// herdr busy spellings without swallowing real failures.
fn is_busy_error(err: &HerdrError) -> bool {
    err.to_string().to_ascii_lowercase().contains("busy")
}

/// Run `call` once, then retry up to 3 times with 250/500/750 ms backoff, but
/// ONLY on a busy error. Any other error returns immediately.
fn retry_busy<T, F: FnMut() -> Result<T, HerdrError>>(mut call: F) -> Result<T, HerdrError> {
    let mut last = None;
    for delay in std::iter::once(0u64).chain(BUSY_RETRY_DELAYS_MS.iter().copied()) {
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        match call() {
            Ok(value) => return Ok(value),
            Err(e) if is_busy_error(&e) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("initial attempt always runs"))
}

/// The slow part of a spawn, factored out so `submit_task` can return its ack
/// before herdr agent-start readiness completes. All side effects are pure
/// data: the caller applies the returned outcome on the poll thread.
pub(crate) fn spawn_detached(
    mut session: SessionState,
    prompt: String,
    herdr: Herdr,
    tmp_dir: std::path::PathBuf,
    sessions: Vec<SessionState>,
    pane_tx: Option<Sender<PaneEvent>>,
) -> SpawnOutcome {
    let mut warnings = Vec::new();
    match cleanup::enforce_budget(&tmp_dir, &sessions, cleanup::max_tmp_bytes()) {
        Ok((_, ws)) => warnings.extend(ws),
        Err(e) => warnings.push(format!("scratch budget check failed: {e}")),
    }
    let tmp = match crate::worktree::ensure_layout(&tmp_dir, &session.id) {
        Ok(t) => t,
        Err(e) => {
            if let Some(tx) = &pane_tx {
                let _ = tx.send(PaneEvent::Failed);
            }
            return SpawnOutcome {
                session,
                warnings,
                error: Some(format!("temp dir: {e}")),
                pane: None,
            };
        }
    };
    session.temp_dir = Some(tmp.to_string_lossy().to_string());

    match crate::worktree::create(Path::new(&session.cwd), &session.id, &tmp_dir) {
        Ok(Some(wt)) => {
            session.cwd = wt.path.to_string_lossy().to_string();
            session.worktree = Some(wt.path.to_string_lossy().to_string());
            session.branch = Some(wt.branch.clone());
            session.base_rev = Some(wt.base_rev.clone());
        }
        Ok(None) => warnings.push(format!(
            "{}: cwd `{}` {}; per-session worktree disabled — attribution falls back to the shared-cwd heuristic",
            session.id,
            session.cwd,
            crate::worktree::fallback_reason(Path::new(&session.cwd))
        )),
        Err(e) => warnings.push(format!(
            "{}: worktree setup failed ({e}); spawning in `{}` with shared-cwd heuristic",
            session.id, session.cwd
        )),
    }

    let env = crate::worktree::pane_env(
        &tmp_dir,
        &session.id,
        crate::worktree::isolate_build_enabled(),
    );
    // V2: herdr can report `agent_pane_busy` transiently under a concurrent
    // 2×2 spawn. Retry with bounded backoff instead of losing the pane; a
    // permanent busy still fails honestly and the allocated ID is preserved.
    let pane = match retry_busy(|| herdr.pane_split(&session.cwd, &env)) {
        Ok(p) => p,
        Err(e) => {
            if let Some(tx) = &pane_tx {
                let _ = tx.send(PaneEvent::Failed);
            }
            warnings.push(format!(
                "{}: pane split failed after bounded busy retry: {e}",
                session.id
            ));
            return SpawnOutcome {
                session,
                warnings,
                error: Some(format!("pane split: {e}")),
                pane: None,
            };
        }
    };
    // Publish the REAL pane id the instant herdr gives it to us, before the
    // slower agent-start/prompt work. `submit_task` uses this to keep the
    // paneId in emitted snapshots correct under fast-ack (conformance).
    if let Some(tx) = &pane_tx {
        let _ = tx.send(PaneEvent::Ready(pane.clone()));
    }
    session.pane_id = pane.clone();
    let agent_name = agent_target(&session.id);
    if let Err(e) = retry_busy(|| herdr.agent_start(&agent_name, &pane)) {
        let _ = herdr.pane_close(&pane);
        warnings.push(format!(
            "{}: agent start failed after bounded busy retry: {e}",
            session.id
        ));
        return SpawnOutcome {
            session,
            warnings,
            error: Some(format!("agent start: {e}")),
            pane: Some(pane),
        };
    }
    if let Err(e) = herdr.agent_prompt(&agent_name, &prompt) {
        warnings.push(format!("{} prompt failed: {e}", session.id));
    }
    SpawnOutcome {
        session,
        warnings,
        error: None,
        pane: Some(pane),
    }
}

impl Daemon {
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
        let started_at = self.now();
        let plan_title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| truncate_words(s, 48))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Plan".to_string());
        let mut new_supervisor_ids: Vec<(String, String)> = Vec::new(); // (id, title)
        let mut new_sessions: Vec<SessionState> = Vec::new();
        let mut prompts: Vec<(String, String)> = Vec::new(); // (session id, prompt)

        for sessions in &parsed.supervisors {
            let sup_id = self.alloc_supervisor();
            let sup_number: u32 = sup_id
                .strip_prefix("SV-")
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            new_supervisor_ids.push((sup_id.clone(), plan_title.clone()));
            for ses in sessions {
                let id = self.alloc_session(sup_number);
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
                    goal: Some(ses.prompt.clone()),
                    definition_of_done: None,
                    iterations: 0,
                    max_iterations: crate::model::default_max_iterations(),
                    wall_clock_deadline: Some(started_at + 3_600_000),
                    last_supervisor_action: None,
                    model: None,
                    session_ref: None,
                    escalated: false,
                    temp_dir: None,
                    worktree: None,
                    branch: None,
                    base_rev: None,
                    backoff_until_ms: 0,
                    consecutive_decider_failures: 0,
                    stuck_strikes: 0,
                    strategy_changed: false,
                    last_tails: std::collections::VecDeque::new(),
                };
                prompts.push((id.clone(), ses.prompt.clone()));
                new_sessions.push(session);
            }
        }

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
            for (id, title) in &new_supervisor_ids {
                let number: u32 = id
                    .strip_prefix("SV-")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                self.register_supervisor_locked(
                    &mut inner,
                    &SupervisorMeta {
                        id: id.clone(),
                        number,
                        title: title.clone(),
                    },
                );
            }
            for session in new_sessions {
                inner.sessions.push(session);
            }
            inner.sessions.sort_by(|a, b| a.id.cmp(&b.id));
            inner.supervisors.sort_by_key(|s| s.number);
            self.save_now(&inner);
        }
        for (id, _) in &new_supervisor_ids {
            let _ = self
                .event_log
                .append(None, "spawn", &format!("supervisor {id}"));
        }
        let spawned: Vec<String> = new_supervisor_ids.into_iter().map(|(id, _)| id).collect();
        self.notify_change();
        RunPlanAck {
            ok: true,
            spawned,
            error: None,
        }
    }

    /// Best-effort, non-fatal pre-spawn hook. The ONLY candidate is an explicit
    /// NOTCH_PRE_SPAWN_HOOK (a shell snippet) supplied by the operator. notch
    /// never forces a house model into OpenCode: the harness default is the
    /// default, and there is no implicit script candidate (HARDENING §3.1).
    /// Missing or failing -> logged, never fails the spawn. A 3 s timeout kills
    /// a hung hook.
    pub(crate) fn pre_spawn_hook(&self) {
        let candidates: Vec<String> = match std::env::var("NOTCH_PRE_SPAWN_HOOK") {
            Ok(h) if !h.trim().is_empty() => vec![h],
            _ => Vec::new(),
        };
        for candidate in candidates {
            if !candidate.contains(char::is_whitespace) && !Path::new(&candidate).exists() {
                continue;
            }
            let mut child = match std::process::Command::new("sh")
                .arg("-c")
                .arg(&candidate)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = self.event_log.append(
                        None,
                        "status",
                        &format!("pre-spawn hook skipped ({e})"),
                    );
                    continue;
                }
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(3000);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        let _ = self.event_log.append(
                            None,
                            "status",
                            &format!("pre-spawn hook ok: {candidate}"),
                        );
                        return;
                    }
                    Ok(None) if std::time::Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let mut inner = lock(&self.inner);
                        push_warning(
                            &mut inner,
                            format!("pre-spawn hook timed out ({candidate}); spawning anyway"),
                        );
                        return;
                    }
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                    Err(e) => {
                        let _ = self.event_log.append(
                            None,
                            "status",
                            &format!("pre-spawn hook error ({e})"),
                        );
                        return;
                    }
                }
            }
        }
        // No candidate present: fine — do not depend on the hook.
    }

    /// Split a pane, start opencode inside it, prompt it. Creates the
    /// per-session temp dir. Returns Err on failure.
    pub(crate) fn spawn_one(&self, session: &mut SessionState, prompt: &str) -> Result<(), String> {
        // A3's watchdog closes panes through supervisor::snapshot_worktree;
        // install our exact snapshot before the first session can be killed.
        crate::worktree::register_snapshot_hook();
        self.pre_spawn_hook();
        let sessions = { lock(&self.inner).sessions.clone() };
        let outcome = spawn_detached(
            session.clone(),
            prompt.to_string(),
            self.herdr.clone(),
            self.config.tmp_dir.clone(),
            sessions,
            None,
        );
        let SpawnOutcome { session: updated, warnings, error, pane } = outcome;
        *session = updated;
        {
            let mut inner = lock(&self.inner);
            for warning in warnings {
                push_warning(&mut inner, warning);
            }
        }
        if let Some(pane) = pane {
            let _ = self.event_log.append(
                Some(session.id.clone()),
                "spawn",
                &format!("pane={pane} cwd={}", session.cwd),
            );
        }
        match error {
            Some(e) => Err(e),
            None => Ok(()),
        }
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
            inner
                .supervisors
                .iter()
                .find(|s| s.id == supervisor)
                .map(|s| s.number)
        };
        let Some(sup_number) = sup_number else {
            return SpawnAck {
                ok: false,
                id: None,
                error: Some(format!("unknown supervisor `{supervisor}`")),
            };
        };
        let id = self.alloc_session(sup_number);
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
            goal: Some(prompt.to_string()),
            definition_of_done: None,
            iterations: 0,
            max_iterations: crate::model::default_max_iterations(),
            wall_clock_deadline: Some(now + 3_600_000),
            last_supervisor_action: None,
            model: None,
            session_ref: None,
            escalated: false,
            temp_dir: None,
            worktree: None,
            branch: None,
            base_rev: None,
            backoff_until_ms: 0,
            consecutive_decider_failures: 0,
            stuck_strikes: 0,
            strategy_changed: false,
            last_tails: std::collections::VecDeque::new(),
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
        {
            let mut inner = lock(&self.inner);
            inner.sessions.push(session);
            inner.sessions.sort_by(|a, b| a.id.cmp(&b.id));
            self.save_now(&inner);
        }
        self.notify_change();
        SpawnAck {
            ok: true,
            id: Some(id),
            error: None,
        }
    }

    /// `submit_task` (bridge.md v1.2). Allocates `SV-<n>` (title = provided
    /// title, or the task head truncated to 48 chars on a word boundary) plus
    /// `S<n>-OC01`, and spawns through the same path as run-plan: pane split
    /// with `NOTCH_SESSION_ID` → agent start → agent prompt with the FULL
    /// (untruncated) task text. Validation failure allocates nothing.
    pub fn submit_task(
        &self,
        task: String,
        cwd: Option<String>,
        title: Option<String>,
        definition_of_done: Option<String>,
        model: Option<String>,
        supervisor_id: Option<String>,
    ) -> SubmitTaskAck {
        fn fail(error: String) -> SubmitTaskAck {
            SubmitTaskAck {
                ok: false,
                supervisor_id: None,
                session_id: None,
                error: Some(error),
            }
        }

        let task_trim = task.trim();
        if task_trim.is_empty() {
            return fail("task must not be empty".to_string());
        }
        let cwd_string = match cwd {
            Some(c) => c.trim().to_string(),
            None => match std::env::current_dir() {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(e) => return fail(format!("cannot resolve the working directory: {e}")),
            },
        };
        if cwd_string.is_empty() {
            return fail("cwd must not be empty".to_string());
        }
        // Canonicalize so the pane split receives an absolute path herdr can
        // use verbatim (relative paths resolve against herdr's cwd, not ours).
        let cwd_path = match std::fs::canonicalize(Path::new(&cwd_string)) {
            Ok(p) => p,
            Err(_) => {
                return fail(format!("cwd `{cwd_string}` does not exist"));
            }
        };
        if !cwd_path.is_dir() {
            return fail(format!("cwd `{}` is not a directory", cwd_path.display()));
        }
        let cwd_string = cwd_path.to_string_lossy().to_string();
        let dod = definition_of_done
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty());
        let model = model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());

        // Resolve the supervisor: reroute into an existing one when
        // `supervisor_id` is set (nothing else allocated), else allocate.
        let (sup_id, sup_number, sup_title, allocating_supervisor) = match &supervisor_id {
            Some(id) => {
                let existing = {
                    let inner = lock(&self.inner);
                    inner
                        .supervisors
                        .iter()
                        .find(|s| &s.id == id)
                        .map(|s| (s.id.clone(), s.number, s.title.clone()))
                };
                match existing {
                    Some((id, number, title)) => (id, number, title, false),
                    None => {
                        return SubmitTaskAck {
                            ok: false,
                            supervisor_id: None,
                            session_id: None,
                            error: Some(format!("unknown supervisor `{id}`")),
                        }
                    }
                }
            }
            None => {
                let title_final = title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(|t| truncate_words(t, 48))
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| truncate_words(task_trim, 48));
                let sup_id = self.alloc_supervisor();
                let sup_number: u32 = sup_id
                    .strip_prefix("SV-")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                (sup_id, sup_number, title_final, true)
            }
        };
        let session_id = self.alloc_session(sup_number);

        let now = self.now();
        let supervisor = SupervisorMeta {
            id: sup_id.clone(),
            number: sup_number,
            title: sup_title,
        };
        let session = SessionState {
            id: session_id.clone(),
            sup: sup_id.clone(),
            status: Status::Working,
            title: truncate_words(task_trim, 48),
            task: single_line(task_trim),
            cwd: cwd_string.clone(),
            pane_id: String::new(),
            started_at: now,
            last_action: None,
            last_revision: None,
            latency_ms: None,
            failure_reason: None,
            failure_count: 0,
            revision_changed_at_ms: now,
            attribution_baseline: None,
            goal: Some(task_trim.to_string()),
            definition_of_done: dod,
            iterations: 0,
            max_iterations: crate::model::default_max_iterations(),
            wall_clock_deadline: Some(now + 3_600_000),
            last_supervisor_action: None,
            model,
            session_ref: None,
            escalated: false,
            temp_dir: None,
            worktree: None,
            branch: None,
            base_rev: None,
            backoff_until_ms: 0,
            consecutive_decider_failures: 0,
            stuck_strikes: 0,
            strategy_changed: false,
            last_tails: std::collections::VecDeque::new(),
        };

        // P2 fast ack: persist the allocated id + session FIRST, return the
        // ack, and let a detached worker do the slow herdr readiness work.
        // Failures are applied by supervise_once via SpawnOutcome and surface
        // as snapshot warnings + failed status (never as a late ack error).
        let spawn_session = session.clone();
        let spawn_prompt = task.clone();
        let herdr = self.herdr.clone();
        let tmp_dir = self.config.tmp_dir.clone();
        let existing = { lock(&self.inner).sessions.clone() };
        let tx = self.spawn_sender();
        let (pane_tx, pane_rx) = mpsc::channel::<PaneEvent>();
        std::thread::spawn(move || {
            let outcome = spawn_detached(
                spawn_session,
                spawn_prompt,
                herdr,
                tmp_dir,
                existing,
                Some(pane_tx),
            );
            let _ = tx.send(outcome);
        });

        let ok = true;
        let error: Option<String> = None;

        {
            let mut inner = lock(&self.inner);
            if allocating_supervisor {
                self.register_supervisor_locked(&mut inner, &supervisor);
            }
            inner.sessions.push(session);
            inner.sessions.sort_by(|a, b| a.id.cmp(&b.id));
            inner.supervisors.sort_by_key(|s| s.number);
            self.save_now(&inner);
        }

        // Publish the real herdr pane id BEFORE the ack/snapshots. The worker
        // signals this event-driven channel the instant herdr returns a pane;
        // the generous bound absorbs a loaded host without holding the fast
        // ack hostage to a gated/slow herdr call. If the id truly never
        // arrives, leave paneId honestly empty (no fake) — the full
        // SpawnOutcome still applies it on poll and the conformance test
        // fails rather than being weakened.
        match pane_rx.recv_timeout(PANE_ID_ACK_WAIT) {
            Ok(PaneEvent::Ready(pane_id)) => {
                let mut inner = lock(&self.inner);
                if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == session_id) {
                    s.pane_id = pane_id;
                    self.save_now(&inner);
                }
            }
            Ok(PaneEvent::Failed) => {
                // Failure lands via SpawnOutcome; do not hold the ack.
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = self.event_log.append(
                    Some(session_id.clone()),
                    "status",
                    "pane pending at ack; real paneId will appear after poll",
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Worker died — its failure surfaces via the SpawnOutcome.
            }
        }
        if allocating_supervisor {
            let _ = self
                .event_log
                .append(None, "spawn", &format!("supervisor {sup_id}"));
        }
        self.notify_change();
        SubmitTaskAck {
            ok,
            supervisor_id: Some(sup_id),
            session_id: Some(session_id),
            error,
        }
    }
}

#[cfg(test)]
mod busy_retry_tests {
    //! V2 verifier finding: a herdr `agent_pane_busy` transient lost one of
    //! four panes in the live 2×2 run. RED first — these assert bounded retry
    //! around pane split / agent start and honest failure with the ID intact.

    use super::spawn_detached;
    use crate::herdr::{Herdr, HerdrError};
    use crate::model::SessionState;
    use crate::status::Status;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn base_session() -> SessionState {
        SessionState {
            id: "S2-OC02".to_string(),
            sup: "SV-2".to_string(),
            status: Status::Working,
            title: "probe".to_string(),
            task: "probe".to_string(),
            cwd: std::env::temp_dir().to_string_lossy().to_string(),
            pane_id: String::new(),
            started_at: 1_700_000_000_000,
            last_action: None,
            last_revision: None,
            latency_ms: None,
            failure_reason: None,
            failure_count: 0,
            revision_changed_at_ms: 0,
            attribution_baseline: None,
            goal: None,
            definition_of_done: None,
            iterations: 0,
            max_iterations: 8,
            wall_clock_deadline: None,
            last_supervisor_action: None,
            model: None,
            session_ref: None,
            escalated: false,
            temp_dir: None,
            worktree: None,
            branch: None,
            base_rev: None,
            backoff_until_ms: 0,
            consecutive_decider_failures: 0,
            stuck_strikes: 0,
            strategy_changed: false,
            last_tails: std::collections::VecDeque::new(),
        }
    }

    #[derive(Default)]
    struct BusyHerdr {
        busy_splits: Mutex<u32>,
        split_calls: Mutex<u32>,
        busy_starts: Mutex<u32>,
        start_calls: Mutex<u32>,
    }

    impl BusyHerdr {
        fn fake(self: &Arc<Self>) -> Herdr {
            let state = Arc::clone(self);
            Herdr::with_runner(move |args: &[String]| {
                let a = args.get(1).cloned().unwrap_or_default();
                let b = args.get(2).cloned().unwrap_or_default();
                let busy = || {
                    Err(HerdrError::Failed {
                        args: args.to_vec(),
                        code: Some(1),
                        stderr: "agent_pane_busy: pane already hosts an agent".to_string(),
                    })
                };
                match (a.as_str(), b.as_str()) {
                    ("pane", "split") => {
                        *state.split_calls.lock().unwrap() += 1;
                        let mut n = state.busy_splits.lock().unwrap();
                        if *n > 0 {
                            *n -= 1;
                            return busy();
                        }
                        Ok(r#"{"id":"cli:pane:split","result":{"type":"pane_info","pane":{"pane_id":"w9:p1"}}}"#.to_string())
                    }
                    ("agent", "start") => {
                        *state.start_calls.lock().unwrap() += 1;
                        let mut n = state.busy_starts.lock().unwrap();
                        if *n > 0 {
                            *n -= 1;
                            return busy();
                        }
                        Ok(r#"{"id":"cli:agent:start","result":{"type":"agent_started","agent":{"pane_id":"w9:p1","agent_status":"working","revision":1}}}"#.to_string())
                    }
                    ("agent", "prompt") => Ok(r#"{"id":"cli:agent:prompt","result":{"type":"agent_prompted","agent":{"pane_id":"w9:p1","agent_status":"working","revision":2}}}"#.to_string()),
                    ("pane", "close") => Ok(r#"{"id":"cli:pane:close","result":{"type":"ok"}}"#.to_string()),
                    _ => Err(HerdrError::Failed { args: args.to_vec(), code: Some(2), stderr: "unexpected".into() }),
                }
            })
        }
    }

    #[test]
    fn busy_pane_split_once_then_success_keeps_id_and_working() {
        let busy = Arc::new(BusyHerdr {
            busy_splits: Mutex::new(1),
            ..Default::default()
        });
        let root = std::env::temp_dir().join(format!("notchd-busy-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let outcome = spawn_detached(
            base_session(),
            "probe".to_string(),
            busy.fake(),
            PathBuf::from(&root),
            Vec::new(),
            None,
        );
        assert!(outcome.error.is_none(), "transient busy must be retried: {:?}", outcome.error);
        assert_eq!(outcome.session.id, "S2-OC02", "allocated ID must survive");
        assert_eq!(outcome.session.status, Status::Working, "session must end working");
        assert_eq!(outcome.session.pane_id, "w9:p1");
        assert_eq!(*busy.split_calls.lock().unwrap(), 2, "one busy response then success");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn permanent_busy_pane_split_fails_after_bounded_retries_keeping_id() {
        let busy = Arc::new(BusyHerdr {
            busy_splits: Mutex::new(100),
            ..Default::default()
        });
        let root = std::env::temp_dir().join(format!("notchd-busy-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let outcome = spawn_detached(
            base_session(),
            "probe".to_string(),
            busy.fake(),
            PathBuf::from(&root),
            Vec::new(),
            None,
        );
        let err = outcome.error.expect("permanent busy must fail honestly");
        assert!(err.contains("busy"), "error must name the busy cause: {err}");
        assert_eq!(outcome.session.id, "S2-OC02", "failed spawn must not drop the ID");
        assert!(outcome.session.pane_id.is_empty());
        assert!(
            outcome.warnings.iter().any(|w| w.contains("busy") && w.contains("S2-OC02")),
            "warning must carry the honest reason: {:?}",
            outcome.warnings
        );
        let calls = *busy.split_calls.lock().unwrap();
        assert!(calls > 1 && calls <= 4, "retry must be bounded (initial + 3): {calls}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
