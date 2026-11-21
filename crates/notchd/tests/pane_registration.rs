#[path = "fixtures/headless_herdr.rs"]
mod headless_herdr;

use headless_herdr::HeadlessHerdr;
use notchd::{Config, Daemon, Herdr, SystemClock};
use serde_json::json;
use std::sync::Arc;

#[test]
fn open_pane_survives_poll_before_agent_registration() {
    let server = HeadlessHerdr::start();
    let herdr = Herdr::from_env();
    let pane = herdr
        .pane_split(server.root.to_str().unwrap(), &[])
        .unwrap();
    assert!(herdr.agent_list().unwrap().is_empty());
    let daemon = open_session(&server, herdr.clone(), &pane, None);

    daemon.poll_once();
    let snapshot = serde_json::to_value(daemon.snapshot()).unwrap();
    assert_eq!(snapshot["sessions"][0]["status"], "working");
    assert!(snapshot["sessions"][0]["lastRevision"].is_null());

    daemon.reconcile();
    assert_eq!(
        serde_json::to_value(daemon.snapshot()).unwrap()["sessions"][0]["status"],
        "working"
    );

    server.report(&pane, "idle");
    daemon.poll_once();
    let snapshot = serde_json::to_value(daemon.snapshot()).unwrap();
    assert_eq!(snapshot["sessions"][0]["status"], "idle");
    assert!(snapshot["sessions"][0]["lastRevision"].is_u64());

    herdr.pane_close(&pane).unwrap();
    assert!(herdr
        .pane_list()
        .unwrap()
        .iter()
        .all(|entry| entry.pane_id != pane));
    daemon.poll_once();
    assert_eq!(
        serde_json::to_value(daemon.snapshot()).unwrap()["sessions"][0]["status"],
        "done"
    );
    daemon.flush_state();
}

fn open_session(server: &HeadlessHerdr, herdr: Herdr, pane: &str, goal: Option<&str>) -> Daemon {
    let state = server.root.join("notch-state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(
        state.join("state.json"),
        json!({
            "version":1,
            "supervisors": [{"id":"SV-1", "number":1, "title":"Pane registration"}],
            "sessions": [{
                "id":"S1-OC01", "sup":"SV-1", "status":"working",
                "title":"Pending agent", "task":"Run task", "cwd":server.root,
                "paneId":pane, "goal":goal
            }]
        })
        .to_string(),
    )
    .unwrap();
    Daemon::open(
        Config {
            state_dir: state,
            tmp_dir: server.root.join("tmp"),
            opencode_decider: false,
            ..Config::default()
        },
        herdr,
        Arc::new(SystemClock),
    )
    .unwrap()
}

#[test]
fn failed_opencode_probe_surfaces_without_stopping_herdr_polling() {
    let server = HeadlessHerdr::start();
    let herdr = Herdr::from_env();
    let pane = herdr
        .pane_split(server.root.to_str().unwrap(), &[])
        .unwrap();
    server.report(&pane, "idle");
    // Exercise an actual executable lookup failure, with no OpenCode process.
    std::env::set_var("NOTCH_OPENCODE_CMD", server.root.join("missing-opencode"));
    let daemon = open_session(&server, herdr, &pane, Some("Check local task"));
    daemon.poll_once();
    let snapshot = daemon.snapshot();
    assert_eq!(snapshot.sessions[0].status, notchd::status::Status::Idle);
    assert!(
        snapshot
            .warnings
            .iter()
            .any(|warning| warning.starts_with("OpenCode session probe failed")),
        "probe error disappeared: {:?}",
        snapshot.warnings
    );
    daemon.flush_state();
}
