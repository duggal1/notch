//! Append-only state ledger with epoch fencing and parked refused writes.
//!
//! Stolen (structure only) from agentbox:
//! - `main/store/work-items.mts:70-113` — one `O_APPEND` write of one buffer
//!   per line; a short write throws instead of leaving a half line.
//! - `shared/work-items.mts:963-983` — every claim mints a monotonically
//!   increasing epoch; an agent line under a superseded epoch is DISCARDED by
//!   the fold (the zombie-writer fence). A founder/system line carries no
//!   epoch and is never fenced.
//! - `main/store/pending-writes.mts:59-131` — a refused write is PARKED on
//!   disk for the next legitimate holder instead of dropped; collecting it
//!   MOVES the file to `collected/`, it never deletes the evidence.
//!
//! Landed as **replay-over-checkpoint**: `state.json` stays a byte-compatible
//! checkpoint; this ledger is authoritative for the mutations it records and
//! is folded over the checkpoint at startup. Folding is idempotent (whole
//! entity upserts keyed by id, last-seq-wins), so a stale or missing checkpoint
//! cannot change the answer. Nothing here changes `model.rs` or `state.json`.
//!
//! Deliberately NOT stolen: the inode/fd generation-pin fold cache
//! (`work-items.mts:174-225`) — notch has one writer process; porting it would
//! add invalidation complexity with no benefit.

use crate::model::StateFile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// State-dir file names. Additive: `events.jsonl`'s frozen shape is untouched.
pub const LEDGER_FILE: &str = "ledger.jsonl";
pub const PENDING_DIR: &str = "pending-writes";
/// A line over this is hostile/torn and skipped on read (agentbox MAX_LINE_BYTES).
pub const MAX_LINE_BYTES: usize = 256 * 1024;
/// Trailing window kept in memory; older bytes are covered by the checkpoint.
pub const MAX_LEDGER_BYTES: u64 = 8 * 1024 * 1024;
/// Cap on parked files per target so a stuck writer cannot fill the disk.
const MAX_PENDING_PER_TARGET: usize = 1024;

/// Serializes every guarded append and claim across all `Ledger` handles, so a
/// handle opened before a peer's claim cannot make a stale decision.
static LEDGER_LOCK: Mutex<()> = Mutex::new(());

fn ledger_guard() -> MutexGuard<'static, ()> {
    LEDGER_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Who wrote a line. `System` is the daemon/checkpoint writer and is never
/// fenced; `Agent` is a worker under a claim epoch and is fenced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    System,
    Agent,
}

/// One ledger operation. `Upsert`/`Remove` carry a full entity, keyed by id,
/// which is what makes replay idempotent over any checkpoint base.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    /// Mint epoch `epoch` for `target` and hand the claim to `holder`.
    Claim { holder: String, lease_until_ms: u64 },
    /// Extend a still-held lease. Carries no state change on its own.
    Heartbeat { holder: String, lease_until_ms: u64 },
    /// Hand a claim back. Only a current-epoch release is accepted.
    Release,
    /// Full-entity assignment. `entity` names a `StateFile` vector and is
    /// validated on replay: an unknown name is dropped, never stored.
    Upsert {
        entity: String,
        id: String,
        value: Value,
    },
    /// Tombstone for a full-entity assignment.
    Remove { entity: String, id: String },
    /// Scalar high-water marks kept outside the entity vectors.
    SetSeq { esc_seq: u64, msg_seq: u64 },
    /// Epoch high-water marks carried across a ledger rotation so fencing
    /// stays correct after old lines are dropped.
    EpochAnchor { epochs: Vec<(String, u64)> },
    /// A checkpoint delta as ONE append: one O_APPEND write and one fsync for
    /// the whole diff instead of one syscall per changed entity.
    Batch { ops: Vec<Op> },
}

/// One append-only line. `refused` marks a write the fence rejected: it is
/// audited in the ledger but folded into the parking area, never into state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Line {
    pub seq: u64,
    pub ts: u64,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub holder: Option<String>,
    #[serde(default)]
    pub epoch: Option<u64>,
    #[serde(default)]
    pub target: String,
    pub op: Op,
    #[serde(default)]
    pub refused: Option<String>,
}

