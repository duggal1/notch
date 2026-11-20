//! In-process message bus.
//!
//! Cross-supervisor messages route through the orchestrator ONLY — there is
//! deliberately no path from one supervisor's endpoint to another's mailbox.
//!
//! - A supervisor holds a `SupervisorSink`, whose only method is `send_to`,
//!   which enqueues into the orchestrator inbox. It cannot reach any mailbox.
//! - The orchestrator (`OrchestratorBus::drain`) pops the inbox, assigns the
//!   sequence number, records the event in the log (via the `on_event` hook),
//!   and delivers into the target supervisor's mailbox. Ordering is FIFO.
//! - The orchestrator also owns the route table; supervisors never see it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::clock::Clock;
use crate::error::NotchError;

#[derive(Clone, Debug)]
pub struct BusMessage {
    pub seq: u64,
    pub from: String,
    pub to: String,
    pub session: Option<String>,
    pub kind: String,
    pub detail: String,
    pub ts: u64,
}

/// Send side handed to a supervisor. The only thing it can do is enqueue
/// into the orchestrator's inbox. No method exists to reach another mailbox.
#[derive(Clone)]
pub struct SupervisorSink {
    from: String,
    tx: Sender<BusMessage>,
}

impl SupervisorSink {
    pub fn from(&self) -> &str {
        &self.from
    }

    /// Queue a message for the orchestrator to route. `seq` is assigned by
    /// the orchestrator at drain time.
    pub fn send_to(
        &self,
        to: &str,
        session: Option<String>,
        kind: &str,
        detail: &str,
        ts: u64,
    ) -> Result<(), crate::error::NotchError> {
        if to.is_empty() {
            return Err(crate::error::NotchError::Protocol(
                "empty target supervisor".to_string(),
            ));
        }
        // cw/1 rule: routing is by id only. A wildcard/broadcast TO is never a
        // legal destination — reject at the send edge, not at drain time.
        if is_wildcard_target(to) {
            return Err(crate::error::NotchError::Protocol(format!(
                "wildcard or broadcast target `{to}` is not allowed"
            )));
        }
        self.tx
            .send(BusMessage {
                seq: 0,
                from: self.from.clone(),
                to: to.to_string(),
                session,
                kind: kind.to_string(),
                detail: detail.to_string(),
                ts,
            })
            .map_err(|_| crate::error::NotchError::Protocol("orchestrator inbox closed".to_string()))
    }
}

pub struct OrchestratorBus {
    inbox_tx: Sender<BusMessage>,
    inbox_rx: Mutex<Receiver<BusMessage>>,
    routes: Mutex<HashMap<String, Sender<BusMessage>>>,
    seq: AtomicU64,
}

impl OrchestratorBus {
    pub fn new() -> Self {
        let (inbox_tx, inbox_rx) = mpsc::channel();
        Self {
            inbox_tx,
            inbox_rx: Mutex::new(inbox_rx),
            routes: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
        }
    }

    /// Register a supervisor. Returns its send endpoint and the receiving
    /// half of its mailbox. Called once per supervisor by the daemon.
    pub fn register(&self, supervisor: &str) -> (SupervisorSink, Receiver<BusMessage>) {
        let (tx, rx) = mpsc::channel();
        self.routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(supervisor.to_string(), tx);
        (
            SupervisorSink {
                from: supervisor.to_string(),
                tx: self.inbox_tx.clone(),
            },
            rx,
        )
    }

    /// Clone of the orchestrator inbox sender — the ONLY handle a controlled
    /// worker bridge is allowed to hold. It can enqueue into the inbox but has
    /// no route table and no mailbox receivers, so it cannot deliver directly
    /// to a session or supervisor. Delivery happens only in `drain`.
    pub fn inbox_sender(&self) -> mpsc::Sender<BusMessage> {
        self.inbox_tx.clone()
    }

    /// Is this supervisor id registered? Used to validate bus sends.
    pub fn is_registered(&self, supervisor: &str) -> bool {
        self.routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(supervisor)
    }

