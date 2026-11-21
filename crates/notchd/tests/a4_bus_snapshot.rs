//! V2 verifier finding: a live `message` must appear in `snapshot.busMessages`
//! in the frozen bridge.md §1.3 shape (camelCase, newest-first, capped).

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const T0: u64 = 1_700_000_000_000;
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notch-a4-bus-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fake_herdr() -> Herdr {
    let n = Arc::new(AtomicUsize::new(0));
    Herdr::with_runner(move |args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("pane", "split") => {
                let i = n.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(json!({"result": {"pane": {"pane_id": format!("w{i}:p{i}")}}}).to_string())
            }
            ("agent", "list") => {
                Ok(json!({"result": {"type": "agent_list", "agents": []}}).to_string())
            }
            _ => Ok(json!({"result": {"type": "ok"}}).to_string()),
        }
    })
}

fn daemon(root: &Path) -> Daemon {
    Daemon::open(
        Config {
            state_dir: root.join("state"),
            tmp_dir: root.join("tmp"),
            stuck_after_secs: 600,
            poll_interval_ms: 2000,
            opencode_decider: false,
        },
        fake_herdr(),
        Arc::new(FakeClock::new(T0)),
    )
    .unwrap()
}

#[test]
fn live_message_is_in_snapshot_bus_messages_with_frozen_shape() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/nonexistent/notch-a4-hook");
    let dir = root();
    let d = daemon(&dir);
    let a = d.submit_task("a".into(), Some(dir.to_string_lossy().into()), None, None, None, None);
    let b = d.submit_task("b".into(), Some(dir.to_string_lossy().into()), None, None, None, None);
    let from = a.session_id.unwrap();
    let to = b.session_id.unwrap();

    let ack = d.message(&from, &to, "handoff", "first line of the body\nsecond line");
    assert!(ack.ok, "message failed: {:?}", ack.error);
    let id = ack.message_id.expect("message id");

    let snap = serde_json::to_value(d.snapshot()).unwrap();
    let bus = snap["busMessages"].as_array().expect("busMessages must be present after a live message");
    assert_eq!(bus.len(), 1, "exactly one live message: {bus:?}");
    let entry = &bus[0];
    assert_eq!(entry["id"], json!(id));
    assert_eq!(entry["from"], json!(from));
    assert_eq!(entry["to"], json!(to));
    assert_eq!(entry["type"], json!("handoff"));
    assert_eq!(entry["preview"], json!("first line of the body"));
    assert_eq!(entry["at"], json!(T0));
    // Key set is frozen: id, from, to, type, preview, at — nothing else.
    let keys: Vec<&String> = entry.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 6, "frozen busMessages key set: {keys:?}");
}

#[test]
fn live_bus_send_and_drain_is_projected_into_snapshot_bus_messages() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/nonexistent/notch-a4-hook");
    let dir = root();
    let d = daemon(&dir);
    // Two supervisors must exist for bus routing.
    let seed = d.submit_task("seed".into(), Some(dir.to_string_lossy().into()), None, None, None, None);
    assert!(seed.ok);
    let second = d.submit_task("second".into(), Some(dir.to_string_lossy().into()), None, None, None, None);
    assert!(second.ok);
    let from = seed.supervisor_id.unwrap();
    let to = second.supervisor_id.unwrap();
    assert_ne!(from, to);

    d.bus_send(&from, &to, None, "handoff", "live bus body").unwrap();
    let drained = d.bus_drain();
    assert_eq!(drained, 1, "the bus must route the message");

    let snap = serde_json::to_value(d.snapshot()).unwrap();
    let bus = snap["busMessages"].as_array().expect(
        "a routed bus message is logged in events.jsonl but missing from snapshot.busMessages"
    );
    assert!(
        bus.iter().any(|m| m["from"] == json!(from)
            && m["to"] == json!(to)
            && m["type"] == json!("handoff")
            && m["preview"] == json!("live bus body")),
        "routed bus message was not projected: {bus:?}"
    );
}
