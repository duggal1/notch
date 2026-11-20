use notchd::{Config, Daemon, Herdr, SystemClock};
use serde_json::json;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct HeadlessHerdr {
    process: Child,
    root: PathBuf,
}

impl Drop for HeadlessHerdr {
    fn drop(&mut self) {
        // Stop only the headless server this test started.
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn open_pane_survives_poll_before_agent_registration() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(format!("/tmp/notch-pane-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let config_file = root.join("config.toml");
    std::fs::write(&config_file, "").unwrap();
    // This integration test has a dedicated process. Only its Herdr subprocesses
    // inherit these paths, and no command can reach the user's Herdr server.
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        std::env::set_var(key, &root);
    }
    for key in [
        "HERDR_SESSION",
        "HERDR_SOCKET_PATH",
        "HERDR_CLIENT_SOCKET_PATH",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
    ] {
        std::env::remove_var(key);
    }
    std::env::set_var("HERDR_CONFIG_PATH", &config_file);
    std::env::set_var("HERDR_DISABLE_SOUND", "1");
    std::env::set_var("NOTCH_HERDR_CMD", "herdr");

    let process = Command::new("herdr")
        .arg("server")
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("start installed Herdr in headless mode");
    let mut server = HeadlessHerdr { process, root };
    let socket = server.root.join("herdr/herdr.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(&socket).is_err() {
        assert!(server.process.try_wait().unwrap().is_none());
        assert!(
            Instant::now() < deadline,
            "Herdr socket did not become ready"
        );
        std::thread::yield_now();
    }

    let workspace = Command::new("herdr")
        .args([
            "workspace",
            "create",
            "--cwd",
            server.root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(workspace.status.success(), "{workspace:?}");
    let herdr = Herdr::from_env();
    let initial_panes = herdr.pane_list().unwrap();
    std::env::set_var("HERDR_PANE_ID", &initial_panes[0].pane_id);
    let pane = herdr
        .pane_split(server.root.to_str().unwrap(), &[])
        .unwrap();
    assert!(herdr.agent_list().unwrap().is_empty());
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
                "paneId":pane
            }]
        })
        .to_string(),
    )
    .unwrap();
    let daemon = Daemon::open(
        Config {
            state_dir: state,
            tmp_dir: server.root.join("tmp"),
            opencode_decider: false,
            ..Config::default()
        },
        herdr.clone(),
        Arc::new(SystemClock),
    )
    .unwrap();

    daemon.poll_once();
    let snapshot = serde_json::to_value(daemon.snapshot()).unwrap();
    assert_eq!(snapshot["sessions"][0]["status"], "working");
    assert!(snapshot["sessions"][0]["lastRevision"].is_null());

    daemon.reconcile();
    assert_eq!(
        serde_json::to_value(daemon.snapshot()).unwrap()["sessions"][0]["status"],
        "working"
    );

    let reported = Command::new("herdr")
        .args([
            "pane",
            "report-agent",
            "--source",
            "notch-test",
            "--agent",
            "opencode",
            "--state",
            "idle",
            &pane,
        ])
        .output()
        .unwrap();
    assert!(reported.status.success(), "{reported:?}");
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
