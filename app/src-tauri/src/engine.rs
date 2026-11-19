//! Shell ↔ notchd wiring. notchd owns ALL orchestration logic (tree,
//! registry, watchdog, re-adopt, plan executor, attribution, cleanup, events).
//! This module owns ONLY:
//!   1. construction (state dir, state.json/registry load, reconcile — exactly
//!      what `notchd serve` does at startup),
//!   2. the background poll thread (`run_poll_loop`) and the subscriber
//!      flusher (`flush_subscriber_snapshots`, 25 ms, same as the socket
//!      layer's flusher),
//!   3. the NDJSON-line → `app.emit("snapshot", <snapshot object>)` bridge
//!      with a 150 ms coalescing window (no firehose).
//!
//! NOT used here: the Unix socket. The webview talks Tauri commands/events
//! directly; the socket exists for external clients (`notchd serve`).

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notchd::Daemon;

/// Spawn the poll loop + subscriber flusher, mirroring `notchd serve`:
///   reconcile() at startup (caller) → poll_once every poll_interval_ms.
/// The AtomicBool shutdown flag is effectively `process exit`: the regular
/// app quits via its Quit menu item (app.exit), killing all threads. Kept as
/// a parameter for symmetry with `serve_with_shutdown` and future use.
pub fn spawn_daemon_threads(daemon: Arc<Daemon>) -> std::sync::mpsc::Receiver<String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let shutdown = Arc::new(AtomicBool::new(false));

    // Poll thread: herdr poll + watchdog, every poll_interval_ms (default 2 s).
    let poll = Arc::clone(&daemon);
    let poll_shutdown = Arc::clone(&shutdown);
    std::thread::Builder::new()
        .name("notchd-poll".into())
        .spawn(move || poll.run_poll_loop(poll_shutdown))
        .ok();

    // Subscriber flusher: pushes pending snapshots to subscribers, debounced
    // to one push / 100 ms (notchd's own debounce). Same as the socket layer.
    let flush = Arc::clone(&daemon);
    let flush_shutdown = Arc::clone(&shutdown);
    std::thread::Builder::new()
        .name("notchd-flush".into())
        .spawn(move || {
            while !flush_shutdown.load(Ordering::SeqCst) {
                flush.flush_subscriber_snapshots();
                std::thread::sleep(Duration::from_millis(25));
            }
        })
        .ok();

    let (_sub_id, sub_rx) = daemon.register_subscriber();
    sub_rx
}

/// Drain notchd's subscriber channel onto `app.emit("snapshot", …)`.
/// Each notchd line is `{"event":"snapshot","snapshot":{…}}`; the webview
/// receives only the inner snapshot object (its `listen('snapshot')` payload).
/// Coalescing: at most one emit per 150 ms; newest wins.
pub fn spawn_emit_thread(app: tauri::AppHandle, rx: Receiver<String>) {
    std::thread::Builder::new()
        .name("notch-emit".into())
        .spawn(move || {
            use tauri::Emitter;
            let mut last_emit = Instant::now() - Duration::from_millis(150);
            let mut pending: Option<serde_json::Value> = None;

            let parse = |line: &str| -> Option<serde_json::Value> {
                let v: serde_json::Value = serde_json::from_str(line).ok()?;
                v.get("snapshot").cloned()
            };

            loop {
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(line) => pending = parse(&line).or(pending),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
                while let Ok(line) = rx.try_recv() {
                    if let Some(v) = parse(&line) {
                        pending = Some(v); // keep the newest only
                    }
                }
                if let Some(snap) = pending.take() {
                    if last_emit.elapsed() >= Duration::from_millis(150) {
                        let _ = app.emit("snapshot", &snap);
                        crate::tray::update_status_from_snapshot(&app, &snap);
                        last_emit = Instant::now();
                    } else {
                        pending = Some(snap); // rate-limited; flush on the next tick
                    }
                }
            }
            if let Some(snap) = pending {
                let _ = app.emit("snapshot", &snap);
            }
        })
        .ok();
}
