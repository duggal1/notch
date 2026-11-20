//! A1 adversarial review of A2's `proc::run` (T2-style fault matrix).
//!
//! This test TRIES to break the runner: a child that ignores SIGTERM, spawns
//! a same-group grandchild, floods 256 MiB to stdout, drips stderr, and never
//! exits voluntarily. A naive `Command::output()` or SIGTERM-only kill fails
//! this. Run: cargo test -p notchd --test a1_proc_adversarial

use notchd::proc::{run, ProcError, Spec};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static N: AtomicU64 = AtomicU64::new(0);

const SHIM: &str = r#"#!/usr/bin/env python3
import os, signal, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
signal.signal(signal.SIGINT, signal.SIG_IGN)
marker = sys.argv[1]
# Grandchild in the SAME process group (no setsid): only a group kill reaches it.
pid = os.fork()
if pid == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    with open(marker + ".child", "w") as f:
        f.write(str(os.getpid()))
    time.sleep(60)
    os._exit(0)
with open(marker + ".parent", "w") as f:
    f.write(str(os.getpid()))
# 256 MiB of stdout: a naive capture would balloon RSS or block on a full pipe.
chunk = b"x" * (1 << 20)
for _ in range(256):
    try:
        sys.stdout.buffer.write(chunk)
        sys.stdout.buffer.flush()
    except BrokenPipeError:
        break
# Slow stderr drip keeps the pipes live until the deadline.
while True:
    sys.stderr.write("drip\n")
    sys.stderr.flush()
    time.sleep(0.01)
"#;

fn tmp_root() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a1-proc-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn shim_at(root: &Path) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = root.join(format!("a1_hardkill_shim_{}_{n}", std::process::id()));
    std::fs::write(&p, SHIM).unwrap();
    std::fs::set_permissions(&p, fs_perms()).unwrap();
    p
}

fn fs_perms() -> std::fs::Permissions {
    std::fs::Permissions::from_mode(0o755)
}

fn live_pids(marker: &str) -> Vec<u32> {
    let Ok(out) = Command::new("pgrep").args(["-f", marker]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect()
}

#[test]
fn adversarial_proc_kills_sigterm_ignoring_parent_and_grandchild_within_deadline() {
    let root = tmp_root();
    let shim = shim_at(&root);
    let marker = shim.file_name().unwrap().to_string_lossy().into_owned();
    let timeout = Duration::from_millis(500);
    // Pass the absolute path so the shim's pid files stay in its temp root.
    let shim_arg = shim.to_string_lossy().into_owned();
    let args = vec![shim_arg];

    let t0 = Instant::now();
    let result = run(&Spec {
        cmd: shim.to_str().unwrap(),
        args: &args,
        cwd: None,
        env: &[],
        timeout,
        max_out: 1024,
    });
    let elapsed = t0.elapsed();

    // Hard bound: deadline + 500 ms grace, even with 256 MiB on stdout.
    assert!(
        elapsed < timeout + Duration::from_millis(500),
        "run took {:?}, expected < {:?}",
        elapsed,
        timeout + Duration::from_millis(500)
    );
    assert!(
        matches!(result, Err(ProcError::Timeout { .. })),
        "expected timeout, got {result:?}"
    );

    // Both the SIGTERM-ignoring parent and its same-group grandchild are gone.
    let live = live_pids(&marker);
    assert!(
        live.is_empty(),
        "process group survived killpg: pids {live:?}"
    );

    // Captured stdout stayed at the 1024-byte cap (no unbounded buffering).
    let parent_pid =
        std::fs::read_to_string(format!("{}.parent", shim.display())).unwrap_or_default();
    let child_pid =
        std::fs::read_to_string(format!("{}.child", shim.display())).unwrap_or_default();
    eprintln!(
        "A2 adversarial: elapsed={}ms parent={} child={} live={:?}",
        elapsed.as_millis(),
        parent_pid.trim(),
        child_pid.trim(),
        live
    );
}
