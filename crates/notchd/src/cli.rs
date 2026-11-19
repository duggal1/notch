//! CLI — thin wrappers over the library.
//!
//! If a live `notchd serve` daemon is reachable on the socket, commands are
//! sent to it over NDJSON (same protocol as Swift/Tauri clients) so a
//! mutating CLI call can never be silently overwritten by the daemon's own
//! state writes. If no daemon is running, commands fall back to operating
//! in-process on the state dir. `serve` itself is always in-process.

use crate::clock::SystemClock;
use crate::config::Config;
use crate::daemon::Daemon;
use crate::herdr::Herdr;
use crate::paths;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const USAGE: &str = "notchd — OpenCode orchestration control plane

USAGE:
  notchd serve [--opencode-decider|--deterministic]   Run the daemon (socket at $NOTCH_SOCK)
  notchd status                                        Print daemon status JSON
  notchd snapshot                                      Print the snapshot JSON
  notchd run-plan <file>                               Spawn supervisors/sessions from a plan
  notchd submit '<task>' [--cwd <dir>] [--title <t>] [--dod <definition of done>] [--model <m>] [--supervisor <SV-n>]   Submit a task and spawn it
  notchd message --from S1-OC01 --to S1-OC02 --type handoff --body '…'   Deliver a bus message to a session
  notchd pause <SV-n> | resume <SV-n>                                    Pause/resume the supervisor's decisions
  notchd kill-switch                                                     Close every managed pane and mark sessions failed
  notchd kill <id> [--reason <reason>]                 Kill a session (pane close + cleanup)
  notchd reprompt <id> <text>                          Re-prompt a session
  notchd cleanup [--id <id>]                           Sweep dead sessions (or one session's temp dir)
  notchd logs <id> [--tail n]                          Print recent pane output lines
  notchd focus <id>                                    Focus the session's pane in herdr

Commands talk to a running `notchd serve` over the socket when one is
reachable; otherwise they operate on the state dir in-process.
";

pub fn run(argv: Vec<String>) -> i32 {
    let mut args = argv.into_iter();
    let _program = args.next().unwrap_or_else(|| "notchd".to_string());
    let cmd = args.next().unwrap_or_else(|| "help".to_string());
    let rest: Vec<String> = args.collect();

    let match_value = match cmd.as_str() {
        "serve" => cmd_serve(&rest),
        "status" => with_target(|t| match t {
            Target::Live(mut c) => match c.request(serde_json::json!({"cmd": "status"})) {
                Ok(v) => print_ok(&v),
                Err(e) => fail(&e),
            },
            Target::Local(d) => {
                println!("{}", serde_json::to_string(&d.status()).unwrap_or_default());
                0
            }
        }),
        "snapshot" => with_target(|t| match t {
            Target::Live(mut c) => match c.request(serde_json::json!({"cmd": "snapshot"})) {
                Ok(v) => {
                    let snap = v.get("snapshot").cloned().unwrap_or(v);
                    println!("{}", serde_json::to_string_pretty(&snap).unwrap_or_default());
                    0
                }
                Err(e) => fail(&e),
            },
            Target::Local(d) => {
                println!("{}", serde_json::to_string_pretty(&d.snapshot()).unwrap_or_default());
                0
            }
        }),
        "run-plan" => {
            let Some(file) = rest.first() else {
                eprintln!("usage: notchd run-plan <file>\n{USAGE}");
                return 2;
            };
            with_target(|t| match t {
                Target::Live(mut c) => match c.request(serde_json::json!({"cmd": "run_plan", "path": file})) {
                    Ok(v) => ack_or_fail(&v),
                    Err(e) => fail(&e),
                },
                Target::Local(d) => {
                    let ack = d.run_plan(std::path::Path::new(file));
                    println!("{}", serde_json::to_string(&ack).unwrap_or_default());
                    if ack.ok { 0 } else { 1 }
                }
            })
        }
        "submit" => {
            let Some(task) = rest.first().cloned() else {
                eprintln!("usage: notchd submit '<task>' [--cwd <dir>] [--title <title>] [--dod <definition of done>]\n{USAGE}");
                return 2;
            };
            let cwd = flag_value(&rest, "--cwd");
            let title = flag_value(&rest, "--title");
            let dod = flag_value(&rest, "--dod");
            let model = flag_value(&rest, "--model");
            let supervisor = flag_value(&rest, "--supervisor");
            with_target(|t| match t {
                Target::Live(mut c) => {
                    let mut req = serde_json::json!({"cmd": "submit_task", "task": task});
                    if let Some(dir) = &cwd {
                        req["cwd"] = serde_json::Value::String(dir.clone());
                    }
                    if let Some(title) = &title {
                        req["title"] = serde_json::Value::String(title.clone());
                    }
                    if let Some(dod) = &dod {
                        req["definitionOfDone"] = serde_json::Value::String(dod.clone());
                    }
                    if let Some(model) = &model {
                        req["model"] = serde_json::Value::String(model.clone());
                    }
                    if let Some(sup) = &supervisor {
                        req["supervisorId"] = serde_json::Value::String(sup.clone());
                    }
                    match c.request(req) {
                        Ok(v) => ack_or_fail(&v),
                        Err(e) => fail(&e),
                    }
                }
                Target::Local(d) => {
                    let ack = d.submit_task(task, cwd, title, dod, model, supervisor);
                    let ok = ack.ok;
                    let mut obj = serde_json::to_value(&ack)
                        .ok()
                        .and_then(|s| s.as_object().cloned())
                        .unwrap_or_default();
                    obj.insert("event".into(), "ack".into());
                    obj.insert("cmd".into(), "submit_task".into());
                    println!("{}", serde_json::Value::Object(obj));
                    if ok { 0 } else { 1 }
                }
            })
        }
        "message" => {
            let from = flag_value(&rest, "--from").unwrap_or_default();
            let to = flag_value(&rest, "--to").unwrap_or_default();
            let kind = flag_value(&rest, "--type").unwrap_or_default();
            let body = match flag_value(&rest, "--body") {
                Some(b) => b,
                None => {
                    eprintln!("usage: notchd message --from <id> --to <id> --type <task|result|question|handoff|escalate> --body '<text>'\n{USAGE}");
                    return 2;
                }
            };
            if from.is_empty() || to.is_empty() || kind.is_empty() {
                eprintln!("usage: notchd message --from <id> --to <id> --type <task|result|question|handoff|escalate> --body '<text>'\n{USAGE}");
                return 2;
            }
            with_target(|t| match t {
                Target::Live(mut c) => match c.request(serde_json::json!({
                    "cmd": "message", "from": from, "to": to, "type": kind, "body": body,
                })) {
                    Ok(v) => ack_or_fail(&v),
                    Err(e) => fail(&e),
                },
                Target::Local(d) => {
                    let ack = d.message(&from, &to, &kind, &body);
                    let ok = ack.ok;
                    print_merged_ack("message", serde_json::to_value(&ack).unwrap_or_default());
                    if ok { 0 } else { 1 }
                }
            })
        }
        "pause" => {
            let Some(id) = rest.first().cloned() else {
                eprintln!("usage: notchd pause <SV-n>\n{USAGE}");
                return 2;
            };
            with_target(|t| match t {
                Target::Live(mut c) => match c.request(serde_json::json!({"cmd": "pause_supervisor", "id": id})) {
                    Ok(v) => ack_or_fail(&v),
                    Err(e) => fail(&e),
                },
                Target::Local(d) => match d.pause_supervisor(&id) {
                    Ok(a) => {
                        print_merged_ack("pause_supervisor", serde_json::to_value(&a).unwrap_or_default());
                        0
                    }
                    Err(e) => fail(&e),
                },
            })
        }
        "resume" => {
            let Some(id) = rest.first().cloned() else {
                eprintln!("usage: notchd resume <SV-n>\n{USAGE}");
                return 2;
            };
            with_target(|t| match t {
                Target::Live(mut c) => match c.request(serde_json::json!({"cmd": "resume_supervisor", "id": id})) {
                    Ok(v) => ack_or_fail(&v),
                    Err(e) => fail(&e),
                },
                Target::Local(d) => match d.resume_supervisor(&id) {
                    Ok(a) => {
                        print_merged_ack("resume_supervisor", serde_json::to_value(&a).unwrap_or_default());
                        0
                    }
                    Err(e) => fail(&e),
                },
            })
        }
        "kill-switch" => {
            with_target(|t| match t {
                Target::Live(mut c) => match c.request(serde_json::json!({"cmd": "kill_switch"})) {
                    Ok(v) => ack_or_fail(&v),
                    Err(e) => fail(&e),
                },
                Target::Local(d) => {
                    let ack = d.kill_switch();
                    print_merged_ack("kill_switch", serde_json::to_value(&ack).unwrap_or_default());
                    0
                }
            })
        }
        "kill" => {
            let Some(id) = rest.first().cloned() else {
                eprintln!("usage: notchd kill <id> [--reason <reason>]\n{USAGE}");
                return 2;
            };
            let reason = flag_value(&rest, "--reason");
            with_target(|t| match t {
                Target::Live(mut c) => {
                    let mut req = serde_json::json!({"cmd": "kill", "id": id});
                    if let Some(r) = &reason {
                        req["reason"] = serde_json::Value::String(r.clone());
                    }
                    match c.request(req) {
                        Ok(v) => ack_or_fail(&v),
                        Err(e) => fail(&e),
                    }
                }
                Target::Local(d) => match d.kill(&id, reason) {
                    Ok(a) => print_ok(&serde_json::json!({"event":"ack","cmd":"kill","ok":true,"id":a.id,"reason":a.reason})),
                    Err(e) => fail(&e),
                },
            })
        }
        "reprompt" => {
            let Some(id) = rest.first().cloned() else {
                eprintln!("usage: notchd reprompt <id> <text>\n{USAGE}");
                return 2;
            };
            let text = rest[1..].join(" ");
            if text.is_empty() {
                eprintln!("usage: notchd reprompt <id> <text>\n{USAGE}");
                return 2;
            }
            with_target(|t| match t {
                Target::Live(mut c) => match c.request(serde_json::json!({"cmd":"reprompt","id":id,"text":text})) {
                    Ok(v) => ack_or_fail(&v),
                    Err(e) => fail(&e),
                },
                Target::Local(d) => match d.reprompt(&id, &text) {
                    Ok(_) => print_ok(&serde_json::json!({"event":"ack","cmd":"reprompt","ok":true,"id":id})),
                    Err(e) => fail(&e),
                },
            })
        }
        "cleanup" => {
            let id = flag_value(&rest, "--id");
            with_target(|t| match t {
                Target::Live(mut c) => {
                    let mut req = serde_json::json!({"cmd": "cleanup"});
                    if let Some(i) = &id {
                        req["id"] = serde_json::Value::String(i.clone());
                    }
                    match c.request(req) {
                        Ok(v) => ack_or_fail(&v),
                        Err(e) => fail(&e),
                    }
                }
                Target::Local(d) => {
                    let ack = d.cleanup(id.as_deref());
                    println!("{}", serde_json::to_string(&ack).unwrap_or_default());
                    0
                }
            })
        }
        "logs" => {
            let Some(id) = rest.first().cloned() else {
                eprintln!("usage: notchd logs <id> [--tail n]\n{USAGE}");
                return 2;
            };
            let tail = flag_value(&rest, "--tail").and_then(|v| v.parse::<usize>().ok());
            with_target(|t| match t {
                Target::Live(mut c) => {
                    let mut req = serde_json::json!({"cmd":"logs","id":id});
                    if let Some(n) = tail {
                        req["tail"] = serde_json::Value::from(n as u64);
                    }
                    match c.request(req) {
                        Ok(v) => {
                            if v.get("event").and_then(|e| e.as_str()) == Some("logs") {
                                if let Some(lines) = v.get("lines").and_then(|l| l.as_array()) {
                                    for line in lines {
                                        if let Some(l) = line.as_str() {
                                            println!("{l}");
                                        }
                                    }
                                    return 0;
                                }
                            }
                            fail(v.get("error").and_then(|e| e.as_str()).unwrap_or("logs failed"))
                        }
                        Err(e) => fail(&e),
                    }
                }
                Target::Local(d) => match d.logs(&id, tail) {
                    Ok(lines) => {
                        for line in lines {
                            println!("{line}");
                        }
                        0
                    }
                    Err(e) => fail(&e),
                },
            })
        }
        "focus" => {
            let Some(id) = rest.first().cloned() else {
                eprintln!("usage: notchd focus <id>\n{USAGE}");
                return 2;
            };
            with_target(|t| match t {
                Target::Live(mut c) => match c.request(serde_json::json!({"cmd":"focus","id":id})) {
                    Ok(v) => ack_or_fail(&v),
                    Err(e) => fail(&e),
                },
                Target::Local(d) => match d.focus(&id) {
                    Ok(_) => print_ok(&serde_json::json!({"event":"ack","cmd":"focus","ok":true,"id":id})),
                    Err(e) => fail(&e),
                },
            })
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };

    match_value
}

/// Create the state dir if needed and bind the socket. A LIVE daemon's
/// socket is never clobbered: probe first. ConnectionRefused means the
/// file is a stale leftover -> remove and bind. A successful probe means
/// another notchd is already serving -> refuse to start. A transient error
/// (e.g. EAGAIN) fails loudly without touching anything.
fn bind_socket(path: &std::path::Path) -> std::io::Result<std::os::unix::net::UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!("{} is already served by another notchd", path.display()),
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(path); // stale socket; reclaim
            }
            Err(e) => {
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("{} is temporarily unavailable ({e})", path.display()),
                ));
            }
        }
    }
    std::os::unix::net::UnixListener::bind(path)
}

