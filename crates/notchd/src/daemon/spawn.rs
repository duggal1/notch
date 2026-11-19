//! Spawning: plans, single spawns, `submit_task`, and the pre-spawn hook.
//! Every path allocates IDs through the registry, splits a herdr pane,
//! starts the opencode harness in it, and prompts it with the full text.
use super::acks::{RunPlanAck, SpawnAck, SubmitTaskAck};
use super::types::{agent_target, lock, push_warning, Daemon};
use crate::cleanup;
use crate::model::{single_line, truncate_words, SessionState, SupervisorMeta};
use crate::plan;
use crate::status::Status;
use std::path::Path;

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
        let mut reg = self.registry();
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
            let sup_id = reg.allocate_supervisor();
            let sup_number: u32 = sup_id
                .strip_prefix("SV-")
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            new_supervisor_ids.push((sup_id.clone(), plan_title.clone()));
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
            self.save_state(&inner);
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

    /// Best-effort, non-fatal pre-spawn hook. Resolves, in order:
    /// NOTCH_PRE_SPAWN_HOOK (a shell snippet), the repo's
    /// tools/opencode-default-model.sh (via CARGO_MANIFEST_DIR), then that
    /// same path relative to the CWD. Missing or failing -> logged, never
    /// fails the spawn. A 3 s timeout kills a hung hook.
    pub(crate) fn pre_spawn_hook(&self) {
        let candidates: Vec<String> = match std::env::var("NOTCH_PRE_SPAWN_HOOK") {
            Ok(h) if !h.trim().is_empty() => vec![h],
            _ => vec![
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../tools/opencode-default-model.sh"
                )
                .to_string(),
                "tools/opencode-default-model.sh".to_string(),
            ],
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
        self.pre_spawn_hook();
        let tmp = cleanup::session_tmp_dir(&self.config.tmp_dir, &session.id);
        std::fs::create_dir_all(&tmp).map_err(|e| format!("temp dir {}: {e}", tmp.display()))?;
        session.temp_dir = Some(tmp.to_string_lossy().to_string());

        let env = vec![("NOTCH_SESSION_ID".to_string(), session.id.clone())];
        let pane = self
            .herdr
            .pane_split(&session.cwd, &env)
            .map_err(|e| format!("pane split: {e}"))?;
        session.pane_id = pane.clone();

        // The interactive opencode harness ignores --model (verified 2026-11-19:
        // twice — a 30 s startup timeout, then agent_name_not_found). The model
        // field is the DECIDER model only. The interactive session runs the
        // configured default; never pass --model here.
        let agent_name = agent_target(&session.id);
        let start_result = self.herdr.agent_start(&agent_name, &pane);
        if let Err(e) = start_result {
            // We own the pane we just made — close it rather than leak it.
            let _ = self.herdr.pane_close(&pane);
            return Err(format!("agent start: {e}"));
        }
        if let Err(e) = self.herdr.agent_prompt(&agent_target(&session.id), prompt) {
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
        let mut reg = self.registry();
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
                let sup_id = reg.allocate_supervisor();
                let sup_number: u32 = sup_id
                    .strip_prefix("SV-")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                (sup_id, sup_number, title_final, true)
            }
        };
        let session_id = reg.allocate_session(sup_number);
        self.save_registry(&reg);

        let now = self.now();
        let supervisor = SupervisorMeta {
            id: sup_id.clone(),
            number: sup_number,
            title: sup_title,
        };
        let mut session = SessionState {
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
            backoff_until_ms: 0,
            consecutive_decider_failures: 0,
            stuck_strikes: 0,
            strategy_changed: false,
            last_tails: std::collections::VecDeque::new(),
        };

        // Same spawn path as run-plan. The agent prompt carries the FULL
        // task text, untruncated.
        let spawn_result = self.spawn_one(&mut session, &task);

        let mut ok = true;
        let mut error = None;
        if let Err(e) = spawn_result {
            session.status = Status::Failed;
            session.failure_reason = Some("spawn_failed".to_string());
            ok = false;
            error = Some(format!("spawn failed: {e}"));
            {
                let mut inner = lock(&self.inner);
                push_warning(&mut inner, format!("{session_id} spawn failed: {e}"));
            }
            let _ = self.event_log.append(
                Some(session_id.clone()),
                "escalate",
                &format!("spawn_failed: {e}"),
            );
        }

        {
            let mut inner = lock(&self.inner);
            if allocating_supervisor {
                self.register_supervisor_locked(&mut inner, &supervisor);
            }
            inner.sessions.push(session);
            inner.sessions.sort_by(|a, b| a.id.cmp(&b.id));
            inner.supervisors.sort_by_key(|s| s.number);
            self.save_state(&inner);
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