/// A fenced-out write, parked on disk until a legitimate holder collects it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkedWrite {
    pub seq: u64,
    pub ts: u64,
    pub target: String,
    pub holder: Option<String>,
    pub epoch: Option<u64>,
    pub reason: String,
    pub op: Op,
    /// Location of a successfully parked or collected payload.
    #[serde(default)]
    pub file: Option<PathBuf>,
}

/// Result of folding a ledger over a checkpoint.
pub struct ReplayResult {
    pub state: StateFile,
    /// Epoch high-water mark per target, derived from claim lines only.
    pub epochs: HashMap<String, u64>,
    /// Current claim holder + lease deadline per target.
    pub claims: HashMap<String, (String, u64)>,
    /// Lines the fence refused (also parked on disk by `replay_over_checkpoint`).
    pub refused: Vec<ParkedWrite>,
    pub applied: usize,
    pub last_seq: u64,
    pub warnings: Vec<String>,
}

fn encode(line: &Line) -> io::Result<Vec<u8>> {
    let mut buf = serde_json::to_vec(line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("ledger encode: {e}")))?;
    if buf.len() > MAX_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "ledger line too large ({} bytes, max {MAX_LINE_BYTES})",
                buf.len()
            ),
        ));
    }
    buf.push(b'\n');
    Ok(buf)
}

/// Read every whole, in-budget, parseable line. A torn tail line and an
/// oversize/hostile line are skipped; nothing is approximated (agentbox's
/// `readLinesFrom` / `foldRawInto` rule).
pub fn read_lines(path: &Path) -> io::Result<Vec<Line>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let start = if bytes.len() > MAX_LEDGER_BYTES as usize {
        bytes.len() - MAX_LEDGER_BYTES as usize
    } else {
        0
    };
    let slice = &bytes[start..];
    let end = slice
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    if end == 0 {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&slice[..end]);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() || raw.len() > MAX_LINE_BYTES {
            continue;
        }
        if let Ok(line) = serde_json::from_str::<Line>(raw) {
            out.push(line);
        }
    }
    Ok(out)
}

/// Only the epoch/claim half of the fold, used to keep a live handle current.
fn note_claim(
    line: &Line,
    epochs: &mut HashMap<String, u64>,
    claims: &mut HashMap<String, (String, u64)>,
) {
    note_op(line, &line.op, epochs, claims);
}

fn note_op(
    line: &Line,
    op: &Op,
    epochs: &mut HashMap<String, u64>,
    claims: &mut HashMap<String, (String, u64)>,
) {
    if line.refused.is_some() {
        return;
    }
    match op {
        Op::Batch { ops } => {
            for child in ops {
                note_op(line, child, epochs, claims);
            }
        }
        Op::Claim {
            holder,
            lease_until_ms,
        } => {
            if let Some(e) = line.epoch {
                if e >= epochs.get(&line.target).copied().unwrap_or(0) {
                    epochs.insert(line.target.clone(), e);
                    claims.insert(line.target.clone(), (holder.clone(), *lease_until_ms));
                }
            }
        }
        Op::Release => {
            if line
                .epoch
                .is_some_and(|e| e >= epochs.get(&line.target).copied().unwrap_or(0))
            {
                claims.remove(&line.target);
            }
        }
        Op::Heartbeat {
            holder,
            lease_until_ms,
        } => {
            let held = claims.get(&line.target).is_some_and(|(h, _)| h == holder);
            let current = epochs.get(&line.target).copied().unwrap_or(0);
            if held && line.source == Source::Agent && line.epoch == Some(current)
                || line.source == Source::System
            {
                claims.insert(line.target.clone(), (holder.clone(), *lease_until_ms));
            }
        }
        Op::EpochAnchor { epochs: anchor } => {
            for (target, epoch) in anchor {
                if *epoch >= epochs.get(target).copied().unwrap_or(0) {
                    epochs.insert(target.clone(), *epoch);
                }
            }
        }
        _ => {}
    }
}

