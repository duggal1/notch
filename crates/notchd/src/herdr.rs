//! herdr client — the ONLY surface notchd has into OpenCode/Ghostty.
//!
//! Every invocation goes through one module. Tests inject a fake runner via
//! `Herdr::with_runner`; production uses `Herdr::from_env` (command name from
//! `NOTCH_HERDR_CMD`, default `herdr`).
//!
//! The runner receives the full argv, command first — exactly what
//! `std::process::Command` would execute.
//!
//! All parses are tolerant: unknown fields are ignored, the CLI envelope
//! `{ "id": ..., "result": { ... } }` is unwrapped when present, and any
//! failure is reported as `HerdrError` — never panics, never wedges the
//! daemon.

use crate::proc::{
    self, ProcError, Spec, TIMEOUT_HERDR_LIST, TIMEOUT_HERDR_MUTATE, TIMEOUT_HERDR_READ,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

/// Cap on captured herdr stdout/stderr. Children keep draining past this so a
/// 1 GiB listing can never block the poll thread on a full pipe.
const HERDR_MAX_OUT: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum HerdrError {
    Io(String),
    Failed {
        args: Vec<String>,
        code: Option<i32>,
        stderr: String,
    },
    Parse(String),
}

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HerdrError::Io(e) => write!(f, "herdr io: {e}"),
            HerdrError::Failed { args, code, stderr } => write!(
                f,
                "herdr {cmd} failed (exit {code}): {stderr}",
                cmd = args.join(" "),
                code = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            ),
            HerdrError::Parse(e) => write!(f, "herdr parse: {e}"),
        }
    }
}

impl std::error::Error for HerdrError {}

/// One entry of `herdr agent list` / `herdr pane list`. Unknown fields are
/// tolerated by serde's default behavior.
#[derive(Clone, Debug, Deserialize)]
pub struct AgentEntry {
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub agent_status: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub terminal_title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub agent_session: Option<serde_json::Value>,
}

impl AgentEntry {
    /// `agent_session.value` from `herdr agent list` — the OpenCode session id
    /// (matches `opencode api GET /api/session`'s `id`).
    pub fn agent_session_value(&self) -> Option<String> {
        self.agent_session
            .as_ref()
            .and_then(|v| v.get("value").and_then(|x| x.as_str()))
            .map(str::to_string)
    }
}

type Runner = Arc<dyn Fn(&[String]) -> Result<String, HerdrError> + Send + Sync>;

#[derive(Clone)]
pub struct Herdr {
    cmd: String,
    runner: Option<Runner>,
}

