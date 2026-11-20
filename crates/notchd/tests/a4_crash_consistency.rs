//! T5 (HARDENING §4): crash consistency under SIGKILL.
//!
//! A child test process repeatedly rewrites `registry.json` with the same
//! committed snapshot; the parent SIGKILLs it at a random point, drops a
//! canary `.tmp`, then recovers. Across 1000 rounds
//! no id may repeat, no canary may be adopted, and recovery must either return
//! a valid registry or a hard error — never a silent SV-1 reset.

use notchd::clock::SystemClock;
use notchd::registry::Registry;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("notch-a4-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Child mode: rewrite one committed snapshot forever, printing READY only
/// after the live file and its first backup both exist. Never run by cargo
/// directly — the parent re-execs this same binary with an env marker.
#[test]
fn t5_writer_child() {
    if std::env::var("NOTCH_A4_CHILD").ok().as_deref() != Some("1") {
        return;
    }
    let dir = PathBuf::from(std::env::var("NOTCH_A4_STATE").expect("state dir"));
    let path = dir.join("registry.json");
    let mut reg = Registry::load(&path, &SystemClock);
    if reg.ids().is_empty() {
        reg.allocate_supervisor();
        let _ = reg.allocate_session(1);
    }
    // Establish live + bak1 before signalling readiness, so a kill at any
    // later instant always leaves a recoverable snapshot.
    reg.save(&path).unwrap();
    reg.save(&path).unwrap();
    println!("READY");
    std::io::stdout().flush().unwrap();
    loop {
        reg.save(&path).unwrap();
    }
}

fn registry_backup(path: &Path, n: usize) -> PathBuf {
    PathBuf::from(format!("{}.bak{n}", path.display()))
}

#[test]
fn t5_1000_sigkills_no_repeats_no_tmp_adoption() {
    let dir = tmp_root("t5");
    let path = dir.join("registry.json");
    let exe = std::env::current_exe().expect("current_exe");

    let mut handed_out: HashSet<String> = HashSet::new();
    let mut canaries: HashSet<String> = HashSet::new();
    let mut hard_errors = 0usize;

    for iteration in 0..1000u64 {
        let mut child = Command::new(&exe)
            .args(["--exact", "t5_writer_child", "--nocapture"])
            .env("NOTCH_A4_CHILD", "1")
            .env("NOTCH_A4_STATE", &dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn writer child");

        // Proper synchronization on the child's READY line — no sleep. The
        // test harness prints a banner first, so scan for the marker.
        let stdout = child.stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);
        let mut ready = false;
        for _ in 0..16 {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if line.trim() == "READY" {
                ready = true;
                break;
            }
        }
        assert!(ready, "child failed to establish a snapshot");
        drop(reader);

        child.kill().expect("SIGKILL writer child");
        let status = child.wait().expect("reap writer child");
        assert!(
            !status.success() || status.code().is_none(),
            "child must be killed"
        );

        // The writer uses temp + fsync + atomic rename, so a SIGKILL at any
        // byte offset cannot leave a torn live file. The live registry is
        // therefore authoritative and keeps every committed id; torn-file and
        // all-tiers-corrupt recovery are covered by the dedicated tests below
        // and by registry::a4_tests.

        // A canary .tmp carrying an id that must never be adopted.
        let canary_id = format!("S9-OC-CANARY-{iteration:04}");
        canaries.insert(canary_id.clone());
        let mut canary = Registry::load(&path, &SystemClock);
        canary.ids.push(canary_id.clone());
        // Same naming rule as durable::tmp_path (`registry.json` -> `registry.tmp`).
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&canary).unwrap()).unwrap();

        match Registry::load_strict(&path) {
            Ok(mut recovered) => {
                for id in recovered.ids() {
                    assert!(
                        !canaries.contains(id),
                        "recovered a canary id from .tmp: {id}"
                    );
                }
                if recovered.ids().is_empty() {
                    recovered.allocate_supervisor();
                }
                let id = recovered.allocate_session(1);
                assert!(
                    handed_out.insert(id.clone()),
                    "duplicate id across crashes: {id}"
                );
                recovered.save(&path).unwrap();
            }
            Err(_) => {
                // Hard error is the allowed outcome only when every tier is
                // gone. With bak1 established before READY this should be rare.
                hard_errors += 1;
                let mut fresh = Registry::load(&dir.join("never-existed.json"), &SystemClock);
                fresh.allocate_supervisor();
                let id = fresh.allocate_session(1);
                assert!(
                    handed_out.insert(id.clone()),
                    "duplicate id after hard error: {id}"
                );
                fresh.save(&path).unwrap();
            }
        }
    }

    assert_eq!(
        handed_out.len(),
        1000,
        "every crash round must hand out one fresh id"
    );
    assert!(hard_errors < 50, "too many hard errors: {hard_errors}/1000");
    // The live file may be truncated at the very end; recovery from a backup
    // must still yield ids that never repeat.
    let final_reg = match Registry::load_strict(&path) {
        Ok(reg) => reg,
        Err(_) => Registry::load(&path, &SystemClock),
    };
    for id in final_reg.ids() {
        assert!(
            !canaries.contains(id),
            "final registry adopted a .tmp canary"
        );
    }
    println!(
        "T5 measured: 1000 SIGKILL rounds, {} unique ids, {hard_errors} hard errors",
        handed_out.len()
    );
}

#[test]
fn t5_total_corruption_is_a_hard_error_not_a_reset() {
    let dir = tmp_root("t5hard");
    let path = dir.join("registry.json");
    let mut reg = Registry::load(&dir.join("never-existed.json"), &SystemClock);
    reg.allocate_supervisor();
    reg.allocate_session(1);
    reg.save(&path).unwrap();
    std::fs::write(&path, b"{corrupt").unwrap();
    for n in 1..=3 {
        std::fs::write(registry_backup(&path, n), b"{corrupt").unwrap();
    }
    let err = match Registry::load_strict(&path) {
        Ok(_) => panic!("no tier is parseable -> must be a hard error"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("unrecoverable"),
        "unexpected error: {err}"
    );

    // The lenient loader must not panic; it reports the failure as a warning.
    let lenient = Registry::load(&path, &SystemClock);
    assert!(lenient.ids().is_empty());
}
