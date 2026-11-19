//! opencode CLI client — read-only activity API probe + supervisor decider
//! calls. Same injectable runner pattern as `herdr.rs` in spirit; the CLI
//! command is `opencode` (override with `NOTCH_OPENCODE_CMD`).
//!
//! Surfaces used (bridge.md v1.3 §2/§8 — nothing else):
//! - `opencode api GET /api/session` (read-only; activity signal)
//! - `opencode models`               (read-only; model list)
//! - `opencode run --model <m> <prompt>`  (decider calls only; never the
//!   orchestrator control path)
//!
//! All parses are tolerant; failures are `HerdrError`, never panics.

use crate::herdr::HerdrError;
use serde::Deserialize;
use std::sync::Arc;

type Runner = Arc<dyn Fn(&[String]) -> Result<String, HerdrError> + Send + Sync>;

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

    fn invoke_argv(&self, argv: &[String]) -> Result<String, HerdrError> {
        if let Some(runner) = &self.runner {
            return runner(argv);
        }
        let out = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .map_err(|e| HerdrError::Io(format!("{}: {e}", argv[0])))?;
        if !out.status.success() {
            return Err(HerdrError::Failed {
                args: argv[1..].to_vec(),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    pub fn api_session_list(&self) -> Result<Vec<ApiSession>, HerdrError> {
        let argv = vec![self.cmd.clone(), "api".into(), "GET".into(), "/api/session".into()];
        let out = self.invoke_argv(&argv)?;
        parse_api_session_list(&out)
    }

    pub fn models(&self) -> Result<Vec<String>, HerdrError> {
        let argv = vec![self.cmd.clone(), "models".into()];
        let out = self.invoke_argv(&argv)?;
        Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
    }

    /// One headless decider call. NOT on the orchestrator control path.
    pub fn run_decider(&self, model: &str, prompt: &str) -> Result<String, HerdrError> {
        let argv = vec![
            self.cmd.clone(),
            "run".into(),
            "--model".into(),
            model.to_string(),
            prompt.to_string(),
        ];
        self.invoke_argv(&argv)
    }

    /// Persistent-session decider call (perf fix). Continues ONE long-lived
    /// archived session instead of paying Node cold start + service connect +
    /// session creation + exit on every judgment. `--session` creates the
    /// session on first use and continues it thereafter (verified against
    /// opencode v2.3.2 / `opencode run --help`, 2026-11-19). `run --model` is
    /// deliberately kept as the argv prefix so the injectable-runner seam
    /// (`Opencode::with_runner`, used by the existing test suite) keeps
    /// matching `starts_with("run --model")`; the extra flags are additive.
    pub fn run_decider_session(
        &self,
        model: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<String, HerdrError> {
        let argv = vec![
            self.cmd.clone(),
            "run".into(),
            "--model".into(),
            model.to_string(),
            "--session".into(),
            session_id.to_string(),
            "--auto".into(),
            "--archived".into(),
            prompt.to_string(),
        ];
        self.invoke_argv(&argv)
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
