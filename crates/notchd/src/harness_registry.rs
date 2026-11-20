//! Definition-driven harness registry — the one superior feature stolen from
//! agentbox (`main/harnesses.mts`, `main/harnesses/common.mts`).
//!
//! The shape, not their 34-operation breadth:
//! - A [`HarnessDefinition`] is PURE DATA — ids, declared capabilities, and the
//!   flags that build a worker command line. No process, filesystem, socket,
//!   credential, or window code lives here.
//! - One adapter per engine behind [`AgentHarness`]. The registry owns the
//!   dispatch table, so the daemon never grows an `if engine == …` branch.
//! - What an engine can do is what it DECLARES, not what its adapter happens to
//!   implement. A negotiated decline (`DelegatingHarness::declining`) refuses
//!   before the adapter is touched — the ACP lesson agentbox paid for.
//! - Optional reads report [`Response::Absent`] instead of a fake zero; unknown
//!   engines are rejected at the boundary before any side effect.
//! - Registered is not offered: `admitted` data is separate from `get()`.
//!
//! This module creates no process, no window, no multiplexer, and no workspace.
//! Building an argv is pure string assembly ([`worker_args`]); executing it is
//! the adapter's job, through notch's existing bounded `proc::run` seam.
//!
//! Compiled standalone by `tests/w2_harness_registry*.rs` via `#[path]` until
//! the one-line `pub mod` wiring is landed by the owner of `lib.rs` (contract
//! request in COORD.md), hence the dead_code allowance for API completeness.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

/// Hard cap on registered engines. A daemon must not grow its dispatch table
/// without bound (notch D8 / HARDENING §3.9).
pub const MAX_ENGINES: usize = 8;
/// Longest legal engine id.
pub const MAX_ENGINE_ID_LEN: usize = 32;
/// Longest single argv token in a definition's worker command line.
pub const MAX_ARGV_TOKEN_LEN: usize = 256;
/// Maximum number of literal tokens in a definition's worker command line.
pub const MAX_ARGV_TOKENS: usize = 64;
/// The multiplexer notch is built on; used when a caller names no engine.
pub const DEFAULT_ENGINE: &str = "herdr";

/// The complete, closed operation set. Adding one is a registry change, never
/// a call-site branch. `Models` is the only optional read: declining it means
/// "unanswerable", not "zero".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HarnessOperation {
    Spawn,
    Read,
    Prompt,
    Kill,
    Focus,
    Status,
    Wait,
    Models,
}

impl HarnessOperation {
    /// Every operation, in declaration order (used by exhaustive tests).
    pub const ALL: [Self; 8] = [
        Self::Spawn,
        Self::Read,
        Self::Prompt,
        Self::Kill,
        Self::Focus,
        Self::Status,
        Self::Wait,
        Self::Models,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Read => "read",
            Self::Prompt => "prompt",
            Self::Kill => "kill",
            Self::Focus => "focus",
            Self::Status => "status",
            Self::Wait => "wait",
            Self::Models => "models",
        }
    }

    /// Optional reads return [`Response::Absent`] when declined; every other
    /// operation returns [`HarnessError::Unsupported`]. Never a fake zero.
    pub fn is_optional_read(self) -> bool {
        matches!(self, Self::Models)
    }
}

impl fmt::Display for HarnessOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Literal tokens of a worker command line, built from data in a fixed order:
/// `prefix ++ pre_payload ++ [payload] ++ post_payload`.
///
/// A key left out is absent, not defaulted — an engine that takes no `--model`
/// flag is never handed somebody else's. This is agentbox's "the command line
/// is data" rule, reduced to the smallest honest shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunFlags {
    pub prefix: Vec<String>,
    pub pre_payload: Vec<String>,
    pub post_payload: Vec<String>,
}

/// Pure argv assembly. No execution, no shell, no process.
pub fn worker_args(run: &RunFlags, req: &Request) -> Vec<String> {
    let mut args =
        Vec::with_capacity(run.prefix.len() + run.pre_payload.len() + run.post_payload.len() + 1);
    args.extend(run.prefix.iter().cloned());
    args.extend(run.pre_payload.iter().cloned());
    args.push(req.text.clone());
    args.extend(run.post_payload.iter().cloned());
    args
}

