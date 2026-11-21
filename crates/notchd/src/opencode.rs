//! opencode CLI client — read-only activity API probe + supervisor decider
//! calls. Same injectable runner pattern as `herdr.rs` in spirit; the CLI
//! command is `opencode` (override with `NOTCH_OPENCODE_CMD`).
//!
//! Surfaces used (bridge.md v1.3 §2/§8 — nothing else):
//! - `opencode api GET /api/session` (read-only; activity signal)
//! - `opencode models`               (read-only; model list)
//! - `opencode run [--model <m>] --auto --session ses_notch_<id> <prompt>`
//!   (decider calls only; never the orchestrator control path). `--model` is
//!   omitted when the model is `auto`, so the operator's harness default is used.
//!
//! **Every subprocess goes through `crate::proc::run`.** No local spawn, no
//! local timeout loop. Deadlines come from A2's frozen constants, with
//! `NOTCH_OPENCODE_TIMEOUT_MS` as an ops/test override.
//!
//! Per user override 2026-11-20 the decider has FULL tool access: `--auto` is
//! intentionally kept so tool calls never block on permission prompts, and
//! there is no empty scratch dir — the judge runs in the session's real cwd
//! so it can read real files and command output as evidence.

use crate::decider;
use crate::herdr::HerdrError;
use crate::proc::{self, ProcError, Spec, TIMEOUT_DECIDER, TIMEOUT_OPENCODE_API};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

type Runner = Arc<dyn Fn(&[String]) -> Result<String, HerdrError> + Send + Sync>;

/// Default hard deadline for `api` / `models` (A2's frozen constant, 8 s).
pub const DEFAULT_TIMEOUT_MS: u64 = TIMEOUT_OPENCODE_API.as_millis() as u64;
/// Default hard deadline for a decider `run` (A2's frozen constant, 90 s).
pub const DEFAULT_DECIDER_TIMEOUT_MS: u64 = TIMEOUT_DECIDER.as_millis() as u64;
/// Ops/test override for the per-call deadline.
pub const TIMEOUT_ENV: &str = "NOTCH_OPENCODE_TIMEOUT_MS";

fn timeout_or(default_ms: u64) -> u64 {
    let Ok(raw) = std::env::var(TIMEOUT_ENV) else {
        return default_ms;
    };
    match raw.trim().parse::<u64>() {
        Ok(ms) if ms > 0 => ms,
        Ok(_) => {
            eprintln!("opencode: {TIMEOUT_ENV}={raw:?} must be > 0; using default {default_ms}ms");
            default_ms
        }
        Err(e) => {
            eprintln!("opencode: invalid {TIMEOUT_ENV}={raw:?} ({e}); using default {default_ms}ms");
            default_ms
        }
    }
}

/// Per-call deadline for the read-only surfaces.
pub fn timeout_ms() -> u64 {
    timeout_or(DEFAULT_TIMEOUT_MS)
}

/// Per-call deadline for a decider judgment.
pub fn decider_timeout_ms() -> u64 {
    timeout_or(DEFAULT_DECIDER_TIMEOUT_MS)
}

/// `true` when `e` is the typed timeout error from the subprocess seam.
pub fn is_timeout_error(e: &HerdrError) -> bool {
    matches!(e, HerdrError::Io(msg) if msg.starts_with("opencode timeout after "))
}

/// `true` when the decider call itself was rate limited (429/quota in stderr).
pub fn is_rate_limit_error(e: &HerdrError) -> bool {
    decider::contains_rate_limit(&e.to_string())
}

fn timeout_error(ms: u64) -> HerdrError {
    HerdrError::Io(format!("opencode timeout after {ms}ms"))
}

#[derive(Clone)]
pub struct Opencode {
    cmd: String,
    runner: Option<Runner>,
}

