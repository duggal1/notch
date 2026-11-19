//! Serde types mirroring the notch wire contract (docs/bridge.md §1.3).
//!
//! NOTE: the app's data path does NOT deserialize through these — events are
//! emitted as raw JSON straight from notchd's own serializer. These types
//! exist (a) as a readable contract reference and (b) so tests can roundtrip
//! the exact v1 shape (camelCase keys, the four statuses, unknown-field
//! tolerance). The React frontend receives these objects VERBATIM through
//! `app.emit("snapshot", snapshot)`. Keys are camelCase; unknown fields in v1
//! are tolerated on read (serde ignores them) and never emitted by us.
//!
//! `Status` is exactly the four DESIGN.md states; empty slots are `null`
//! (there is no `null` status anywhere in this file).

use serde::{Deserialize, Serialize};

pub const SNAPSHOT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Working,
    #[default]
    Idle,
    Failed,
    Done,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Idle => "idle",
            Status::Failed => "failed",
            Status::Done => "done",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Orchestrator {
    pub id: String,
    pub status: Status,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Supervisor {
    pub id: String,
    pub status: Status,
    pub working: u32,
    pub idle: u32,
    pub failed: u32,
    pub done: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub sup: String,
    pub status: Status,
    pub title: String,
    pub task: String,
    pub cwd: String,
    pub pane_id: String,
    /// ms since epoch
    pub started_at: i64,
    /// Best effort, single line, from pane output. Empty when unknown.
    pub last_action: String,
    /// herdr pane revision (heartbeat). 0 when unknown.
    pub last_revision: u64,
    /// Rolling average step latency. `null` until measured.
    pub latency_ms: Option<f64>,
    /// Present when status == "failed" (e.g. "stuck" | "spawn_failed" | "blocked_waiting").
    pub failure_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub file: String,
    pub sessions: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attribution {
    pub session: String,
    pub file: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub version: u32,
    /// ms since epoch
    pub generated_at: i64,
    pub orchestrator: Orchestrator,
    /// Ordered by id (SV-1, SV-2, … numerically aware, not lexicographic).
    pub supervisors: Vec<Supervisor>,
    /// Ordered by id.
    pub sessions: Vec<Session>,
    /// Bus events/sec, float.
    pub eps: f64,
    pub conflicts: Vec<Conflict>,
    /// Cap 24, newest first.
    pub attribution: Vec<Attribution>,
    pub warnings: Vec<String>,
}

impl Snapshot {
    /// A legal empty tree (bridge.md §1.3): no supervisors, no sessions, no load.
    pub fn empty(now_ms: i64) -> Self {
        Snapshot {
            version: SNAPSHOT_VERSION,
            generated_at: now_ms,
            orchestrator: Orchestrator {
                id: "ORCH".into(),
                status: Status::Idle,
            },
            supervisors: Vec::new(),
            sessions: Vec::new(),
            eps: 0.0,
            conflicts: Vec::new(),
            attribution: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// Sort sessions / supervisors by the numeric part of their ids:
/// `S1-OC2 < S1-OC10 < S2-OC1`, `SV-2 < SV-10`.
pub fn compare_ids(a: &str, b: &str) -> std::cmp::Ordering {
    fn numeric_parts(s: &str) -> Vec<u64> {
        s.split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    }
    let pa = numeric_parts(a);
    let pb = numeric_parts(b);
    match pa.cmp(&pb) {
        std::cmp::Ordering::Equal => a.cmp(b),
        other => other,
    }
}