impl Herdr {
    /// Command name from `NOTCH_HERDR_CMD` (default `herdr`). The override
    /// exists so tests and headless setups can point at a scripted fake.
    pub fn from_env() -> Self {
        let cmd = std::env::var("NOTCH_HERDR_CMD")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "herdr".to_string());
        Self { cmd, runner: None }
    }

    pub fn with_runner<F>(f: F) -> Self
    where
        F: Fn(&[String]) -> Result<String, HerdrError> + Send + Sync + 'static,
    {
        Self {
            cmd: "<fake>".to_string(),
            runner: Some(Arc::new(f)),
        }
    }

    /// Read-only herdr call (agent list / pane list): 5 s deadline.
    fn invoke(&self, args: &[&str], timeout: Duration) -> Result<String, HerdrError> {
        let argv: Vec<String> = std::iter::once(self.cmd.clone())
            .chain(args.iter().map(|s| s.to_string()))
            .collect();
        self.invoke_argv(&argv, timeout)
    }

    /// Mutating herdr call (split/start/prompt/wait/close/focus): 10 s deadline.
    fn invoke_mut(&self, args: &[&str]) -> Result<String, HerdrError> {
        self.invoke(args, TIMEOUT_HERDR_MUTATE)
    }

    fn invoke_owned(&self, argv: &[String], timeout: Duration) -> Result<String, HerdrError> {
        let full: Vec<String> = std::iter::once(self.cmd.clone())
            .chain(argv.iter().cloned())
            .collect();
        self.invoke_argv(&full, timeout)
    }

    fn invoke_argv(&self, argv: &[String], timeout: Duration) -> Result<String, HerdrError> {
        if let Some(runner) = &self.runner {
            return runner(argv);
        }
        if argv.is_empty() {
            return Err(HerdrError::Io("herdr: empty argv".to_string()));
        }
        let spec = Spec {
            cmd: &argv[0],
            args: &argv[1..],
            cwd: None,
            env: &[],
            timeout,
            max_out: HERDR_MAX_OUT,
        };
        match proc::run(&spec) {
            Ok(out) => Ok(out.stdout),
            Err(ProcError::Spawn(e)) => Err(HerdrError::Io(e)),
            Err(ProcError::Timeout { after }) => Err(HerdrError::Io(format!(
                "herdr timeout after {}ms",
                after.as_millis()
            ))),
            Err(ProcError::Failed { code, stderr }) => Err(HerdrError::Failed {
                args: argv[1..].to_vec(),
                code,
                stderr,
            }),
        }
    }

    fn list_entries(&self, args: &[&str], result_key: &str) -> Result<Vec<AgentEntry>, HerdrError> {
        let out = self.invoke(args, TIMEOUT_HERDR_LIST)?;
        parse_list_output(&out, result_key)
    }

    pub fn agent_list(&self) -> Result<Vec<AgentEntry>, HerdrError> {
        self.list_entries(&["agent", "list"], "agents")
    }

    pub fn pane_list(&self) -> Result<Vec<AgentEntry>, HerdrError> {
        self.list_entries(&["pane", "list"], "panes")
    }

    /// `herdr pane split --direction right --cwd <cwd> --env K=V ...` — returns
    /// the new pane id. Tolerant of the emitted envelope shape; the exact
    /// success shape is not documented in herdr's public schema, so several
    /// plausible layouts are accepted (see UNVERIFIED in the report).
    pub fn pane_split(&self, cwd: &str, env: &[(String, String)]) -> Result<String, HerdrError> {
        let mut args: Vec<String> = vec![
            "pane".into(),
            "split".into(),
            "--direction".into(),
            "right".into(),
            "--cwd".into(),
            cwd.to_string(),
        ];
        for (k, v) in env {
            args.push("--env".into());
            args.push(format!("{k}={v}"));
        }
        let out = self.invoke_owned(&args, TIMEOUT_HERDR_MUTATE)?;
        parse_split_output(&out)
    }

    /// `herdr agent start <name> --kind opencode --pane <pane_id>`
    pub fn agent_start(&self, name: &str, pane_id: &str) -> Result<(), HerdrError> {
        self.invoke_mut(&[
            "agent", "start", name, "--kind", "opencode", "--pane", pane_id,
        ])
        .map(|_| ())
    }

    /// `herdr agent prompt <target> <text>` (no --wait; the daemon polls).
    pub fn agent_prompt(&self, target: &str, text: &str) -> Result<(), HerdrError> {
        self.invoke_mut(&["agent", "prompt", target, text])
            .map(|_| ())
    }

    /// `herdr agent read <target> --lines <n>` — raw text (or a tolerated
    /// JSON envelope carrying the text).
    pub fn agent_read(&self, target: &str, lines: usize) -> Result<String, HerdrError> {
        let out = self.invoke(
            &["agent", "read", target, "--lines", &lines.to_string()],
            TIMEOUT_HERDR_READ,
        )?;
        Ok(parse_read_output(&out))
    }

    /// `herdr agent wait <target> --until idle --until done --until blocked`
    pub fn agent_wait(&self, target: &str) -> Result<(), HerdrError> {
        self.invoke_mut(&[
            "agent", "wait", target, "--until", "idle", "--until", "done", "--until", "blocked",
        ])
        .map(|_| ())
    }

    /// `herdr pane close <pane_id>` (positional argument in herdr's CLI).
    pub fn pane_close(&self, pane_id: &str) -> Result<(), HerdrError> {
        self.invoke_mut(&["pane", "close", pane_id]).map(|_| ())
    }

    /// `herdr agent focus <target>`
    pub fn agent_focus(&self, target: &str) -> Result<(), HerdrError> {
        self.invoke_mut(&["agent", "focus", target]).map(|_| ())
    }
}

