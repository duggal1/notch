//! The poll pass: seed (clone) → IO (herdr/git, no locks) → apply (one
//! short critical section). This is where watchdog decisions are applied
//! (suspect nudge, stuck escalation, opt-in pane close) and where blocked
//! sessions are handed to a human instead of being re-prompted.
//!
//! All liveness timing uses the monotonic clock. A scheduler/VM stall longer
//! than three poll intervals grants grace: revision timestamps are reset and
//! no kill/nudge/escalation can fire in that pass.
use super::attribution::record_attribution_locked;
use super::types::{agent_target, lock, push_warning, Daemon};
use crate::attribution;
use crate::cleanup;
use crate::model::truncate;
use crate::status::Status;
use crate::supervisor::{decide_on_failure_with_reason, reprompt_text, FailureAction};
use crate::watchdog::{Liveness, Obs, Watchdog};
use std::collections::HashSet;
use std::path::Path;

const SUSPECT_NUDGE: &str =
    "Quick check-in: are you still making progress? Continue from the latest state.";

impl Daemon {
    /// Monotonic instant for every liveness decision.
    pub(crate) fn mono(&self) -> u64 {
        self.clock.mono_ms()
    }

    /// Opt-in pane close on a Stuck session. Production default is ON; callers
    /// that disable it accept that verified-stuck panes stay open.
    pub fn set_kill_on_stuck(&self, enabled: bool) {
        lock(&self.watchdog).set_kill_on_stuck(enabled);
    }

    /// Flush the coalesced 1 Hz state writer synchronously. Useful at shutdown
    /// and for tests that must observe a just-committed field on disk.
    pub fn flush_state(&self) {
        self.shutdown_save();
    }

