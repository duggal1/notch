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
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

type Runner = Arc<dyn Fn(&[String]) -> Result<String, HerdrError> + Send + Sync>;

/// Default hard deadline for a single opencode subprocess call (milliseconds).
pub const DEFAULT_TIMEOUT_MS: u64 = 1500;
/// Environment override for the per-call deadline.
pub const TIMEOUT_ENV: &str = "NOTCH_OPENCODE_TIMEOUT_MS";
/// Short grace for reaping a child that has already closed its pipes.
const REAP_GRACE_MS: u64 = 250;

/// Resolve the per-call deadline safely. Missing, non-numeric, zero or
/// overflowing values fall back to [`DEFAULT_TIMEOUT_MS`] and warn on stderr
/// (the daemon captures stderr into its logs).
pub fn timeout_ms() -> u64 {
    let Ok(raw) = std::env::var(TIMEOUT_ENV) else {
        return DEFAULT_TIMEOUT_MS;
    };
    match raw.trim().parse::<u64>() {
        Ok(ms) if ms > 0 => ms,
        Ok(_) => {
            eprintln!(
                "opencode: {TIMEOUT_ENV}={raw:?} must be > 0; using default {DEFAULT_TIMEOUT_MS}ms"
            );
            DEFAULT_TIMEOUT_MS
        }
        Err(e) => {
            eprintln!(
                "opencode: invalid {TIMEOUT_ENV}={raw:?} ({e}); using default {DEFAULT_TIMEOUT_MS}ms"
            );
            DEFAULT_TIMEOUT_MS
        }
    }
}

/// `true` when `e` is the typed timeout error from the subprocess seam.
pub fn is_timeout_error(e: &HerdrError) -> bool {
    matches!(e, HerdrError::Io(msg) if msg.starts_with("opencode timeout after "))
}

fn timeout_error(ms: u64) -> HerdrError {
    HerdrError::Io(format!("opencode timeout after {ms}ms"))
}

/// Poll `try_wait` until the child exits or `deadline` passes. Avoids an
/// unbounded `wait()` if a child closes its pipes but lingers.
fn wait_bounded(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Run one opencode CLI call behind a hard deadline.
///
/// A single reader thread drains stdout then stderr so a hung child cannot
/// wedge the caller; `recv_timeout` caps the wait. On expiry the child is
/// killed and reaped before the typed timeout error is returned. The reader
/// thread observes pipe EOF once the child dies and exits on its own, so no
/// thread is leaked per call.
fn invoke_process(argv: &[String]) -> Result<String, HerdrError> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let ms = timeout_ms();
    let timeout = Duration::from_millis(ms);

    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| HerdrError::Io(format!("{}: {e}", argv[0])))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| HerdrError::Io("opencode: missing stdout pipe".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| HerdrError::Io("opencode: missing stderr pipe".into()))?;
    let (tx, rx) = mpsc::channel();
    // Detached: on kill the pipes close, the reads return EOF, the thread ends.
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = stderr.read_to_end(&mut err);
        let _ = tx.send((out, err));
    });

    let (out, err) = match rx.recv_timeout(timeout) {
        Ok(pair) => pair,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            let _ = child.wait(); // reap: no zombie/orphan left behind
            return Err(timeout_error(ms));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Reader died unexpectedly; never wedge the poll because of it.
            let grace = Instant::now() + Duration::from_millis(REAP_GRACE_MS);
            if wait_bounded(&mut child, grace).is_none() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(timeout_error(ms));
            }
            (Vec::new(), Vec::new())
        }
    };

    let status = match wait_bounded(
        &mut child,
        Instant::now() + Duration::from_millis(REAP_GRACE_MS),
    ) {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(timeout_error(ms));
        }
    };

    if !status.success() {
        return Err(HerdrError::Failed {
            args: argv[1..].to_vec(),
            code: status.code(),
            stderr: String::from_utf8_lossy(&err).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out).to_string())
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

    fn invoke_argv(&self, argv: &[String]) -> Result<String, HerdrError> {
        if let Some(runner) = &self.runner {
            return runner(argv);
        }
        invoke_process(argv)
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
