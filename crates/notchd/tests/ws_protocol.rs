//! Real-WebSocket protocol tests: handshake, commands, deltas and latency.

#[path = "fixtures/ws/harness.rs"]
mod harness;

use harness::*;
use notchd::ws::snapshot_deltas;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[test]
fn ws_connect_hello_has_version_pid_and_snapshot() {
    let server = WsDaemon::start("hello");
    let mut c = server.connect();
    let hello = c.read_value();
    assert_eq!(hello["event"], "hello");
    assert_eq!(hello["version"], 1);
    assert!(hello["pid"].as_u64().is_some());
    let snap = &hello["snapshot"];
    assert_eq!(snap["version"], 1);
    assert_eq!(snap["orchestrator"]["id"], "ORCH");
    assert!(snap["sessions"].as_array().unwrap().is_empty());
    // Exactly one hello: the next frame is not another hello.
    c.send(&json!({"cmd": "ping"}));
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
    assert!(ack["pong"].as_u64().is_some());
}

#[test]
fn ws_commands_roundtrip_and_errors() {
    let server = WsDaemon::start("cmds");
    let work = server.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let cwd = work.to_string_lossy().to_string();
    let mut c = server.connect();
    let _hello = c.read_value();

    // submit_task -> ack with ids
    let ack = c.rpc_ack("submit_task", &json!({
        "cmd": "submit_task", "task": "wire roundtrip", "cwd": cwd,
        "title": "Roundtrip", "definitionOfDone": "tests pass"
    }));
    assert_eq!(ack["cmd"], "submit_task", "{ack}");
    assert_eq!(ack["ok"], true, "{ack}");
    let session = ack["sessionId"].as_str().unwrap().to_string();
    assert_eq!(session, "S1-OC01");

    // snapshot command replies with a snapshot frame containing the session.
    let snap = c.rpc(&json!({"cmd": "snapshot"}));
    assert_eq!(snap["event"], "snapshot", "{snap}");
    assert!(snap["snapshot"]["sessions"].as_array().unwrap().iter().any(|s| s["id"] == session));

    // focus / reprompt / pause / resume / cleanup / list_models all ack.
    for req in [
        json!({"cmd": "focus", "id": session}),
        json!({"cmd": "reprompt", "id": session, "text": "continue"}),
        json!({"cmd": "pause_supervisor", "id": "SV-1"}),
        json!({"cmd": "resume_supervisor", "id": "SV-1"}),
        json!({"cmd": "list_models"}),
        json!({"cmd": "subscribe"}),
    ] {
        let cmd = req["cmd"].as_str().unwrap().to_string();
        let r = c.rpc_ack(&cmd, &req);
        assert_eq!(r["event"], "ack", "{cmd}: {r}");
        assert_eq!(r["cmd"], cmd, "{r}");
        assert_eq!(r["ok"], true, "{cmd}: {r}");
    }

    // Unknown command and missing cmd are machine-readable errors.
    let unknown = c.rpc_ack("nope", &json!({"cmd": "nope"}));
    assert_eq!(unknown["ok"], false);
    assert_eq!(unknown["error"], "unknown_command");
    let missing = c.rpc_ack("", &json!({}));
    assert_eq!(missing["ok"], false);
    assert_eq!(missing["error"], "missing_cmd");

    // Malformed text frame and binary frame are rejected without killing the
    // connection (the next ping still works).
    c.send_text("not json at all");
    let bad = c.read_value();
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["error"], "invalid_json");
    c.send_binary(vec![0, 1, 2]);
    let bad_bin = c.read_value();
    assert_eq!(bad_bin["ok"], false);
    assert_eq!(bad_bin["error"], "binary_not_supported");
    let pong = c.rpc_ack("ping", &json!({"cmd": "ping"}));
    assert_eq!(pong["ok"], true, "connection survived malformed frames");
}

#[test]
fn ws_bus_message_delta_is_pushed_as_a_real_event() {
    let server = WsDaemon::start("busdelta");
    let work = server.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let cwd = work.to_string_lossy().to_string();
    let mut c = server.connect();
    let _hello = c.read_value();

    let mut sessions = Vec::new();
    for i in 0..2 {
        let a = c.rpc_ack("submit_task", &json!({"cmd":"submit_task","task":format!("bus task {i}"),"cwd":cwd}));
        assert_eq!(a["ok"], true, "{a}");
        sessions.push(a["sessionId"].as_str().unwrap().to_string());
    }
    // Drain the two snapshots so the bus_message test starts clean.
    let _ = c.wait_for(Duration::from_millis(600), |v| {
        v["event"] == "snapshot"
            && v["snapshot"]["sessions"].as_array().map(|s| s.len() == 2).unwrap_or(false)
    });

    let ack = c.rpc_ack("message", &json!({
        "cmd": "message", "from": sessions[0], "to": sessions[1],
        "type": "handoff", "body": "please take over the auth work"
    }));
    assert_eq!(ack["ok"], true, "{ack}");
    let msg_id = ack["messageId"].as_str().unwrap().to_string();

    let frame = c.wait_for(Duration::from_millis(800), |v| {
        v["event"] == "bus_message" && v["busMessage"]["id"] == msg_id
    });
    let frame = frame.unwrap_or_else(|| panic!("no bus_message delta for {msg_id}"));
    assert_eq!(frame["busMessage"]["from"], sessions[0]);
    assert_eq!(frame["busMessage"]["to"], sessions[1]);
    assert_eq!(frame["busMessage"]["type"], "handoff");
    assert_eq!(frame["busMessage"]["preview"], "please take over the auth work");
}

