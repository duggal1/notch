//! NDJSON socket server (bridge.md §2). One JSON object per line, either
//! direction. The server sends `{ "event": "hello", "version": 1, "pid" }`
//! on connect. `subscribe` replies with an ack, an immediate snapshot, then
//! pushes a fresh snapshot on every change, debounced to one push / 100 ms.
//!
//! The protocol is unchanged and this module is one *consumer* of the
//! library API — it never owns state of its own.

use crate::daemon::Daemon;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

/// Serve forever (until the process is interrupted). Removes any stale
/// socket file first.
pub fn serve(listener: UnixListener, daemon: Arc<Daemon>) -> std::io::Result<()> {
    serve_with_shutdown(listener, daemon, Arc::new(AtomicBool::new(false)))
}

/// Serve until `shutdown` is set. Polls accept() on a 10 ms cadence so the
/// flag is honored promptly.
pub fn serve_with_shutdown(
    listener: UnixListener,
    daemon: Arc<Daemon>,
    shutdown: Arc<AtomicBool>,
) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    // Background flusher: pushes pending snapshots to subscribers, debounced
    // to one push / 100 ms.
    {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                daemon.flush_subscriber_snapshots();
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        });
    }
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let daemon = Arc::clone(&daemon);
                std::thread::spawn(move || {
                    let _ = handle_conn(stream, daemon);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    Ok(())
}

fn handle_conn(stream: UnixStream, daemon: Arc<Daemon>) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    let writer_stream = stream.try_clone()?;
    let (tx, rx) = channel::<String>();

    // Hello first.
    let hello = serde_json::json!({
        "event": "hello",
        "version": 1,
        "pid": std::process::id(),
    });
    let _ = tx.send(hello.to_string());

    // Writer thread: owns the socket's write half.
    std::thread::spawn(move || {
        let mut out = writer_stream;
        for line in rx {
            if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    });

    let mut subscriber_id: Option<u64> = None;
    let send = |tx: &Sender<String>, v: serde_json::Value| {
        let _ = tx.send(v.to_string());
    };
    let ack = |tx: &Sender<String>, cmd: &str, ok: bool, extra: serde_json::Value| {
        let mut obj = serde_json::Map::new();
        obj.insert("event".into(), "ack".into());
        obj.insert("cmd".into(), cmd.into());
        obj.insert("ok".into(), ok.into());
        if let Some(map) = extra.as_object() {
            for (k, v) in map {
                obj.insert(k.clone(), v.clone());
            }
        }
        send(tx, serde_json::Value::Object(obj));
    };

    let reader_stream = stream.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // client went away
            Ok(_) => {}
            Err(_) => break,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                ack(&tx, "", false, serde_json::json!({"error": "invalid_json"}));
                continue;
            }
        };
        let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        match cmd {
            "ping" => {
                ack(&tx, "ping", true, serde_json::json!({"pong": now_ms()}));
            }
            "snapshot" => {
                send(&tx, serde_json::from_str(&daemon.snapshot_line()).unwrap_or_else(|_| serde_json::json!({"event":"snapshot","snapshot":null})));
            }
            "subscribe" => {
                ack(&tx, "subscribe", true, serde_json::json!({}));
                // Immediate fresh snapshot.
                let snapshot_line = daemon.snapshot_line();
                send(&tx, serde_json::from_str(&snapshot_line).unwrap_or_else(|_| serde_json::json!({"event":"snapshot","snapshot":null})));
                // Register for change pushes.
                let (id, sub_rx) = daemon.register_subscriber();
                subscriber_id = Some(id);
                let tx2 = tx.clone();
                std::thread::spawn(move || {
                    for line in sub_rx {
                        if tx2.send(line).is_err() {
                            break;
                        }
                    }
                });
            }
            "run_plan" => {
                let path = v.get("path").and_then(|p| p.as_str()).unwrap_or("");
                let ack_result = daemon.run_plan(std::path::Path::new(path));
                if ack_result.ok {
                    ack(&tx, "run_plan", true, serde_json::json!({"spawned": ack_result.spawned}));
                } else {
                    ack(
                        &tx,
                        "run_plan",
                        false,
                        serde_json::json!({"error": ack_result.error.unwrap_or_else(|| "unknown error".to_string())}),
                    );
                }
            }
            "kill" => {
                let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let reason = v.get("reason").and_then(|x| x.as_str()).map(str::to_string);
                match daemon.kill(&id, reason) {
                    Ok(a) => ack(&tx, "kill", true, serde_json::json!({"id": a.id, "reason": a.reason})),
                    Err(e) => ack(&tx, "kill", false, serde_json::json!({"error": e})),
                }
            }
            "reprompt" => {
                let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                match daemon.reprompt(&id, &text) {
                    Ok(_) => ack(&tx, "reprompt", true, serde_json::json!({"id": id})),
                    Err(e) => ack(&tx, "reprompt", false, serde_json::json!({"error": e})),
                }
            }
            "cleanup" => {
                let id = v.get("id").and_then(|x| x.as_str());
                let a = daemon.cleanup(id);
                ack(&tx, "cleanup", true, serde_json::json!({"removed": a.removed.unwrap_or(0)}));
            }
            "logs" => {
                let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let tail = v.get("tail").and_then(|x| x.as_u64()).map(|n| n as usize);
                match daemon.logs(&id, tail) {
                    Ok(lines) => send(&tx, serde_json::json!({"event": "logs", "id": id, "lines": lines})),
                    Err(e) => ack(&tx, "logs", false, serde_json::json!({"error": e})),
                }
            }
            "focus" => {
                let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                match daemon.focus(&id) {
                    Ok(_) => ack(&tx, "focus", true, serde_json::json!({"id": id})),
                    Err(e) => ack(&tx, "focus", false, serde_json::json!({"error": e})),
                }
            }
            "spawn" => {
                let supervisor = v.get("supervisor").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let prompt = v.get("prompt").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let cwd = v.get("cwd").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if supervisor.is_empty() || prompt.is_empty() || cwd.is_empty() || title.is_empty() {
                    ack(&tx, "spawn", false, serde_json::json!({"error": "supervisor, prompt, cwd and title are required"}));
                    continue;
                }
                match daemon.spawn(&supervisor, &prompt, &cwd, &title) {
                    a if a.ok => ack(&tx, "spawn", true, serde_json::json!({"id": a.id})),
                    a => ack(&tx, "spawn", false, serde_json::json!({"error": a.error.unwrap_or_else(|| "spawn failed".into())})),
                }
            }
            "status" => {
                send(&tx, serde_json::json!(daemon.status()));
            }
            "" => {
                ack(&tx, "", false, serde_json::json!({"error": "missing_cmd"}));
            }
            other => {
                ack(&tx, other, false, serde_json::json!({"error": "unknown_command"}));
            }
        }
    }

    if let Some(id) = subscriber_id {
        daemon.unregister_subscriber(id);
    }
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