    /// One watchdog + supervision pass. `run_poll_loop` drives it every
    /// `poll_interval_ms`; tests drive it manually with a fake clock.
    pub fn poll_once(&self) {
        let mono = self.mono();
        let gap = lock(&self.watchdog).note_poll(mono);
        let stall_gap = gap.filter(|g| Watchdog::is_stall_gap(*g, self.config.poll_interval_ms));
        let stalled = match stall_gap {
            Some(g) => {
                let granted = lock(&self.watchdog).grant_stall_grace();
                if !granted {
                    let detail = format!(
                        "watchdog stall: {g}ms gap; grace streak exhausted, classifying normally"
                    );
                    let mut inner = lock(&self.inner);
                    push_warning(&mut inner, detail.clone());
                    let _ = self.event_log.append(None, "watchdog", &detail);
                    drop(inner);
                }
                granted
            }
            None => {
                lock(&self.watchdog).clear_stall_grace_streak();
                false
            }
        };
        if stalled {
            // The daemon (not the session) went quiet: a long GC pause, a VM
            // pause, a suspended process. Reset every grace period and skip
            // all liveness actions for this pass.
            let mut inner = lock(&self.inner);
            {
                // Reset the grace window AND re-arm per-episode actions: a
                // stall ends the previous silence episode, so the next
                // genuine stuck episode must escalate again.
                let mut wd = lock(&self.watchdog);
                for s in inner.sessions.iter_mut() {
                    if !s.status.is_terminal() {
                        s.revision_changed_at_ms = mono;
                        wd.rearm_episode(&s.id);
                    }
                }
            }
            let detail = format!(
                "watchdog stall: poll gap {}ms > {}ms; grace granted",
                gap.unwrap_or(0),
                self.config.poll_interval_ms.saturating_mul(3)
            );
            push_warning(&mut inner, detail.clone());
            let _ = self.event_log.append(None, "watchdog", &detail);
            self.save_state(&inner);
            drop(inner);
            self.notify_change();
        }

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
        // session_ref/model reconciliation, the watchdog's busy second
        // opinion, and the decider loop. Failure is tolerated.
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
            cwd: String,
            task_head: String,
            failure_count: u32,
            last_revision: Option<u64>,
            revision_changed_at_ms: u64,
            session_ref: Option<String>,
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
                    cwd: s.cwd.clone(),
                    task_head: s.task.clone(),
                    failure_count: s.failure_count,
                    last_revision: s.last_revision,
                    revision_changed_at_ms: s.revision_changed_at_ms,
                    session_ref: s.session_ref.clone(),
                    baseline: s.attribution_baseline.clone(),
                })
                .collect()
        };

        // Pass B — herdr/git IO. No locks held.
        struct UpdateEffect {
            idx: usize,
            id: String,
            pane_id: String,
            cwd: String,
            revision: Option<u64>,
            revision_changed: bool,
            status: Status,
            last_action: Option<String>,
            files: Vec<String>,
            reprompt_text: Option<String>,
            escalate: Option<(String, String)>,
            needs_human: Option<String>,
            mark_done: bool,
            liveness: Liveness,
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
            // OpenCode's own busy marker is the second opinion on a frozen
            // herdr revision. Some(false) is deliberately NOT evidence.
            let oc_busy = seed
                .session_ref
                .as_ref()
                .and_then(|r| api_sessions.as_ref()?.iter().find(|s| &s.id == r))
                .map(|s| !crate::opencode::is_idle(s));
            let quiet_for_ms = if seed.revision_changed_at_ms == 0 {
                // Never persisted: a fresh process has no baseline. Grant a
                // full grace window instead of treating 0 as ancient history.
                0
            } else {
                mono.saturating_sub(seed.revision_changed_at_ms)
            };
            let liveness = if stalled {
                Liveness::Alive
            } else {
                lock(&self.watchdog).classify(Obs {
                    status: mapped,
                    revision_changed_now: revision_changed || seed.revision_changed_at_ms == 0,
                    oc_busy,
                    quiet_for_ms,
                })
            };
            match mapped {
                Status::Failed => {
                    let tail = self.herdr.agent_read(&agent_target(&seed.id), 20).ok();
                    match decide_on_failure_with_reason(reason.as_deref(), seed.failure_count, tail) {
                        FailureAction::Reprompt { tail } => {
                            let text = reprompt_text(&seed.task_head, &tail);
                            match self.herdr.agent_prompt(&agent_target(&seed.id), &text) {
                                Ok(()) => effects.push(Effect::Update(Box::new(UpdateEffect {
                                    idx: seed.idx,
                                    id: seed.id,
                                    pane_id: seed.pane_id,
                                    cwd: seed.cwd,
                                    revision: Some(entry.revision),
                                    revision_changed: true,
                                    status: Status::Working,
                                    last_action: None,
                                    files: Vec::new(),
                                    reprompt_text: Some(text),
                                    escalate: None,
                                    needs_human: None,
                                    mark_done: false,
                                    liveness: Liveness::Alive,
                                    baseline: None,
                                }))),
                                Err(e) => effects.push(Effect::Update(Box::new(UpdateEffect {
                                    idx: seed.idx,
                                    id: seed.id,
                                    pane_id: seed.pane_id,
                                    cwd: seed.cwd,
                                    revision: Some(entry.revision),
                                    revision_changed,
                                    status: Status::Failed,
                                    last_action: None,
                                    files: Vec::new(),
                                    reprompt_text: None,
                                    escalate: Some((
                                        reason.clone().unwrap_or_else(|| "blocked_waiting".into()),
                                        format!("re-prompt failed: {e}"),
                                    )),
                                    needs_human: None,
                                    mark_done: false,
                                    liveness: Liveness::Alive,
                                    baseline: None,
                                }))),
                            }
                        }
                        FailureAction::Escalate => {
                            effects.push(Effect::Update(Box::new(UpdateEffect {
                                idx: seed.idx,
                                id: seed.id,
                                pane_id: seed.pane_id,
                                cwd: seed.cwd,
                                revision: Some(entry.revision),
                                revision_changed,
                                status: Status::Failed,
                                last_action: None,
                                files: Vec::new(),
                                reprompt_text: None,
                                escalate: Some((
                                    reason.clone().unwrap_or_else(|| "blocked_waiting".into()),
                                    reason.unwrap_or_else(|| "blocked_waiting".into()),
                                )),
                                needs_human: None,
                                mark_done: false,
                                liveness: Liveness::Alive,
                                baseline: None,
                            })))
                        }
                        FailureAction::NeedsHuman { reason: why } => {
                            // A permission prompt is not a failed session and
                            // must never be answered with typed instructions.
                            effects.push(Effect::Update(Box::new(UpdateEffect {
                                idx: seed.idx,
                                id: seed.id,
                                pane_id: seed.pane_id,
                                cwd: seed.cwd,
                                revision: Some(entry.revision),
                                revision_changed,
                                status: Status::Idle,
                                last_action: None,
                                files: Vec::new(),
                                reprompt_text: None,
                                escalate: None,
                                needs_human: Some(why),
                                mark_done: false,
                                liveness: Liveness::Alive,
                                baseline: None,
                            })))
                        }
                    }
                }
                Status::Done => {
                    effects.push(Effect::Update(Box::new(UpdateEffect {
                        idx: seed.idx,
                        id: seed.id,
                        pane_id: seed.pane_id,
                        cwd: seed.cwd,
                        revision: Some(entry.revision),
                        revision_changed,
                        status: Status::Done,
                        last_action: None,
                        files: Vec::new(),
                        reprompt_text: None,
                        escalate: None,
                        needs_human: None,
                        mark_done: true,
                        liveness: Liveness::Alive,
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
                        pane_id: seed.pane_id,
                        cwd: seed.cwd,
                        revision: Some(entry.revision),
                        revision_changed,
                        status: mapped,
                        last_action,
                        files,
                        reprompt_text: None,
                        escalate: None,
                        needs_human: None,
                        mark_done: false,
                        liveness,
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
        let mut pending_nudges: Vec<(String, String)> = Vec::new();
        // Stuck kills are deferred until AFTER the escalation drain: a
        // verified verdict must be on the record before any pane is closed.
        let mut pending_kills: Vec<(String, String, String)> = Vec::new();
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
                            pane_id,
                            cwd,
                            revision,
                            revision_changed,
                            status,
                            last_action,
                            files,
                            reprompt_text,
                            escalate,
                            needs_human,
                            mark_done,
                            liveness,
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
                                s.revision_changed_at_ms = mono;
                                lock(&self.watchdog).mark_revision_change(&id, mono);
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
                            s.revision_changed_at_ms = mono;
                            let detail = truncate(text.lines().next().unwrap_or(""), 80);
                            let _ = self.event_log.append(Some(id.clone()), "reprompt", &detail);
                        } else if let Some(why) = needs_human {
                            // No prompt, no pane close, no temp cleanup. The
                            // human owns the permission decision now.
                            s.status = Status::Idle;
                            s.failure_reason = Some(why.clone());
                            pending_escalations.push((
                                id.clone(),
                                "needs_you".to_string(),
                                format!("blocked_waiting: {why}"),
                            ));
                            let _ = self.event_log.append(
                                Some(id.clone()),
                                "needs_you",
                                "blocked_waiting; pane and temp dir kept",
                            );
                        } else if let Some((kind, detail)) = escalate {
                            s.failure_count = s.failure_count.saturating_add(1);
                            s.status = Status::Failed;
                            s.failure_reason = Some(kind.clone());
                            pending_escalations.push((id.clone(), kind, detail));
                            cleaned.push(id.clone());
                        } else if mark_done {
                            s.status = Status::Done;
                            s.failure_reason = None;
                            let _ = self.event_log.append(Some(id.clone()), "status", "done");
                            cleaned.push(id.clone());
                        } else {
                            match liveness {
                                Liveness::Alive => {
                                    if s.status != status {
                                        let from = s.status.as_str();
                                        s.status = status;
                                        let _ = self.event_log.append(
                                            Some(id.clone()),
                                            "status",
                                            &format!("{from} -> {}", status.as_str()),
                                        );
                                    }
                                }
                                Liveness::Suspect => {
                                    let quiet = mono.saturating_sub(s.revision_changed_at_ms);
                                    if lock(&self.watchdog).take_suspect_action(&id, mono) {
                                        pending_nudges.push((id.clone(), SUSPECT_NUDGE.to_string()));
                                        pending_escalations.push((
                                            id.clone(),
                                            "stuck_suspect".to_string(),
                                            format!("no revision change for {quiet}ms; nudged"),
                                        ));
                                        let _ = self.event_log.append(
                                            Some(id.clone()),
                                            "watchdog",
                                            &format!("suspect: quiet {quiet}ms; nudge + escalation"),
                                        );
                                    }
                                }
                                Liveness::Stuck => {
                                    let quiet = mono.saturating_sub(s.revision_changed_at_ms);
                                    if lock(&self.watchdog).take_stuck_action(&id, mono) {
                                        pending_escalations.push((
                                            id.clone(),
                                            "stuck".to_string(),
                                            format!("no revision change for {quiet}ms"),
                                        ));
                                        let _ = self.event_log.append(
                                            Some(id.clone()),
                                            "watchdog",
                                            &format!("stuck: quiet {quiet}ms"),
                                        );
                                    }
                                    if lock(&self.watchdog).should_close_pane(Liveness::Stuck) {
                                        pending_kills.push((id.clone(), pane_id.clone(), cwd.clone()));
                                    } else {
                                        pending_warnings.push(format!(
                                            "{id} stuck; kill_on_stuck is OFF — pane kept, escalation only"
                                        ));
                                    }
                                }
                            }
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
            // IO after the effect loop so escalate_session can take the lock.
            for (id, text) in pending_nudges {
                drop(inner);
                let _ = self.herdr.agent_prompt(&agent_target(&id), &text);
                inner = lock(&self.inner);
            }
            for (id, kind, detail) in pending_escalations {
                drop(inner);
                let _ = self.escalate_session(&id, &kind, &detail);
                inner = lock(&self.inner);
            }
            // Verdict recorded first; now snapshot WIP and close. A failed
            // snapshot vetoes the close so uncommitted work is never lost.
            for (id, pane_id, cwd) in pending_kills {
                match crate::supervisor::snapshot_worktree(Path::new(&cwd), &id, "stuck") {
                    Ok(sha) => {
                        push_warning(&mut inner, format!("{id} stuck; WIP snapshot {sha:?}"));
                        match self.herdr.pane_close(&pane_id) {
                            Ok(()) => {
                                push_warning(
                                    &mut inner,
                                    format!("{id} stuck (no revision change); pane {pane_id} closed"),
                                );
                                if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == id) {
                                    s.status = Status::Failed;
                                    s.failure_reason = Some("stuck".to_string());
                                }
                                cleaned.push(id);
                            }
                            Err(e) => push_warning(
                                &mut inner,
                                format!("{id} stuck (no revision change); pane close failed: {e}"),
                            ),
                        }
                    }
                    Err(e) => push_warning(
                        &mut inner,
                        format!("{id} stuck; WIP snapshot failed ({e}); pane close skipped"),
                    ),
                }
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
