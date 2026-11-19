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
    let daemon = match Daemon::open(config, Herdr::from_env(), Arc::new(SystemClock)) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!("notchd: cannot open daemon state: {e}");
            return 1;
        }
    };
    daemon.reconcile();

    let sock_path = paths::default_sock_path();
    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&sock_path); // stale socket
    let listener = match std::os::unix::net::UnixListener::bind(&sock_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("notchd: cannot bind {}: {e}", sock_path.display());
            return 1;
        }
    };

    let _poll = {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || daemon.run_poll_loop(shutdown))
    };
    println!(
        "notchd serving on {} (state {})",
        sock_path.display(),
        daemon.config.state_dir.display()
    );
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

/// Talk to a live daemon if one is reachable, else operate in-process.
fn with_target(f: impl FnOnce(Target) -> i32) -> i32 {
    let sock = paths::default_sock_path();
    if sock.exists() {
        if let Ok(stream) = UnixStream::connect(&sock) {
            if let Ok(client) = LiveClient::connect(stream) {
                return f(Target::Live(Box::new(client)));
            }
        }
        let _ = std::fs::remove_file(&sock); // stale socket; reclaim it
    }
    match Daemon::open(Config::from_env(), Herdr::from_env(), Arc::new(SystemClock)) {
        Ok(d) => f(Target::Local(Box::new(d))),
        Err(e) => {
            eprintln!("notchd: cannot open daemon state: {e}");
            1
        }
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
