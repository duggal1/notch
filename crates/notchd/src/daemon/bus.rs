//! Cross-supervisor bus access and the `message` command (bridge.md
//! v1.3 §8 A). All traffic goes through the orchestrator — there is no
//! session→session path. `message` delivers the body verbatim to the target
//! session as a normal prompt and records a `busMessages` entry.
use super::acks::MessageAck;
use super::types::{agent_target, lock, wire, Daemon, BUS_MESSAGES_CAP};
use crate::bus::BusMessage;
use crate::decider;
use crate::error::NotchError;
use crate::model::BusMessageEntry;

impl Daemon {
    /// Send `detail` from supervisor `from` to supervisor `to` over the
    /// orchestrator bus. Both ids must be registered; the message is
    /// attributed and logged at drain time (FIFO).
    pub fn bus_send(
        &self,
        from: &str,
        to: &str,
        session: Option<String>,
        kind: &str,
        detail: &str,
    ) -> Result<(), String> {
        wire(self.bus_send_inner(from, to, session, kind, detail))
    }

    pub(crate) fn bus_send_inner(
        &self,
        from: &str,
        to: &str,
        session: Option<String>,
        kind: &str,
        detail: &str,
    ) -> Result<(), NotchError> {
        if !self.bus.is_registered(from) {
            return Err(NotchError::NotFound(format!("unknown supervisor `{from}`")));
        }
        if !self.bus.is_registered(to) {
            return Err(NotchError::NotFound(format!("unknown supervisor `{to}`")));
        }
        let sinks = lock(&self.sinks);
        let Some(sink) = sinks.get(from) else {
            return Err(NotchError::State(format!(
                "supervisor `{from}` has no bus endpoint"
            )));
        };
        sink.send_to(to, session, kind, detail, self.now())
    }

    /// Route everything pending in the orchestrator inbox. Returns how many
    /// were delivered. Ordering is FIFO.
    pub fn bus_drain(&self) -> usize {
        let log = &self.event_log;
        self.bus.drain(&mut |msg: &BusMessage| {
            let _ = log.append(
                msg.session.clone(),
                "status",
                &format!("bus {}->{}: {} {}", msg.from, msg.to, msg.kind, msg.detail),
            );
        })
    }

    pub fn mailbox_drain(&self, supervisor: &str) -> Vec<BusMessage> {
        let mailboxes = lock(&self.mailboxes);
        let Some(rx) = mailboxes.get(supervisor) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    /// Bus message (bridge.md v1.3 §8 A). The body is delivered VERBATIM to
    /// the target session as a normal prompt via `herdr agent prompt`.
    /// Cross-supervisor traffic goes through this path — there is no other.
    pub fn message(&self, from: &str, to: &str, kind: &str, body: &str) -> MessageAck {
        fn fail(error: NotchError) -> MessageAck {
            MessageAck {
                ok: false,
                message_id: None,
                error: Some(error.to_string()),
            }
        }
        const KINDS: [&str; 5] = ["task", "result", "question", "handoff", "escalate"];
        if !KINDS.contains(&kind) {
            return fail(NotchError::Protocol(format!(
                "type must be one of {}",
                KINDS.join("|")
            )));
        }
        let body_raw = body;
        if body_raw.trim().is_empty() {
            return fail(NotchError::Protocol("body must not be empty".to_string()));
        }
        if body_raw.chars().count() > 4000 {
            return fail(NotchError::Protocol("body exceeds 4000 chars".to_string()));
        }
        let exists = |id: &str| {
            let inner = lock(&self.inner);
            inner.sessions.iter().any(|s| s.id == id)
        };
        if !exists(from) {
            return fail(NotchError::NotFound(format!("unknown session `{from}`")));
        }
        if !exists(to) {
            return fail(NotchError::NotFound(format!("unknown session `{to}`")));
        }
        // Deliver VERBATIM to the target as a normal prompt.
        if let Err(e) = self.herdr.agent_prompt(&agent_target(to), body_raw) {
            return fail(NotchError::Herdr(format!("delivery failed: {e}")));
        }
        let now = self.now();
        let mut inner = lock(&self.inner);
        inner.msg_seq = inner.msg_seq.saturating_add(1);
        let msg_id = format!("msg_{}", inner.msg_seq);
        let preview: String = body_raw
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(140)
            .collect();
        inner.bus_messages.insert(
            0,
            BusMessageEntry {
                id: msg_id.clone(),
                from: from.to_string(),
                to: to.to_string(),
                kind: kind.to_string(),
                preview,
                at: now,
            },
        );
        if inner.bus_messages.len() > BUS_MESSAGES_CAP {
            inner.bus_messages.truncate(BUS_MESSAGES_CAP);
        }
        let _ = self.event_log.append(
            Some(from.to_string()),
            "message",
            &format!("{from}->{to} {kind}: {}", decider::log_head(body_raw, 120)),
        );
        self.save_state(&inner);
        drop(inner);
        self.notify_change();
        MessageAck {
            ok: true,
            message_id: Some(msg_id),
            error: None,
        }
    }
}
