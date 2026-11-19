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
use std::path::Path;
use std::time::Duration;

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

/// Run one child under a hard deadline. See module docs.
pub fn run(_spec: &Spec) -> Result<Out, ProcError> {
    todo!("proc::run implementation lands in section 3.1")
}