    /// Drain the orchestrator inbox. For each message: assign the next
    /// sequence number, invoke `on_event` (event log + EPS are recorded
    /// there), then deliver to the target mailbox. Returns the number of
    /// messages routed. Ordering is FIFO per drain.
    pub fn drain(&self, on_event: &mut dyn FnMut(&BusMessage)) -> usize {
        let mut routed = 0usize;
        loop {
            let msg = {
                let rx = self.inbox_rx.lock().unwrap_or_else(|e| e.into_inner());
                match rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(_) => break,
                }
            };
            let mut msg = match msg {
                Some(m) => m,
                None => break,
            };
            msg.seq = self.seq.fetch_add(1, Ordering::SeqCst);
            on_event(&msg);
            let target = {
                let routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
                routes.get(&msg.to).cloned()
            };
            if let Some(tx) = target {
                // A dead mailbox (supervisor gone) drops the message; the
                // event is already logged, which is what we want.
                let _ = tx.send(msg);
            }
            routed += 1;
        }
        routed
    }
}

impl Default for OrchestratorBus {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// W2-3: controlled worker A2A ingress. RED STUB — implemented in GREEN commit.
// ---------------------------------------------------------------------------

/// A narrow, read-only-plus-enqueue surface for a worker MCP bridge. It holds
/// ONLY the orchestrator inbox sender (no mailbox receiver, no route table it
/// can write), so a worker can ask the orchestrator to route a message but can
/// never deliver to, read, or mutate another session directly.
pub struct A2aBridgeIngress {
    clock: Arc<dyn Clock>,
    tx: mpsc::Sender<BusMessage>,
    /// session id -> owning supervisor id, refreshed by daemon glue.
    routes: Mutex<HashMap<String, String>>,
    /// ids a worker may address. Exact match only; no wildcards.
    known: Mutex<HashSet<String>>,
    policy: A2aRegistry,
    submitted: AtomicU64,
}

impl A2aBridgeIngress {
    pub fn new(clock: Arc<dyn Clock>, tx: mpsc::Sender<BusMessage>) -> Self {
        Self {
            policy: A2aRegistry::new(clock.clone()),
            clock,
            tx,
            routes: Mutex::new(HashMap::new()),
            known: Mutex::new(HashSet::new()),
            submitted: AtomicU64::new(0),
        }
    }

    pub fn set_routes(&self, routes: HashMap<String, String>) {
        *self.routes.lock().unwrap_or_else(|e| e.into_inner()) = routes;
    }

    pub fn set_known(&self, known: HashSet<String>) {
        *self.known.lock().unwrap_or_else(|e| e.into_inner()) = known;
    }