/// Browser-safe definition: names, flags, declared capabilities, admission.
/// It imports no process, credentials, filesystem, or transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessDefinition {
    pub id: String,
    pub label: String,
    pub capabilities: BTreeSet<HarnessOperation>,
    pub run: Option<RunFlags>,
    /// Registered is not offered. An unadmitted engine stays resolvable by id
    /// (legacy recovery records need that) but is absent from `offered_engines`.
    pub admitted: bool,
}

impl HarnessDefinition {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            capabilities: BTreeSet::new(),
            run: None,
            admitted: false,
        }
    }

    pub fn with_capability(mut self, op: HarnessOperation) -> Self {
        self.capabilities.insert(op);
        self
    }

    pub fn with_run(mut self, run: RunFlags) -> Self {
        self.run = Some(run);
        self
    }

    /// Declares this engine as one a caller may choose. Validation refuses an
    /// admission without a spawn capability and a worker command line.
    pub fn offered(mut self) -> Self {
        self.admitted = true;
        self
    }

    pub fn supports(&self, op: HarnessOperation) -> bool {
        self.capabilities.contains(&op)
    }
}

/// The herdr adapter: the only engine that may create a managed pane.
/// Kept as data so no daemon call site needs to know herdr's flag spelling.
pub fn herdr_definition() -> HarnessDefinition {
    HarnessDefinition::new("herdr", "herdr")
        .with_capability(HarnessOperation::Spawn)
        .with_capability(HarnessOperation::Read)
        .with_capability(HarnessOperation::Prompt)
        .with_capability(HarnessOperation::Kill)
        .with_capability(HarnessOperation::Focus)
        .with_capability(HarnessOperation::Status)
        .with_capability(HarnessOperation::Wait)
        .with_run(RunFlags {
            prefix: vec!["pane".into(), "split".into()],
            pre_payload: vec!["--direction".into(), "right".into(), "--cwd".into()],
            post_payload: Vec::new(),
        })
        .offered()
}

/// OpenCode is a judge/model source, not a managed pane engine. It is
/// registered (so old records resolve) but deliberately NOT offered, and it
/// declares no Spawn/Kill/Focus capability: it cannot make a workspace.
pub fn opencode_definition() -> HarnessDefinition {
    HarnessDefinition::new("opencode", "OpenCode")
        .with_capability(HarnessOperation::Status)
        .with_capability(HarnessOperation::Models)
}

/// Typed failures at the registry boundary. `Display` is wire-safe: no wrapper
/// prose, matching notch's `NotchError` convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HarnessError {
    UnknownEngine(String),
    Unsupported {
        engine: String,
        op: HarnessOperation,
    },
    InvalidDefinition(String),
    DuplicateEngine(String),
    RegistryFull,
    /// The adapter itself failed (its own runner/deadline error).
    Adapter(String),
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEngine(id) => write!(f, "unknown harness engine: {id}"),
            Self::Unsupported { engine, op } => {
                write!(f, "{engine} does not support {op}")
            }
            Self::InvalidDefinition(reason) => write!(f, "invalid harness definition: {reason}"),
            Self::DuplicateEngine(id) => write!(f, "duplicate harness: {id}"),
            Self::RegistryFull => write!(f, "harness registry is full ({MAX_ENGINES})"),
            Self::Adapter(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for HarnessError {}

/// One registry call. `target` is a managed session/pane id (never a wildcard),
/// `text` is the prompt or cwd payload, `lines` bounds a tail read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub op: HarnessOperation,
    pub target: String,
    pub text: String,
    pub lines: usize,
}

impl Request {
    pub fn new(op: HarnessOperation, target: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            op,
            target: target.into(),
            text: text.into(),
            lines: 200,
        }
    }

    pub fn with_lines(mut self, lines: usize) -> Self {
        self.lines = lines;
        self
    }
}

