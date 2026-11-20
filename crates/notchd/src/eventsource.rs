//! Event-driven agent activity source for `notchd serve`.
//!
//! The 2 s CLI poll is no longer the only way agent activity reaches the
//! daemon. For every pane we actually manage (a session's `paneId` in the
//! existing state), this module opens a herdr `events.subscribe` stream for
//! `pane.agent_status_changed` (protocol v22, one new socket connection per
//! request) and applies the status change immediately. If a subscription
//! cannot be established — or the stream goes quiet/drops — the existing CLI
//! poll (`Daemon::poll_once`, which calls `herdr agent list`) is the fallback.
//!
//! Every firing signal is logged with `kind:"agent_source"` in `events.jsonl`
//! and on stderr (`signal=herdr_event` / `signal=cli_poll_fallback`), so which
//! path drove a change is never a guess. The event path touches only panes
//! recorded in the daemon state: events for foreign panes are observed but
//! never acted on. No OpenCode internals or sub-agents are involved.

use crate::daemon::Daemon;
use crate::herdr::{self, AgentStatusEvent};
use crate::status::Status;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Counters surfaced to operators and tests. Each path increments at the
/// moment it fires; nothing is inferred from sleeps.
#[derive(Default)]
pub struct SourceStats {
    events_seen: AtomicU64,
    managed_events: AtomicU64,
    herdr_events: AtomicU64,
    cli_polls: AtomicU64,
}

impl SourceStats {
    pub fn events_seen(&self) -> u64 {
        self.events_seen.load(Ordering::SeqCst)
    }
    pub fn managed_events(&self) -> u64 {
        self.managed_events.load(Ordering::SeqCst)
    }
    pub fn herdr_events(&self) -> u64 {
        self.herdr_events.load(Ordering::SeqCst)
    }
    pub fn cli_polls(&self) -> u64 {
        self.cli_polls.load(Ordering::SeqCst)
    }
}

/// Distinct panes the daemon currently manages. Derived from the existing
/// snapshot — never a second pane registry.
pub fn managed_panes(daemon: &Daemon) -> Vec<String> {
    let mut panes: Vec<String> = daemon
        .snapshot()
        .sessions
        .into_iter()
        .map(|s| s.pane_id)
        .filter(|p| !p.is_empty())
        .collect();
    panes.sort();
    panes.dedup();
    panes
}

fn log_signal(daemon: &Daemon, signal: &str, detail: &str) {
    let line = format!("signal={signal} {detail}");
    let _ = daemon.event_log.append(None, "agent_source", &line);
    eprintln!("notchd: agent_source {line}");
}

impl Daemon {
    /// Apply one pushed herdr status event to the sessions that own that pane.
    /// Returns true when something actually changed. This is the fast path:
    /// no `herdr agent list`, no 2 s wait. The periodic CLI poll still owns
    /// watchdog and supervisor decisions.
    pub(crate) fn apply_agent_status_event(&self, pane_id: &str, agent_status: &str) -> bool {
        let (mapped, reason) = Status::from_herdr(agent_status);
        let mono = self.mono();
        let mut applied: Vec<(String, &'static str, &'static str)> = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            for s in inner.sessions.iter_mut() {
                if s.pane_id != pane_id || s.status.is_terminal() || s.status == mapped {
                    continue;
                }
                let from = s.status.as_str();
                s.status = mapped;
                s.failure_reason = if mapped == Status::Failed {
                    reason.clone()
                } else {
                    None
                };
                s.revision_changed_at_ms = mono;
                applied.push((s.id.clone(), from, mapped.as_str()));
            }
            if !applied.is_empty() {
                self.save_state(&inner);
            }
        }
        for (id, from, to) in &applied {
            let _ = self
                .event_log
                .append(Some(id.clone()), "status", &format!("herdr event {from} -> {to}"));
            self.watchdog
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .mark_revision_change(id, mono);
        }
        if !applied.is_empty() {
            self.notify_change();
            // No 100 ms debounce wait: subscribers (socket + WS) get the
            // change on this thread, immediately.
            self.flush_subscribers_immediate();
        }
        !applied.is_empty()
    }
}

/// A live event source. Every successful per-pane subscription forwards into
/// one merged channel. When none can be established the channel simply stays
/// empty and the CLI-poll timeout drives the daemon, unchanged.
pub struct EventDrivenSource {
    tx: Sender<AgentStatusEvent>,
    rx: Receiver<AgentStatusEvent>,
    socket: PathBuf,
    subscribed: HashSet<String>,
    shutdown: Arc<AtomicBool>,
    stats: Arc<SourceStats>,
    source_name: &'static str,
}

impl EventDrivenSource {
    /// Connect using herdr's configured private socket.
    pub fn connect(daemon: &Daemon, shutdown: Arc<AtomicBool>) -> Self {
        Self::connect_at(&herdr::herdr_socket_path(), &managed_panes(daemon), shutdown)
    }

