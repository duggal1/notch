//! Protocol edge cases for the NDJSON socket daemon.
//! Naming: socket_edge_<case>.

#[path = "fixtures/socket/harness.rs"]
mod harness;

use harness::*;
use serde_json::json;

/// Subscribe twice from one socket. ACTUAL behavior (documented): the server
/// registers a SECOND subscriber per `subscribe` command; the client gets two
/// subscribe acks, two immediate snapshots, and after every change — two
/// snapshot pushes (one per registration) on the same connection.
#[test]
fn socket_edge_subscribe_twice_one_socket() {
    let d = TestDaemon::start("edge-sub2");
    let _ = d.run_plan();
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();

    // First subscribe.
    c.send_line(r#"{"cmd":"subscribe"}"#);
    let ack1 = c.read_value();
    let snap1 = c.read_value();
    assert_eq!(ack1["event"], "ack");
    assert_eq!(ack1["cmd"], "subscribe");
    assert_eq!(ack1["ok"], true);
    assert_eq!(snap1["event"], "snapshot");

    // Second subscribe on the SAME socket.
    c.send_line(r#"{"cmd":"subscribe"}"#);
    let ack2 = c.read_value();
    let snap2 = c.read_value();
    assert_eq!(ack2["event"], "ack");
    assert_eq!(ack2["cmd"], "subscribe");
    assert_eq!(ack2["ok"], true);
    assert_eq!(snap2["event"], "snapshot");

    // Cause a change; BOTH registrations push a snapshot.
    {
        let mut k = TestConn::connect(&d.sock);
        let _hello = k.hello();
        let ack = k.rpc(&json!({"cmd":"kill","id":"S1-OC01","reason":"edge-two-subs"}));
        assert_eq!(ack["ok"], true);
    }
    let has_marker = |v: &serde_json::Value| {
        v["event"] == "snapshot"
            && v["snapshot"]["sessions"].as_array().map(|ss| {
                ss.iter().any(|s| s["failureReason"] == "edge-two-subs")
            }).unwrap_or(false)
    };
    let first = c.wait_for_line(deadline_in(2000), has_marker);
    let second = c.wait_for_line(deadline_in(2000), has_marker);
    assert!(first.is_some(), "first push never arrived");
    assert!(
        second.is_some(),
        "second push (duplicate registration) never arrived — behavior changed?"
    );
    eprintln!("socket_edge_subscribe_twice_one_socket: received 2 pushes for one kill (one per registration)");
}

/// kill of a nonexistent id -> ok:false. ACTUAL error payload (documented):
/// the machine-readable code `unknown_session`, not a sentence. Acceptable
/// per bridge.md §2 Errors ("machine-readable code or plain sentence"), but
/// worth flagging: the UI will see the raw code.
#[test]
fn socket_edge_kill_nonexistent_id() {
    let d = TestDaemon::start("edge-kill404");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    let ack = c.rpc(&json!({"cmd":"kill","id":"S9-OC99"}));
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "kill");
    assert_eq!(ack["ok"], false);
    let err = ack["error"].as_str().expect("error must be a string");
    assert!(!err.is_empty(), "error must be a non-empty string");
    eprintln!("socket_edge_kill_nonexistent_id: actual error payload is {err:?}");
    assert_eq!(err, "unknown_session");
    // Daemon alive afterwards.
    assert_eq!(c.rpc(&json!({"cmd":"ping"}))["ok"], true);
}

/// run_plan with a path that does not exist -> ok:false, daemon alive.
#[test]
fn socket_edge_run_plan_nonexistent_path() {
    let d = TestDaemon::start("edge-plan404");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    let ack = c.rpc(&json!({"cmd":"run_plan","path":"/nonexistent/notch-fuzz/no-such-plan.json"}));
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "run_plan");
    assert_eq!(ack["ok"], false);
    let err = ack["error"].as_str().expect("error must be a string");
    eprintln!("socket_edge_run_plan_nonexistent_path: actual error payload is {err:?}");
    assert!(!err.is_empty());
    // Daemon alive and able to serve afterwards.
    assert_eq!(c.rpc(&json!({"cmd":"ping"}))["ok"], true);
    let snap = c.rpc(&json!({"cmd":"snapshot"}));
    assert_eq!(snap["event"], "snapshot");
}