/// A registry answer. `Absent` is a truthful "this engine cannot answer", not
/// an empty value that a UI could mistake for a real zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Value {
        engine: String,
        op: HarnessOperation,
        text: String,
    },
    Absent {
        engine: String,
        op: HarnessOperation,
    },
}

impl Response {
    pub fn supported(&self) -> bool {
        matches!(self, Self::Value { .. })
    }

    pub fn engine(&self) -> &str {
        match self {
            Self::Value { engine, .. } | Self::Absent { engine, .. } => engine,
        }
    }

    pub fn op(&self) -> HarnessOperation {
        match self {
            Self::Value { op, .. } | Self::Absent { op, .. } => *op,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Value { text, .. } => Some(text),
            Self::Absent { .. } => None,
        }
    }
}

/// The execution seam: an adapter turns a request into a response. It may call
/// `proc::run` or a herdr client, but it never creates a window or a new
/// multiplexer — that choice is already fixed by the definition's capabilities.
pub type Invoker = Arc<dyn Fn(&Request) -> Result<Response, HarnessError> + Send + Sync>;

/// One adapter per engine. `available` is the negotiated truth for THIS engine
/// and can decline an operation the invoker is perfectly able to perform.
pub trait AgentHarness: Send + Sync {
    fn definition(&self) -> &HarnessDefinition;

    fn available(&self, _op: HarnessOperation) -> bool {
        true
    }

    fn invoke(&self, req: &Request) -> Result<Response, HarnessError>;
}

/// Adapter over an injected invoker. Built with a definition and an optional
/// set of negotiated declines; the registry, not the invoker, enforces them.
pub struct DelegatingHarness {
    definition: HarnessDefinition,
    invoker: Invoker,
    declined: BTreeSet<HarnessOperation>,
}

impl DelegatingHarness {
    pub fn new(definition: HarnessDefinition, invoker: Invoker) -> Self {
        Self {
            definition,
            invoker,
            declined: BTreeSet::new(),
        }
    }

    /// Declare operations this engine did not negotiate. Having the invoker
    /// code is not enough to make them callable (agentbox's ACP regression).
    pub fn declining(mut self, ops: impl IntoIterator<Item = HarnessOperation>) -> Self {
        self.declined.extend(ops);
        self
    }
}

impl AgentHarness for DelegatingHarness {
    fn definition(&self) -> &HarnessDefinition {
        &self.definition
    }

    fn available(&self, op: HarnessOperation) -> bool {
        !self.declined.contains(&op)
    }

    fn invoke(&self, req: &Request) -> Result<Response, HarnessError> {
        (self.invoker)(req)
    }
}