    /// Accept a worker message and enqueue it for the ORCHESTRATOR only. The
    /// FROM/TO session pair travels as metadata; delivery is by `drain` into
    /// the target supervisor's mailbox. No caller can read a mailbox here.
    pub fn submit(
        &self,
        id: &str,
        from: &str,
        to: &str,
        kind: &str,
        body: &str,
    ) -> Result<(), NotchError> {
        if id.is_empty() || id.len() > 128 {
            return Err(NotchError::Protocol("message id must be 1..128 chars".into()));
        }
        if body.trim().is_empty() || body.chars().count() > 4000 {
            return Err(NotchError::Protocol("body must be non-empty and <=4000 chars".into()));
        }
        let known = self.known.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if !known.contains(from) {
            return Err(NotchError::NotFound(format!("unknown sender `{from}`")));
        }
        self.policy.validate(kind, to, &known)?;
        if self.policy.is_duplicate(id) {
            return Err(NotchError::Protocol(format!("duplicate message id `{id}`")));
        }
        let routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let from_sup = routes
            .get(from)
            .ok_or_else(|| NotchError::NotFound(format!("no supervisor route for `{from}`")))?
            .clone();
        let to_sup = routes
            .get(to)
            .ok_or_else(|| NotchError::NotFound(format!("no supervisor route for `{to}`")))?
            .clone();
        drop(routes);

        let msg = BusMessage {
            seq: 0, // assigned by the orchestrator at drain time
            from: from_sup,
            to: to_sup,
            session: Some(to.to_string()),
            kind: kind.to_string(),
            detail: format!("{from}->{to} {body}"),
            ts: self.clock.now_ms(),
        };
        self.tx
            .send(msg)
            .map_err(|_| NotchError::Protocol("orchestrator inbox closed".into()))?;
        self.submitted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn submitted_count(&self) -> u64 {
        self.submitted.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// A2A policy ported from cw/1 (the user's own protocol code). Everything below
// is in-process state: no `.comms/` directory, no bash CLI, no disk writes.
// ---------------------------------------------------------------------------

/// bridge.md v1.3 §8 A — the complete, frozen A2A message-type set. Do NOT add
/// cw/1's `INFO/ALERT/REQUEST/REPLY` here: the snapshot wire contract is fixed.
pub const MESSAGE_TYPES: [&str; 5] = ["task", "result", "question", "handoff", "escalate"];
pub const DEFAULT_LEASE_TTL_SECS: u64 = 900;
pub const MAX_LEASE_TTL_SECS: u64 = 3600;

/// Hard caps: every map in [`A2aRegistry`] is bounded, so a long-lived daemon
/// cannot grow memory without limit (the Rust equivalent of "no goroutine leak").
pub const MAX_LEASES: usize = 4096;
pub const MAX_UNREAD_SESSIONS: usize = 4096;
pub const MAX_SEEN_IDS: usize = 8192;
pub const MAX_BUS_EVENTS: usize = 256;

/// Is `kind` one of the v1.3 A2A types?
pub fn is_valid_message_type(kind: &str) -> bool {
    MESSAGE_TYPES.contains(&kind)
}

/// A target that is not a single, literal id. cw/1 allows `all` for INFO/ALERT
/// and `team:<x>` groups; we reject every wildcard outright because our type
/// set has no broadcast-safe equivalent and routing is id-only.
pub fn is_wildcard_target(to: &str) -> bool {
    let t = to.trim();
    if t.is_empty() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    lower == "all"
        || lower == "broadcast"
        || lower.starts_with("team:")
        || lower.starts_with("cw-")
        || t.contains('*')
        || t.contains(',')
}

/// Validate a recipient against the set of known ids. Rejects wildcards and
/// unknown ids; comparison is exact (no case folding — ids are registry-owned).
pub fn validate_target(to: &str, known: &HashSet<String>) -> Result<(), NotchError> {
    if is_wildcard_target(to) {
        return Err(NotchError::Protocol(format!(
            "wildcard or broadcast target `{to}` is not allowed"
        )));
    }
    if !known.contains(to) {
        return Err(NotchError::NotFound(format!("unknown target `{to}`")));
    }
    Ok(())
}

/// A bus-level event. `class` is `reject` (bad type / wildcard / unknown TO)
/// or `conflict` (two sessions claiming one file). The daemon glue decides
/// which `events.jsonl` kind to append; the bus owns the evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusEvent {
    pub class: String,
    pub detail: String,
}

#[derive(Clone, Debug)]
struct Lease {
    session: String,
    claimed_at_ms: u64,
    expires_at_ms: u64,
}

/// Drop expired leases and enforce the hard lease cap (oldest first). Called
/// with the lease mutex already held.
fn prune_leases(leases: &mut HashMap<String, Vec<Lease>>, now: u64) {
    for holders in leases.values_mut() {
        holders.retain(|l| l.expires_at_ms > now);
    }
    leases.retain(|_, holders| !holders.is_empty());

    if leases.len() > MAX_LEASES {
        let mut all: Vec<(u64, String, String)> = Vec::new();
        for (path, holders) in leases.iter() {
            for lease in holders {
                all.push((lease.claimed_at_ms, path.clone(), lease.session.clone()));
            }
        }
        all.sort_by_key(|entry| entry.0);
        let excess = all.len().saturating_sub(MAX_LEASES);
        for (_, path, session) in all.into_iter().take(excess) {
            if let Some(holders) = leases.get_mut(&path) {
                holders.retain(|l| l.session != session);
            }
        }
        leases.retain(|_, holders| !holders.is_empty());
    }
}

#[derive(Default)]
struct Dedupe {
    order: VecDeque<String>,
    set: HashSet<String>,
}

/// In-process home for the A2A coordination primitives ported from cw/1:
/// TTL file leases, unread mail counters, id dedupe and policy validation.
/// One instance is cheap (four mutexes and bounded maps); construct it with
/// the daemon's clock so tests can drive expiry with [`crate::FakeClock`].
pub struct A2aRegistry {
    clock: Arc<dyn Clock>,
    leases: Mutex<HashMap<String, Vec<Lease>>>,
    unread: Mutex<HashMap<String, usize>>,
    seen: Mutex<Dedupe>,
    events: Mutex<VecDeque<BusEvent>>,
}

impl A2aRegistry {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            leases: Mutex::new(HashMap::new()),
            unread: Mutex::new(HashMap::new()),
            seen: Mutex::new(Dedupe::default()),
            events: Mutex::new(VecDeque::new()),
        }
    }