fn cmd_serve(rest: &[String]) -> i32 {
    let mut config = Config::from_env();
    let mut opencode_decider = false;
    for flag in rest {
        match flag.as_str() {
            "--opencode-decider" => opencode_decider = true,
            "--deterministic" => {} // the default; accepted for symmetry
            other => {
                eprintln!("unknown flag: {other}\n{USAGE}");
                return 2;
            }
        }
    }
    if opencode_decider {
        config.opencode_decider = true;
        eprintln!("notchd: --opencode-decider is not implemented; using the deterministic policy");
    }
    let daemon = match open_daemon_with(Some(config)) {
        Ok(d) => Arc::new(d),
        Err(code) => return code,
    };
    daemon.reconcile();

    let listener = {
        let default_path = paths::default_sock_path();
        match bind_socket(&default_path) {
            Ok(listener) => {
                println!("notchd serving on {} (state {})", default_path.display(), daemon.config.state_dir.display());
                listener
            }
            Err(e) => {
                // bridge.md §0: if the default path is unwritable and
                // NOTCH_SOCK was not explicitly set, fall back to a temp path.
                if std::env::var_os("NOTCH_SOCK").is_some() {
                    eprintln!("notchd: cannot bind {}: {e}", default_path.display());
                    return 1;
                }
                let fallback = paths::fallback_sock_path();
                match bind_socket(&fallback) {
                    Ok(listener) => {
                        eprintln!("notchd: {} unwritable ({e}); using fallback {}", default_path.display(), fallback.display());
                        println!("notchd serving on {} (state {})", fallback.display(), daemon.config.state_dir.display());
                        listener
                    }
                    Err(e2) => {
                        eprintln!("notchd: cannot bind {}: {e2}", fallback.display());
                        return 1;
                    }
                }
            }
        }
    };

    let _poll = {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || daemon.run_poll_loop(shutdown))
    };
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let shutdown = Arc::new(AtomicBool::new(false));
    match crate::socket::serve_with_shutdown(listener, daemon, shutdown) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("notchd: socket serve error: {e}");
            1
        }
    }
}

