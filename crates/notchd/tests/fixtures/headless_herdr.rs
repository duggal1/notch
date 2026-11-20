use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static HERDR_ENV: Mutex<()> = Mutex::new(());

pub struct HeadlessHerdr {
    _env: MutexGuard<'static, ()>,
    process: Child,
    pub root: PathBuf,
}

impl Drop for HeadlessHerdr {
    fn drop(&mut self) {
        // Stop only the headless server this test started.
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl HeadlessHerdr {
    pub fn start() -> Self {
        let env = HERDR_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = PathBuf::from(format!("/tmp/notch-herdr-{}-{nonce}", std::process::id()));
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
        std::env::set_var("SHELL", "/bin/sh");
        std::env::set_var("NOTCH_HERDR_CMD", "herdr");

        let process = Command::new("herdr")
            .arg("server")
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("start installed Herdr in headless mode");
        let mut server = HeadlessHerdr {
            _env: env,
            process,
            root,
        };
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
        let initial_panes = notchd::Herdr::from_env().pane_list().unwrap();
        std::env::set_var("HERDR_PANE_ID", &initial_panes[0].pane_id);
        server
    }

    pub fn report(&self, pane: &str, status: &str) {
        let reported = Command::new("herdr")
            .args([
                "pane",
                "report-agent",
                "--source",
                "notch-test",
                "--agent",
                "opencode",
                "--state",
                status,
                pane,
            ])
            .output()
            .unwrap();
        assert!(reported.status.success(), "{reported:?}");
    }
}
