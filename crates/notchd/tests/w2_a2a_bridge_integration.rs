//! W2-3 end-to-end: worker MCP bridge -> orchestrator ingress -> bus -> target
//! supervisor mailbox. No direct session→session path; no OpenCode process.

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static N: AtomicU64 = AtomicU64::new(0);

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-w2bridge-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fake_herdr() -> Herdr {
    Herdr::with_runner(|args: &[String]| {
        let a = args.get(1).map(String::as_str).unwrap_or("");
        let b = args.get(2).map(String::as_str).unwrap_or("");
        match (a, b) {
            ("pane", "split") => Ok(r#"{"result":{"type":"pane_info","pane":{"pane_id":"w9:p1"}}}"#.to_string()),
            ("agent", "start") => Ok(r#"{"result":{"type":"agent_started","agent":{"pane_id":"w9:p1","agent_status":"working","revision":1}}}"#.to_string()),
            ("agent", "prompt") => Ok(r#"{"result":{"type":"agent_prompted","agent":{"pane_id":"w9:p1","agent_status":"working","revision":2}}}"#.to_string()),
            ("agent", "list") => Ok(r#"{"result":{"agents":[]}}"#.to_string()),
            ("pane", "close") => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
            _ => Err(HerdrError::Failed { args: args.to_vec(), code: Some(2), stderr: "unexpected".into() }),
        }
    })
}

fn daemon_with_two_sessions(r: &Path) -> Arc<Daemon> {
    std::fs::create_dir_all(r.join("tmp")).unwrap();
    let config = Config {
        state_dir: r.join("state"),
        tmp_dir: r.join("tmp"),
        stuck_after_secs: 3600,
        poll_interval_ms: 2000,
        opencode_decider: false,
    };
    let d = Arc::new(Daemon::open(config, fake_herdr(), Arc::new(FakeClock::new(1000))).unwrap());
    let plan = r.join("plan.json");
    let cwd = r.to_string_lossy();
    std::fs::write(
        &plan,
        format!(
            r#"{{"stuck_after_secs":3600,"supervisors":[{{"cwd":"{cwd}","sessions":[{{"prompt":"one","title":"one"}}]}},{{"cwd":"{cwd}","sessions":[{{"prompt":"two","title":"two"}}]}}]}}"#
        ),
    )
    .unwrap();
    let ack = d.run_plan(&plan);
    assert!(ack.ok, "plan failed: {:?}", ack.error);
    assert_eq!(ack.spawned, vec!["SV-1", "SV-2"]);
    d
}

fn call(bridge: &notchd::mcp_bridge::McpBridge, args: Value, id: u64) -> Value {
    let req = json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": "send_message", "arguments": args},
    });
    serde_json::from_str(&bridge.handle(&req.to_string())).unwrap()
}

#[test]
fn worker_message_travels_only_through_the_orchestrator() {
    let r = root("route");
    let d = daemon_with_two_sessions(&r);
    let bridge = d.worker_mcp_bridge("S1-OC01");

    // Read-only view reflects the real daemon tree.
    let peers = serde_json::from_str::<Value>(
        &bridge.handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_peers","arguments":{}}}"#),
    )
    .unwrap();
    let text = peers["result"]["content"][0]["text"].as_str().unwrap();
    let peers_json: Value = serde_json::from_str(text).unwrap();
    let ids: Vec<&str> = peers_json["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["S1-OC01", "S2-OC01"]);

    // FROM is bound to this bridge's session; TO is the other supervisor.
    let resp = call(
        &bridge,
        json!({"id": "e2e-1", "to": "S2-OC01", "type": "handoff", "body": "verified evidence ready"}),
        2,
    );
    assert!(resp.get("error").is_none(), "send failed: {resp}");
    assert_ne!(
        resp["result"]["isError"],
        json!(true),
        "send failed: {resp}"
    );

    // The message is NOT in any mailbox until the orchestrator drains.
    assert!(d.mailbox_drain("SV-2").is_empty(), "no direct delivery");
    assert_eq!(d.bus_drain(), 1, "exactly one enqueued message");
    let delivered = d.mailbox_drain("SV-2");
    assert_eq!(delivered.len(), 1, "target supervisor received the message");
    assert_eq!(delivered[0].from, "SV-1");
    assert_eq!(delivered[0].to, "SV-2");
    assert_eq!(delivered[0].session.as_deref(), Some("S2-OC01"));
    assert!(delivered[0].detail.contains("S1-OC01->S2-OC01"));
    assert!(delivered[0].detail.contains("verified evidence ready"));
    assert!(
        d.mailbox_drain("SV-1").is_empty(),
        "source supervisor got nothing"
    );
    assert_eq!(d.bus_drain(), 0, "exactly-once routing");
}

#[test]
fn worker_cannot_reach_another_session_directly() {
    let r = root("nospam");
    let d = daemon_with_two_sessions(&r);
    let bridge = d.worker_mcp_bridge("S1-OC01");
    // An explicit FROM is refused even if it names a real session.
    let resp = call(
        &bridge,
        json!({"id": "e2e-2", "from": "S2-OC01", "to": "S2-OC01", "type": "task", "body": "impersonate"}),
        1,
    );
    assert_eq!(resp["result"]["isError"], json!(true));
    assert_eq!(d.bus_drain(), 0, "impersonation never reaches the bus");
}