    /// Connect to an explicit socket path (operator override / test seam).
    pub fn connect_at(path: &Path, panes: &[String], shutdown: Arc<AtomicBool>) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut source = Self {
            tx,
            rx,
            socket: path.to_path_buf(),
            subscribed: HashSet::new(),
            shutdown,
            stats: Arc::new(SourceStats::default()),
            source_name: "cli_poll",
        };
        source.subscribe(panes);
        source.source_name = if source.subscribed.is_empty() {
            "cli_poll"
        } else {
            "herdr_events"
        };
        source
    }

    /// Pure injection seam: drive the loop from a channel without a socket.
    pub fn with_receiver(rx: Receiver<AgentStatusEvent>, shutdown: Arc<AtomicBool>) -> Self {
        let (tx, _keepalive) = mpsc::channel();
        Self {
            tx,
            rx,
            socket: herdr::herdr_socket_path(),
            subscribed: HashSet::new(),
            shutdown,
            stats: Arc::new(SourceStats::default()),
            source_name: "herdr_events",
        }
    }

    pub fn stats(&self) -> Arc<SourceStats> {
        Arc::clone(&self.stats)
    }

    pub fn source_name(&self) -> &'static str {
        self.source_name
    }

    /// Subscribe to any newly managed panes. A failed subscription never
    /// cancels the others; the CLI poll keeps the daemon alive either way.
    fn subscribe(&mut self, panes: &[String]) {
        for pane in panes {
            if pane.is_empty() || self.subscribed.contains(pane) {
                continue;
            }
            match herdr::subscribe_agent_status_at(&self.socket, pane, &self.shutdown) {
                Ok(pane_rx) => {
                    let tx = self.tx.clone();
                    let pane = pane.clone();
                    let shutdown = Arc::clone(&self.shutdown);
                    std::thread::spawn(move || {
                        while !shutdown.load(Ordering::SeqCst) {
                            match pane_rx.recv() {
                                Ok(event) => {
                                    if tx.send(event).is_err() {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    });
                    self.subscribed.insert(pane);
                }
                Err(e) => {
                    eprintln!("notchd: herdr event subscription for {pane} failed: {e}");
                }
            }
        }
        if !self.subscribed.is_empty() {
            self.source_name = "herdr_events";
        }
    }

    fn refresh(&mut self, daemon: &Daemon) {
        self.subscribe(&managed_panes(daemon));
    }

    fn handle_event(&self, daemon: &Daemon, event: AgentStatusEvent) {
        self.stats.events_seen.fetch_add(1, Ordering::SeqCst);
        if !managed_panes(daemon).contains(&event.pane_id) {
            log_signal(
                daemon,
                "herdr_event_unmanaged",
                &format!("pane={} ignored", event.pane_id),
            );
            return;
        }
        self.stats.managed_events.fetch_add(1, Ordering::SeqCst);
        self.stats.herdr_events.fetch_add(1, Ordering::SeqCst);
        log_signal(
            daemon,
            "herdr_event",
            &format!(
                "pane={} status={} workspace={}",
                event.pane_id, event.agent_status, event.workspace_id
            ),
        );
        daemon.apply_agent_status_event(&event.pane_id, &event.agent_status);
    }

    fn full_poll(&mut self, daemon: &Daemon) {
        self.stats.cli_polls.fetch_add(1, Ordering::SeqCst);
        log_signal(
            daemon,
            "cli_poll_fallback",
            "periodic refresh, watchdog and supervision",
        );
        daemon.poll_once();
        daemon.flush_subscribers_immediate();
        self.refresh(daemon);
    }
}

/// Drive `daemon` from `source` until `shutdown` is set. On a herdr event the
/// status is applied immediately; on timeout the existing CLI poll runs.
pub fn run_source(daemon: Arc<Daemon>, shutdown: Arc<AtomicBool>, mut source: EventDrivenSource) {
    let interval = Duration::from_millis(daemon.config.poll_interval_ms.max(10));
    let mut next_full = Instant::now() + interval;
    let mut stream_alive = true;
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        if stream_alive {
            // Never block for a whole poll interval: shutdown must be
            // observable promptly even when the interval is hours long.
            let wait = next_full
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100));
            match source.rx.recv_timeout(wait) {
                Ok(event) => source.handle_event(&daemon, event),
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= next_full {
                        source.full_poll(&daemon);
                        next_full = Instant::now() + interval;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    log_signal(&daemon, "cli_poll_fallback", "herdr event stream closed");
                    stream_alive = false;
                }
            }
        } else {
            wait_until(&shutdown, next_full);
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            source.full_poll(&daemon);
            next_full = Instant::now() + interval;
        }
    }
}

fn wait_until(shutdown: &AtomicBool, deadline: Instant) {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || shutdown.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

/// Production entry point: choose herdr events when available, CLI poll
/// otherwise, and return the live counters for inspection.
pub fn run(daemon: Arc<Daemon>, shutdown: Arc<AtomicBool>) -> Arc<SourceStats> {
    let source = EventDrivenSource::connect(&daemon, Arc::clone(&shutdown));
    let stats = source.stats();
    log_signal(
        &daemon,
        "source_started",
        &format!("primary={}", source.source_name()),
    );
    run_source(daemon, shutdown, source);
    stats
}
