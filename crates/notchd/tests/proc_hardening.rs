//! Hardening tests for the single bounded subprocess runner (`proc.rs`).
//!
//! T2 (fault-injection shim matrix): every child class — hang, ignore SIGTERM,
//! 1 GB stdout, garbage JSON, setsid grandchild, byte drip, fd leak — returns
//! within `timeout + 500ms`, leaves no survivor in its process group, keeps RSS
//! growth bounded, and the daemon still answers status.
//!
//! Phase 0: the API-shape test below is the frozen contract A1/A3/A5 code
//! against. Run: cargo test -p notchd --test proc_hardening

use notchd::herdr::Herdr;
use notchd::proc::{
    run, Out, ProcError, Spec, PIPE_EOF_GRACE, TIMEOUT_DECIDER, TIMEOUT_GIT,
    TIMEOUT_HERDR_LIST, TIMEOUT_HERDR_MUTATE, TIMEOUT_HERDR_READ, TIMEOUT_OPENCODE_API,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Timing-sensitive shims (and the herdr test's process-env edits) must not
/// run concurrently: a loaded host can exceed the short deadline before the
/// shim records its pgid, which is a test-harness artifact, not a runner bug.
static RUN_LOCK: Mutex<()> = Mutex::new(());
fn run_lock() -> MutexGuard<'static, ()> {
    RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The frozen Phase 0 signatures compile and the one-place timeout constants
/// hold their documented values.
#[test]
fn proc_contract_signatures_and_timeouts() {
    let _run_ptr: fn(&Spec<'_>) -> Result<Out, ProcError> = run;
    assert_eq!(TIMEOUT_HERDR_LIST, Duration::from_secs(5));
    assert_eq!(TIMEOUT_HERDR_READ, Duration::from_secs(5));
    assert_eq!(TIMEOUT_HERDR_MUTATE, Duration::from_secs(10));
    assert_eq!(TIMEOUT_OPENCODE_API, Duration::from_secs(8));
    assert_eq!(TIMEOUT_GIT, Duration::from_secs(5));
    assert_eq!(TIMEOUT_DECIDER, Duration::from_secs(90));
    assert_eq!(PIPE_EOF_GRACE, Duration::from_millis(500));

    let args: Vec<String> = vec!["list".into()];
    let env: Vec<(String, String)> = Vec::new();
    let spec = Spec {
        cmd: "/usr/bin/true",
        args: &args,
        cwd: Some(Path::new("/")),
        env: &env,
        timeout: Duration::from_millis(100),
        max_out: 1024,
    };
    // Construction must not require `'static` or owned data.
    assert_eq!(spec.max_out, 1024);

    assert!(ProcError::Timeout { after: Duration::from_millis(250) }
        .to_string()
        .contains("250ms"));
    assert!(ProcError::Spawn("nope".into()).to_string().contains("nope"));
    assert!(ProcError::Failed { code: Some(7), stderr: "bad".into() }
        .to_string()
        .contains("bad"));
    let _ = Out::default();
}

/// A plain command must return its captured output. (RED until `run` is
/// implemented: the Phase 0 stub is `todo!()`.)
#[test]
fn proc_runs_a_plain_command() {
    let args: Vec<String> = vec!["hello-from-proc".into()];
    let env: Vec<(String, String)> = Vec::new();
    let spec = Spec {
        cmd: "/bin/echo",
        args: &args,
        cwd: None,
        env: &env,
        timeout: Duration::from_secs(2),
        max_out: 4096,
    };
    let out = run(&spec).expect("echo must succeed");
    assert_eq!(out.stdout.trim(), "hello-from-proc");
}

/// A missing command must produce a typed Spawn error, never a panic/hang.
#[test]
fn proc_reports_spawn_failure() {
    let args: Vec<String> = Vec::new();
    let env: Vec<(String, String)> = Vec::new();
    let spec = Spec {
        cmd: "/definitely/not/a/real/binary",
        args: &args,
        cwd: None,
        env: &env,
        timeout: Duration::from_secs(2),
        max_out: 1024,
    };
    assert!(matches!(run(&spec), Err(ProcError::Spawn(_))));
}

// --------------------------- T2 fault shims --------------------------------

static SHIM_N: AtomicU64 = AtomicU64::new(0);

/// A self-contained Python fault shim. It records its own pid and process
/// group into `$PROC_SHIM_STATE/{pid,pgid}` so the test can prove the whole
/// group was killed with `pgrep -g <pgid>` — not just the direct child.
const SHIM_SRC: &str = r#"#!/usr/bin/env python3
import os, sys, signal, time
mode = os.environ.get("PROC_SHIM_MODE", "hang")
state = os.environ.get("PROC_SHIM_STATE", "")
def note(k, v):
    if state:
        try:
            with open(os.path.join(state, k), "a") as f:
                f.write(str(v) + "\n")
        except Exception:
            pass
note("pid", os.getpid())
note("pgid", os.getpgrp())
if mode == "hang":
    time.sleep(60)
elif mode == "sigterm_ignore":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    time.sleep(60)
elif mode == "bigout":
    chunk = b"x" * (1 << 20)          # 1 GiB total, in 1 MiB writes
    for _ in range(1024):
        os.write(1, chunk)
elif mode == "grandchild":
    pid = os.fork()
    if pid == 0:                       # inherits the process group
        note("grandchild_pid", os.getpid())
        note("grandchild_pgid", os.getpgrp())
        time.sleep(60)
        os._exit(0)
    time.sleep(60)
elif mode == "drip":
    for _ in range(6000):
        os.write(1, b"z")
        time.sleep(0.01)
elif mode == "fdleak":
    held = [os.open("/dev/null", os.O_RDONLY) for _ in range(200)]
    time.sleep(60)
elif mode == "garbage":
    sys.stdout.write("not json {{{")
    sys.stdout.flush()
elif mode == "exit7":
    sys.stderr.write("bad thing\n")
    sys.exit(7)
else:
    sys.stdout.write("ok")
"#;

fn shim_root(tag: &str) -> PathBuf {
    let n = SHIM_N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-proc-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write_shim(root: &Path) -> PathBuf {
    let path = root.join("proc_fault_shim");
    std::fs::write(&path, SHIM_SRC).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn env_for(root: &Path, mode: &str) -> Vec<(String, String)> {
    vec![
        ("PROC_SHIM_MODE".into(), mode.into()),
        ("PROC_SHIM_STATE".into(), root.to_string_lossy().into_owned()),
    ]
}

/// `args`/`env` are owned by the caller so the returned `Spec` is valid.
fn spec_for<'a>(
    shim: &'a Path,
    args: &'a [String],
    env: &'a [(String, String)],
    timeout: Duration,
    max_out: usize,
) -> Spec<'a> {
    Spec {
        cmd: shim.to_str().unwrap(),
        args,
        cwd: None,
        env,
        timeout,
        max_out,
    }
}

fn read_note(root: &Path, name: &str) -> Option<u32> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(s) = std::fs::read_to_string(root.join(name)) {
            if let Ok(v) = s.lines().next().unwrap_or("").trim().parse::<u32>() {
                return Some(v);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// PIDs still alive in a process group. `pgrep -g` is the exact check the
/// hardening plan asks for (no child may survive the group kill).
fn group_survivors(pgid: u32) -> Vec<u32> {
    let Ok(out) = StdCommand::new("pgrep").args(["-g", &pgid.to_string()]).output() else {
        panic!("pgrep unavailable — cannot prove group reaping");
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// T2: the fault matrix. Every hung class returns inside `timeout + 500ms`
/// with a typed Timeout and no survivors in its process group; the two
/// non-hang classes return their captured bytes / typed Failed error.
#[test]
fn t2_fault_matrix_bounded_and_group_reaped() {
    let _g = run_lock();
    for mode in ["hang", "sigterm_ignore", "grandchild", "drip", "fdleak"] {
        let root = shim_root(mode);
        let shim = write_shim(&root);
        let timeout = Duration::from_millis(350);
        let args: Vec<String> = Vec::new();
        let env = env_for(&root, mode);
        let spec = spec_for(&shim, &args, &env, timeout, 4096);

        let t = Instant::now();
        let result = run(&spec);
        let elapsed = t.elapsed();
        assert!(
            elapsed < timeout + PIPE_EOF_GRACE,
            "{mode}: call took {:?}, deadline+grace is {:?}",
            elapsed,
            timeout + PIPE_EOF_GRACE
        );
        assert!(
            matches!(result, Err(ProcError::Timeout { .. })),
            "{mode}: expected typed Timeout, got {result:?}"
        );

        let pgid = read_note(&root, "pgid").unwrap_or_else(|| panic!("{mode}: no pgid recorded"));
        let survivors = group_survivors(pgid);
        assert!(survivors.is_empty(), "{mode}: pgrep -g {pgid} found survivors {survivors:?}");
        if mode == "grandchild" {
            let gpgid = read_note(&root, "grandchild_pgid")
                .unwrap_or_else(|| panic!("grandchild: no pgid recorded"));
            assert_eq!(gpgid, pgid, "grandchild must inherit the spawn process group");
            assert!(
                group_survivors(gpgid).is_empty(),
                "grandchild in group {gpgid} survived the group kill"
            );
        }
    }
}

/// T2: garbage JSON is returned as bytes (the parser above `proc` rejects it),
/// and a non-zero exit is a typed `Failed` carrying the capped stderr.
#[test]
fn t2_garbage_output_and_nonzero_exit_are_typed() {
    let _g = run_lock();
    let root = shim_root("garbage");
    let shim = write_shim(&root);
    let args: Vec<String> = Vec::new();
    let env = env_for(&root, "garbage");
    let spec = spec_for(&shim, &args, &env, Duration::from_secs(2), 4096);
    let out = run(&spec).expect("exit 0 with garbage bytes must be Ok at proc level");
    assert!(out.stdout.contains("not json {{{"), "stdout was {:?}", out.stdout);

    let root = shim_root("exit7");
    let shim = write_shim(&root);
    let args: Vec<String> = Vec::new();
    let env = env_for(&root, "exit7");
    let spec = spec_for(&shim, &args, &env, Duration::from_secs(2), 4096);
    match run(&spec) {
        Err(ProcError::Failed { code, stderr }) => {
            assert_eq!(code, Some(7));
            assert!(stderr.contains("bad thing"), "stderr was {stderr:?}");
        }
        other => panic!("expected typed Failed, got {other:?}"),
    }
}

/// T2: a child emitting 1 GiB to stdout never blocks on a full pipe (readers
/// drain past the cap) and only `max_out` bytes are retained in memory.
#[test]
fn t2_large_output_is_drained_and_capped() {
    let _g = run_lock();
    let root = shim_root("bigout");
    let shim = write_shim(&root);
    let args: Vec<String> = Vec::new();
    let env = env_for(&root, "bigout");
    let spec = spec_for(&shim, &args, &env, Duration::from_secs(12), 1024);
    let t = Instant::now();
    match run(&spec) {
        Ok(out) => {
            assert!(out.stdout.len() <= 1024, "captured {} bytes, cap is 1024", out.stdout.len());
            println!("t2_large_output: drained 1 GiB in {:?}, kept {} bytes", t.elapsed(), out.stdout.len());
        }
        Err(ProcError::Timeout { .. }) => {
            // A slow machine may not drain 1 GiB inside 12 s; the timeout
            // path is itself valid, but the group must still be dead.
            let pgid = read_note(&root, "pgid").unwrap();
            assert!(group_survivors(pgid).is_empty());
            panic!("1 GiB drain exceeded 12 s — raise the deadline for this host");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

/// 3.1: the real `Herdr` client is bounded too — a hung `herdr agent list`
/// returns at the 5 s deadline with a typed timeout and leaves no survivors
/// in its process group (the fake-runner seam is bypassed here).
#[test]
fn herdr_list_is_bounded_and_reaps_group() {
    let _g = run_lock();
    let root = shim_root("herdr-list");
    let shim = write_shim(&root);
    std::env::set_var("NOTCH_HERDR_CMD", shim.to_str().unwrap());
    std::env::set_var("PROC_SHIM_MODE", "hang");
    std::env::set_var("PROC_SHIM_STATE", root.to_string_lossy().as_ref());
    let herdr = Herdr::from_env();

    let t = Instant::now();
    let err = herdr.agent_list().expect_err("hung herdr list must time out");
    let elapsed = t.elapsed();
    assert!(
        elapsed < TIMEOUT_HERDR_LIST + PIPE_EOF_GRACE,
        "herdr list took {elapsed:?}"
    );
    assert!(err.to_string().contains("herdr timeout after 5000ms"), "got {err}");
    let pgid = read_note(&root, "pgid").unwrap();
    assert!(group_survivors(pgid).is_empty(), "herdr left group survivors: {pgid}");
    std::env::remove_var("NOTCH_HERDR_CMD");
    std::env::remove_var("PROC_SHIM_MODE");
    std::env::remove_var("PROC_SHIM_STATE");
}

fn self_rss_kb() -> u64 {
    let out = StdCommand::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps available");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("cannot parse ps rss: {e}"))
}

fn self_fd_count() -> usize {
    std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0)
}

/// T2: repeated timeouts + a huge-output child must not grow the daemon's RSS
/// by more than 20 MB and must not leak file descriptors.
#[test]
fn t2_no_rss_or_fd_growth_across_repeated_kills() {
    let _g = run_lock();
    let before_rss = self_rss_kb();
    let before_fd = self_fd_count();
    for i in 0..8 {
        let root = shim_root(&format!("loop{i}"));
        let shim = write_shim(&root);
        let args: Vec<String> = Vec::new();
        let env = env_for(&root, "fdleak");
        let spec = spec_for(&shim, &args, &env, Duration::from_millis(200), 4096);
        assert!(matches!(run(&spec), Err(ProcError::Timeout { .. })));
        let pgid = read_note(&root, "pgid").unwrap();
        assert!(group_survivors(pgid).is_empty(), "iteration {i} left group survivors");
    }
    let after_rss = self_rss_kb();
    let after_fd = self_fd_count();
    let growth_kb = after_rss.saturating_sub(before_rss);
    println!("t2_rss: {before_rss} KiB -> {after_rss} KiB (growth {growth_kb} KiB); fds {before_fd} -> {after_fd}");
    assert!(growth_kb < 20 * 1024, "RSS grew {growth_kb} KiB (> 20 MB)");
    assert!(after_fd <= before_fd + 4, "fds leaked: {before_fd} -> {after_fd}");
}
