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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;

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
    ) -> Result<(), String> {
        if to.is_empty() {
            return Err("empty target supervisor".to_string());
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
            .map_err(|_| "orchestrator inbox closed".to_string())
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