// ---------- target selection ----------

enum Target {
    Live(Box<LiveClient>),
    Local(Box<Daemon>),
}

/// Open the daemon. `Some(config)` is used by `serve` (its config comes
/// from the env incl. --opencode-decider); `None` opens from the env.
/// Falls back to a temp state dir when Application Support is unwritable
/// and NOTCH_STATE_DIR was not explicitly set (bridge.md §0). Probes with a
/// write first, because a read-only dir can exist without being writable.
fn open_daemon_with(primary: Option<Config>) -> Result<Daemon, i32> {
    let explicit = std::env::var_os("NOTCH_STATE_DIR").is_some();
    let primary = primary.unwrap_or_else(Config::from_env);
    let state_dir = primary.state_dir.clone();
    let writable = std::fs::create_dir_all(&state_dir)
        .and_then(|()| std::fs::write(state_dir.join(".notchd-write-probe"), b"probe"))
        .map(|()| {
            let _ = std::fs::remove_file(state_dir.join(".notchd-write-probe"));
            true
        })
        .unwrap_or(false);
    if writable {
        return Daemon::open(primary, Herdr::from_env(), Arc::new(SystemClock)).map_err(|e| {
            eprintln!("notchd: cannot open daemon state: {e}");
            1
        });
    }
    if explicit {
        // NOTCH_STATE_DIR was set explicitly — report, don't second-guess it.
        eprintln!("notchd: state dir {} is not writable", state_dir.display());
        return Err(1);
    }
    eprintln!(
        "notchd: state dir {} is not writable; falling back to a temp dir",
        state_dir.display()
    );
    let fallback = Config {
        state_dir: std::env::temp_dir().join("notch"),
        ..primary
    };
    Daemon::open(fallback, Herdr::from_env(), Arc::new(SystemClock)).map_err(|e| {
        eprintln!("notchd: cannot open daemon state: {e}");
        1
    })
}

