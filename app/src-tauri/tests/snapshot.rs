//! serde wire-contract tests against docs/bridge.md §1.3.
//!
//! - keys are camelCase on the wire, never snake_case
//! - the four statuses roundtrip verbatim
//! - extra (unknown) fields in v1 are tolerated on read
//! - the four statuses are exactly "working" | "idle" | "failed" | "done"

use notch::snapshot::{compare_ids, Snapshot, Status};

/// The exact §1.3 example object (plus realistic values).
const EXAMPLE: &str = r#"{
  "version": 1,
  "generatedAt": 1763520000000,
  "orchestrator": { "id": "ORCH", "status": "working" },
  "supervisors": [
    { "id": "SV-1", "status": "working", "working": 2, "idle": 0, "failed": 0, "done": 0 },
    { "id": "SV-2", "status": "failed", "working": 0, "idle": 0, "failed": 1, "done": 1 }
  ],
  "sessions": [
    {
      "id": "S1-OC01", "sup": "SV-1", "status": "working",
      "title": "Refactor auth middleware",
      "task": "Refactor auth middleware",
      "cwd": "/Users/x/proj",
      "paneId": "w80:p2",
      "startedAt": 1763520000000,
      "lastAction": "editing src/auth.rs",
      "lastRevision": 12,
      "latencyMs": 840.0,
      "failureReason": null
    },
    {
      "id": "S2-OC03", "sup": "SV-2", "status": "failed",
      "title": "Fix flaky watchdog test",
      "task": "Fix flaky watchdog test",
      "cwd": "/Users/x/proj",
      "paneId": "w80:p4",
      "startedAt": 1763520001000,
      "lastAction": "",
      "lastRevision": 9,
      "latencyMs": null,
      "failureReason": "stuck"
    }
  ],
  "eps": 6.5,
  "conflicts": [ { "file": "src/auth.rs", "sessions": ["S1-OC01", "S2-OC03"] } ],
  "attribution": [ { "session": "S1-OC01", "file": "src/auth.rs" } ],
  "warnings": ["one warning"]
}"#;

#[test]
fn example_parses_and_roundtrips() {
    let snap: Snapshot = serde_json::from_str(EXAMPLE).expect("parse §1.3 example");
    assert_eq!(snap.version, 1);
    assert_eq!(snap.generated_at, 1_763_520_000_000);
    assert_eq!(snap.orchestrator.id, "ORCH");
    assert_eq!(snap.supervisors.len(), 2);
    assert_eq!(snap.sessions.len(), 2);
    assert_eq!(snap.sessions[0].pane_id, "w80:p2");
    assert_eq!(snap.sessions[0].last_revision, 12);
    assert_eq!(snap.sessions[0].latency_ms, Some(840.0));
    assert_eq!(snap.sessions[1].failure_reason.as_deref(), Some("stuck"));
    assert_eq!(
        snap.conflicts[0].sessions,
        vec!["S1-OC01".to_string(), "S2-OC03".to_string()]
    );

    let text = serde_json::to_string(&snap).expect("serialize");
    let snap2: Snapshot = serde_json::from_str(&text).expect("re-parse");
    assert_eq!(snap, snap2);
}

#[test]
fn wire_keys_are_camel_case_not_snake_case() {
    let snap: Snapshot = serde_json::from_str(EXAMPLE).unwrap();
    let text = serde_json::to_string(&snap).unwrap();
    for key in [
        "\"generatedAt\"",
        "\"paneId\"",
        "\"startedAt\"",
        "\"lastAction\"",
        "\"lastRevision\"",
        "\"latencyMs\"",
        "\"failureReason\"",
        "\"eps\"",
        "\"conflicts\"",
        "\"attribution\"",
        "\"warnings\"",
    ] {
        assert!(text.contains(key), "missing camelCase key {key} in {text}");
    }
    for banned in [
        "\"generated_at\"",
        "\"pane_id\"",
        "\"started_at\"",
        "\"last_action\"",
        "\"last_revision\"",
        "\"latency_ms\"",
        "\"failure_reason\"",
    ] {
        assert!(
            !text.contains(banned),
            "snake_case leaked onto the wire: {banned}"
        );
    }
}

#[test]
fn the_four_statuses_roundtrip() {
    for (status, wire) in [
        (Status::Working, "\"working\""),
        (Status::Idle, "\"idle\""),
        (Status::Failed, "\"failed\""),
        (Status::Done, "\"done\""),
    ] {
        assert_eq!(serde_json::to_string(&status).unwrap(), wire);
        let back: Status = serde_json::from_str(wire).unwrap();
        assert_eq!(back, status);
    }
}

#[test]
fn unknown_extra_fields_are_tolerated() {
    // A future v1.x adds fields; the shell must keep parsing.
    let with_extra = EXAMPLE.replace(
        "\"eps\": 6.5,",
        "\"futureField\": {\"nested\": [1, 2, 3]}, \"eps\": 6.5,",
    );
    let snap: Snapshot =
        serde_json::from_str(&with_extra).expect("unknown top-level field tolerated");
    assert_eq!(snap.eps, 6.5);

    let session_extra = EXAMPLE.replace(
        "\"latencyMs\": 840.0,",
        "\"latencyMs\": 840.0, \"newSessionField\": \"hello\",",
    );
    let snap: Snapshot =
        serde_json::from_str(&session_extra).expect("unknown session field tolerated");
    assert_eq!(snap.sessions[0].latency_ms, Some(840.0));
}

#[test]
fn empty_tree_is_legal() {
    let snap: Snapshot = serde_json::from_str(
        r#"{
        "version": 1, "generatedAt": 1763520000000,
        "orchestrator": { "id": "ORCH", "status": "idle" },
        "supervisors": [], "sessions": [], "eps": 0,
        "conflicts": [], "attribution": [], "warnings": []
    }"#,
    )
    .unwrap();
    assert!(snap.supervisors.is_empty() && snap.sessions.is_empty());
    assert_eq!(snap.eps, 0.0);

    let text = serde_json::to_string(&snap).unwrap();
    let snap2: Snapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(snap, snap2);
}

#[test]
fn ids_sort_numerically_not_lexicographically() {
    let mut sessions = vec!["S2-OC1", "S1-OC10", "S1-OC2"];
    sessions.sort_by(|a, b| compare_ids(a, b));
    assert_eq!(sessions, vec!["S1-OC2", "S1-OC10", "S2-OC1"]);

    let mut sups = vec!["SV-10", "SV-2", "SV-1"];
    sups.sort_by(|a, b| compare_ids(a, b));
    assert_eq!(sups, vec!["SV-1", "SV-2", "SV-10"]);
}