/// A live handle to one state dir's ledger. Clone-free: every mutation takes
/// the global lock and re-reads the file so a stale handle cannot win a race.
pub struct Ledger {
    dir: PathBuf,
    path: PathBuf,
    last_seq: u64,
    epochs: HashMap<String, u64>,
    claims: HashMap<String, (String, u64)>,
}

impl Ledger {
    pub fn open(state_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(state_dir)?;
        let mut ledger = Ledger {
            dir: state_dir.to_path_buf(),
            path: state_dir.join(LEDGER_FILE),
            last_seq: 0,
            epochs: HashMap::new(),
            claims: HashMap::new(),
        };
        // The append-only file exists from first open, so external appenders
        // (and crash-injection harnesses) can always open it by path.
        if !ledger.path.exists() {
            crate::durable::append_durable(&ledger.path, b"")?;
        }
        // A SIGKILL mid-append can leave a partial line with no trailing
        // newline. It is uncommitted by definition; drop it before the next
        // append so a fresh whole line cannot be glued onto it and lost by the
        // fold's "whole lines only" rule.
        let bytes = fs::read(&ledger.path)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            let keep = bytes
                .iter()
                .rposition(|&b| b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            let f = OpenOptions::new().write(true).open(&ledger.path)?;
            f.set_len(keep as u64)?;
            f.sync_data()?;
        }
        ledger.refresh();
        Ok(ledger)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    pub fn current_epoch(&self, target: &str) -> u64 {
        self.epochs.get(target).copied().unwrap_or(0)
    }

    fn refresh(&mut self) {
        self.last_seq = 0;
        self.epochs.clear();
        self.claims.clear();
        if let Ok(lines) = read_lines(&self.path) {
            for line in &lines {
                if line.seq > self.last_seq {
                    self.last_seq = line.seq;
                }
                note_claim(line, &mut self.epochs, &mut self.claims);
            }
        }
    }

    /// Called with [`ledger_guard`] held. Appends one whole buffer with a
    /// single O_APPEND write; carries epoch high-water marks across rotation.
    fn append_locked(&mut self, line: Line) -> io::Result<u64> {
        if crate::events::rotate_if_oversized(&self.path, MAX_LEDGER_BYTES)? {
            let anchor = Op::EpochAnchor {
                epochs: self.epochs.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            };
            let first = Line {
                seq: self.last_seq + 1,
                ts: line.ts,
                source: Source::System,
                holder: None,
                epoch: None,
                target: "ORCH".into(),
                op: anchor,
                refused: None,
            };
            crate::durable::append_durable(&self.path, &encode(&first)?)?;
            self.last_seq = first.seq;
        }
        crate::durable::append_durable(&self.path, &encode(&line)?)?;
        self.last_seq = line.seq;
        note_claim(&line, &mut self.epochs, &mut self.claims);
        Ok(line.seq)
    }

    /// Append a daemon/system line. System lines carry no epoch and are never
    /// fenced (agentbox's founder rule), so checkpoints can always advance.
    pub fn append(
        &mut self,
        source: Source,
        holder: Option<&str>,
        epoch: Option<u64>,
        target: &str,
        op: Op,
        ts: u64,
    ) -> io::Result<u64> {
        let _guard = ledger_guard();
        let line = Line {
            seq: self.last_seq + 1,
            ts,
            source,
            holder: holder.map(str::to_string),
            // Only an agent line may be fenced. A system line's epoch is dropped.
            epoch: if source == Source::Agent { epoch } else { None },
            target: target.to_string(),
            op,
            refused: None,
        };
        self.append_locked(line)
    }

    /// Mint the next epoch for `target` and claim it for `holder`.
    pub fn claim(&mut self, holder: &str, target: &str, lease_ms: u64, ts: u64) -> io::Result<u64> {
        let _guard = ledger_guard();
        self.refresh();
        let epoch = self.epochs.get(target).copied().unwrap_or(0) + 1;
        let line = Line {
            seq: self.last_seq + 1,
            ts,
            source: Source::Agent,
            holder: Some(holder.to_string()),
            epoch: Some(epoch),
            target: target.to_string(),
            op: Op::Claim {
                holder: holder.to_string(),
                lease_until_ms: ts.saturating_add(lease_ms),
            },
            refused: None,
        };
        self.append_locked(line)?;
        Ok(epoch)
    }

    /// Append an agent line under `epoch`. If the fence has since advanced,
    /// the write is AUDITED as refused in the ledger and PARKED on disk — it is
    /// never applied to state and never silently dropped.
    pub fn append_guarded(
        &mut self,
        holder: &str,
        epoch: u64,
        target: &str,
        op: Op,
        ts: u64,
    ) -> Result<u64, Box<ParkedWrite>> {
        let _guard = ledger_guard();
        self.refresh();
        let current = self.epochs.get(target).copied().unwrap_or(0);
        let line = Line {
            seq: self.last_seq + 1,
            ts,
            source: Source::Agent,
            holder: Some(holder.to_string()),
            epoch: Some(epoch),
            target: target.to_string(),
            op,
            refused: (epoch < current).then(|| "stale_epoch".to_string()),
        };
        self.append_locked(line.clone())
            .map_err(|e| Box::new(parked_from_line(&line, &format!("append failed: {e}"))))?;
        if let Some(reason) = &line.refused {
            let mut parked = parked_from_line(&line, reason);
            // AgentBox main/store/pending-writes.mts:59-84 returns a receipt so
            // the refused writer can find its payload after reclaiming.
            match park(&self.dir, &parked) {
                Ok(path) => parked.file = Some(path),
                Err(error) => parked.reason = format!("{reason}: parking failed: {error}"),
            }
            return Err(Box::new(parked));
        }
        Ok(line.seq)
    }
}

fn parked_from_line(line: &Line, reason: &str) -> ParkedWrite {
    ParkedWrite {
        seq: line.seq,
        ts: line.ts,
        target: line.target.clone(),
        holder: line.holder.clone(),
        epoch: line.epoch,
        reason: reason.to_string(),
        op: line.op.clone(),
        file: None,
    }
}

/// Keep only path-safe characters. A holder/target id reaches a filename; a
/// planted `../` must never escape the state dir.
fn safe_component(value: &str, max: usize) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.chars().take(max).collect()
    }
}