impl Opencode {
    /// Command name from `NOTCH_OPENCODE_CMD` (default `opencode`).
    pub fn from_env() -> Self {
        let cmd = std::env::var("NOTCH_OPENCODE_CMD")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "opencode".to_string());
        Self { cmd, runner: None }
    }

    pub fn with_runner<F>(f: F) -> Self
    where
        F: Fn(&[String]) -> Result<String, HerdrError> + Send + Sync + 'static,
    {
        Self {
            cmd: "<fake-opencode>".to_string(),
            runner: Some(Arc::new(f)),
        }
    }

    /// The one production seam. `cwd` is passed through so a judge can work in
    /// the session's real worktree; `timeout` is always bounded by `proc::run`.
    fn invoke_argv(
        &self,
        argv: &[String],
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Result<String, HerdrError> {
        if let Some(runner) = &self.runner {
            return runner(argv);
        }
        if argv.is_empty() {
            return Err(HerdrError::Io("opencode: empty argv".into()));
        }
        let spec = Spec {
            cmd: &argv[0],
            args: &argv[1..],
            cwd,
            env: &[],
            timeout,
            max_out: 4 * 1024 * 1024,
        };
        match proc::run(&spec) {
            Ok(out) => Ok(out.stdout),
            Err(ProcError::Spawn(e)) => Err(HerdrError::Io(e)),
            Err(ProcError::Timeout { after }) => {
                Err(timeout_error(after.as_millis().min(u64::MAX as u128) as u64))
            }
            Err(ProcError::Failed { code, stderr }) => Err(HerdrError::Failed {
                args: argv[1..].to_vec(),
                code,
                stderr,
            }),
        }
    }

    fn read_seam(&self, argv: &[String]) -> Result<String, HerdrError> {
        self.invoke_argv(argv, None, Duration::from_millis(timeout_ms()))
    }

    pub fn api_session_list(&self) -> Result<Vec<ApiSession>, HerdrError> {
        let argv = vec![self.cmd.clone(), "api".into(), "GET".into(), "/api/session".into()];
        let out = self.read_seam(&argv)?;
        parse_api_session_list(&out)
    }

    pub fn models(&self) -> Result<Vec<String>, HerdrError> {
        let argv = vec![self.cmd.clone(), "models".into()];
        let out = self.read_seam(&argv)?;
        Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
    }

    /// One headless decider call, no session reuse. NOT on the control path.
    pub fn run_decider(&self, model: &str, prompt: &str) -> Result<String, HerdrError> {
        self.run_decider_in(model, prompt, None)
    }

    /// Headless decider call in an explicit cwd. With no cwd the daemon's own
    /// cwd is used — never an empty scratch dir (user override 2026-11-20).
    pub fn run_decider_in(
        &self,
        model: &str,
        prompt: &str,
        cwd: Option<&Path>,
    ) -> Result<String, HerdrError> {
        let mut argv = vec![self.cmd.clone(), "run".into()];
        // `auto` omits --model so OpenCode uses the operator's own default.
        if !decider::is_default_model(model) {
            argv.push("--model".into());
            argv.push(model.to_string());
        }
        // FULL ACCESS by user decision: do not block on permission prompts.
        argv.push("--auto".into());
        argv.push(prompt.to_string());
        self.invoke_argv(&argv, cwd, Duration::from_millis(decider_timeout_ms()))
    }

    /// Persistent-session decider call. `--session` creates the session on
    /// first use and continues it thereafter (verified against opencode
    /// v2.3.2). Callers pass a per-notch-session `ses_notch_<id>` id so one
    /// session's context never leaks into another's judgment.
    pub fn run_decider_session(
        &self,
        model: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<String, HerdrError> {
        self.run_decider_session_in(model, session_id, prompt, None)
    }

    /// Same as [`Self::run_decider_session`] but with the session's real cwd.
    pub fn run_decider_session_in(
        &self,
        model: &str,
        session_id: &str,
        prompt: &str,
        cwd: Option<&Path>,
    ) -> Result<String, HerdrError> {
        let mut argv = vec![self.cmd.clone(), "run".into()];
        if !decider::is_default_model(model) {
            argv.push("--model".into());
            argv.push(model.to_string());
        }
        argv.push("--auto".into());
        argv.push("--session".into());
        argv.push(session_id.to_string());
        argv.push("--archived".into());
        argv.push(prompt.to_string());
        self.invoke_argv(&argv, cwd, Duration::from_millis(decider_timeout_ms()))
    }
}

/// One session entry from `/api/session`. Unknown fields are tolerated.
#[derive(Clone, Debug)]
pub struct ApiSession {
    pub id: String,
    pub busy: Option<u64>,
    pub idle: Option<u64>,
    pub model: Option<String>,
    pub directory: Option<String>,
}

/// Idle iff `time.idle >= time.busy`. A session with no busy marker has not
/// started working yet (treated as idle); a busy marker with no idle yet is
/// working.
pub fn is_idle(s: &ApiSession) -> bool {
    match (s.busy, s.idle) {
        (None, _) => true,
        (Some(busy), Some(idle)) => idle >= busy,
        (Some(_), None) => false,
    }
}

#[derive(Deserialize)]
struct WireSession {
    #[serde(default)]
    id: String,
    #[serde(default)]
    model: Option<serde_json::Value>,
    #[serde(default)]
    time: Option<WireTime>,
    #[serde(default)]
    location: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct WireTime {
    #[serde(default)]
    busy: Option<u64>,
    #[serde(default)]
    idle: Option<u64>,
}

pub fn parse_api_session_list(out: &str) -> Result<Vec<ApiSession>, HerdrError> {
    let trimmed = out.trim();
    let v: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| HerdrError::Parse(format!("{e}")))?;
    // Tolerate both the live envelope { cursor, data: [...] } and a bare array.
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| v.as_array())
        .ok_or_else(|| HerdrError::Parse("no `data` array in /api/session output".to_string()))?;
    let mut sessions = Vec::with_capacity(arr.len());
    for item in arr {
        let Ok(w) = serde_json::from_value::<WireSession>(item.clone()) else {
            continue;
        };
        if w.id.is_empty() {
            continue;
        }
        let model = w.model.as_ref().and_then(|m| {
            if let Some(s) = m.as_str() {
                return Some(s.to_string());
            }
            let provider = m.get("providerID").and_then(|p| p.as_str());
            let id = m.get("id").and_then(|i| i.as_str());
            match (provider, id) {
                (Some(p), Some(i)) => Some(format!("{p}/{i}")),
                (None, Some(i)) => Some(i.to_string()),
                _ => None,
            }
        });
        let directory = w
            .location
            .as_ref()
            .and_then(|l| l.get("directory").and_then(|d| d.as_str()).map(str::to_string));
        sessions.push(ApiSession {
            id: w.id,
            busy: w.time.as_ref().and_then(|t| t.busy),
            idle: w.time.as_ref().and_then(|t| t.idle),
            model,
            directory,
        });
    }
    Ok(sessions)
}