    fn log(&self, class: &str, detail: impl Into<String>) {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if events.len() >= MAX_BUS_EVENTS {
            events.pop_front();
        }
        events.push_back(BusEvent {
            class: class.to_string(),
            detail: detail.into(),
        });
    }

    pub fn events(&self) -> Vec<BusEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub fn event_count(&self) -> usize {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn clear_events(&self) {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Return and clear the pending events, for daemon glue that appends them
    /// to `events.jsonl` exactly once (drain, never replay).
    pub fn drain_events(&self) -> Vec<BusEvent> {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *events).into_iter().collect()
    }

    // ---- TTL file leases -------------------------------------------------

    /// Claim `path` for `session`. The same session renews; a different live
    /// owner is refused (`false`) and a `conflict` event naming both ids is
    /// logged. TTL is clamped to cw/1's 1..=3600s range.
    pub fn claim(&self, session: &str, path: &str, ttl_secs: u64) -> bool {
        if session.is_empty() || path.is_empty() {
            return false;
        }
        let ttl = ttl_secs.clamp(1, MAX_LEASE_TTL_SECS);
        let now = self.clock.now_ms();
        let expires_at_ms = now.saturating_add(ttl.saturating_mul(1000));

        let mut leases = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        prune_leases(&mut leases, now);
        let holders = leases.entry(path.to_string()).or_default();
        let conflict = holders
            .iter()
            .find(|l| l.session != session)
            .map(|l| l.session.clone());
        if let Some(owner) = conflict {
            drop(leases);
            self.log(
                "conflict",
                format!("claim {path} by {session} conflicts with owner {owner}"),
            );
            return false;
        }
        if let Some(mine) = holders.iter_mut().find(|l| l.session == session) {
            mine.expires_at_ms = expires_at_ms;
        } else {
            holders.push(Lease {
                session: session.to_string(),
                claimed_at_ms: now,
                expires_at_ms,
            });
        }
        true
    }

    /// Release a path held by `session`. Only the owner may release; expired
    /// leases are always releasable. Returns `true` if a lease was removed.
    pub fn release(&self, session: &str, path: &str) -> bool {
        let now = self.clock.now_ms();
        let mut leases = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        prune_leases(&mut leases, now);
        let Some(holders) = leases.get_mut(path) else {
            return false;
        };
        let before = holders.len();
        holders.retain(|l| l.session != session);
        let removed = holders.len() != before;
        if holders.is_empty() {
            leases.remove(path);
        }
        removed
    }

    /// Live owners of `path`, after pruning expired leases. Normally 0 or 1.
    pub fn owners(&self, path: &str) -> Vec<String> {
        let now = self.clock.now_ms();
        let mut leases = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        prune_leases(&mut leases, now);
        leases
            .get(path)
            .map(|holders| holders.iter().map(|l| l.session.clone()).collect())
            .unwrap_or_default()
    }

    /// Number of live leases across all paths (after pruning).
    pub fn lease_count(&self) -> usize {
        let now = self.clock.now_ms();
        let mut leases = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        prune_leases(&mut leases, now);
        leases.values().map(Vec::len).sum()
    }

    // ---- unread mail -----------------------------------------------------

    /// A message has been routed to `session`; increment its unread counter.
    pub fn note_delivery(&self, session: &str) {
        if session.is_empty() {
            return;
        }
        let mut unread = self.unread.lock().unwrap_or_else(|e| e.into_inner());
        if unread.len() >= MAX_UNREAD_SESSIONS && !unread.contains_key(session) {
            if let Some(idle) = unread
                .iter()
                .find(|(_, count)| **count == 0)
                .map(|(id, _)| id.clone())
            {
                unread.remove(&idle);
            } else {
                unread.clear();
            }
        }
        *unread.entry(session.to_string()).or_insert(0) += 1;
    }

    pub fn unread_for(&self, session: &str) -> usize {
        self.unread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session)
            .copied()
            .unwrap_or(0)
    }

