//! State model. `SessionState`/`SupervisorMeta` persist to `state.json`
//! (hot fields the daemon needs). `AttributionEntry`/`ConflictEntry` persist
//! inside the same file and are also the wire shape for the snapshot.

use crate::status::Status;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub id: String,
    pub sup: String,
    pub status: Status,
    pub title: String,
    pub task: String,
    pub cwd: String,
    #[serde(rename = "paneId", default)]
    pub pane_id: String,
    #[serde(rename = "startedAt", default)]
    pub started_at: u64,
    #[serde(rename = "lastAction", default)]
    pub last_action: Option<String>,
    #[serde(rename = "lastRevision", default)]
    pub last_revision: Option<u64>,
    #[serde(rename = "latencyMs", default)]
    pub latency_ms: Option<u64>,
    #[serde(rename = "failureReason", default)]
    pub failure_reason: Option<String>,

    // Runtime bookkeeping. Not persisted to state.json.
    #[serde(skip)]
    pub failure_count: u32,
    #[serde(skip)]
    pub revision_changed_at_ms: u64,
    /// Files already dirty when this session was first sampled. A file in
    /// this set cannot be attributed to this session (in a shared repo the
    /// dirty file may belong to another agent). Runtime-only; after restart,
    /// the first sample becomes the new baseline.
    #[serde(skip)]
    pub attribution_baseline: Option<std::collections::HashSet<String>>,

    // v1.3 session control record (persisted).
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(rename = "definitionOfDone", default)]
    pub definition_of_done: Option<String>,
    #[serde(default)]
    pub iterations: u32,
    #[serde(rename = "maxIterations", default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(rename = "wallClockDeadline", default)]
    pub wall_clock_deadline: Option<u64>,
    #[serde(rename = "lastSupervisorAction", default)]
    pub last_supervisor_action: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(rename = "sessionRef", default)]
    pub session_ref: Option<String>,
    /// Absolute path of this session's temp dir, recorded at creation.
    /// `None` for state files predating the sweep-safety fix — such sessions
    /// are treated as untracked (their dirs survive unless empty).
    #[serde(rename = "tempDir", default)]
    pub temp_dir: Option<String>,
    // Isolation fields (HARDENING §2, A5). Additive and back-compatible: old
    // state files omit all three; deserialization yields None and the daemon
    // never treats a missing worktree as an error. Schema owner: A4.
    /// Absolute path of this session's private git worktree.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Branch backing the worktree (`notch/<session id>`).
    #[serde(default)]
    pub branch: Option<String>,
    /// Revision the worktree was forked from; attribution is "commits since".
    #[serde(default)]
    pub base_rev: Option<String>,
    /// Set when an escalation is recorded for this session; it is then
    /// excluded from the decision loop (watchdog still applies).
    #[serde(default)]
    pub escalated: bool,

    // v1.3 runtime-only decision-loop bookkeeping (not persisted).
    #[serde(skip)]
    pub backoff_until_ms: u64,
    #[serde(skip)]
    pub consecutive_decider_failures: u32,
    #[serde(skip)]
    pub stuck_strikes: u32,
    #[serde(skip)]
    pub strategy_changed: bool,
    #[serde(skip)]
    pub last_tails: std::collections::VecDeque<String>,
}

pub fn default_max_iterations() -> u32 {
    8
}

impl SessionState {
    pub fn task_from_prompt(prompt: &str) -> String {
        single_line(prompt)
    }
}

