//! Red tests for the event-driven agent source: herdr events primary, CLI
//! poll fallback, signal logging, and strict managed-pane filtering. The fake
//! herdr is a real Unix socket speaking the documented v22 JSON-line protocol.

use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::eventsource::{managed_panes, run_source, EventDrivenSource};
use notchd::herdr::{Herdr, HerdrError};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-eventsource-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fake_herdr(calls: Arc<Mutex<Vec<Vec<String>>>>) -> Herdr {
    Herdr::with_runner(move |args: &[String]| {
        calls.lock().unwrap().push(args.to_vec());
        match (args.get(1).map(String::as_str), args.get(2).map(String::as_str)) {
            (Some("agent"), Some("list")) => Ok(json!({"result":{"agents":[]}}).to_string()),
            (Some("pane"), Some("split")) => Ok(json!({"result":{"pane":{"pane_id":"w90:p7"}}}).to_string()),
            (Some("agent"), Some("start")) => Ok(json!({"result":{"type":"ok"}}).to_string()),
            _ => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected: {}", args.join(" ")),
            }),
        }
    })
}

struct FakeSocket {
    path: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    event_tx: Sender<String>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    _root: PathBuf,
}

impl FakeSocket {
    fn start(tag: &str) -> Self {
        let root = scratch(tag);
        let path = root.join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, event_rx): (Sender<String>, Receiver<String>) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let r = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).ok();
                        stream.set_read_timeout(Some(Duration::from_millis(200))).ok();
                        let mut reader = BufReader::new(stream);
                        let mut line = String::new();
                        let got = reader.read_line(&mut line);
                        if matches!(got, Ok(0) | Err(_)) || line.trim().is_empty() {
                            continue;
                        }
                        r.lock().unwrap().push(line.trim().to_string());
                        let ack = json!({"id":"fake","result":{"type":"subscription_started"}});
                        if writeln!(reader.get_mut(), "{ack}").is_err() || reader.get_mut().flush().is_err() {
                            continue;
                        }
                        // Keep this connection alive and deliver every
                        // pushed event: a real subscription is long-lived.
                        reader.get_mut().set_read_timeout(Some(Duration::from_millis(20))).ok();
                        while !s.load(Ordering::SeqCst) {
                            if let Ok(event) = event_rx.recv_timeout(Duration::from_millis(20)) {
                                let _ = writeln!(reader.get_mut(), "{event}");
                                let _ = reader.get_mut().flush();
                            }
                            let mut b = [0u8; 1];
                            let _ = reader.get_mut().read(&mut b);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::yield_now(),
                    Err(_) => break,
                }
            }
        });
        Self { path, requests, event_tx, stop, handle: Some(handle), _root: root }
    }

    fn push(&self, event: Value) {
        self.event_tx.send(event.to_string()).unwrap();
    }
}

impl Drop for FakeSocket {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_dir_all(&self._root);
    }
}

type TestDaemon = (Arc<Daemon>, Arc<Mutex<Vec<Vec<String>>>>, PathBuf);

fn daemon_with_session(tag: &str, poll_ms: u64) -> TestDaemon {
    let root = scratch(tag);
    let config = Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs: 900,
        poll_interval_ms: poll_ms,
        opencode_decider: false,
    };
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    let calls: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let d = Arc::new(Daemon::open(config, fake_herdr(Arc::clone(&calls)), Arc::new(SystemClock)).unwrap());
    let ack = d.submit_task("fleet event task".into(), None, None, None, None, None);
    assert!(ack.ok);
    (d, calls, root)
}

fn until<F: Fn() -> bool>(timeout: Duration, f: F) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() { return true; }
        thread::yield_now();
    }
    f()
}

