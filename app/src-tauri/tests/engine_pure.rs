//! Pure bits of the engine: pane-id parsing (herdr 0.9.5 output shapes).
//!
//! Integration against the real engine (empty tree, re-adopt, temp-dir state)
//! is in engine_integration.rs. Destructive herdr verbs are never exercised.

use notch::engine::parse_pane_id;

#[test]
fn pane_id_plain_token() {
    assert_eq!(parse_pane_id("w80:p2\n"), Some("w80:p2".to_string()));
    assert_eq!(parse_pane_id("created pane w80:p2 successfully"), Some("w80:p2".to_string()));
    assert_eq!(parse_pane_id("w8W:p5 split into w8W:p6"), Some("w8W:p5".to_string()));
}

#[test]
fn pane_id_json_envelope() {
    // What `herdr pane split` would most likely emit (the CLI wraps everything in {id, result}).
    let json = r#"{"id":"cli:pane:split","result":{"pane_id":"w80:p2","opened":true}}"#;
    assert_eq!(parse_pane_id(json), Some("w80:p2".to_string()));

    let nested = r#"{"id":"cli:pane:split","result":{"pane":{"id":"w1A:p7"}}}"#;
    assert_eq!(parse_pane_id(nested), Some("w1A:p7".to_string()));

    let ndjson = "{\"id\":\"cli:pane:split\",\"result\":{\"pane_id\":\"w80:p3\"}}\n{\"id\":\"cli:agent:list\",\"result\":{}}\n";
    assert_eq!(parse_pane_id(ndjson.lines().next().unwrap()), Some("w80:p3".to_string()));
}

#[test]
fn pane_id_unparseable() {
    assert_eq!(parse_pane_id("ok"), None);
    assert_eq!(parse_pane_id(""), None);
    assert_eq!(parse_pane_id(r#"{"id":"x","result":{"something":"else"}}"#), None);
    // safer to return None than to invent an id
    assert_eq!(parse_pane_id("pane p2"), None);
}
