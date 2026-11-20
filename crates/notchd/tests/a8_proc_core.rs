//! A8 proc::run REAL tests — re-established after A11 removed proc_hardening.rs
//! upstream. Real child processes, real signals, real pipes; no fake process.
//!
//! - hung child is killed at the deadline; its whole process group is reaped.
//! - a pipe flood far past max_out drains without deadlock; only max_out bytes
//!   are retained.
//!
//! Run: cargo test -p notchd --test a8_proc_core

use notchd::proc::{run, ProcError, Spec, PIPE_EOF_GRACE};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static N: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a8proccore-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

const SHIM: &str = r#"#!/usr/bin/env python3
import os, sys, time
state, mode = sys.argv[1], sys.argv[2]
with open(os.path.join(state, "pgid"), "w") as f:
    f.write(str(os.getpgrp()))
if mode == "hang":
    time.sleep(60)
else:
    chunk = b"x" * (1 << 16)
    for _ in range(128):          # 8 MiB, far past the 1024-byte cap
        os.write(1, chunk)
"#;

fn write_shim(r: &Path) -> PathBuf {
    let p = r.join("shim");
    std::fs::write(&p, SHIM).unwrap();
    std::fs::set_permissions(&p, PermissionsExt::from_mode(0o755)).unwrap();
    p
}

fn spec<'a>(shim: &'a Path, args: &'a [String], timeout: Duration, max_out: usize) -> Spec<'a> {
    Spec {
        cmd: shim.to_str().unwrap(),
        args,
        cwd: None,
        env: &[],
        timeout,
        max_out,
    }
}

fn read_pgid(r: &Path) -> Option<u32> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(s) = std::fs::read_to_string(r.join("pgid")) {
            if let Ok(v) = s.trim().parse() {
                return Some(v);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
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

#[test]
fn hung_child_killed_at_deadline_and_group_reaped() {
    let r = root();
    let shim = write_shim(&r);
    let args: Vec<String> = vec![r.to_string_lossy().into_owned(), "hang".into()];
    let timeout = Duration::from_millis(800);
    let t = Instant::now();
    let result = run(&spec(&shim, &args, timeout, 1024));
    let elapsed = t.elapsed();
    assert!(
        matches!(result, Err(ProcError::Timeout { .. })),
        "expected Timeout, got {result:?}"
    );
    assert!(
        elapsed < timeout + PIPE_EOF_GRACE + Duration::from_millis(500),
        "run took {elapsed:?}"
    );
    let pgid = read_pgid(&r).expect("child recorded its pgid");
    assert!(
        survivors(pgid).is_empty(),
        "process group {pgid} survived the kill"
    );
    eprintln!("PROC HUNG CHILD: HELD (timeout {elapsed:?}, group {pgid} reaped)");
}

#[test]
fn pipe_flood_past_cap_drains_without_deadlock() {
    let r = root();
    let shim = write_shim(&r);
    let args: Vec<String> = vec![r.to_string_lossy().into_owned(), "flood".into()];
    let out = run(&spec(&shim, &args, Duration::from_secs(5), 1024))
        .expect("8 MiB flood must complete without deadlock");
    assert!(
        out.stdout.len() <= 1024,
        "captured {} bytes, cap is 1024",
        out.stdout.len()
    );
    eprintln!(
        "PROC PIPE FLOOD: HELD (8 MiB drained, {} bytes kept)",
        out.stdout.len()
    );
}