/// message with a 4001-character body -> rejected.
#[test]
fn socket_edge_message_body_4001_rejected() {
    let d = TestDaemon::start("edge-body4001");
    let _ = d.run_plan();
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    let body = "a".repeat(4001);
    let ack = c.rpc(&json!({
        "cmd": "message",
        "from": "S1-OC01",
        "to": "S1-OC02",
        "type": "task",
        "body": body,
    }));
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "message");
    assert_eq!(ack["ok"], false);
    let err = ack["error"].as_str().expect("error must be a string");
    eprintln!("socket_edge_message_body_4001_rejected: actual error payload is {err:?}");
    assert_eq!(err, "body exceeds 4000 chars");
    // 4000 chars IS accepted (boundary).
    let ack = c.rpc(&json!({
        "cmd": "message",
        "from": "S1-OC01",
        "to": "S1-OC02",
        "type": "task",
        "body": "a".repeat(4000),
    }));
    assert_eq!(ack["ok"], true, "4000-char body must be accepted: {ack}");
    // Daemon alive afterwards.
    assert_eq!(c.rpc(&json!({"cmd":"ping"}))["ok"], true);
}

/// message with a 3-character body -> accepted, bus message logged.
#[test]
fn socket_edge_message_body_3_chars_accepted() {
    let d = TestDaemon::start("edge-body3");
    let _ = d.run_plan();
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    let ack = c.rpc(&json!({
        "cmd": "message",
        "from": "S1-OC01",
        "to": "S1-OC02",
        "type": "task",
        "body": "abc",
    }));
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "message");
    assert_eq!(ack["ok"], true, "3-char body must be accepted: {ack}");
    assert!(
        ack["messageId"].as_str().map(|s| s.starts_with("msg_")).unwrap_or(false),
        "messageId missing: {ack}"
    );
    // The bus message shows up in the snapshot (cap 30, newest first).
    let snap = d.snapshot();
    let bus = snap["snapshot"]["busMessages"].as_array().expect("busMessages array").clone();
    assert!(
        bus.iter().any(|m| m["from"] == "S1-OC01" && m["to"] == "S1-OC02" && m["preview"] == "abc"),
        "bus message not recorded in snapshot: {bus:?}"
    );
}

/// logs of an unknown id -> ACTUAL behavior (documented): ack ok:false with
/// the machine-readable code `unknown_session`. No `lines` payload is sent.
/// (The `lines: []` variant does NOT occur — the socket maps Err to an ack.)
#[test]
fn socket_edge_logs_unknown_id() {
    let d = TestDaemon::start("edge-logs404");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    let ack = c.rpc(&json!({"cmd":"logs","id":"S9-OC99"}));
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "logs");
    assert_eq!(ack["ok"], false);
    let err = ack["error"].as_str().expect("error must be a string");
    eprintln!("socket_edge_logs_unknown_id: actual behavior is ack ok:false, error {err:?} (no lines payload)");
    assert_eq!(err, "unknown_session");
    assert!(ack.get("lines").is_none(), "no lines payload is expected");
    // Known id works and returns a lines array (fake herdr prints one line).
    let _ = d.run_plan();
    let line = c.send_and_read(&json!({"cmd":"logs","id":"S1-OC01"}));
    assert_eq!(line["event"], "logs");
    assert_eq!(line["id"], "S1-OC01");
    assert!(line["lines"].as_array().is_some());
    // Daemon alive afterwards.
    assert_eq!(c.rpc(&json!({"cmd":"ping"}))["ok"], true);
}
