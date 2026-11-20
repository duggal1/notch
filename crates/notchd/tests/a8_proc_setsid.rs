//! A8 process test — DOCUMENTS A KNOWN LIMITATION, does not fix it.
//!
//! `proc::run` kills the child's process group on timeout. A grandchild that
//! calls `setsid()` escapes into a NEW process group, so `killpg(parent_pgid)`
//! cannot reach it. This test demonstrates that escape with real processes and
//! real signals, then reaps the escaped grandchild itself so no orphan is left.
//! Per SOLUTION_INSTRUCTION §7 we do NOT build a process supervisor for this.
//!
//! Run: cargo test -p notchd --test a8_proc_setsid

use notchd::proc::{run, ProcError, Spec, PIPE_EOF_GRACE};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static N: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a8setsid-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Child forks a grandchild that calls `setsid()` (new session + new process
/// group). The parent waits for the grandchild to record its pgid, then both
/// sleep. Files are the synchronization channel; no test-side sleep is used.
const SHIM: &str = r#"#!/usr/bin/env python3
import os, sys, time
state = sys.argv[1]
def note(k, v):
    with open(os.path.join(state, k), "w") as f:
        f.write(str(v))
note("parent_pid", os.getpid())
note("parent_pgid", os.getpgrp())
pid = os.fork()
if pid == 0:
    os.setsid()                      # escapes into a new process group
    note("grandchild_pid", os.getpid())
    note("grandchild_pgid", os.getpgrp())
    time.sleep(60)
    os._exit(0)
# parent waits until the grandchild has recorded its escaped pgid
deadline = time.monotonic() + 5
while not os.path.exists(os.path.join(state, "grandchild_pgid")):
    if time.monotonic() > deadline:
        break
    time.sleep(0.01)
time.sleep(60)
"#;

fn read_u32(p: &Path, name: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(s) = std::fs::read_to_string(p.join(name)) {
            if let Ok(v) = s.trim().parse::<u32>() {
                return v;
            }
        }
        assert!(Instant::now() < deadline, "no {name} recorded");
        std::thread::yield_now();
    }
}

fn survivors(pgid: u32) -> Vec<u32> {
    let out = StdCommand::new("pgrep")
        .args(["-g", &pgid.to_string()])
        .output()
        .expect("pgrep available");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn kill_pid(pid: u32) {
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Guard: always reap the escaped grandchild, even on assertion failure.
struct Reap(u32);
impl Drop for Reap {
    fn drop(&mut self) {
        kill_pid(self.0);
    }
}

#[test]
fn setsid_grandchild_escapes_group_kill_limitation_documented() {
    let r = root();
    let shim = r.join("setsid_shim");
    std::fs::write(&shim, SHIM).unwrap();
    std::fs::set_permissions(&shim, PermissionsExt::from_mode(0o755)).unwrap();

    let args: Vec<String> = vec![r.to_string_lossy().into_owned()];
    let env: Vec<(String, String)> = Vec::new();
    let spec = Spec {
        cmd: shim.to_str().unwrap(),
        args: &args,
        cwd: None,
        env: &env,
        timeout: Duration::from_secs(2),
        max_out: 1024,
    };
    let t = Instant::now();
    let result = run(&spec);
    let elapsed = t.elapsed();
    assert!(
        matches!(result, Err(ProcError::Timeout { .. })),
        "expected typed Timeout, got {result:?}"
    );
    // The escaped grandchild also inherits stdout/stderr, so BOTH pipe readers
    // wait their full PIPE_EOF_GRACE: the bound is timeout + 2x grace, not
    // timeout + 1x. That extra latency is part of the documented limitation.
    assert!(
        elapsed < spec.timeout + 2 * PIPE_EOF_GRACE + Duration::from_millis(500),
        "run took {elapsed:?}"
    );

    let parent_pgid = read_u32(&r, "parent_pgid");
    let gpid = read_u32(&r, "grandchild_pid");
    let grandchild_pgid = read_u32(&r, "grandchild_pgid");
    let _reap = Reap(gpid);

    // The child's own group was killed and reaped by proc::run.
    let parent_survivors = survivors(parent_pgid);
    assert!(
        parent_survivors.is_empty(),
        "proc::run must reap its own group: {parent_survivors:?}"
    );

    // setsid put the grandchild in a DIFFERENT group; killpg(parent) missed it.
    // This is the current limitation, documented by this test — not fixed.
    assert_ne!(
        grandchild_pgid, parent_pgid,
        "setsid must create a new process group"
    );
    let escaped = survivors(grandchild_pgid);
    assert!(
        escaped.contains(&gpid),
        "documented limitation: setsid grandchild {gpid} escaped killpg({parent_pgid}); survivors={escaped:?}"
    );
    eprintln!(
        "SETSID LIMITATION DOCUMENTED: parent pgid={parent_pgid} reaped; grandchild pid={gpid} pgid={grandchild_pgid} survived killpg; reaped by test"
    );
}
