//! B6 regression — writer A2 (cw 3982) / tester A7 (cw 5538).
//!
//! A state dir whose checkpoint AND all three rolling backups are corrupt,
//! with nothing in the ledger to rebuild from, has no legitimate source of
//! truth. The daemon must HARD-ERROR on boot — it must never silently reset
//! the supervisor registry to SV-1 / an empty state.
//!
//! RED on unfixed code (e544b8c / 7b29c4f): `Daemon::open` calls the lenient
//! `state::load_state`, which converts unrecoverable corruption into
//! `StateFile::empty()` and returns `Ok`, so the daemon boots with an empty
//! registry (SV-1). A2 makes these GREEN by wiring the existing strict
//! `state::load_or_recover` + `prime_registry` into every boot path.

use notchd::{Config, Daemon, Herdr, SystemClock};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn state_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("notch-b6-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn corrupt(path: &Path) {
    // Structurally broken JSON plus invalid UTF-8: no candidate can parse.
    std::fs::write(path, b"\x00\xff{not-valid-json").unwrap();
}

fn corrupt_every_checkpoint(dir: &Path) {
    for name in [
        "state.json",
        "state.json.bak1",
        "state.json.bak2",
        "state.json.bak3",
    ] {
        corrupt(&dir.join(name));
    }
    // Deliberately no ledger.jsonl: there is no authoritative source to
    // rebuild from, so the ONLY alternative to a hard error is an SV-1 reset.
}

/// Core contract, exercised directly at the public boot seam.
#[test]
fn corrupt_checkpoint_plus_three_backups_is_a_hard_error_at_daemon_open() {
    let dir = state_dir("open");
    corrupt_every_checkpoint(&dir);
    let config = Config {
        state_dir: dir.clone(),
        tmp_dir: dir.clone(),
        ..Config::default()
    };

    let opened = Daemon::open(config, Herdr::from_env(), Arc::new(SystemClock));
    assert!(
        opened.is_err(),
        "corrupt state.json + all 3 backups (no ledger) must be a HARD ERROR at \\
         Daemon::open, not a silent SV-1 reset to empty state. \\
         On unfixed code load_state() returns StateFile::empty() and open() succeeds."
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Same contract end-to-end: `notchd serve` must refuse to start (exit non-zero
/// with a clear message) rather than coming up on an empty state and binding
/// its socket. On unfixed code the socket appears and the process stays alive.
#[test]
fn serve_refuses_to_boot_on_unrecoverable_state() {
    let dir = state_dir("serve");
    corrupt_every_checkpoint(&dir);
    let sock = std::env::temp_dir().join(format!(
        "notch-b6-serve-{}-{}.sock",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&sock);

    let mut child = Command::new(env!("CARGO_BIN_EXE_notchd"))
        .arg("serve")
        .env("NOTCH_STATE_DIR", &dir)
        .env("NOTCH_SOCK", &sock)
        .env("NOTCH_HERDR_CMD", "/usr/bin/true")
        .env("NOTCH_OPENCODE_CMD", "/usr/bin/true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn notchd serve");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut exited = false;
    let mut code = None;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            exited = true;
            code = status.code();
            break;
        }
        if UnixStream::connect(&sock).is_ok() {
            // The daemon came up and is serving: that is the silent SV-1 reset.
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let output = child.wait_with_output().expect("wait_with_output");
    let stderr_text = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout_text = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(
        exited,
        "notchd serve silently booted a socket on an unrecoverable state dir \\
         (SV-1 reset); expected a hard boot error. stdout={stdout_text} stderr={stderr_text}"
    );
    assert_ne!(
        code,
        Some(0),
        "serve must exit non-zero on unrecoverable state; code={code:?} stdout={stdout_text} stderr={stderr_text}"
    );
    let combined = format!("{stderr_text}{stdout_text}");
    assert!(
        combined.contains("state") || combined.contains("unrecoverable") || combined.contains("open daemon"),
        "boot failure must carry a clear message, got: {combined}"
    );

    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_dir_all(&dir);
}
