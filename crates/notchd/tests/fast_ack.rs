//! P2 (E2E latency finding): `submit_task` must ack fast — allocate and
//! persist the id, return, then finish herdr spawn on a background worker.
//! Live baseline: p50 6195 ms / p95 6992 ms while pane→working was 12 ms and
//! ack→pane 8.5 ms, i.e. the wait was inside the synchronous submit path.
//!
//! RED on current main: the fake herdr blocks pane_split for 3 s and the ack
//! waits for it. Run: cargo test -p notchd --test fast_ack

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

fn config(root: &Path) -> Config {
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 3600,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

fn root(tag: &str) -> std::path::PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-fastack-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// herdr whose pane_split waits on `gate` before answering. A synchronous
/// submit_task therefore blocks for `Duration` before it can ack.
fn gated_herdr(gate: Arc<Mutex<Receiver<()>>>, fail: bool) -> Herdr {
    Herdr::with_runner(move |args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("pane", "split") => {
                let rx = gate.lock().unwrap_or_else(|e| e.into_inner());
                let _ = rx.recv_timeout(Duration::from_secs(3));
                drop(rx);
                if fail {
                    return Err(HerdrError::Io("pane split unavailable".to_string()));
                }
                Ok(json!({"result": {"pane": {"pane_id": "wF:p1"}}}).to_string())
            }
            ("agent", "list") => Ok(json!({"result": {"agents": []}}).to_string()),
            ("agent", "read") => Ok("working\n".to_string()),
            _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
        }
    })
}

fn session_snapshot(daemon: &Daemon, id: &str) -> Value {
    let snap: Value = serde_json::to_value(daemon.snapshot()).unwrap();
    snap["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("session {id} missing from snapshot: {snap}"))
}

#[test]
fn submit_task_acks_before_slow_spawn_and_persists_id() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root("ack");
    let (tx, rx): (Sender<()>, Receiver<()>) = mpsc::channel();
    let daemon = Daemon::open(
        config(&r),
        gated_herdr(Arc::new(Mutex::new(rx)), false),
        Arc::new(FakeClock::new(T0)),
    )
    .unwrap();

    let t = Instant::now();
    let ack = daemon.submit_task(
        "fast ack work".to_string(),
        Some(r.to_string_lossy().into_owned()),
        Some("fast".to_string()),
        None,
        None,
        None,
    );
    let ack_ms = t.elapsed().as_millis();
    println!("submit→ack: {ack_ms}ms");

    // Release the blocked herdr spawn so the daemon can be torn down.
    let _ = tx.send(());
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);
    let id = ack
        .session_id
        .clone()
        .unwrap_or_else(|| "S1-OC01".to_string());
    assert!(
        ack_ms < 1000,
        "submit→ack took {ack_ms}ms; the ack must not wait for herdr agent-start"
    );

    // The id was allocated AND persisted before the ack returned.
    let raw = std::fs::read_to_string(r.join("state").join("registry.json")).unwrap();
    let reg: Value = serde_json::from_str(&raw).unwrap();
    let ids: Vec<&str> = reg["ids"]
        .as_array()
        .expect("registry ids array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        ids.contains(&id.as_str()),
        "id {id} not persisted before ack: {reg}"
    );

    // Spawn continues asynchronously: after the gate opens the pane appears.
    let mut pane_attached = false;
    for _ in 0..40 {
        daemon.poll_once();
        let s = session_snapshot(&daemon, &id);
        if s["paneId"] == json!("wF:p1") {
            pane_attached = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        pane_attached,
        "async spawn never attached the pane: {}",
        session_snapshot(&daemon, &id)
    );
}

/// A background spawn failure must NOT turn the ack into an error: it is
/// surfaced through snapshot warnings (and the session becomes failed) while
/// the caller already has its id.
#[test]
fn background_spawn_failure_surfaces_in_snapshot_warnings() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/usr/bin/true");
    let r = root("fail");
    let (tx, rx): (Sender<()>, Receiver<()>) = mpsc::channel();
    let daemon = Daemon::open(
        config(&r),
        gated_herdr(Arc::new(Mutex::new(rx)), true),
        Arc::new(FakeClock::new(T0)),
    )
    .unwrap();

    let t = Instant::now();
    let ack = daemon.submit_task(
        "failure still acks".to_string(),
        Some(r.to_string_lossy().into_owned()),
        None,
        None,
        None,
        None,
    );
    let ack_ms = t.elapsed().as_millis();
    let _ = tx.send(());
    assert!(
        ack.ok,
        "fast ack contract: spawn failures are post-ack: {:?}",
        ack.error
    );
    assert!(ack_ms < 1000, "failure path also blocked {ack_ms}ms");
    let id = ack.session_id.unwrap_or_else(|| "S1-OC01".to_string());

    let mut warned = false;
    for _ in 0..40 {
        daemon.poll_once();
        let snap: Value = serde_json::to_value(daemon.snapshot()).unwrap();
        let has_warning = snap["warnings"]
            .as_array()
            .map(|a| a.iter().any(|w| w.as_str().unwrap_or("").contains(&id)))
            .unwrap_or(false);
        let failed = session_snapshot(&daemon, &id)["status"] == json!("failed");
        if has_warning && failed {
            warned = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        warned,
        "spawn failure must surface via snapshot warning + failed status"
    );
}