fn pending_dir(state_dir: &Path, target: &str) -> PathBuf {
    state_dir.join(PENDING_DIR).join(safe_component(target, 64))
}

/// Park a refused write. `create_new` refuses to follow a planted symlink and
/// makes re-parking on every replay idempotent (same deterministic filename).
pub fn park(state_dir: &Path, parked: &ParkedWrite) -> io::Result<PathBuf> {
    let dir = pending_dir(state_dir, &parked.target);
    if fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "pending-writes target is a symlink",
        ));
    }
    fs::create_dir_all(&dir)?;
    let existing = fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    if existing >= MAX_PENDING_PER_TARGET {
        // Bounded parking. Oldest-first by filename (seq is zero-padded) so a
        // stuck writer cannot fill the disk; the ledger still audits the line.
        let mut names: Vec<PathBuf> = fs::read_dir(&dir)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        names.sort();
        if let Some(oldest) = names.into_iter().next() {
            let _ = fs::remove_file(oldest);
        }
    }
    let holder = safe_component(parked.holder.as_deref().unwrap_or("unknown"), 40);
    let reason = safe_component(&parked.reason, 24);
    let name = format!("{:020}-{}-{}.json", parked.seq, holder, reason);
    // A collected message has already been delivered to a legitimate holder.
    // Replay re-parks refused ledger lines on every startup, so without this
    // check a collected write would resurrect in the pending queue forever.
    let collected = dir.join("collected").join(&name);
    if collected.exists() {
        return Ok(collected);
    }
    let file = dir.join(name);
    let bytes = serde_json::to_vec_pretty(parked)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("park encode: {e}")))?;
    match OpenOptions::new().write(true).create_new(true).open(&file) {
        Ok(mut f) => {
            f.write_all(&bytes)?;
            f.sync_data()?;
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(file),
        Err(e) => return Err(e),
    }
    Ok(file)
}

