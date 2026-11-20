//! Snapshot production (bridge.md §1.3) and the socket-facing
//! subscriber list (register/unregister/debounced flush).
use super::types::{lock, Daemon, BUS_MESSAGES_CAP, DEBOUNCE_MS};
use crate::model::{AttributionEntry, BusMessageEntry};
use crate::snapshot::{OrchestratorSnapshot, SessionSnapshot, Snapshot, SupervisorSnapshot};
use crate::status::Status;
use std::path::Path;
use std::sync::mpsc::Receiver;

/// Project routed bus traffic from `events.jsonl` into the frozen v1.3
/// `busMessages` shape. `bus_drain` logs `bus FROM->TO: KIND DETAIL`;
/// the `message` command records its own entry directly in the tree. Merging
/// both here means the dashboard comms tab shows real traffic, not just
/// fixture data. Malformed or unrelated lines are skipped.
fn bus_events_from_log(path: &Path) -> Vec<BusMessageEntry> {
    let mut out = Vec::new();
    let Ok(bytes) = crate::events::read_tail(path, crate::events::BUS_LOG_TAIL_BYTES) else {
        return out;
    };
    for line in bytes.split(|b| *b == b'\n') {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("kind").and_then(|k| k.as_str()) != Some("status") {
            continue;
        }
        let Some(detail) = value.get("detail").and_then(|d| d.as_str()) else {
            continue;
        };
        let Some(rest) = detail.strip_prefix("bus ") else {
            continue;
        };
        let Some((route, body)) = rest.split_once(": ") else {
            continue;
        };
        let Some((from, to)) = route.split_once("->") else {
            continue;
        };
        let mut parts = body.splitn(2, ' ');
        let kind = parts.next().unwrap_or("").to_string();
        if !crate::bus::is_valid_message_type(&kind) {
            continue;
        }
        let preview: String = parts.next().unwrap_or("").chars().take(140).collect();
        out.push(BusMessageEntry {
            id: format!(
                "bus_{}_{}",
                value.get("ts").and_then(|t| t.as_u64()).unwrap_or(0),
                out.len()
            ),
            from: from.to_string(),
            to: to.to_string(),
            kind,
            preview,
            at: value.get("ts").and_then(|t| t.as_u64()).unwrap_or(0),
        });
    }
    out
}

impl Daemon {
    /// Bump the snapshot version and mark a change pending; subscribers are
    /// pushed by `flush_subscriber_snapshots` (100 ms debounce).
    pub(crate) fn notify_change(&self) {
        {
            let mut inner = lock(&self.inner);
            inner.snapshot_version = inner.snapshot_version.saturating_add(1);
        }
        *lock(&self.pending_change) = Some(self.now());
    }

    pub fn snapshot_version(&self) -> u64 {
        lock(&self.inner).snapshot_version
    }

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
                        sessions
                            .iter()
                            .filter(|s| s.sup == sup.id)
                            .map(|s| s.status),
                    ),
                    working,
                    idle,
                    failed,
                    done,
                    title: if sup.title.is_empty() {
                        sup.id.clone()
                    } else {
                        sup.title.clone()
                    },
                    paused: inner.paused_supervisors.contains(&sup.id),
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
                goal: s.goal,
                definition_of_done: s.definition_of_done,
                iterations: Some(s.iterations),
                max_iterations: Some(s.max_iterations),
                wall_clock_deadline: s.wall_clock_deadline,
                last_supervisor_action: s.last_supervisor_action,
                model: s.model,
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
            decisions: inner.decisions.iter().take(12).cloned().collect(),
            escalations: inner.escalations.clone(),
            bus_messages: crate::snapshot::cap_bus_messages_newest_first(
                &inner
                    .bus_messages
                    .iter()
                    .cloned()
                    .chain(bus_events_from_log(self.event_log.path()))
                    .collect::<Vec<_>>(),
                BUS_MESSAGES_CAP,
            ),
            warnings: inner.warnings.clone(),
        }
    }

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
