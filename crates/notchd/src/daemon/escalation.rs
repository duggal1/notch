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
        // One critical section for the whole mutation. Build the persisted
        // snapshot under the lock (state_file takes inner -> watchdog, so lock
        // order is unchanged), then release it before ANY file IO — a hung disk
        // must not wedge every command and snapshot (audit: IO under lock).
        let (esc_id, state) = {
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
            (esc_id, self.state_file(&inner))
        };
        let _ = self.event_log.append(
            Some(id.to_string()),
            "escalate",
            &format!("{kind}: {}", decider::log_head(reason, 140)),
        );
        self.save_state_snapshot(state);

        // A blocked-on-human escalation is the ONLY kind the human can answer.
        // The card digest is what the signed answer must match.
        if kind == "needs_you" {
            if let Err(e) = self.issue_approval(&esc_id, id, kind, reason) {
                push_warning(
                    &mut lock(&self.inner),
                    format!("{esc_id}: approval card failed: {e}"),
                );
            }
        }
        esc_id
    }

    /// Issue an answerable card. Refuses every kind except `needs_you` and any
    /// malformed id/session/reason (no injected instructions in a card).
    pub fn issue_approval(
        &self,
        id: &str,
        session: &str,
        kind: &str,
        reason: &str,
    ) -> Result<String, String> {
        if kind != "needs_you" {
            return Err("only needs_you escalations are answerable".to_string());
        }
        // Reject injected instructions in the raw reason before log_head can
        // collapse them into a plausible-looking one-line card.
        let trimmed = reason.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > 240
            || trimmed
                .chars()
                .any(|c| c == '\n' || c == '\r' || c.is_control())
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
            &format!(
                "{id}: needs_you card issued; digest {}...",
                &digest[..8.min(digest.len())]
            ),
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
        inbox
            .answer(id, allow, note, at, card, mode)
            .map_err(String::from)?;
        // Consume exactly once; a replay of these bytes cannot verify again.
        let taken = inbox
            .take_answer(id)
            .ok_or_else(|| "approval answer already consumed".to_string())?;
        let answer_mode = AnswerMode::parse(&taken.mode)
            .ok_or_else(|| "answer mode outside the whitelist".to_string())?;
        if answer_mode.is_permission_grant() {
            let grants = GRANTS.get_or_init(|| Mutex::new(HashMap::new()));
            grants.lock().unwrap_or_else(|e| e.into_inner()).insert(
                (self.config.state_dir.clone(), session.clone()),
                answer_mode,
            );
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
        GRANTS.get().and_then(|g| g.lock().ok()).and_then(|g| {
            g.get(&(self.config.state_dir.clone(), session.to_string()))
                .copied()
        })
    }

    /// Direct signature check, exposed for the control-plane glue/tests. This
    /// never signs: the authority stays inside the inbox.
    pub fn approval_signature_valid(&self, answer: &SignedAnswer) -> bool {
        self.approval_inbox().signature_valid(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Clock, FakeClock};
    use crate::config::Config;
    use crate::herdr::Herdr;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Injected clock (the documented Clock seam, not a mock of the unit under
    /// test) that blocks on one chosen `now_ms` call — the one made inside
    /// `event_log.append`. This lets the test park the writer at the IO point
    /// with no sleeps, then ask whether `inner` is still held.
    struct GatedClock {
        fake: FakeClock,
        calls: AtomicU64,
        target: u64,
        hit: mpsc::Sender<()>,
        release: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl Clock for GatedClock {
        fn now_ms(&self) -> u64 {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n == self.target {
                let _ = self.hit.send(());
                let rx = self.release.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(rx) = rx.as_ref() {
                    let _ = rx.recv();
                }
            }
            self.fake.now_ms()
        }
        fn mono_ms(&self) -> u64 {
            self.fake.mono_ms()
        }
    }

    fn root(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("notchd-escio-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("tmp")).unwrap();
        p
    }

    fn daemon(r: &Path, clock: Arc<GatedClock>) -> Daemon {
        let herdr = Herdr::with_runner(|args: &[String]| match args.get(1).map(String::as_str) {
            Some("agent") => Ok(r#"{"result":{"agents":[]}}"#.to_string()),
            _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
        });
        Daemon::open(
            Config {
                state_dir: r.join("state"),
                tmp_dir: r.join("tmp"),
                stuck_after_secs: 3600,
                poll_interval_ms: 2000,
                opencode_decider: false,
            },
            herdr,
            clock,
        )
        .unwrap()
    }

    #[test]
    fn escalation_event_io_runs_without_the_inner_lock() {
        let (hit_tx, hit_rx) = mpsc::channel();
        let (rel_tx, rel_rx) = mpsc::channel();
        let clock = Arc::new(GatedClock {
            fake: FakeClock::new(1000),
            calls: AtomicU64::new(0),
            target: 4, // 1: self.now(), 2/3: log_decision, 4: escalation event append
            hit: hit_tx,
            release: Mutex::new(Some(rel_rx)),
        });
        let r = root("lock");
        let d = Arc::new(daemon(&r, clock.clone()));
        clock.calls.store(0, Ordering::SeqCst); // ignore calls made during open

        let (done_tx, done_rx) = mpsc::channel();
        let dw = Arc::clone(&d);
        let writer = std::thread::spawn(move || {
            dw.escalate_session("S1-OC01", "blocked", "test reason");
            let _ = done_tx.send(());
        });
        hit_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("writer must reach the gated event-log IO");

        // While the writer is parked in file IO, a snapshot (which locks
        // `inner`) must still finish. On the unfixed code the lock is held here.
        let (snap_tx, snap_rx) = mpsc::channel();
        let ds = Arc::clone(&d);
        let snapper = std::thread::spawn(move || {
            let _ = ds.snapshot();
            let _ = snap_tx.send(());
        });
        snap_rx
            .recv_timeout(Duration::from_millis(1500))
            .expect("snapshot blocked while escalation IO holds `inner`");

        let _ = rel_tx.send(());
        done_rx.recv_timeout(Duration::from_secs(5)).expect("writer finish");
        writer.join().unwrap();
        snapper.join().unwrap();
        let _ = std::fs::remove_dir_all(&r);
    }
}
