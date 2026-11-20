//! W2-2 RED: replay-over-checkpoint. `state.json` stays a source-compatible
//! checkpoint; the append-only ledger is authoritative and replayed over it.
//! Written BEFORE the implementation.

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use notchd::ledger::{replay, replay_over_checkpoint, Ledger, Op, Source};
use notchd::model::StateFile;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);
const T0: u64 = 1_700_000_000_000;

fn root() -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notch-w2-replay-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn session(id: &str, title: &str) -> Value {
    json!({"id": id, "sup": "SV-1", "status": "working", "title": title, "task": title, "cwd": "/tmp"})
}

fn supervisor(id: &str, number: u32) -> Value {
    json!({"id": id, "number": number, "title": "charter"})
}

#[test]
fn replay_over_checkpoint_reproduces_full_state() {
    let dir = root();
    let mut ledger = Ledger::open(&dir).unwrap();
    ledger
        .append(Source::System, None, None, "SV-1", Op::Upsert {
            entity: "supervisor".into(), id: "SV-1".into(), value: supervisor("SV-1", 1),
        }, 10)
        .unwrap();
    ledger
        .append(Source::System, None, None, "S1-OC01", Op::Upsert {
            entity: "session".into(), id: "S1-OC01".into(), value: session("S1-OC01", "v2"),
        }, 11)
        .unwrap();
    ledger
        .append(Source::System, None, None, "S1-OC01", Op::Upsert {
            entity: "bus".into(), id: "msg_1".into(),
            value: json!({"id":"msg_1","from":"S1-OC01","to":"S1-OC02","type":"handoff","preview":"hi","at":12}),
        }, 12)
        .unwrap();
    ledger
        .append(Source::System, None, None, "S1-OC01", Op::Upsert {
            entity: "escalation".into(), id: "esc_1".into(),
            value: json!({"id":"esc_1","session":"S1-OC01","supervisor":"SV-1","kind":"stuck","reason":"quiet","at":13}),
        }, 13)
        .unwrap();
    ledger
        .append(Source::System, None, None, "ORCH", Op::SetSeq { esc_seq: 7, msg_seq: 9 }, 14)
        .unwrap();

    // A deliberately STALE checkpoint: the ledger, not the checkpoint, wins.
    let mut base = StateFile::empty();
    base.sessions.push(serde_json::from_value(session("S1-OC01", "stale-checkpoint")).unwrap());
    let rr = replay_over_checkpoint(&dir, base).unwrap();
    assert!(rr.warnings.is_empty(), "{:?}", rr.warnings);
    let state = serde_json::to_value(&rr.state).unwrap();

    assert_eq!(state["supervisors"].as_array().unwrap().len(), 1);
    assert_eq!(state["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(state["sessions"][0]["title"], json!("v2"), "ledger must override a stale checkpoint");
    assert_eq!(state["bus_messages"].as_array().unwrap().len(), 1);
    assert_eq!(state["bus_messages"][0]["id"], json!("msg_1"));
    assert_eq!(state["escalations"].as_array().unwrap().len(), 1);
    assert_eq!(state["esc_seq"], json!(7));
    assert_eq!(state["msg_seq"], json!(9));
    assert_eq!(rr.last_seq, 5);
}

#[test]
fn replay_is_idempotent_and_tombstones_remove() {
    let dir = root();
    let mut ledger = Ledger::open(&dir).unwrap();
    ledger.append(Source::System, None, None, "S1-OC01", Op::Upsert {
        entity: "session".into(), id: "S1-OC01".into(), value: session("S1-OC01", "doomed"),
    }, 10).unwrap();
    ledger.append(Source::System, None, None, "S1-OC01", Op::Remove {
        entity: "session".into(), id: "S1-OC01".into(),
    }, 11).unwrap();
    let lines = notchd::ledger::read_lines(&dir.join("ledger.jsonl")).unwrap();
    let first = replay(StateFile::empty(), &lines);
    let second = replay(StateFile::empty(), &lines);
    assert_eq!(
        serde_json::to_value(&first.state).unwrap(),
        serde_json::to_value(&second.state).unwrap(),
        "replay must be a pure function of (base, ledger)"
    );
    assert!(first.state.sessions.is_empty(), "a tombstone must remove the entity");
}

#[test]
fn unknown_entities_are_dropped_not_stored() {
    let dir = root();
    let mut ledger = Ledger::open(&dir).unwrap();
    ledger.append(Source::System, None, None, "X", Op::Upsert {
        entity: "not_a_state_field".into(), id: "x".into(), value: json!({"x": 1}),
    }, 10).unwrap();
    let lines = notchd::ledger::read_lines(&dir.join("ledger.jsonl")).unwrap();
    let rr = replay(StateFile::empty(), &lines);
    assert!(rr.state.sessions.is_empty());
    let v = serde_json::to_value(&rr.state).unwrap();
    assert!(v.get("not_a_state_field").is_none(), "unknown ledger entity must never become state");
    assert!(!rr.warnings.is_empty(), "a dropped entity must be reported");
}

fn fake_herdr() -> Herdr {
    Herdr::with_runner(move |args: &[String]| {
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        let sub2 = args.get(2).map(String::as_str).unwrap_or("");
        match (sub, sub2) {
            ("pane", "split") => Ok(json!({"result": {"pane": {"pane_id": "w1:p1"}}}).to_string()),
            ("agent", "list") => Ok(json!({"result": {"type": "agent_list", "agents": []}}).to_string()),
            _ => Ok(json!({"result": {"type": "ok"}}).to_string()),
        }
    })
}

fn daemon(dir: &PathBuf) -> Daemon {
    Daemon::open(
        Config {
            state_dir: dir.join("state"),
            tmp_dir: dir.join("tmp"),
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
fn daemon_replay_reproduces_state_after_reopen() {
    std::env::set_var("NOTCH_PRE_SPAWN_HOOK", "/nonexistent/notch-w2-hook");
    let dir = root();
    let from;
    let to;
    {
        let d = daemon(&dir);
        let a = d.submit_task("a".into(), Some(dir.to_string_lossy().into()), None, None, None, None);
        let b = d.submit_task("b".into(), Some(dir.to_string_lossy().into()), None, None, None, None);
        from = a.session_id.unwrap();
        to = b.session_id.unwrap();
        let ack = d.message(&from, &to, "handoff", "persisted through the ledger");
        assert!(ack.ok, "message failed: {:?}", ack.error);
        let snap = serde_json::to_value(d.snapshot()).unwrap();
        assert_eq!(snap["busMessages"].as_array().unwrap().len(), 1);
    }
    // Reopen a fresh daemon over the same state dir. The ledger must replay.
    let d2 = daemon(&dir);
    let snap = serde_json::to_value(d2.snapshot()).unwrap();
    let sessions = snap["sessions"].as_array().unwrap();
    assert!(sessions.iter().any(|s| s["id"] == json!(from)), "session lost across reopen: {sessions:?}");
    assert!(sessions.iter().any(|s| s["id"] == json!(to)));
    let bus = snap["busMessages"].as_array().unwrap();
    assert!(bus.iter().any(|m| m["preview"] == json!("persisted through the ledger")),
        "bus message lost across reopen: {bus:?}");
}
