//! W2-3 RED-first: narrow worker MCP bridge. Read tools cannot mutate; the
//! only mutating tool enqueues into the orchestrator, never session→session.

use notchd::mcp_bridge::{BridgeBackend, BridgeMessage, McpBridge, TOOL_NAMES};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Enqueue = (String, String, String, String, String); // id, from, to, kind, body

#[derive(Default)]
struct Recorder {
    enqueues: Mutex<Vec<Enqueue>>,
    peers: Vec<String>,
    messages: Vec<BridgeMessage>,
}

impl BridgeBackend for Recorder {
    fn peers(&self) -> Vec<String> {
        self.peers.clone()
    }
    fn messages(&self) -> Vec<BridgeMessage> {
        self.messages.clone()
    }
    fn enqueue(
        &self,
        id: &str,
        from: &str,
        to: &str,
        kind: &str,
        body: &str,
    ) -> Result<(), String> {
        if to == "reject-me" {
            return Err("backend refused target".into());
        }
        self.enqueues.lock().unwrap().push((
            id.into(),
            from.into(),
            to.into(),
            kind.into(),
            body.into(),
        ));
        Ok(())
    }
}

fn bridge(rec: Arc<Recorder>, self_id: &str) -> McpBridge {
    McpBridge::new(rec, self_id)
}

fn request(bridge: &McpBridge, method: &str, params: Value, id: u64) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    serde_json::from_str(&bridge.handle(&req.to_string()))
        .expect("bridge must return valid JSON-RPC")
}

fn call(bridge: &McpBridge, name: &str, args: Value, id: u64) -> Value {
    request(
        bridge,
        "tools/call",
        json!({"name": name, "arguments": args}),
        id,
    )
}

fn ok(resp: &Value) -> bool {
    resp.get("error").is_none()
        && resp
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(Value::as_bool)
            != Some(true)
}

fn text_of(resp: &Value) -> String {
    resp.get("result")
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[test]
fn tool_surface_is_read_only_plus_one_controlled_send() {
    let rec = Arc::new(Recorder::default());
    let b = bridge(rec, "S1-OC01");
    assert_eq!(McpBridge::tool_names(), &TOOL_NAMES);

    let listed = request(&b, "tools/list", json!({}), 1);
    assert!(ok(&listed), "tools/list must succeed: {listed}");
    let names: Vec<String> = serde_json::from_str::<Value>(&text_of(&listed))
        .unwrap()
        .get("tools")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|t| {
            t.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_eq!(names, vec!["list_peers", "list_messages", "send_message"]);
    // No tool can kill, spawn, edit files, sign approvals, or grant permissions.
    for forbidden in [
        "kill",
        "spawn",
        "reprompt",
        "write_file",
        "approve",
        "answer_approval",
        "set_mode",
        "shell",
        "bash",
    ] {
        assert!(
            !names.iter().any(|n| n.contains(forbidden)),
            "forbidden tool leaked: {forbidden}"
        );
    }
    assert!(
        call(&b, "kill", json!({"id": "S2-OC01"}), 2)
            .get("error")
            .is_some()
            || !ok(&call(&b, "kill", json!({"id": "S2-OC01"}), 3)),
        "unknown mutating tool must be rejected"
    );
}

#[test]
fn read_tools_do_not_mutate_anything() {
    let rec = Arc::new(Recorder {
        enqueues: Mutex::new(Vec::new()),
        peers: vec!["S1-OC01".into(), "S2-OC01".into()],
        messages: vec![BridgeMessage {
            id: "m1".into(),
            from: "S1-OC01".into(),
            to: "S2-OC01".into(),
            kind: "handoff".into(),
            body: "done".into(),
            at: 7,
        }],
    });
    let b = bridge(rec.clone(), "S1-OC01");

    let peers = call(&b, "list_peers", json!({}), 1);
    assert!(ok(&peers));
    let peers_json: Value = serde_json::from_str(&text_of(&peers)).unwrap();
    assert_eq!(
        peers_json
            .get("peers")
            .and_then(Value::as_array)
            .unwrap()
            .len(),
        2
    );

    let msgs = call(&b, "list_messages", json!({}), 2);
    assert!(ok(&msgs));
    let msgs_json: Value = serde_json::from_str(&text_of(&msgs)).unwrap();
    assert_eq!(
        msgs_json
            .get("messages")
            .and_then(Value::as_array)
            .unwrap()
            .len(),
        1
    );

    assert!(
        rec.enqueues.lock().unwrap().is_empty(),
        "read tools must never enqueue or mutate"
    );
}

#[test]
fn send_message_preserves_from_to_and_is_authorized_by_run_id() {
    let rec = Arc::new(Recorder::default());
    let b = bridge(rec.clone(), "S1-OC01");
    let resp = call(
        &b,
        "send_message",
        json!({"id": "m1", "to": "S2-OC01", "type": "handoff", "body": "tests fail at auth.rs:12"}),
        1,
    );
    assert!(ok(&resp), "valid send must succeed: {resp}");
    let got = rec.enqueues.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0],
        (
            "m1".to_string(),
            "S1-OC01".to_string(), // FROM comes from the run id, never an argument
            "S2-OC01".to_string(), // TO preserved verbatim
            "handoff".to_string(),
            "tests fail at auth.rs:12".to_string(),
        )
    );
}