/// Unwrap herdr's `{ "id": ..., "result": { ... } }` envelope when present.
fn unwrap_result(v: &serde_json::Value) -> &serde_json::Value {
    match v.get("result") {
        Some(r) if r.is_object() || r.is_array() => r,
        _ => v,
    }
}

fn parse_list_output(out: &str, result_key: &str) -> Result<Vec<AgentEntry>, HerdrError> {
    let trimmed = out.trim();
    let v: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| HerdrError::Parse(format!("{e}")))?;
    let result = unwrap_result(&v);
    let arr = result
        .get(result_key)
        .and_then(|a| a.as_array())
        .or_else(|| v.get(result_key).and_then(|a| a.as_array()))
        .ok_or_else(|| HerdrError::Parse(format!("no `{result_key}` array in herdr output")))?;
    let mut entries = Vec::with_capacity(arr.len());
    for item in arr {
        // Unknown fields and missing optionals are tolerated; a non-object entry is skipped.
        if let Ok(entry) = serde_json::from_value::<AgentEntry>(item.clone()) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Parse `herdr agent list` output specifically (used by tests and reconcile).
pub fn parse_agent_list_output(out: &str) -> Result<Vec<AgentEntry>, HerdrError> {
    parse_list_output(out, "agents")
}

/// Parse `herdr pane list` output (kept for the pane-list telemetry path).
pub fn parse_pane_list_output(out: &str) -> Result<Vec<AgentEntry>, HerdrError> {
    parse_list_output(out, "panes")
}

fn parse_split_output(out: &str) -> Result<String, HerdrError> {
    let trimmed = out.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let result = unwrap_result(&v);
        let candidates: Vec<&serde_json::Value> = vec![
            result.pointer("/pane/pane_id"),
            result.pointer("/pane_id"),
            result.pointer("/root_pane/pane_id"),
            result.pointer("/panes/0/pane_id"),
            result.pointer("/created_pane/pane_id"),
        ]
        .into_iter()
        .flatten()
        .collect();
        for c in candidates {
            if let Some(s) = c.as_str() {
                if !s.is_empty() {
                    return Ok(s.to_string());
                }
            }
        }
        if let Some(s) = result.as_str() {
            if looks_like_pane_id(s) {
                return Ok(s.to_string());
            }
        }
        return Err(HerdrError::Parse(format!(
            "no pane id found in herdr pane split output: {trimmed}"
        )));
    }
    // Raw pane id on stdout (no envelope)
    if looks_like_pane_id(trimmed) {
        return Ok(trimmed.to_string());
    }
    Err(HerdrError::Parse(format!(
        "herdr pane split output was not parseable JSON and not a pane id: {trimmed}"
    )))
}

fn looks_like_pane_id(s: &str) -> bool {
    // herdr pane ids look like "w80:p1"
    let mut parts = s.split(':');
    let (Some(a), Some(b)) = (parts.next(), parts.next()) else {
        return false;
    };
    parts.next().is_none()
        && a.len() >= 2
        && a.starts_with('w')
        && a[1..].chars().all(|c| c.is_ascii_alphanumeric())
        && b.starts_with('p')
        && b[1..].chars().all(|c| c.is_ascii_digit())
}

fn parse_read_output(out: &str) -> String {
    let trimmed = out.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let result = unwrap_result(&v);
        for ptr in ["/text", "/read/text", "/output", "/content"] {
            if let Some(s) = result.pointer(ptr).and_then(|s| s.as_str()) {
                return s.to_string();
            }
        }
    }
    trimmed.to_string()
}

// ---------- private event socket (herdr protocol v22) ----------
//
// Facts are from `herdr api schema --json` (protocol 22, schema_version 1):
// `events.subscribe` takes `params.subscriptions[]`; a
// `pane.agent_status_changed` subscription REQUIRES `pane_id`; the pushed
// envelope is `{"event":"pane.agent_status_changed","data":{pane_id,
// workspace_id, agent_status, ...}}`. Each request uses a NEW connection —
// a second write on the same socket is not a request. No SOCKET_API.md exists
// on this host, so the bundled schema is the authoritative source.

