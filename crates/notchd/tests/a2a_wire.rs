//! Additive message metadata. The v1.3 `snapshot.busMessages` shape is
//! frozen; new fields live on a separate `A2aMessage` envelope only.

use notchd::model::{A2aMessage, BusMessageEntry};
use serde_json::Value;

#[test]
fn a2a_message_roundtrips_all_additive_fields() {
    let msg = A2aMessage {
        id: "msg_42".to_string(),
        at: 1_763_520_000_000,
        from: "S1-OC01".to_string(),
        to: "S2-OC01".to_string(),
        kind: "handoff".to_string(),
        body: "tests fail at auth.rs:12".to_string(),
        claimed_path: Some("src/auth.rs".to_string()),
        delivered_at: Some(1_763_520_000_500),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: A2aMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);

    let v: Value = serde_json::from_str(&json).unwrap();
    for key in [
        "id",
        "at",
        "from",
        "to",
        "type",
        "body",
        "claimedPath",
        "deliveredAt",
    ] {
        assert!(v.get(key).is_some(), "missing additive key {key}: {json}");
    }
    assert_eq!(v["type"], "handoff");
    assert_eq!(v["claimedPath"], "src/auth.rs");
    assert_eq!(v["deliveredAt"], 1_763_520_000_500u64);
}

#[test]
fn optional_fields_are_absent_when_unset() {
    let msg = A2aMessage::new("msg_1", 1, "a", "b", "task", "do the thing");
    let json = serde_json::to_string(&msg).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert!(v.get("claimedPath").is_none(), "{json}");
    assert!(v.get("deliveredAt").is_none(), "{json}");

    // Old JSON without the optional fields still deserializes.
    let old = r#"{"id":"m","at":2,"from":"a","to":"b","type":"result","body":"ok"}"#;
    let parsed: A2aMessage = serde_json::from_str(old).unwrap();
    assert_eq!(parsed.claimed_path, None);
    assert_eq!(parsed.delivered_at, None);
}

#[test]
fn a2a_message_projects_into_the_frozen_bus_entry_shape() {
    let msg = A2aMessage {
        id: "msg_7".to_string(),
        at: 99,
        from: "S1-OC01".to_string(),
        to: "S2-OC01".to_string(),
        kind: "question".to_string(),
        body: "line one\nline two\nline three".to_string(),
        claimed_path: Some("src/x.rs".to_string()),
        delivered_at: Some(100),
    };
    let entry: BusMessageEntry = msg.to_bus_entry();
    assert_eq!(entry.id, "msg_7");
    assert_eq!(entry.from, "S1-OC01");
    assert_eq!(entry.to, "S2-OC01");
    assert_eq!(entry.kind, "question");
    assert_eq!(entry.preview, "line one");
    assert_eq!(entry.at, 99);

    let v = serde_json::to_value(&entry).unwrap();
    let keys: std::collections::BTreeSet<&str> =
        v.as_object().unwrap().keys().map(String::as_str).collect();
    let expected: std::collections::BTreeSet<&str> = ["id", "from", "to", "type", "preview", "at"]
        .into_iter()
        .collect();
    assert_eq!(keys, expected, "v1.3 busMessages shape changed: {v}");
}

#[test]
fn snapshot_bus_entry_roundtrips_through_state_json() {
    let entry = BusMessageEntry {
        id: "msg_9".to_string(),
        from: "S1-OC01".to_string(),
        to: "S2-OC01".to_string(),
        kind: "escalate".to_string(),
        preview: "loop detected".to_string(),
        at: 123,
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: BusMessageEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, entry.id);
    assert_eq!(back.kind, entry.kind);
    assert_eq!(back.preview, entry.preview);
    assert_eq!(back.at, entry.at);
}