#[test]
fn worker_cannot_impersonate_another_session() {
    let rec = Arc::new(Recorder::default());
    let b = bridge(rec.clone(), "S1-OC01");
    let resp = call(
        &b,
        "send_message",
        json!({"id": "m1", "from": "S9-OC99", "to": "S2-OC01", "type": "task", "body": "do it"}),
        1,
    );
    assert!(!ok(&resp), "a `from` argument must be rejected: {resp}");
    assert!(
        rec.enqueues.lock().unwrap().is_empty(),
        "impersonation must not enqueue"
    );
}

#[test]
fn wildcards_unknown_types_and_empty_bodies_are_rejected() {
    let rec = Arc::new(Recorder::default());
    let b = bridge(rec.clone(), "S1-OC01");
    for (id, args) in [
        (
            1,
            json!({"id": "m1", "to": "all", "type": "task", "body": "x"}),
        ),
        (
            2,
            json!({"id": "m2", "to": "team:core", "type": "task", "body": "x"}),
        ),
        (
            3,
            json!({"id": "m3", "to": "S2-OC01", "type": "INFO", "body": "x"}),
        ),
        (
            4,
            json!({"id": "m4", "to": "S2-OC01", "type": "task", "body": ""}),
        ),
        // Test-fixture correction (not an assertion change): the backend's
        // refusal path is keyed on `reject-me`, not a valid target.
        (
            5,
            json!({"id": "m5", "to": "reject-me", "type": "task", "body": "x"}),
        ),
    ] {
        let resp = call(&b, "send_message", args, id);
        // The backend-rejected case is an isError result, the rest are error frames.
        if id == 5 {
            assert!(
                !ok(&resp),
                "backend refusal must surface as an error: {resp}"
            );
        } else {
            assert!(!ok(&resp), "invalid send must be rejected: {resp}");
        }
    }
    assert_eq!(
        rec.enqueues.lock().unwrap().len(),
        0,
        "no invalid message may reach the bus"
    );
}

#[test]
fn duplicate_message_ids_are_dropped_at_the_bridge() {
    let rec = Arc::new(Recorder::default());
    let b = bridge(rec.clone(), "S1-OC01");
    let args = json!({"id": "dup-1", "to": "S2-OC01", "type": "question", "body": "why?"});
    assert!(ok(&call(&b, "send_message", args.clone(), 1)));
    assert!(
        !ok(&call(&b, "send_message", args, 2)),
        "duplicate id must be dropped"
    );
    assert_eq!(
        rec.enqueues.lock().unwrap().len(),
        1,
        "worker retries must not duplicate delivery"
    );
}

#[test]
fn malformed_jsonrpc_never_panics() {
    let rec = Arc::new(Recorder::default());
    let b = bridge(rec, "S1-OC01");
    for bad in ["", "not json", "{}", "[]", "{\"jsonrpc\":\"2.0\",\"id\":1}"] {
        let out = b.handle(bad);
        let v: Value =
            serde_json::from_str(&out).unwrap_or_else(|e| panic!("bad output {out}: {e}"));
        assert!(
            v.get("error").is_some() || v.get("result").is_some(),
            "must answer every frame"
        );
    }
}

#[test]
fn stress_100_messages_are_all_routed_and_bounded() {
    let rec = Arc::new(Recorder::default());
    let b = bridge(rec.clone(), "S1-OC01");
    for i in 0..100 {
        let resp = call(
            &b,
            "send_message",
            json!({"id": format!("m{i}"), "to": "S2-OC01", "type": "task", "body": format!("job {i}")}),
            i,
        );
        assert!(ok(&resp), "message {i} failed: {resp}");
    }
    let got = rec.enqueues.lock().unwrap();
    assert_eq!(got.len(), 100);
    for (i, entry) in got.iter().enumerate() {
        assert_eq!(entry.0, format!("m{i}"));
        assert_eq!(entry.1, "S1-OC01");
        assert_eq!(entry.2, "S2-OC01");
    }
}