#[test]
fn herdr_event_changes_only_the_managed_pane_and_logs_herdr_event_signal() {
    let (daemon, calls, _root) = daemon_with_session("primary", 3_600_000);
    assert_eq!(managed_panes(&daemon), vec!["w90:p7".to_string()]);
    let fake = FakeSocket::start("primary-sock");

    let shutdown = Arc::new(AtomicBool::new(false));
    let source = EventDrivenSource::connect_at(&fake.path, &managed_panes(&daemon), Arc::clone(&shutdown));
    assert_eq!(source.source_name(), "herdr_events");
    let stats = source.stats();
    let d = Arc::clone(&daemon);
    let s = Arc::clone(&shutdown);
    let handle = thread::spawn(move || run_source(d, s, source));

    fake.push(json!({"event":"pane.agent_status_changed","data":{
        "pane_id":"w90:p7","workspace_id":"w90","agent_status":"idle"}}));
    assert!(until(Duration::from_secs(3), || stats.herdr_events() >= 1), "no herdr event processed");
    // The event fired, so NO CLI poll fallback should have run.
    assert_eq!(stats.cli_polls(), 0, "an event must not fall back to the CLI poll");

    let reqs = fake.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1, "one new connection per subscription");
    let req: Value = serde_json::from_str(&reqs[0]).unwrap();
    assert_eq!(req["method"], "events.subscribe");
    assert_eq!(req["params"]["subscriptions"][0]["type"], "pane.agent_status_changed");
    assert_eq!(req["params"]["subscriptions"][0]["pane_id"], "w90:p7");

    assert!(until(Duration::from_secs(2), || {
        daemon.snapshot().sessions.iter().any(|s| s.id == "S1-OC01" && s.status.as_str() == "idle")
    }), "managed session status never changed: {}", serde_json::to_string(&daemon.snapshot().sessions).unwrap());

    // An event for a pane we do not own must be observed but never acted on.
    fake.push(json!({"event":"pane.agent_status_changed","data":{
        "pane_id":"w99:p9","workspace_id":"w99","agent_status":"working"}}));
    assert!(until(Duration::from_secs(2), || stats.events_seen() >= 2), "second event not seen");
    assert_eq!(stats.herdr_events(), 1, "unmanaged pane must not trigger a status update");
    assert_eq!(stats.cli_polls(), 0);
    assert!(daemon.snapshot().sessions.iter().any(|s| s.status.as_str() == "idle"));

    let list_before = calls.lock().unwrap().iter().filter(|a| a.get(1).map(String::as_str) == Some("agent") && a.get(2).map(String::as_str) == Some("list")).count();
    assert_eq!(list_before, 0, "event path performs no CLI agent list");

    let log = std::fs::read_to_string(daemon.config.state_dir.join("events.jsonl")).unwrap();
    assert!(log.contains("agent_source") && log.contains("signal=herdr_event"), "signal not logged: {log}");

    shutdown.store(true, Ordering::SeqCst);
    handle.join().unwrap();
}

#[test]
fn cli_poll_fallback_logs_its_signal_and_drives_the_daemon() {
    let (daemon, calls, root) = daemon_with_session("fallback", 25);
    let missing = root.join("no-such-herdr.sock");
    let shutdown = Arc::new(AtomicBool::new(false));
    let source = EventDrivenSource::connect_at(&missing, &managed_panes(&daemon), Arc::clone(&shutdown));
    assert_eq!(source.source_name(), "cli_poll");
    let stats = source.stats();
    let d = Arc::clone(&daemon);
    let s = Arc::clone(&shutdown);
    let handle = thread::spawn(move || run_source(d, s, source));

    let is_list = |a: &Vec<String>| a.get(1).map(String::as_str) == Some("agent") && a.get(2).map(String::as_str) == Some("list");
    assert!(until(Duration::from_secs(3), || stats.cli_polls() >= 2), "fallback poll counter never advanced");
    assert!(until(Duration::from_secs(3), || calls.lock().unwrap().iter().filter(|a| is_list(a)).count() >= 2), "fallback CLI poll never ran twice");
    let logs = calls.lock().unwrap().iter().filter(|a| is_list(a)).count();
    assert!(logs >= 2, "fallback must run the existing CLI agent list, got {logs}");
    let log = std::fs::read_to_string(daemon.config.state_dir.join("events.jsonl")).unwrap();
    assert!(log.contains("signal=cli_poll_fallback"), "fallback signal not logged: {log}");

    shutdown.store(true, Ordering::SeqCst);
    handle.join().unwrap();
}