static SUB_SEQ: AtomicU64 = AtomicU64::new(1);
const SOCKET_READ_TIMEOUT: Duration = Duration::from_millis(500);

/// Path of herdr's private JSON-line socket. `NOTCH_HERDR_SOCKET` overrides
/// the default (`$XDG_CONFIG_HOME/herdr/herdr.sock` or `$HOME/.config/...`).
pub fn herdr_socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("NOTCH_HERDR_SOCKET") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("herdr/herdr.sock");
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/herdr/herdr.sock")
}

/// One pushed `pane.agent_status_changed` event (schema required fields).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStatusEvent {
    pub pane_id: String,
    pub workspace_id: String,
    pub agent_status: String,
}

/// Parse one JSON line of herdr's subscription-event stream. Lines that are
/// not a status-change envelope are ignored — never a fatal error.
pub fn parse_subscription_event(line: &str) -> Option<AgentStatusEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("event").and_then(Value::as_str) != Some("pane.agent_status_changed") {
        return None;
    }
    let data = v.get("data")?;
    Some(AgentStatusEvent {
        pane_id: data.get("pane_id")?.as_str()?.to_string(),
        workspace_id: data
            .get("workspace_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        agent_status: data.get("agent_status")?.as_str()?.to_string(),
    })
}

/// Subscribe to agent-status changes for ONE pane. A fresh connection is
/// opened for this request, the `subscription_started` ack is verified, and a
/// reader thread forwards parsed events to the returned receiver until the
/// socket closes or `shutdown` is set. Only `pane_id` from our own state is
/// ever subscribed — no wildcard, no pane we do not manage.
pub fn subscribe_agent_status_at(
    socket: &Path,
    pane_id: &str,
    shutdown: &Arc<AtomicBool>,
) -> Result<Receiver<AgentStatusEvent>, HerdrError> {
    let stream = UnixStream::connect(socket)
        .map_err(|e| HerdrError::Io(format!("herdr socket {}: {e}", socket.display())))?;
    stream
        .set_read_timeout(Some(SOCKET_READ_TIMEOUT))
        .map_err(|e| HerdrError::Io(format!("herdr socket timeout: {e}")))?;
    let id = format!("notch-{}", SUB_SEQ.fetch_add(1, Ordering::SeqCst));
    let request = json!({
        "id": id,
        "method": "events.subscribe",
        "params": {"subscriptions": [
            {"type": "pane.agent_status_changed", "pane_id": pane_id}
        ]}
    });
    let mut reader = BufReader::new(stream);
    writeln!(reader.get_mut(), "{request}")
        .map_err(|e| HerdrError::Io(format!("herdr subscribe write: {e}")))?;
    reader
        .get_mut()
        .flush()
        .map_err(|e| HerdrError::Io(format!("herdr subscribe flush: {e}")))?;
    let mut ack = String::new();
    let n = reader
        .read_line(&mut ack)
        .map_err(|e| HerdrError::Io(format!("herdr subscribe ack: {e}")))?;
    if n == 0 {
        return Err(HerdrError::Parse(
            "herdr subscribe closed before ack".into(),
        ));
    }
    let ack_value: Value = serde_json::from_str(ack.trim())
        .map_err(|e| HerdrError::Parse(format!("herdr subscribe ack JSON: {e}")))?;
    if ack_value
        .get("result")
        .and_then(|r| r.get("type"))
        .and_then(Value::as_str)
        != Some("subscription_started")
    {
        return Err(HerdrError::Parse(format!(
            "herdr subscribe rejected: {}",
            ack.trim()
        )));
    }
    let (tx, rx) = mpsc::channel();
    let shutdown = Arc::clone(shutdown);
    std::thread::spawn(move || loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if let Some(event) = parse_subscription_event(&line) {
                    if tx.send(event).is_err() {
                        break;
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(_) => break,
        }
    });
    Ok(rx)
}
