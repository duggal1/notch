//! The ONLY place notchd spawns child processes.
//!
//! Every external CLI call (herdr, opencode, git, the pre-spawn hook, the
//! decider) goes through [`run`]. A hung child can never wedge the poll or
//! watchdog thread: the call is bounded by a hard wall-clock deadline, the
//! whole process group is killed and reaped on expiry, and stdout/stderr are
//! drained on helper threads past the capture cap so a chatty child never
//! blocks on a full pipe.
//!
//! Phase 0 contract (docs/HARDENING.md §2). The signatures and timeout
//! constants here are frozen; other agents code against them.

use std::fmt;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Hard deadline for `herdr agent list` / `herdr pane list` / `herdr agent read`.
pub const TIMEOUT_HERDR_LIST: Duration = Duration::from_secs(5);
/// Hard deadline for the same read surfaces when used via `agent_read`.
pub const TIMEOUT_HERDR_READ: Duration = Duration::from_secs(5);
/// Hard deadline for mutating herdr calls (split/start/prompt/wait/close/focus).
pub const TIMEOUT_HERDR_MUTATE: Duration = Duration::from_secs(10);
/// Hard deadline for `opencode api` (the activity probe).
pub const TIMEOUT_OPENCODE_API: Duration = Duration::from_secs(8);
/// Hard deadline for `git status --porcelain` attribution sampling.
pub const TIMEOUT_GIT: Duration = Duration::from_secs(5);
/// Hard deadline for one decider (`opencode run`) judgment.
pub const TIMEOUT_DECIDER: Duration = Duration::from_secs(90);
/// Grace allowed for pipe EOF after the child has exited.
pub const PIPE_EOF_GRACE: Duration = Duration::from_millis(500);

/// Everything needed to spawn one bounded child process.
pub struct Spec<'a> {
    pub cmd: &'a str,
    pub args: &'a [String],
    pub cwd: Option<&'a Path>,
    pub env: &'a [(String, String)],
    pub timeout: Duration,
    /// Maximum bytes captured per stream. Readers keep draining beyond this
    /// cap so the child never blocks; only the first `max_out` bytes are kept.
    pub max_out: usize,
}

/// Typed failure modes of a bounded process call.
#[derive(Debug)]
pub enum ProcError {
    /// The child could not be spawned (bad command, permission, …).
    Spawn(String),
    /// The deadline expired; the group was killed and the child reaped.
    Timeout { after: Duration },
    /// The child ran but exited non-zero. `stderr` is the (capped) tail.
    Failed { code: Option<i32>, stderr: String },
}

impl fmt::Display for ProcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcError::Spawn(e) => write!(f, "spawn failed: {e}"),
            ProcError::Timeout { after } => {
                write!(f, "timeout after {}ms", after.as_millis())
            }
            ProcError::Failed { code, stderr } => write!(
                f,
                "failed (exit {}): {stderr}",
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            ),
        }
    }
}

impl std::error::Error for ProcError {}

/// Captured output of a successful bounded process call.
#[derive(Debug, Default)]
pub struct Out {
    pub stdout: String,
    pub stderr: String,
}

