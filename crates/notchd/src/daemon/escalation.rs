//! The escalation inbox. Escalated sessions are excluded from the
//! decision loop forever (the watchdog still applies); every escalation is
//! recorded as an entry, a decision entry, and an event.
use super::types::{lock, push_warning, Daemon, MAX_ESCALATIONS};
use crate::decider;
use crate::model::EscalationEntry;

impl Daemon {
    /// Record an escalation: mark the session escalated (excluded from the
    /// decision loop forever), record the escalation + a decision entry, log.
    /// Returns the escalation id.
    pub(crate) fn escalate_session(&self, id: &str, kind: &str, reason: &str) -> String {
        let supervisor = {
            let inner = lock(&self.inner);
            inner
                .sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.sup.clone())
                .unwrap_or_else(|| "unknown".to_string())
        };
        let mut inner = lock(&self.inner);
        inner.esc_seq = inner.esc_seq.saturating_add(1);
        let esc_id = format!("esc_{}", inner.esc_seq);
        let now = self.now();
        inner.escalations.push(EscalationEntry {
            id: esc_id.clone(),
            session: id.to_string(),
            supervisor: supervisor.clone(),
            kind: kind.to_string(),
            reason: decider::log_head(reason, 160),
            at: now,
        });
        if inner.escalations.len() > MAX_ESCALATIONS {
            let drop = inner.escalations.len() - MAX_ESCALATIONS;
            inner.escalations.drain(0..drop);
        }
        if let Some(s) = inner.sessions.iter_mut().find(|s| s.id == id) {
            s.escalated = true;
        }
        push_warning(
            &mut inner,
            format!("{id} escalated: {kind}: {}", decider::log_head(reason, 120)),
        );
        self.log_decision(
            &mut inner,
            &supervisor,
            id,
            "escalate",
            &format!("{kind}: {reason}"),
        );
        let _ = self.event_log.append(
            Some(id.to_string()),
            "escalate",
            &format!("{kind}: {}", decider::log_head(reason, 140)),
        );
        self.save_state(&inner);
        esc_id
    }
}
