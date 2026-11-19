//! Daemon lifecycle: open, reconcile, poll loop, and the injected
//! client seams (`set_opencode`).
use super::state::load_state;
use super::types::{lock, push_warning, Daemon, Inner, Subscribers, EVENTS_MAX_BYTES};
use crate::bus::OrchestratorBus;
use crate::cleanup;
use crate::clock::Clock;
use crate::config::Config;
use crate::events::EventLog;
use crate::herdr::Herdr;
use crate::model::SupervisorMeta;
use crate::status::Status;
use crate::watchdog::Watchdog;
use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
        let paused_supervisors: HashSet<String> = state.paused_supervisors.into_iter().collect();
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
            paused_supervisors,
            decisions: state.decisions.into_iter().collect(),
            escalations: state.escalations.into_iter().collect(),
            bus_messages: state.bus_messages.into_iter().collect(),
            esc_seq: state.esc_seq,
            msg_seq: state.msg_seq,
            models_cache: None,
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
            opencode: Mutex::new(Some(crate::opencode::Opencode::from_env())),
        })
    }

    pub(crate) fn now(&self) -> u64 {
        self.clock.now_ms()
    }

    /// One /api/session fetch via the configured client. Tests inject a
    /// fake via `set_opencode`; the default is `Opencode::from_env()`.
    pub(crate) fn opencode_api_sessions(&self) -> Option<Vec<crate::opencode::ApiSession>> {
        let guard = lock(&self.opencode);
        guard.as_ref().and_then(|oc| oc.api_session_list().ok())
    }

    /// Replace the opencode client (test seam).
    pub fn set_opencode(&self, client: crate::opencode::Opencode) {
        *lock(&self.opencode) = Some(client);
    }

    pub(crate) fn register_supervisor_locked(&self, inner: &mut Inner, sup: &SupervisorMeta) {
        let (sink, rx) = self.bus.register(&sup.id);
        lock(&self.sinks).insert(sup.id.clone(), sink);
        lock(&self.mailboxes).insert(sup.id.clone(), rx);
        inner.supervisors.push(sup.clone());
    }

    /// Re-adopt panes that still reference known session IDs. Sessions
    /// whose panes vanished are marked `done` (cleanup runs). On herdr
    /// failure: warning, sessions unchanged, never a crash.
    pub fn reconcile(&self) {
        let agents = match self.herdr.agent_list() {
            Ok(a) => a,
            Err(e) => {
                let mut inner = lock(&self.inner);
                push_warning(
                    &mut inner,
                    format!("herdr unavailable on start; re-adoption skipped: {e}"),
                );
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

    pub(crate) fn opencode_api_models(&self) -> Result<Vec<String>, String> {
        let guard = lock(&self.opencode);
        match guard.as_ref() {
            Some(oc) => oc.models().map_err(|e| e.to_string()),
            None => Err("opencode client not configured".to_string()),
        }
    }
}
