//! notchd — orchestration control plane for OpenCode sessions (via herdr).
//!
//! Library crate. The Tauri shell (`app/src-tauri`) embeds this library;
//! `src/main.rs` is the thin CLI over the same API. Nothing in the public
//! API assumes the NDJSON socket is the only consumer.
//!
//! Design laws that touch data (DESIGN.md §1): glow is a function of state;
//! status vocabulary is exactly `working|idle|failed|done`; IDs are
//! `S<sup>-OC<nn>` / `SV-<n>` / `ORCH`, allocated only by the registry.

pub mod attribution;
pub mod bus;
pub mod decider;
mod durable;
pub mod error;
pub mod opencode;
pub mod cleanup;
pub mod cli;
pub mod clock;
pub mod config;
pub mod daemon;
pub mod events;
pub mod herdr;
pub mod model;
pub mod paths;
pub mod plan;
pub mod proc;
pub mod registry;
pub mod snapshot;
pub mod socket;
pub mod status;
pub mod supervisor;
pub mod watchdog;
pub mod worktree;
pub mod ws;

pub use clock::{Clock, FakeClock, SystemClock};
pub use config::Config;
pub use daemon::{Daemon, KillAck, KillSwitchAck, MessageAck, ModelsAck, OkAck, RunPlanAck, SpawnAck, StatusInfo, SubmitTaskAck};
pub use error::NotchError;
pub use opencode::{ApiSession, Opencode};
pub use herdr::{AgentEntry, Herdr, HerdrError};
pub use snapshot::Snapshot;
pub use status::Status;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