/// Read a pipe to EOF while retaining at most `cap` bytes.
///
/// The loop deliberately keeps reading after the cap is reached: a child
/// writing 1 GiB to stdout must never block on a full pipe, and the daemon
/// must never hold more than `cap + READ_CHUNK` bytes per stream.
fn drain_capped<R: Read>(mut pipe: R, cap: usize) -> Vec<u8> {
    const READ_CHUNK: usize = 16 * 1024;
    let mut kept = Vec::with_capacity(cap.min(READ_CHUNK));
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if kept.len() < cap {
                    let take = n.min(cap - kept.len());
                    kept.extend_from_slice(&buf[..take]);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    kept
}

/// Kill the entire process group the child was spawned into. `process_group(0)`
/// makes the child's pid its pgid, so descendants that do not deliberately
/// call setsid die too (opencode spawns children).
fn kill_group(pid: u32) {
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Wait up to `PIPE_EOF_GRACE` for a reader thread to deliver its buffer.
/// A detached descendant holding the pipe cannot stall the caller past this.
fn take_pipe(rx: mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    rx.recv_timeout(PIPE_EOF_GRACE).unwrap_or_default()
}

/// Normal-exit variant: once the child is reaped its pipes are at EOF, so a
/// reader thread that has not delivered inside the grace is merely descheduled
/// (seen under heavy parallel test load). Wait for it to finish with a bounded
/// safety cap instead of returning empty output for a successful call.
fn take_pipe_after_exit(rx: mpsc::Receiver<Vec<u8>>, handle: thread::JoinHandle<()>) -> Vec<u8> {
    match rx.recv_timeout(PIPE_EOF_GRACE) {
        Ok(buf) => {
            let _ = handle.join();
            buf
        }
        Err(_) => {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !handle.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
            rx.recv_timeout(Duration::from_millis(100))
                .unwrap_or_default()
        }
    }
}

/// Run one child under a hard deadline. See module docs.
pub fn run(spec: &Spec) -> Result<Out, ProcError> {
    let mut command = Command::new(spec.cmd);
    command
        .args(spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // New process group == child pid. killpg() below reaches grandchildren.
        .process_group(0);
    if let Some(cwd) = spec.cwd {
        command.current_dir(cwd);
    }
    if !spec.env.is_empty() {
        command.envs(spec.env.iter().cloned());
    }
    // Do NOT clear the environment: herdr/opencode need PATH, HOME, proxy vars.

    let mut child = command
        .spawn()
        .map_err(|e| ProcError::Spawn(format!("{}: {e}", spec.cmd)))?;
    let pid = child.id();

    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| ProcError::Spawn(format!("{}: no stdout pipe", spec.cmd)))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| ProcError::Spawn(format!("{}: no stderr pipe", spec.cmd)))?;

    let cap = spec.max_out;
    let (tx_out, rx_out) = mpsc::channel();
    let (tx_err, rx_err) = mpsc::channel();
    // Detached: after the group is killed every pipe reaches EOF and the
    // thread exits on its own. No thread is joined on a blocking read.
    let out_handle = thread::spawn(move || {
        let _ = tx_out.send(drain_capped(stdout_pipe, cap));
    });
    let err_handle = thread::spawn(move || {
        let _ = tx_err.send(drain_capped(stderr_pipe, cap));
    });

    let deadline = Instant::now() + spec.timeout;
    let mut wait_error = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                wait_error = Some(e);
                break None;
            }
        }
        if Instant::now() >= deadline {
            kill_group(pid);
            let _ = child.wait(); // reap: no zombie left behind
            let _ = take_pipe(rx_out);
            let _ = take_pipe(rx_err);
            return Err(ProcError::Timeout {
                after: spec.timeout,
            });
        }
        // Small, bounded poll interval: try_wait is cheap and never wedges.
        thread::sleep(Duration::from_millis(2));
    };

    let status = match status {
        Some(status) => status,
        None => {
            kill_group(pid);
            let _ = child.wait();
            let _ = take_pipe(rx_out);
            let _ = take_pipe(rx_err);
            return Err(ProcError::Failed {
                code: None,
                stderr: format!(
                    "try_wait failed: {}",
                    wait_error
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "unknown".into())
                ),
            });
        }
    };

    // Bounded grace for pipe EOF. If a detached descendant still holds a
    // pipe, we return with an empty capture instead of blocking forever.
    let stdout_bytes = take_pipe_after_exit(rx_out, out_handle);
    let stderr_bytes = take_pipe_after_exit(rx_err, err_handle);
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

    if !status.success() {
        return Err(ProcError::Failed {
            code: status.code(),
            stderr,
        });
    }
    Ok(Out { stdout, stderr })
}
