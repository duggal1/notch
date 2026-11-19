//! Shared harness for the socket fuzz tests. Included with `#[path = ...]`.
//! NOT a cargo test target itself (lives under fixtures/).
//!
//! Every test spawns a REAL `notchd serve` child process with
//!   NOTCH_SOCK=<short /tmp path>   (SUN_LEN is 104 on macOS)
//!   NOTCH_STATE_DIR=<fresh tmp root>
//!   NOTCH_HERDR_CMD / NOTCH_OPENCODE_CMD = scripted fixture fakes
//!   NOTCH_POLL_INTERVAL_MS=3600000  (watchdog/poll loop parked; tests never rely on it)
//! The socket is a real Unix socket carrying real bytes — nothing is mocked
//! inside the process under test.
#![allow(dead_code)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("socket")
        .join(name)
}

fn unique(_tag: &str) -> u64 {
    TMP_COUNTER.fetch_add(1, Ordering::SeqCst)
}

pub fn short_sock(tag: &str) -> PathBuf {
    // Keep below SUN_LEN (104 on macOS).
    PathBuf::from(format!(
        "/tmp/nd-fz-{tag}-{}-{}.sock",
        std::process::id(),
        unique(tag)
    ))
}

/// Connect, retrying ConnectionRefused until `deadline`. The daemon's Unix
/// listener is bound by std with the default backlog (128) and accepts on a
/// 10 ms cadence, so an instantaneous burst of more than ~128 connections
/// can see ECONNREFUSED for the excess; this is what a well-behaved client
/// behind a burst reconnect does (bridge.md: clients behind the same flood
/// reconnect fine).
pub fn connect_with_retry(
    sock: &Path,
    deadline: Duration,
    retry_delay: Duration,
) -> std::io::Result<(UnixStream, u64)> {
    let start = Instant::now();
    let mut refusals = 0u64;
    loop {
        match UnixStream::connect(sock) {
            Ok(s) => return Ok((s, refusals)),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                refusals += 1;
                if start.elapsed() > deadline {
                    return Err(e);
                }
                std::thread::sleep(retry_delay);
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn tmp_root(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "notchd-fz-{tag}-{}-{}",
        std::process::id(),
        unique(tag)
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// One spawned `notchd serve` child plus its socket path and state dir.
pub struct TestDaemon {
    pub child: Child,
    pub sock: PathBuf,
    pub state_dir: PathBuf,
    pub root: PathBuf,
    pub stderr_path: PathBuf,
}

impl TestDaemon {
    pub fn start(tag: &str) -> TestDaemon {
        let root = tmp_root(tag);
        let state_dir = root.join("state");
        let sock = short_sock(tag);
        let _ = std::fs::remove_file(&sock);
        let stderr_path = root.join("daemon.stderr");
        let stderr = std::fs::File::create(&stderr_path).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_notchd"))
            .arg("serve")
            .env("NOTCH_SOCK", &sock)
            .env("NOTCH_STATE_DIR", &state_dir)
            .env("NOTCH_HERDR_CMD", fixture_path("fake_herdr.sh"))
            .env("NOTCH_OPENCODE_CMD", fixture_path("fake_opencode.sh"))
            .env("NOTCH_POLL_INTERVAL_MS", "3600000")
            .env("NOTCH_STUCK_AFTER_SECS", "900")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn notchd serve");
        // Wait until the socket answers with a hello.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match UnixStream::connect(&sock) {
                Ok(stream) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut br = BufReader::new(stream);
                    let mut line = String::new();
                    if br.read_line(&mut line).is_ok() && line.contains("\"hello\"") {
                        break;
                    }
                }
                Err(_) => {}
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "notchd serve did not come up; stderr:\n{}",
                    std::fs::read_to_string(&stderr_path).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        TestDaemon {
            child,
            sock,
            state_dir,
            root,
            stderr_path,
        }
    }

    /// Write the standard 2-supervisor / 2-session plan and return its path.
    pub fn write_plan(&self) -> PathBuf {
        for d in ["a", "b", "c", "d"] {
            std::fs::create_dir_all(self.root.join(d)).unwrap();
        }
        let plan = json!({
            "stuck_after_secs": 900,
            "supervisors": [
                { "sessions": [
                    { "title": "Refactor auth middleware", "prompt": "Refactor the auth middleware to remove the shared session cache", "cwd": self.root.join("a") },
                    { "title": "Fix login race", "prompt": "Fix the race between token refresh and login", "cwd": self.root.join("b") }
                ]},
                { "sessions": [
                    { "title": "Add rate limiter", "prompt": "Add a sliding-window rate limiter to the public API", "cwd": self.root.join("c") },
                    { "title": "Instrument p99 latency", "prompt": "Instrument p99 latency on the charge endpoint", "cwd": self.root.join("d") }
                ]}
            ]
        });
        let path = self.root.join("plan.json");
        std::fs::write(&path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
        path
    }

    /// Run the standard plan; returns the raw ack value (expect ok:true).
    pub fn run_plan(&self) -> Value {
        let plan = self.write_plan();
        let mut c = TestConn::connect(&self.sock);
        let _hello = c.hello();
        c.rpc(&json!({ "cmd": "run_plan", "path": plan }))
    }

    /// RSS in KiB of the child, via `ps` (macOS/BSD). None on failure.
    pub fn rss_kb(&self) -> Option<u64> {
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &self.child.id().to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }

    pub fn events_log(&self) -> String {
        std::fs::read_to_string(self.state_dir.join("events.jsonl")).unwrap_or_default()
    }

    pub fn stderr_log(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    /// Fetch the daemon snapshot over a fresh connection.
    pub fn snapshot(&self) -> Value {
        let mut c = TestConn::connect(&self.sock);
        let _hello = c.hello();
        c.send_and_read(&json!({ "cmd": "snapshot" }))
    }

    pub fn warnings(&self) -> Vec<String> {
        let snap = self.snapshot();
        snap["snapshot"]["warnings"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|w| w.as_str().map(str::to_string))
            .collect()
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One NDJSON client connection with explicit reads/writes.
pub struct TestConn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl TestConn {
    pub fn connect(sock: &Path) -> TestConn {
        Self::connect_with_timeouts(sock, Duration::from_secs(5), Duration::from_secs(30))
    }

    /// Connect, retrying ConnectionRefused (backlog overflow under a connect
    /// burst) until `deadline`. Returns the conn plus the refusal count.
    pub fn connect_retry(sock: &Path, deadline: Duration) -> (TestConn, u64) {
        let (stream, refusals) =
            connect_with_retry(sock, deadline, Duration::from_millis(2)).expect("connect with retry");
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
        let writer = stream.try_clone().unwrap();
        (
            TestConn {
                reader: BufReader::new(stream),
                writer,
            },
            refusals,
        )
    }

    pub fn connect_with_timeouts(
        sock: &Path,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> TestConn {
        let stream = UnixStream::connect(sock).expect("connect socket");
        stream.set_read_timeout(Some(read_timeout)).unwrap();
        stream.set_write_timeout(Some(write_timeout)).unwrap();
        let writer = stream.try_clone().unwrap();
        TestConn {
            reader: BufReader::new(stream),
            writer,
        }
    }

    pub fn hello(&mut self) -> Value {
        let v = self.read_value();
        assert_eq!(v["event"], "hello", "first line must be hello, got {v}");
        v
    }

    pub fn send_raw(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write socket");
        self.writer.flush().expect("flush socket");
    }

    pub fn send_line(&mut self, line: &str) {
        let mut l = String::with_capacity(line.len() + 1);
        l.push_str(line);
        l.push('\n');
        self.send_raw(l.as_bytes());
    }

    /// Read raw bytes until EOF (used to observe a server-initiated close).
    pub fn read_to_end(&mut self) -> Vec<u8> {
        use std::io::Read;
        let mut buf = Vec::new();
        // Best-effort; an error just terminates the read.
        let _ = self.reader.read_to_end(&mut buf);
        buf
    }

    pub fn read_line_result(&mut self) -> std::io::Result<String> {
        let mut l = String::new();
        self.reader.read_line(&mut l).map(|_| l)
    }

    pub fn read_line(&mut self) -> String {
        let mut l = String::new();
        self.reader.read_line(&mut l).expect("read line");
        l
    }

    pub fn read_value(&mut self) -> Value {
        let l = self.read_line();
        serde_json::from_str(l.trim_end())
            .unwrap_or_else(|e| panic!("bad json line: {e}: {l:?}"))
    }

    pub fn send_and_read(&mut self, obj: &Value) -> Value {
        self.send_line(&obj.to_string());
        self.read_value()
    }

    /// Single-command RPC that produces exactly one response line.
    pub fn rpc(&mut self, obj: &Value) -> Value {
        self.send_line(&obj.to_string());
        self.read_value()
    }

    /// Subscribe helper: returns (ack, initial_snapshot).
    pub fn subscribe(&mut self) -> (Value, Value) {
        self.send_line(r#"{"cmd":"subscribe"}"#);
        let ack = self.read_value();
        let snap = self.read_value();
        assert_eq!(ack["event"], "ack");
        assert_eq!(ack["cmd"], "subscribe");
        assert_eq!(ack["ok"], true);
        assert_eq!(snap["event"], "snapshot");
        (ack, snap)
    }

    /// Poll for a line matching `pred` until `deadline`. `pred` receives the
    /// parsed JSON value. Lines that fail to parse are skipped.
    /// Returns the matching value and the elapsed time since the call began.
    pub fn wait_for_line<F>(&mut self, deadline: Instant, mut pred: F) -> Option<(Value, Duration)>
    where
        F: FnMut(&Value) -> bool,
    {
        let start = Instant::now();
        loop {
            if Instant::now() >= deadline {
                return None;
            }
            match self.read_line_result() {
                Ok(l) => {
                    if l.is_empty() {
                        return None; // EOF
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(l.trim_end()) {
                        if pred(&v) {
                            return Some((v, start.elapsed()));
                        }
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(_) => return None,
            }
        }
    }
}

/// Wall-clock deadline `ms` milliseconds from now.
pub fn deadline_in(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
}