/// Every write still waiting on `target`, oldest first. Symlinked entries are
/// ignored: a planted link is never read as a parked message.
pub fn read_pending(state_dir: &Path, target: &str) -> io::Result<Vec<ParkedWrite>> {
    let dir = pending_dir(state_dir, target);
    if fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(Vec::new());
    }
    let rd = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in rd {
        let entry = entry?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(bytes) = fs::read(entry.path()) {
            if let Ok(mut parked) = serde_json::from_slice::<ParkedWrite>(&bytes) {
                parked.file = Some(entry.path());
                out.push(parked);
            }
        }
    }
    out.sort_by_key(|p| (p.ts, p.seq));
    Ok(out)
}

/// Mark everything waiting on `target` as delivered by `by`. Files MOVE to
/// `collected/` rather than vanish, so a later session is not handed the same
/// words twice and nothing becomes unrecoverable (agentbox's rule).
pub fn collect_pending(state_dir: &Path, target: &str, by: &str) -> io::Result<Vec<PathBuf>> {
    let waiting = read_pending(state_dir, target)?;
    if waiting.is_empty() {
        return Ok(Vec::new());
    }
    let dir = pending_dir(state_dir, target);
    let done = dir.join("collected");
    fs::create_dir_all(&done)?;
    let mut moved = Vec::new();
    for parked in waiting {
        let Some(src) = parked.file else { continue };
        let dst = done.join(src.file_name().unwrap_or_default());
        let mut value: Value = match fs::read(&src).and_then(|b| {
            serde_json::from_slice(&b)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        }) {
            Ok(v) => v,
            Err(_) => continue, // a message we cannot move is left where it is — safe
        };
        value["collectedBy"] = json!(by);
        value["collectedAt"] = json!(now_ms());
        if serde_json::to_vec_pretty(&value)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
            .and_then(|b| fs::write(&src, b))
            .is_ok()
            && fs::rename(&src, &dst).is_ok()
        {
            moved.push(dst);
        }
    }
    Ok(moved)
}

/// Map a validated entity onto the matching `StateFile` vector via a Value
/// round-trip: unknown names are dropped, and a shape that does not deserialize
/// leaves the prior value standing instead of becoming state.
fn upsert_entity(state: &mut StateFile, entity: &str, value: &Value, warnings: &mut Vec<String>) {
    let key = match entity {
        "session" => "sessions",
        "supervisor" => "supervisors",
        "bus" => "bus_messages",
        "escalation" => "escalations",
        "decision" => "decisions",
        "attribution" => "attribution",
        "conflict" => "conflicts",
        _ => {
            warnings.push(format!("ledger: dropped unknown entity {entity:?}"));
            return;
        }
    };
    let Some(id) = value.get("id").and_then(|v| v.as_str()) else {
        warnings.push(format!("ledger: {entity} upsert without id dropped"));
        return;
    };
    let mut root = match serde_json::to_value(&*state) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("ledger: checkpoint encode failed: {e}"));
            return;
        }
    };
    let arr = root[key]
        .as_array_mut()
        .expect("StateFile serializes every vector");
    if let Some(pos) = arr
        .iter()
        .position(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
    {
        arr[pos] = value.clone();
    } else {
        arr.push(value.clone());
    }
    match serde_json::from_value(root) {
        Ok(next) => *state = next,
        Err(e) => warnings.push(format!("ledger: {entity} upsert rejected by schema: {e}")),
    }
}