/// Literal engine ids only: lowercase first letter, then lowercase/digit/`-`.
/// This rejects `__proto__`, `constructor`, traversal and case tricks without
/// a regex dependency (agentbox's explicit-engine refusal).
pub fn is_valid_engine_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ENGINE_ID_LEN {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn validate(def: &HarnessDefinition) -> Result<(), HarnessError> {
    let invalid = |reason: String| HarnessError::InvalidDefinition(reason);
    if !is_valid_engine_id(&def.id) {
        return Err(invalid(format!("bad engine id `{}`", def.id)));
    }
    if def.label.trim().is_empty() {
        return Err(invalid(format!("engine `{}` needs a label", def.id)));
    }
    if let Some(run) = &def.run {
        if run.prefix.is_empty() {
            return Err(invalid(format!("engine `{}` has an empty command", def.id)));
        }
        let tokens = run
            .prefix
            .iter()
            .chain(run.pre_payload.iter())
            .chain(run.post_payload.iter());
        let mut count = 0usize;
        for token in tokens {
            count += 1;
            if token.is_empty() {
                return Err(invalid(format!("engine `{}` has an empty flag", def.id)));
            }
            if token.len() > MAX_ARGV_TOKEN_LEN {
                return Err(invalid(format!("engine `{}` has an overlong flag", def.id)));
            }
        }
        if count > MAX_ARGV_TOKENS {
            return Err(invalid(format!(
                "engine `{}` exceeds {MAX_ARGV_TOKENS} argv tokens",
                def.id
            )));
        }
    }
    // Absence of `run` is legal for a spawn-capable engine driven over RPC
    // (agentbox's Codex has no command line at all). `worker_argv` then returns
    // `None` instead of inventing somebody else's flags. Only ADMISSION requires
    // a declared worker command line, because a picker must know how to start
    // an engine it offers.
    if def.admitted && (!def.supports(HarnessOperation::Spawn) || def.run.is_none()) {
        return Err(invalid(format!(
            "engine `{}` cannot be offered without spawn + run flags",
            def.id
        )));
    }
    Ok(())
}

/// The single dispatch table. `invoke` is the only path from a caller to an
/// adapter, so capability enforcement has exactly one enforcement point.
pub struct HarnessRegistry {
    entries: Vec<Box<dyn AgentHarness>>,
}

impl Default for HarnessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HarnessRegistry {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn register<H: AgentHarness + 'static>(&mut self, harness: H) -> Result<(), HarnessError> {
        let def = harness.definition();
        validate(def)?;
        if self.entries.iter().any(|e| e.definition().id == def.id) {
            return Err(HarnessError::DuplicateEngine(def.id.clone()));
        }
        if self.entries.len() >= MAX_ENGINES {
            return Err(HarnessError::RegistryFull);
        }
        self.entries.push(Box::new(harness));
        Ok(())
    }

    /// Resolve an explicit engine. Unknown ids never reach an adapter.
    pub fn get(&self, id: &str) -> Result<&dyn AgentHarness, HarnessError> {
        self.entries
            .iter()
            .find(|h| h.definition().id == id)
            .map(|h| h.as_ref())
            .ok_or_else(|| HarnessError::UnknownEngine(id.to_string()))
    }

    pub fn get_or_default(&self, id: Option<&str>) -> Result<&dyn AgentHarness, HarnessError> {
        self.get(id.unwrap_or(DEFAULT_ENGINE))
    }

    /// Capability negotiation: declared by the definition AND still negotiated
    /// by the adapter for this engine. Method presence is irrelevant.
    pub fn supports(&self, id: &str, op: HarnessOperation) -> bool {
        match self.get(id) {
            Ok(h) => h.definition().supports(op) && h.available(op),
            Err(_) => false,
        }
    }

    /// The one invocation path. A declined or undeclared operation is refused
    /// BEFORE the adapter's invoker runs; an optional read becomes `Absent`.
    pub fn invoke(&self, id: &str, req: &Request) -> Result<Response, HarnessError> {
        let harness = self.get(id)?;
        let declared = harness.definition().supports(req.op);
        let negotiated = harness.available(req.op);
        if !declared || !negotiated {
            if req.op.is_optional_read() {
                return Ok(Response::Absent {
                    engine: id.to_string(),
                    op: req.op,
                });
            }
            return Err(HarnessError::Unsupported {
                engine: id.to_string(),
                op: req.op,
            });
        }
        harness.invoke(req)
    }

    /// Build a worker argv from the ENGINE'S DEFINITION, never from a call-site
    /// branch. An engine without declared spawn capability refuses here.
    pub fn worker_argv(
        &self,
        id: &str,
        req: &Request,
    ) -> Result<Option<Vec<String>>, HarnessError> {
        let harness = self.get(id)?;
        if !harness.definition().supports(HarnessOperation::Spawn)
            || !harness.available(HarnessOperation::Spawn)
        {
            return Err(HarnessError::Unsupported {
                engine: id.to_string(),
                op: HarnessOperation::Spawn,
            });
        }
        Ok(harness
            .definition()
            .run
            .as_ref()
            .map(|run| worker_args(run, req)))
    }

    /// Definitions callers may offer in a picker / plan. Registration alone
    /// does not offer an engine (OpenCode is registered but not offered).
    pub fn offered_engines(&self) -> Vec<HarnessDefinition> {
        self.entries
            .iter()
            .map(|h| h.definition().clone())
            .filter(|def| def.admitted)
            .collect()
    }

    /// All registered definitions, in registration order.
    pub fn definitions(&self) -> Vec<HarnessDefinition> {
        self.entries
            .iter()
            .map(|h| h.definition().clone())
            .collect()
    }
}