/// Talk to a live daemon if one is reachable, else operate in-process.
/// Reclaim ONLY on ConnectionRefused (genuinely stale socket). A transient
/// failure (e.g. ECONNREFUSED is fine, but EAGAIN/timeout is not) means the
/// daemon is alive but unhappy right now — fail loudly rather than fall
/// back to a second in-process writer that races the daemon's state.
fn with_target(f: impl FnOnce(Target) -> i32) -> i32 {
    let sock = paths::default_sock_path();
    if sock.exists() {
        match UnixStream::connect(&sock) {
            Ok(stream) => match LiveClient::connect(stream) {
                Ok(client) => return f(Target::Live(Box::new(client))),
                Err(e) => {
                    eprintln!("notchd: live daemon handshake failed ({e}); not touching its socket or state");
                    return 1;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(&sock); // stale socket; reclaim it
            }
            Err(e) => {
                eprintln!("notchd: live daemon socket temporarily unavailable ({e}); retry");
                return 1;
            }
        }
    }
    match open_daemon_with(None) {
        Ok(d) => f(Target::Local(Box::new(d))),
        Err(code) => code,
    }
}

struct LiveClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl LiveClient {
    fn connect(stream: UnixStream) -> std::io::Result<Self> {
        stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        let reader = BufReader::new(stream.try_clone()?);
        let writer = stream;
        let mut client = LiveClient { reader, writer };
        // Consume the hello line sent on connect.
        let mut line = String::new();
        client.reader.read_line(&mut line)?;
        Ok(client)
    }

    fn request(&mut self, req: serde_json::Value) -> Result<serde_json::Value, String> {
        writeln!(&self.writer, "{}", req).map_err(|e| e.to_string())?;
        self.writer.flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        self.reader.read_line(&mut line).map_err(|e| e.to_string())?;
        serde_json::from_str(line.trim()).map_err(|e| format!("bad response: {e}"))
    }
}

fn print_ok(v: &serde_json::Value) -> i32 {
    println!("{v}");
    0
}

fn ack_or_fail(v: &serde_json::Value) -> i32 {
    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
    if ok {
        println!("{v}");
        0
    } else {
        eprintln!("{}", v.get("error").and_then(|e| e.as_str()).unwrap_or("command failed"));
        1
    }
}

fn fail(e: &str) -> i32 {
    eprintln!("{e}");
    1
}

/// Merge an ack's JSON body with the {event, cmd} frame (local-path output).
fn print_merged_ack(cmd: &str, body: serde_json::Value) {
    let mut obj = body.as_object().cloned().unwrap_or_default();
    obj.insert("event".into(), "ack".into());
    obj.insert("cmd".into(), cmd.into());
    println!("{}", serde_json::Value::Object(obj));
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == flag {
            return iter.next().cloned();
        }
        if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
            return Some(v.to_string());
        }
    }
    None
}
