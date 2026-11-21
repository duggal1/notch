//! Commands: kill, reprompt, cleanup, logs, focus, status, the v1.3
//! supervisor controls (pause/resume), the kill switch, and `list_models`.
//! Errors are returned to the socket/Tauri edge as plain strings
//! (`From<NotchError> for String`); the strings themselves are unchanged.
use super::acks::{KillAck, KillSwitchAck, ModelsAck, OkAck, StatusInfo};
use super::types::{agent_target, lock, push_warning, wire, Daemon, EVENTS_MAX_BYTES};
use crate::cleanup;
use crate::error::NotchError;
use crate::model::truncate;
use crate::status::Status;

impl Daemon {
    pub fn kill(&self, id: &str, reason: Option<String>) -> Result<KillAck, String> {
        wire(self.kill_inner(id, reason))
    }

    pub(crate) fn kill_inner(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<KillAck, NotchError> {
        let reason = reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "killed".to_string());
        let pane = {
            let inner = lock(&self.inner);
            inner
                .sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.pane_id.clone())
        };
        let Some(pane) = pane else {
            return Err(NotchError::NotFound("unknown_session".to_string()));
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
            .agent_prompt(&agent_target(id), text)
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
                match cleanup::sweep(
                    &self.config.tmp_dir,
                    &sessions,
                    &self.config.state_dir.join("events.jsonl"),
                    EVENTS_MAX_BYTES,
                ) {
                    Ok((n, warnings)) => {
                        for warning in &warnings {
                            let _ = self.event_log.append(
                                None,
                                "status",
                                &format!("sweep skipped `{warning}`"),
                            );
                        }
                        let mut inner = lock(&self.inner);
                        for warning in warnings {
                            push_warning(&mut inner, warning);
                        }
                        n
                    }
                    Err(_) => 0,
                }
            }
        };
        let _ = self.event_log.append(
            id.map(str::to_string),
            "cleanup",
            &format!("removed={removed}"),
        );
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
            .agent_read(&agent_target(id), tail)
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
            .agent_focus(&agent_target(id))
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
            inner
                .sessions
                .iter()
                .filter(|s| !s.status.is_terminal())
                .count() as u64
        };
        StatusInfo {
            event: "status",
            pid: std::process::id(),
            sessions,
            uptime_s: self.now().saturating_sub(self.start_ms) / 1000,
        }
    }

    /// Pause a supervisor: its sessions stop receiving decisions; they keep
    /// running untouched. Watchdog still applies.
    pub fn pause_supervisor(&self, id: &str) -> Result<OkAck, String> {
        let exists = {
            let inner = lock(&self.inner);
            inner.supervisors.iter().any(|s| s.id == id)
        };
        if !exists {
            return Err(format!("unknown supervisor `{id}`"));
        }
        {
            let mut inner = lock(&self.inner);
            inner.paused_supervisors.insert(id.to_string());
            self.save_state(&inner);
        }
        let _ = self
            .event_log
            .append(None, "status", &format!("pause: supervisor {id} paused"));
        self.notify_change();
        Ok(OkAck {
            ok: true,
            id: Some(id.to_string()),
            removed: None,
            error: None,
        })
    }

    pub fn resume_supervisor(&self, id: &str) -> Result<OkAck, String> {
        let exists = {
            let inner = lock(&self.inner);
            inner.supervisors.iter().any(|s| s.id == id)
        };
        if !exists {
            return Err(format!("unknown supervisor `{id}`"));
        }
        {
            let mut inner = lock(&self.inner);
            inner.paused_supervisors.remove(id);
            self.save_state(&inner);
        }
        let _ = self
            .event_log
            .append(None, "status", &format!("resume: supervisor {id} resumed"));
        self.notify_change();
        Ok(OkAck {
            ok: true,
            id: Some(id.to_string()),
            removed: None,
            error: None,
        })
    }

    /// Close every managed pane, mark the sessions failed with reason
    /// `kill_switch`, and run cleanup on each. Returns how many panes were
    /// closed (managed = tracked sessions with a pane, excluding `done`).
    /// Close every managed pane, mark touched sessions `failed` with reason
    /// `kill_switch`, and remove their temp dirs. `done` sessions stay `done`
    /// (they already recorded success) but their panes are closed anyway — a
    /// completed agent can leave an idle TUI behind. Sessions whose pane is
    /// already gone: log `pane_already_closed`, continue — one failure never
    /// aborts the batch. `closed` counts panes actually closed. Sessions with
    /// no recorded paneId are skipped.
    pub fn kill_switch(&self) -> KillSwitchAck {
        let targets: Vec<(String, String)> = {
            let inner = lock(&self.inner);
            inner
                .sessions
                .iter()
                .filter(|s| !s.pane_id.is_empty())
                .map(|s| (s.id.clone(), s.pane_id.clone()))
                .collect()
        };
        let mut closed = 0u64;
        for (_id, pane) in &targets {
            match self.herdr.pane_close(pane) {
                Ok(()) => closed += 1,
                Err(_) => {
                    let _ =
                        self.event_log
                            .append(None, "kill", &format!("pane_already_closed {pane}"));
                }
            }
        }
        let mut cleaned: Vec<String> = Vec::new();
        {
            let mut inner = lock(&self.inner);
            for s in inner.sessions.iter_mut() {
                if s.status == Status::Done {
                    // Done sessions stay done; their panes were closed above.
                    continue;
                }
                s.status = Status::Failed;
                s.failure_reason = Some("kill_switch".to_string());
                cleaned.push(s.id.clone());
            }
            let _ =
                self.event_log
                    .append(None, "kill", &format!("kill_switch: closed {closed} panes"));
            self.save_state(&inner);
        }
        for (id, _pane) in &targets {
            let _ = cleanup::cleanup_session(&self.config.tmp_dir, id);
        }
        self.notify_change();
        KillSwitchAck { ok: true, closed }
    }

    /// `opencode models`, cached 5 minutes. On failure: empty list + error.
    pub fn list_models(&self) -> ModelsAck {
        const CACHE_MS: u64 = 5 * 60 * 1000;
        let now = self.now();
        {
            let inner = lock(&self.inner);
            if let Some((at, models)) = &inner.models_cache {
                if now.saturating_sub(*at) < CACHE_MS {
                    return ModelsAck {
                        ok: true,
                        models: models.clone(),
                        error: None,
                    };
                }
            }
        }
        match self.opencode_api_models() {
            Ok(models) => {
                let mut inner = lock(&self.inner);
                inner.models_cache = Some((now, models.clone()));
                drop(inner);
                ModelsAck {
                    ok: true,
                    models,
                    error: None,
                }
            }
            Err(e) => ModelsAck {
                ok: false,
                models: Vec::new(),
                error: Some(format!("opencode models failed: {e}")),
            },
        }
    }
}