    /// Mark a session's pending mail read. Returns the count that was pending.
    pub fn mark_read(&self, session: &str) -> usize {
        self.unread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session)
            .unwrap_or(0)
    }

    /// One-line prompt prefix when mail is pending, `None` at zero. Truthful
    /// and bounded: it reports the exact count, never an estimate.
    pub fn unread_prefix(&self, session: &str) -> Option<String> {
        let count = self.unread_for(session);
        if count == 0 {
            None
        } else {
            Some(format!("you have {count} unread A2A messages"))
        }
    }

    /// Mark read AND return the prefix. Call immediately before delivering a
    /// prompt so the line describes the mail that is about to be delivered.
    pub fn take_unread_prefix(&self, session: &str) -> Option<String> {
        let count = self.mark_read(session);
        if count == 0 {
            None
        } else {
            Some(format!("you have {count} unread A2A messages"))
        }
    }

    /// Prefix `body` with the one-line unread banner (and clear the counter)
    /// exactly when mail was pending. This is the one-line prompt-delivery glue.
    pub fn prefix_body(&self, session: &str, body: &str) -> String {
        match self.take_unread_prefix(session) {
            Some(prefix) => format!("{prefix}\n{body}"),
            None => body.to_string(),
        }
    }

    pub fn unread_session_count(&self) -> usize {
        self.unread.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    // ---- id dedupe -------------------------------------------------------

    /// `true` when `id` was already registered (caller must drop the message).
    /// Bounded ring: the oldest id falls out after [`MAX_SEEN_IDS`] inserts.
    pub fn is_duplicate(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.set.contains(id) {
            return true;
        }
        if seen.order.len() >= MAX_SEEN_IDS {
            if let Some(old) = seen.order.pop_front() {
                seen.set.remove(&old);
            }
        }
        seen.order.push_back(id.to_string());
        seen.set.insert(id.to_string());
        false
    }

    pub fn seen_count(&self) -> usize {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).set.len()
    }

    // ---- validation ------------------------------------------------------

    /// Validate the v1.3 type and recipient of an A2A message. Returns an ack
    /// error and records a `reject` event on any failure.
    pub fn validate(
        &self,
        kind: &str,
        to: &str,
        known: &HashSet<String>,
    ) -> Result<(), NotchError> {
        if !is_valid_message_type(kind) {
            let err = NotchError::Protocol(format!(
                "type must be one of {}",
                MESSAGE_TYPES.join("|")
            ));
            self.log("reject", format!("rejected type={kind} to={to}: {err}"));
            return Err(err);
        }
        if let Err(err) = validate_target(to, known) {
            self.log("reject", format!("rejected type={kind} to={to}: {err}"));
            return Err(err);
        }
        Ok(())
    }
}