fn remove_entity(state: &mut StateFile, entity: &str, id: &str, warnings: &mut Vec<String>) {
    let key = match entity {
        "session" => "sessions",
        "supervisor" => "supervisors",
        "bus" => "bus_messages",
        "escalation" => "escalations",
        "decision" => "decisions",
        "attribution" => "attribution",
        "conflict" => "conflicts",
        _ => {
            warnings.push(format!("ledger: dropped unknown entity {entity:?}"));
            return;
        }
    };
    let mut root = match serde_json::to_value(&*state) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("ledger: checkpoint encode failed: {e}"));
            return;
        }
    };
    if let Some(arr) = root[key].as_array_mut() {
        arr.retain(|e| e.get("id").and_then(|v| v.as_str()) != Some(id));
    }
    match serde_json::from_value(root) {
        Ok(next) => *state = next,
        Err(e) => warnings.push(format!("ledger: {entity} remove rejected by schema: {e}")),
    }
}

/// Apply one op to the fold. Returns true when the epoch fence refused it.
fn apply_op(
    line: &Line,
    op: &Op,
    state: &mut StateFile,
    epochs: &mut HashMap<String, u64>,
    claims: &mut HashMap<String, (String, u64)>,
    warnings: &mut Vec<String>,
) -> bool {
    let current = epochs.get(&line.target).copied().unwrap_or(0);
    if line.source == Source::Agent && line.epoch.is_some_and(|e| e < current) {
        return true;
    }
    match op {
        Op::Batch { ops } => {
            let mut refused = false;
            for child in ops {
                refused |= apply_op(line, child, state, epochs, claims, warnings);
            }
            refused
        }
        Op::Claim { .. } | Op::Release | Op::Heartbeat { .. } | Op::EpochAnchor { .. } => {
            note_op(line, op, epochs, claims);
            false
        }
        Op::Upsert { entity, id, value } => {
            upsert_entity(state, entity, value, warnings);
            let _ = id; // the value carries the same id; validated in upsert_entity
            false
        }
        Op::Remove { entity, id } => {
            remove_entity(state, entity, id, warnings);
            false
        }
        Op::SetSeq { esc_seq, msg_seq } => {
            state.esc_seq = *esc_seq;
            state.msg_seq = *msg_seq;
            false
        }
    }
}

/// Fold every line over a checkpoint. Pure: the same `(base, lines)` always
/// yields the same state, so replay is safe after a crash or a stale checkpoint.
pub fn replay(base: StateFile, lines: &[Line]) -> ReplayResult {
    let mut state = base;
    let mut epochs: HashMap<String, u64> = HashMap::new();
    let mut claims: HashMap<String, (String, u64)> = HashMap::new();
    let mut refused = Vec::new();
    let mut warnings = Vec::new();
    let mut applied = 0usize;
    let mut last_seq = 0u64;

    for line in lines {
        if line.seq <= last_seq {
            warnings.push(format!(
                "ledger: skipped duplicate/out-of-order seq {}",
                line.seq
            ));
            continue;
        }
        last_seq = line.seq;
        if let Some(reason) = &line.refused {
            refused.push(parked_from_line(line, reason));
            continue;
        }
        if apply_op(
            line,
            &line.op,
            &mut state,
            &mut epochs,
            &mut claims,
            &mut warnings,
        ) {
            refused.push(parked_from_line(line, "stale_epoch"));
            continue;
        }
        applied += 1;
    }

    ReplayResult {
        state,
        epochs,
        claims,
        refused,
        applied,
        last_seq,
        warnings,
    }
}

/// Replay a state dir's ledger over a checkpoint, parking any refused lines.
pub fn replay_over_checkpoint(state_dir: &Path, base: StateFile) -> io::Result<ReplayResult> {
    let mut result = replay(base, &read_lines(&state_dir.join(LEDGER_FILE))?);
    let mut failures = 0;
    let mut first_error = None;
    for parked in &mut result.refused {
        match park(state_dir, parked) {
            Ok(path) => parked.file = Some(path),
            Err(error) => {
                failures += 1;
                first_error.get_or_insert((parked.seq, error));
            }
        }
    }
    let parked = result.refused.len() - failures;
    if parked > 0 {
        result
            .warnings
            .push(format!("ledger: {parked} stale writes parked or collected"));
    }
    if let Some((seq, error)) = first_error {
        result.warnings.push(format!(
            "ledger: parking failed for {failures} refused writes (first seq {seq}: {error})"
        ));
    }
    Ok(result)
}
