//! The poll pass: seed (clone) → IO (herdr/git, no locks) → apply (one
//! short critical section). This is where watchdog decisions are applied
//! (stuck kill, blocked-failure re-prompt/escalate, done sweep) and where
//! attribution baselines are tracked.
use super::attribution::record_attribution_locked;
use super::types::{agent_target, lock, push_warning, Daemon};
use crate::attribution;
use crate::cleanup;
use crate::model::truncate;
use crate::status::Status;
use crate::supervisor::{decide_on_failure, reprompt_text, FailureAction};
use std::collections::HashSet;
use std::path::Path;

impl Daemon {
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
        let by_pane: std::collections::HashMap<String, crate::herdr::AgentEntry> =
            agents.into_iter().map(|a| (a.pane_id.clone(), a)).collect();

        // Lazily record each non-terminal session's OpenCode session ref
        // (agent_session.value) once herdr starts reporting it.
        // v1.3: one cached /api/session fetch per poll, shared by the lazy
        // session_ref/model reconciliation and (later) the decider loop.
        // Failure is tolerated: the signal degrades to herdr agent_status.
        let api_sessions: Option<Vec<crate::opencode::ApiSession>> = {
            let has_goal = {
                let inner = lock(&self.inner);
                inner
                    .sessions
                    .iter()
                    .any(|s| !s.status.is_terminal() && s.goal.is_some())
            };
            if has_goal {
                self.opencode_api_sessions()
            } else {
                None
            }
        };
        {
            let mut inner = lock(&self.inner);
            let mut recorded = false;
            for s in inner.sessions.iter_mut() {
                if s.session_ref.is_none() {
                    if let Some(entry) = by_pane.get(&s.pane_id) {
                        if let Some(v) = entry.agent_session_value() {
                            s.session_ref = Some(v);
                            recorded = true;
                        }
                    }
                }
            }
            if recorded {
                self.save_state(&inner);
            }
        }

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
        struct UpdateEffect {
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
        }
        enum Effect {
            Vanished { idx: usize },
            Update(Box<UpdateEffect>),
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
                    let tail = self.herdr.agent_read(&agent_target(&seed.id), 20).ok();
                    match decide_on_failure(seed.failure_count, tail) {
                        FailureAction::Reprompt { tail } => {
                            let text = reprompt_text(&seed.task_head, &tail);
                            match self.herdr.agent_prompt(&agent_target(&seed.id), &text) {
                                Ok(()) => effects.push(Effect::Update(Box::new(UpdateEffect {
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
                                }))),
                                Err(e) => effects.push(Effect::Update(Box::new(UpdateEffect {
                                    idx: seed.idx,
                                    id: seed.id,
                                    revision: Some(entry.revision),
                                    revision_changed,
                                    status: Status::Failed,
                                    last_action: None,
                                    files: Vec::new(),
                                    reprompt_text: None,
                                    reprompt_failed: Some(format!("re-prompt failed: {e}")),
                                    escalate: Some(
                                        reason.unwrap_or_else(|| "blocked_waiting".into()),
                                    ),
                                    mark_done: false,
                                    watchdog_stuck: false,
                                    pane_id: seed.pane_id,
                                    baseline: None,
                                }))),
                            }
                        }
                        FailureAction::Escalate => {
                            effects.push(Effect::Update(Box::new(UpdateEffect {
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
                            })))
                        }
                    }
                }
                Status::Done => {
                    effects.push(Effect::Update(Box::new(UpdateEffect {
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
                    })));
                }
                Status::Idle | Status::Working => {
                    let (baseline_new, files) = if mapped == Status::Working {
                        let sampled = attribution::sample_git(Path::new(&seed.cwd));
                        match &seed.baseline {
                            None => (
                                Some(sampled.into_iter().collect::<HashSet<_>>()),
                                Vec::new(),
                            ),
                            Some(b) => (
                                None,
                                sampled.into_iter().filter(|f| !b.contains(f)).collect(),
                            ),
                        }
                    } else {
                        (None, Vec::new())
                    };
                    let last_action = if mapped == Status::Working {
                        self.herdr
                            .agent_read(&agent_target(&seed.id), 1)
                            .ok()
                            .and_then(|t| {
                                t.lines()
                                    .rfind(|l| !l.trim().is_empty())
                                    .map(|l| truncate(l.trim(), 80))
                            })
                    } else {
                        None
                    };
                    effects.push(Effect::Update(Box::new(UpdateEffect {
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
                    })));
                }
            }
        }

        // Pass C — apply. One short critical section.
        let mut cleaned: Vec<String> = Vec::new();
        let mut changed = false;
        let mut attr_jobs: Vec<(String, Vec<String>)> = Vec::new();
        let mut pending_warnings: Vec<String> = Vec::new();
        let mut pending_escalations: Vec<(String, String, String)> = Vec::new();
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
                    Effect::Update(effect) => {
                        let UpdateEffect {
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
                        } = *effect;
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
                            pending_escalations.push((id.clone(), "blocked".to_string(), detail));
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
                            // A stuck-killed session is exactly what the escalation
                            // inbox exists for — record it (bridge v1.3 §8).
                            pending_escalations.push((
                                id.clone(),
                                "stuck".to_string(),
                                "stuck: no revision change".to_string(),
                            ));
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
            // Drain after the effect loop so escalate_session can take the lock itself.
            for (id, kind, detail) in pending_escalations {
                drop(inner);
                let _ = self.escalate_session(&id, &kind, &detail);
                inner = lock(&self.inner);
            }
            for id in &cleaned {
                let _ = cleanup::cleanup_session(&self.config.tmp_dir, id);
            }
            if changed || !cleaned.is_empty() {
                self.save_state(&inner);
            }
        }
        self.supervise_once(api_sessions.as_deref());
        self.notify_change();
    }
}
