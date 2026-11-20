//! The escalation inbox. Escalated sessions are excluded from the
//! decision loop forever (the watchdog still applies); every escalation is
//! recorded as an entry, a decision entry, and an event.
//!
//! W2-3: a `needs_you` escalation also issues a SIGNED approval card. The
//! human answers over the authenticated control path; only a valid signature
//! over the displayed card can consume it or create a permission grant.
use super::types::{lock, push_warning, Daemon, MAX_ESCALATIONS};
use crate::approvals::{AnswerMode, ApprovalCard, ApprovalInbox, SignedAnswer};
use crate::decider;
use crate::model::EscalationEntry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Approval state is additive and self-contained (no `model.rs`/`state.json`
/// schema change). Instances are keyed by state dir so test daemons never
/// cross-talk, matching the `JUDGE_POOLS` / `SPAWN_CHANNELS` precedent.
static INBOXES: OnceLock<Mutex<HashMap<PathBuf, Arc<ApprovalInbox>>>> = OnceLock::new();
static GRANTS: OnceLock<Mutex<HashMap<(PathBuf, String), AnswerMode>>> = OnceLock::new();

impl Daemon {
    fn approval_inbox(&self) -> Arc<ApprovalInbox> {
        let map = INBOXES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            map.entry(self.config.state_dir.clone())
                .or_insert_with(|| Arc::new(ApprovalInbox::new(self.clock.clone()))),
        )
    }

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
        drop(inner);

        // A blocked-on-human escalation is the ONLY kind the human can answer.
        // The card digest is what the signed answer must match.
        if kind == "needs_you" {
            if let Err(e) = self.issue_approval(&esc_id, id, kind, reason) {
                push_warning(&mut lock(&self.inner), format!("{esc_id}: approval card failed: {e}"));
            }
        }
        esc_id
    }

    /// Issue an answerable card. Refuses every kind except `needs_you` and any
    /// malformed id/session/reason (no injected instructions in a card).
    pub fn issue_approval(&self, id: &str, session: &str, kind: &str, reason: &str) -> Result<String, String> {
        if kind != "needs_you" {
            return Err("only needs_you escalations are answerable".to_string());
        }
        // Reject injected instructions in the raw reason before log_head can
        // collapse them into a plausible-looking one-line card.
        let trimmed = reason.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > 240
            || trimmed.chars().any(|c| c == '\n' || c == '\r' || c.is_control())
        {
            return Err("invalid approval reason".to_string());
        }
        let card = ApprovalCard {
            id: id.to_string(),
            session: session.to_string(),
            kind: kind.to_string(),
            reason: decider::log_head(reason, 160),
            at: self.now(),
        };
        let digest = self.approval_inbox().issue(card).map_err(String::from)?;
        let _ = self.event_log.append(
            Some(session.to_string()),
            "approval",
            &format!("{id}: needs_you card issued; digest {}...", &digest[..8.min(digest.len())]),
        );
        self.notify_change();
        Ok(digest)
    }

    /// Answer a pending `needs_you` card. The signature is created inside
    /// `ApprovalInbox::answer` from the authenticated caller; the answer is
    /// consumed exactly once, and a permission-mode grant is recorded only
    /// for a valid signed answer. Never types anything into a permission pane.
    pub fn answer_approval(
        &self,
        id: &str,
        allow: bool,
        note: Option<String>,
        at: u64,
        card: &str,
        mode: &str,
    ) -> Result<SignedAnswer, String> {
        let inbox = self.approval_inbox();
        let session = inbox
            .pending_cards()
            .into_iter()
            .find(|c| c.id == id)
            .map(|c| c.session)
            .ok_or_else(|| "no pending approval with that id".to_string())?;
        inbox.answer(id, allow, note, at, card, mode).map_err(String::from)?;
        // Consume exactly once; a replay of these bytes cannot verify again.
        let taken = inbox
            .take_answer(id)
            .ok_or_else(|| "approval answer already consumed".to_string())?;
        let answer_mode = AnswerMode::parse(&taken.mode)
            .ok_or_else(|| "answer mode outside the whitelist".to_string())?;
        if answer_mode.is_permission_grant() {
            let grants = GRANTS.get_or_init(|| Mutex::new(HashMap::new()));
            grants
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((self.config.state_dir.clone(), session.clone()), answer_mode);
        }
        {
            let mut inner = lock(&self.inner);
            let supervisor = inner
                .sessions
                .iter()
                .find(|s| s.id == session)
                .map(|s| s.sup.clone())
                .unwrap_or_else(|| "unknown".to_string());
            self.log_decision(
                &mut inner,
                &supervisor,
                &session,
                "approval",
                &format!("{id}: signed answer allow={allow} mode={}", taken.mode),
            );
            self.save_state(&inner);
        }
        let _ = self.event_log.append(
            Some(session.clone()),
            "approval",
            &format!("{id}: answered allow={allow} mode={}", taken.mode),
        );
        self.notify_change();
        Ok(taken)
    }

    /// Cards still waiting for the human (read-only projection).
    pub fn pending_approvals(&self) -> Vec<ApprovalCard> {
        self.approval_inbox().pending_cards()
    }

    /// Standing permission grant for one session, set only by a signed answer.
    pub fn approval_grant(&self, session: &str) -> Option<AnswerMode> {
        GRANTS
            .get()
            .and_then(|g| g.lock().ok())
            .and_then(|g| g.get(&(self.config.state_dir.clone(), session.to_string())).copied())
    }

    /// Direct signature check, exposed for the control-plane glue/tests. This
    /// never signs: the authority stays inside the inbox.
    pub fn approval_signature_valid(&self, answer: &SignedAnswer) -> bool {
        self.approval_inbox().signature_valid(answer)
    }
}