#[test]
fn ws_snapshot_delta_latency_p50_p95_under_200ms() {
    let server = WsDaemon::start("latency");
    let work = server.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let cwd = work.to_string_lossy().to_string();
    let mut c = server.connect();
    let _hello = c.read_value();

    // One session to mutate. `reprompt` is a real command that changes daemon
    // state (event log / eps) and calls notify_change without the heavy
    // durable state writes `submit_task` performs — so this measures the
    // command → subscriber-frame pipeline itself.
    let seed = c.rpc_ack("submit_task", &json!({"cmd":"submit_task","task":"latency seed","cwd":cwd}));
    assert_eq!(seed["ok"], true, "{seed}");
    let sid = seed["sessionId"].as_str().unwrap().to_string();
    // Drain the seed snapshot so the clock starts from a quiet connection.
    assert!(c.wait_for(Duration::from_secs(2), |v| v["event"] == "snapshot").is_some());
    std::thread::sleep(Duration::from_millis(150));

    let mut samples = Vec::new();
    for i in 0..12 {
        let t0 = Instant::now();
        let ack = c.rpc_ack("reprompt", &json!({
            "cmd":"reprompt","id":sid,"text":format!("latency prompt {i}")
        }));
        assert_eq!(ack["ok"], true, "{ack}");
        let pushed = c.wait_for(Duration::from_millis(500), |v| v["event"] == "snapshot");
        assert!(pushed.is_some(), "no snapshot delta after reprompt {i}");
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    eprintln!("ws command→snapshot push latency: p50={p50:.1}ms p95={p95:.1}ms samples={samples:?}");
    assert!(p95 < 200.0, "snapshot push p95 {p95:.1}ms must be < 200ms");
}

#[test]
fn ws_snapshot_deltas_are_new_entries_only() {
    // Pure function test: all four delta kinds, and no replay of old entries.
    let mk = |at: u64| -> Value {
        json!({
            "version": 1,
            "generatedAt": at,
            "busMessages": [{"id":"msg_1","from":"S1-OC01","to":"S1-OC02","type":"task","preview":"old","at":1}],
            "decisions": [{"at":1,"supervisor":"SV-1","session":"S1-OC01","action":"assign","reason":"old"}],
            "escalations": [{"id":"esc_1","session":"S1-OC01","supervisor":"SV-1","kind":"blocked","reason":"old","at":1}],
            "sessions": [{"id":"S1-OC01","lastAction":"old line"}],
        })
    };
    let prev = mk(10);
    let mut cur = mk(20);
    assert!(snapshot_deltas(None, &cur).is_empty(), "startup is never a delta");
    assert!(snapshot_deltas(Some(&prev), &prev).is_empty(), "unchanged is not a delta");

    cur["busMessages"].as_array_mut().unwrap().insert(
        0,
        json!({"id":"msg_2","from":"S1-OC02","to":"S1-OC01","type":"handoff","preview":"new","at":20}),
    );
    cur["decisions"].as_array_mut().unwrap().insert(
        0,
        json!({"at":20,"supervisor":"SV-1","session":"S1-OC02","action":"reprompt","reason":"new"}),
    );
    cur["escalations"].as_array_mut().unwrap().insert(
        0,
        json!({"id":"esc_2","session":"S1-OC02","supervisor":"SV-1","kind":"stuck","reason":"new","at":20}),
    );
    cur["sessions"][0]["lastAction"] = json!("new last action");

    let deltas = snapshot_deltas(Some(&prev), &cur);
    let events: Vec<&str> = deltas.iter().map(|d| d["event"].as_str().unwrap()).collect();
    assert_eq!(events, vec!["bus_message", "escalation", "decision", "session_output"]);
    assert_eq!(deltas[0]["busMessage"]["id"], "msg_2");
    assert_eq!(deltas[1]["escalation"]["id"], "esc_2");
    assert_eq!(deltas[2]["decision"]["action"], "reprompt");
    assert_eq!(deltas[3]["session"], "S1-OC01");
    assert_eq!(deltas[3]["line"], "new last action");
    assert_eq!(deltas[3]["at"], 20);
}