/// First line, collapsed whitespace, truncated to 78 chars.
pub fn single_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&line, 78)
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Single line (all whitespace collapsed), truncated to `max` chars on a word
/// boundary. Never cuts mid-word. A single word longer than `max` is hard-cut
/// at `max` (the documented fallback). No ellipsis is added.
pub fn truncate_words(s: &str, max: usize) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut result = String::new();
    for word in line.split_whitespace() {
        let candidate = if result.is_empty() {
            word.to_string()
        } else {
            format!("{result} {word}")
        };
        if candidate.chars().count() > max {
            break;
        }
        result = candidate;
    }
    if result.is_empty() && !line.is_empty() {
        result = line.chars().take(max).collect();
    }
    result
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SupervisorMeta {
    pub id: String,
    pub number: u32,
    /// Display title for the supervisor row. Empty string means "fall back to
    /// the id" (legacy state files predating this field).
    #[serde(default)]
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttributionEntry {
    pub session: String,
    pub file: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConflictEntry {
    pub file: String,
    pub sessions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionEntry {
    pub at: u64,
    pub supervisor: String,
    pub session: String,
    /// One of: reprompt | done | escalate | verify | assign | backoff
    pub action: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EscalationEntry {
    pub id: String,
    pub session: String,
    pub supervisor: String,
    /// One of (bridge v1.3 §1.3): stuck | loop_detected | budget_exceeded | blocked
    pub kind: String,
    pub reason: String,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BusMessageEntry {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// First 140 chars of the body, single line.
    pub preview: String,
    pub at: u64,
}

/// Additive A2A message metadata (cw/1 port). This is a NEW envelope: the
/// frozen v1.3 `snapshot.busMessages` shape ([`BusMessageEntry`]) is untouched.
/// `claimedPath`/`deliveredAt` are optional and omitted when unset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct A2aMessage {
    pub id: String,
    pub at: u64,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub body: String,
    #[serde(rename = "claimedPath", default, skip_serializing_if = "Option::is_none")]
    pub claimed_path: Option<String>,
    #[serde(rename = "deliveredAt", default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<u64>,
}

impl A2aMessage {
    pub fn new(
        id: impl Into<String>,
        at: u64,
        from: impl Into<String>,
        to: impl Into<String>,
        kind: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            at,
            from: from.into(),
            to: to.into(),
            kind: kind.into(),
            body: body.into(),
            claimed_path: None,
            delivered_at: None,
        }
    }

    /// Project into the frozen v1.3 snapshot shape (preview = first line,
    /// at most 140 chars). Extra optional fields never leak into `busMessages`.
    pub fn to_bus_entry(&self) -> BusMessageEntry {
        BusMessageEntry {
            id: self.id.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            kind: self.kind.clone(),
            preview: self.body.lines().next().unwrap_or("").chars().take(140).collect(),
            at: self.at,
        }
    }
}

/// Persisted form of the whole control plane.
#[derive(Serialize, Deserialize)]
pub struct StateFile {
    pub version: u32,
    #[serde(default)]
    pub supervisors: Vec<SupervisorMeta>,
    #[serde(default)]
    pub sessions: Vec<SessionState>,
    #[serde(default)]
    pub attribution: Vec<AttributionEntry>,
    #[serde(default)]
    pub conflicts: Vec<ConflictEntry>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Owning supervisor number -> nothing; kept for forward compatibility.
    #[serde(default)]
    pub stuck_after_secs: Option<u64>,
    // v1.3 persisted blocks.
    #[serde(default)]
    pub paused_supervisors: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<DecisionEntry>,
    #[serde(default)]
    pub escalations: Vec<EscalationEntry>,
    #[serde(default)]
    pub bus_messages: Vec<BusMessageEntry>,
    #[serde(default)]
    pub esc_seq: u64,
    #[serde(default)]
    pub msg_seq: u64,
}

impl StateFile {
    pub fn empty() -> Self {
        Self {
            version: 1,
            supervisors: Vec::new(),
            sessions: Vec::new(),
            attribution: Vec::new(),
            conflicts: Vec::new(),
            warnings: Vec::new(),
            stuck_after_secs: None,
            paused_supervisors: Vec::new(),
            decisions: Vec::new(),
            escalations: Vec::new(),
            bus_messages: Vec::new(),
            esc_seq: 0,
            msg_seq: 0,
        }
    }
}

#[cfg(test)]
mod a4_tests {
    //! Schema ownership (HARDENING §2): the isolation fields are additive and
    //! every one is `#[serde(default)]`. A pre-isolation state file must load
    //! with all three unset, and a file that carries them must round-trip.
    use super::*;
    use serde_json::{json, Value};

    /// Back-compat fixture: a real v1.3 state file written before worktrees
    /// existed. It has no `worktree` / `branch` / `base_rev` keys.
    const LEGACY_STATE: &str = include_str!("../tests/fixtures/state/legacy-session.json");

    #[test]
    fn legacy_state_fixture_loads_with_isolation_fields_unset() {
        let state: StateFile = serde_json::from_str(LEGACY_STATE).expect("legacy fixture must parse");
        assert_eq!(state.sessions.len(), 1);
        let value = serde_json::to_value(&state.sessions[0]).unwrap();
        // With #[serde(default)] the fields exist on re-serialization and are
        // null. Before they were added, the keys were absent entirely.
        assert!(value.get("worktree").is_some(), "worktree key must survive round-trip");
        assert!(value.get("branch").is_some(), "branch key must survive round-trip");
        assert!(value.get("base_rev").is_some(), "base_rev key must survive round-trip");
        assert!(value["worktree"].is_null());
        assert!(value["branch"].is_null());
        assert!(value["base_rev"].is_null());
    }

    #[test]
    fn isolation_fields_round_trip_when_present() {
        let mut value: Value = serde_json::from_str(LEGACY_STATE).unwrap();
        let session = &mut value["sessions"][0];
        session["worktree"] = json!("/tmp/notch-wt/S1-OC01");
        session["branch"] = json!("notch/S1-OC01");
        session["base_rev"] = json!("deadbeefcafe");
        let parsed: StateFile = serde_json::from_value(value).unwrap();
        let out = serde_json::to_value(&parsed.sessions[0]).unwrap();
        assert_eq!(out["worktree"], json!("/tmp/notch-wt/S1-OC01"));
        assert_eq!(out["branch"], json!("notch/S1-OC01"));
        assert_eq!(out["base_rev"], json!("deadbeefcafe"));
    }
}
