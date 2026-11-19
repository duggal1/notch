//! Daemon — the long-lived control plane. Single process, no model calls.
//!
//! Concurrency layout:
//! - `inner` — the mutable tree behind one mutex.
//! - herdr/git IO happens OUTSIDE the mutex in `poll_once`: a seed pass
//!   clones what's needed, a side-effect pass does IO, then one apply
//!   critical section commits results. Lock order everywhere is
//!   `inner` → `watchdog` → subscriber/pending locks. Never the reverse.
//! - `EventLog` — append-only `events.jsonl` with rotation + EPS window.
//! - `OrchestratorBus` — cross-supervisor routing; supervisors hold only
//!   `SupervisorSink`s; there is no path from one supervisor to another's
//!   mailbox except through the orchestrator.
//!
//! This facade re-exports the public surface (`Daemon` and the ack types);
//! the implementation lives in focused submodules:
//!
//! - [`lifecycle`] — open, reconcile, poll loop, client seams
//! - [`spawn`] — plans, single spawns, `submit_task`, pre-spawn hook
//! - [`poll`] — poll pass: watchdog application, attribution baselines
//! - [`supervise`] — the supervisor decision loop (v1.3)
//! - [`escalation`] — the escalation inbox
//! - [`bus`] — the message bus commands (`bus_send`/`bus_drain`/`message`)
//! - [`commands`] — kill/reprompt/cleanup/logs/focus/status/v1.3 controls
//! - [`snapshot`] — snapshot production + subscriber flush
//! - [`state`] — durable `state.json` persistence
//! - [`attribution`] — per-session file attribution + conflicts

mod acks;
mod attribution;
mod bus;
mod commands;
mod escalation;
mod lifecycle;
mod poll;
mod snapshot;
mod spawn;
mod state;
mod supervise;
mod types;

pub use acks::{
    KillAck, KillSwitchAck, MessageAck, ModelsAck, OkAck, RunPlanAck, SpawnAck, StatusInfo,
    SubmitTaskAck,
};
pub use types::Daemon;
