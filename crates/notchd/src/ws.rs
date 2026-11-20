//! Loopback-only WebSocket endpoint for external clients (CLI, browser tools,
//! scripts). This is NOT the transport the Tauri webview uses — the webview
//! calls the engine in-process and listens to Tauri events, which is lower
//! latency than a loopback round-trip. One event pipeline: the daemon's
//! existing subscriber stream (the same one the NDJSON socket flushes) is the
//! sole source of change notifications; there is no second 2 s poll and no
//! second bus.
//!
//! # Wire shape (v1)
//!
//! A connection is a stream of UTF-8 JSON text frames, one object per frame
//! (NDJSON-shaped JSON, but framed by WebSocket instead of newlines).
//!
//! Server → client, on connect exactly once:
//!
//! ```jsonc
//! {"event":"hello","version":1,"pid":1234,"snapshot":{ /* bridge.md §1.3 */ }}
//! ```
//!
//! Server → client, pushed as changes happen (100 ms debounce, like §2):
//!
//! ```jsonc
//! {"event":"snapshot","snapshot":{ /* §1.3 */ },"lag":0}
//! {"event":"fleet","fleet":{"generatedAt":1763520000000,"topStatus":"working","units":[ /* all managed units */ ]}}
//! {"event":"bus_message","busMessage":{"id":"msg_1","from":"S1-OC01","to":"S1-OC02","type":"handoff","preview":"…","at":1763520000000}}
//! {"event":"decision","decision":{"at":1763520000000,"supervisor":"SV-1","session":"S1-OC01","action":"reprompt","reason":"…"}}
//! {"event":"escalation","escalation":{"id":"esc_1","session":"S1-OC01","supervisor":"SV-1","kind":"stuck","reason":"…","at":1763520000000}}
//! {"event":"session_output","session":"S1-OC01","line":"last pane line","at":1763520000000}
//! ```
//!
//! `bus_message` / `decision` / `escalation` are REAL deltas diffed from the
//! snapshot stream (new entries only; the initial snapshot is not replayed as
//! deltas). `session_output` is emitted when a session's `lastAction` (the only
//! output tail the daemon publishes) changes — it is best-effort, not a raw
//! pane stream. `lag` counts frames dropped for this client since its last
//! successfully delivered snapshot; a normal client always sees `lag: 0`.
//!
//! `fleet` (additive) is a flat, ordered view of EVERY notch-managed unit —
//! orchestrator, supervisors and sessions — derived from the very same
//! snapshot, with `id`, `kind`, `status`, `topStatus` (the global rollup),
//! `paneId`, `workspace`, `lastAction` and `revision`. It is pushed once right
//! after `hello` on connect and again, immediately, whenever the derived view
//! changes. It uses the same bounded outbox as every other frame, so lag and
//! backpressure are unchanged. `hello` and `snapshot` frames are byte-identical
//! to v1.
//!
//! Client → server (same command names and fields as bridge.md §2):
//! `ping`, `snapshot`, `submit_task`, `message`, `kill`, `reprompt`, `focus`,
//! `cleanup`, `pause_supervisor`, `resume_supervisor`, `kill_switch`,
//! `list_models` (plus `run_plan`, `logs`, `spawn`, `status`, `subscribe`).
//! Every one is answered with an `ack`/`event` frame, e.g.:
//!
//! ```jsonc
//! {"cmd":"ping"}                    -> {"event":"ack","cmd":"ping","ok":true,"pong":1763520000000}
//! {"cmd":"submit_task","task":"…"} -> {"event":"ack","cmd":"submit_task","ok":true,"supervisorId":"SV-1","sessionId":"S1-OC01"}
//! {"cmd":"snapshot"}                -> {"event":"snapshot","snapshot":{ /* §1.3 */ }}
//! {"cmd":"unknown"}                 -> {"event":"ack","cmd":"unknown","ok":false,"error":"unknown_command"}
//! ```
//!
//! Subscription is automatic on connect (there is no `subscribe` handshake).
//! Clients should answer the server's periodic WebSocket Ping with Pong
//! (tungstenite/browser clients do this automatically). A malformed text frame
//! gets `{"event":"ack","cmd":"","ok":false,"error":"invalid_json"}`; a binary
//! frame gets `error:"binary_not_supported"`. Close is echoed and the socket is
//! released.
//!
//! # Transport / limits
//!
//! Binds `127.0.0.1` only (default port 7407, override with `NOTCH_WS_PORT`;
//! `0` requests an ephemeral port). At most [`MAX_CLIENTS`] connections are
//! admitted; the rest complete the HTTP upgrade and are immediately closed
//! with WebSocket code 1012 (restart) before any state is touched.
//! Handshake auth (3.7): any `Origin` is rejected, `Authorization: Bearer` is
//! required and compared in constant time, and `Host` must be the bound
//! `127.0.0.1:<port>`. No external network, no TLS. The endpoint stays UNWIRED
//! until `NOTCH_WS=1`; the `notchd serve` path in `cli.rs` starts it when that
//! flag is set (see the `ws_cli_wiring` test).
//!
//! # Browser-compatible opt-in (`NOTCH_WS_BROWSER=1`)
//!
//! A real webview/browser always sends `Origin` and cannot set a custom
//! `Authorization` header, so strict mode is unusable from `ui/`. With the
//! explicit opt-in `NOTCH_WS_BROWSER=1` (and `NOTCH_WS=1`), a handshake carrying
//! the SAME 64-hex token in the query string — `ws://127.0.0.1:<port>/?token=<64hex>`
//! — is accepted even with an `Origin` header, provided `Host` is still exactly
//! `127.0.0.1:<port>`. This is the deliberate tradeoff: the token is 32 bytes
//! from `/dev/urandom` (unguessable, per-daemon), and the exact Host check still
//! blocks DNS rebinding. No token / wrong token / wrong Host is rejected.
//! Strict mode REMAINS THE DEFAULT and keeps rejecting every `Origin`; the
//! legacy Bearer handshake is unchanged. The query token is additive — strict
//! clients keep using the header. This is a v1.4 wire-contract change for
//! `docs/bridge.md` (lead-owned).
//! Each client has a bounded outbox (64 frames): when it fills, the oldest
//! snapshot (else oldest event) is dropped and `lag` is incremented. Client
//! commands are read and answered inline and are never queued or dropped, so a
//! slow reader can only stall itself — never another client or the daemon.
//!
//! # Integration
//!
//! `serve` is deliberately transport-only: the caller owns the listener and
//! the shutdown flag. To make it always-on in `notchd serve`, bind and spawn
//! next to the NDJSON socket:
//!
//! ```ignore
//! let ws_listener = notchd::ws::bind(notchd::ws::port_from_env())?;
//! let ws_daemon = Arc::clone(&daemon);
//! let ws_shutdown = Arc::clone(&shutdown);
//! std::thread::spawn(move || { let _ = notchd::ws::serve(ws_listener, ws_daemon, ws_shutdown); });
//! ```

use crate::daemon::Daemon;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tungstenite::http::{self, Request};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tungstenite::{
    accept_hdr_with_config, accept_with_config, Error as WsError, Message, WebSocket,
};

/// Default TCP port (`ws://127.0.0.1:7407`).
pub const DEFAULT_PORT: u16 = 7407;
/// Hard cap on simultaneously connected clients.
pub const MAX_CLIENTS: usize = 16;

const OUTBOX_CAP: usize = 64;
const READ_TIMEOUT: Duration = Duration::from_millis(20);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(30);
const PONG_TIMEOUT: Duration = Duration::from_secs(75);
const MAX_FRAME_BYTES: usize = 256 * 1024;
const FLUSH_TICK: Duration = Duration::from_millis(10);
const DEBOUNCE_POLL: Duration = Duration::from_millis(100);

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

/// Port from `NOTCH_WS_PORT` (default [`DEFAULT_PORT`]; `0` = ephemeral).
pub fn port_from_env() -> u16 {
    std::env::var("NOTCH_WS_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Bind a WebSocket listener on loopback only. Never binds `0.0.0.0`.
pub fn bind(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}

/// WS stays UNWIRED until the operator opts in with the exact value `NOTCH_WS=1`.
pub fn ws_enabled() -> bool {
    std::env::var("NOTCH_WS").map(|v| v == "1").unwrap_or(false)
}

/// Length-independent-in-value byte comparison: every byte is examined, and
/// the lengths are checked only after the loop.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let n = a.len().max(b.len());
    let mut diff = 0u8;
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0 && a.len() == b.len()
}

/// Read or create the loopback bearer token. 32 bytes from `/dev/urandom`,
/// hex-encoded, in a 0600 file inside a 0700 state dir. A corrupt file is
/// regenerated rather than trusted.
pub fn load_or_create_token(state_dir: &Path) -> io::Result<String> {
    fs::create_dir_all(state_dir)?;
    fs::set_permissions(state_dir, fs::Permissions::from_mode(0o700))?;
    let path = crate::paths::ws_token_path_in(state_dir);
    if let Ok(text) = fs::read_to_string(&path) {
        let t = text.trim();
        if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            return Ok(t.to_string());
        }
    }
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &hex)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp, &path)?;
    Ok(hex)
}

/// Strict mode is the default. `NOTCH_WS_BROWSER=1` is the only opt-in to the
/// query-token / Origin-tolerant handshake; any other value keeps strict mode.
pub fn browser_ws_enabled() -> bool {
    std::env::var("NOTCH_WS_BROWSER")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Extract the `token` parameter from a URI query. Hex tokens need no URL
/// decoding; any extra parameters are ignored, a missing one is rejected by
/// the caller's constant-time comparison against the 64-hex daemon token.
pub fn token_from_query(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
        .map(|v| v.split('#').next().unwrap_or(v))
}

/// The full handshake policy for both modes.
///
/// * strict (default): reject any `Origin`, require `Authorization: Bearer
///   <token>`, require exact `Host`. This is the pre-v1.4 policy, unchanged.
/// * browser opt-in: require `?token=<64hex>` and exact `Host`; an `Origin`
///   header is allowed because browsers cannot hide it. The token is the same
///   unguessable per-daemon value, so a hostile page cannot authenticate.
pub fn handshake_auth_ok(
    origin: Option<&str>,
    authorization: Option<&str>,
    host: Option<&str>,
    query: Option<&str>,
    expected_token: &str,
    expected_host: &str,
    browser_mode: bool,
) -> bool {
    let host_ok = host
        .map(|h| constant_time_eq(h.as_bytes(), expected_host.as_bytes()))
        .unwrap_or(false);
    if !host_ok {
        return false;
    }
    if browser_mode {
        let provided = token_from_query(query).unwrap_or("");
        return constant_time_eq(provided.as_bytes(), expected_token.as_bytes());
    }
    if origin.is_some() {
        return false;
    }
    let Some(auth) = authorization else {
        return false;
    };
    let Some(provided) = auth.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(provided.as_bytes(), expected_token.as_bytes())
}

/// Pre-v1.4 strict policy, kept as a public seam so existing callers/tests do
/// not change. Equivalent to `handshake_auth_ok(..., browser_mode = false)`.
pub fn auth_headers_ok(
    origin: Option<&str>,
    authorization: Option<&str>,
    host: Option<&str>,
    expected_token: &str,
    expected_host: &str,
) -> bool {
    handshake_auth_ok(
        origin,
        authorization,
        host,
        None,
        expected_token,
        expected_host,
        false,
    )
}

fn auth_rejection() -> http::Response<Option<String>> {
    http::Response::builder()
        .status(http::StatusCode::UNAUTHORIZED)
        .body(Some("unauthorized".to_string()))
        .unwrap_or_else(|_| http::Response::new(None))
}

/// Serve until `shutdown` is set. Caller owns the listener; [`bind`] makes one.
///
/// Spawns two internal helpers: a flusher that drives the daemon's existing
/// 100 ms debounced subscriber stream (idempotent if the NDJSON socket also
/// runs), and one pump that diffs snapshots into delta frames and fans them
/// out to the bounded per-client outboxes.
pub fn serve(
    listener: TcpListener,
    daemon: Arc<Daemon>,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    serve_with_active(listener, daemon, shutdown, Arc::new(AtomicUsize::new(0)))
}

/// Same as [`serve`], with the admitted-client counter supplied by the caller.
///
/// The counter is incremented only after a client passes capacity admission
/// and decremented when its connection thread exits. Tests use it as a
/// deterministic readiness signal instead of guessing with fixed sleeps.
pub fn serve_with_active(
    listener: TcpListener,
    daemon: Arc<Daemon>,
    shutdown: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
) -> io::Result<()> {
    serve_with_mode(listener, daemon, shutdown, active, browser_ws_enabled())
}

/// Same as [`serve_with_active`] with the handshake mode supplied explicitly.
/// Production reads `NOTCH_WS_BROWSER`; tests pass the mode directly so the
/// two policies can be exercised against real sockets without process-global
/// environment races.
pub fn serve_with_mode(
    listener: TcpListener,
    daemon: Arc<Daemon>,
    shutdown: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    browser_mode: bool,
) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let expected_host = format!("127.0.0.1:{}", listener.local_addr()?.port());
    let token = load_or_create_token(&daemon.config.state_dir)?;
    let clients: Arc<Mutex<Vec<Client>>> = Arc::new(Mutex::new(Vec::new()));

    // WS is the real-time transport: flush pending changes as soon as they are
    // marked instead of waiting out the NDJSON socket's 100 ms debounce. The
    // guard makes this a no-op on a quiet daemon, and double-flushes from the
    // socket's own flusher remain harmless (pending_change is cleared here).
    {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                if daemon.has_pending_change() {
                    daemon.flush_subscribers_immediate();
                }
                std::thread::sleep(FLUSH_TICK);
            }
        });
    }

    // One event pipeline: subscribe once, diff, fan out.
    {
        let daemon = Arc::clone(&daemon);
        let clients = Arc::clone(&clients);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || event_pump(daemon, clients, shutdown));
    }

    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let admitted = active
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                        (n < MAX_CLIENTS).then_some(n + 1)
                    })
                    .is_ok();
                if !admitted {
                    // Over capacity: handshake just far enough to send a close,
                    // on a detached thread so the accept loop never stalls.
                    std::thread::spawn(move || reject(stream));
                    continue;
                }
                let daemon = Arc::clone(&daemon);
                let clients = Arc::clone(&clients);
                let shutdown = Arc::clone(&shutdown);
                let active = Arc::clone(&active);
                let token = token.clone();
                let expected_host = expected_host.clone();
                std::thread::spawn(move || {
                    handle_conn(
                        stream,
                        daemon,
                        clients,
                        shutdown,
                        &token,
                        &expected_host,
                        browser_mode,
                    );
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    // Best-effort clean close for everyone still attached.
    for client in clients.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        client.outbox.close();
    }
    Ok(())
}

fn reject(stream: TcpStream) {
    if let Ok(mut ws) = accept_with_config(stream, None) {
        let _ = ws.send(Message::Close(Some(CloseFrame {
            code: CloseCode::Restart,
            reason: "server full".into(),
        })));
        let _ = ws.close(None);
    }
}

// ---------- per-client bounded outbox ----------

#[derive(Clone, Debug)]
pub enum OutFrame {
    /// A full §1.3 snapshot; these are the first frames dropped on overflow.
    Snapshot(Value),
    /// A derived `bus_message` / `decision` / `escalation` / `session_output`.
    Event(Value),
}

impl OutFrame {
    fn into_value(self) -> Value {
        match self {
            Self::Snapshot(v) | Self::Event(v) => v,
        }
    }
}

/// Backpressure policy shared by every client outbox. Appends `frame`, and if
/// the queue is full, removes the oldest snapshot first (else the oldest
/// frame), incrementing `lag` once per drop. Never blocks. `lag` is surfaced
/// to the client on the next delivered snapshot. Exposed for wire tests.
pub fn enqueue_bounded(
    queue: &mut VecDeque<OutFrame>,
    cap: usize,
    frame: OutFrame,
    lag: &mut u64,
) -> bool {
    let mut dropped = false;
    if queue.len() >= cap {
        let victim = queue
            .iter()
            .position(|f| matches!(f, OutFrame::Snapshot(_)))
            .unwrap_or(0);
        let _ = queue.remove(victim);
        *lag = lag.saturating_add(1);
        dropped = true;
    }
    queue.push_back(frame);
    dropped
}

struct OutboxInner {
    queue: VecDeque<OutFrame>,
    /// Dropped frames since the last delivered snapshot (surfaced as `lag`).
    lag: u64,
    closed: bool,
}

struct Outbox {
    inner: Mutex<OutboxInner>,
}

impl Outbox {
    fn new() -> Self {
        Self {
            inner: Mutex::new(OutboxInner {
                queue: VecDeque::with_capacity(OUTBOX_CAP),
                lag: 0,
                closed: false,
            }),
        }
    }

    /// Never blocks. On overflow prefer dropping the oldest snapshot; if none
    /// is queued, drop the oldest event. Counts the drop in `lag`.
    fn push(&self, frame: OutFrame) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !g.closed {
            let OutboxInner { queue, lag, .. } = &mut *g;
            enqueue_bounded(queue, OUTBOX_CAP, frame, lag);
        }
    }

    fn try_pop(&self) -> Option<(Value, u64, bool)> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let frame = g.queue.pop_front()?;
        let lag = g.lag;
        let is_snapshot = matches!(frame, OutFrame::Snapshot(_));
        if is_snapshot {
            g.lag = 0;
        }
        Some((frame.into_value(), lag, is_snapshot))
    }

    fn close(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.closed = true;
        g.queue.clear();
    }
}

fn with_lag(mut v: Value, lag: u64) -> Value {
    if lag > 0 {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("lag".into(), json!(lag));
        }
    }
    v
}

struct Client {
    id: u64,
    outbox: Arc<Outbox>,
}

// ---------- the single event pipeline ----------

fn event_pump(daemon: Arc<Daemon>, clients: Arc<Mutex<Vec<Client>>>, shutdown: Arc<AtomicBool>) {
    let (sub_id, rx) = daemon.register_subscriber();
    // Seed from the current state so nothing already in the snapshot is
    // replayed as a "delta" when the first change arrives.
    let seeded = serde_json::to_value(daemon.snapshot()).ok();
    let mut prev = seeded.clone();
    let mut last_fleet = seeded.as_ref().and_then(|s| fleet_frame(&daemon, s));

    while !shutdown.load(Ordering::SeqCst) {
        match rx.recv_timeout(DEBOUNCE_POLL) {
            Ok(line) => {
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let current = v.get("snapshot").cloned().unwrap_or(Value::Null);
                let deltas = snapshot_deltas(prev.as_ref(), &current);
                prev = Some(current.clone());

                // C3: the fleet view comes from A4's single Rust function in
                // snapshot.rs using this exact snapshot and the real ledger
                // seq/epoch. Equal views are not re-pushed.
                let fleet = fleet_frame(&daemon, &current);
                let fleet_changed = fleet.is_some() && last_fleet != fleet;
                last_fleet = fleet.clone();

                let mut frames = Vec::with_capacity(deltas.len() + 2);
                frames.push(OutFrame::Snapshot(v));
                if fleet_changed {
                    frames.push(OutFrame::Event(fleet.expect("fleet is Some when changed")));
                }
                frames.extend(deltas.into_iter().map(OutFrame::Event));

                let guard = clients.lock().unwrap_or_else(|e| e.into_inner());
                for client in guard.iter() {
                    for frame in &frames {
                        client.outbox.push(frame.clone());
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    daemon.unregister_subscriber(sub_id);
}

/// The additive `fleet` frame is A4's single Rust derivation (C3): WS, the
/// Tauri event and `fleet_state` all call `snapshot::fleet_view` on the same
/// snapshot with the same real ledger seq/epoch. No WS-local derivation.
/// `None` means the ledger could not be read; never fabricate seq/epoch zeroes.
pub fn fleet_frame(daemon: &Daemon, snapshot: &Value) -> Option<Value> {
    let ledger = crate::ledger::Ledger::open(&daemon.config.state_dir).ok()?;
    let fleet =
        crate::snapshot::fleet_view(snapshot, ledger.last_seq(), ledger.current_epoch("ORCH"));
    Some(json!({ "event": "fleet", "fleet": fleet }))
}

/// Pure delta extractor used by the event pump: given the previous snapshot
/// (or `None` at startup) and the current one, return the NEW bus messages,
/// decisions, escalations and session-output lines as event frames. Exposed so
/// the wire shape can be tested directly; no daemon state is touched.
pub fn snapshot_deltas(previous: Option<&Value>, current: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(prev) = previous else {
        return out;
    };

    // Id-keyed collections: busMessages and escalations.
    for (field, event, key) in [
        ("busMessages", "bus_message", "busMessage"),
        ("escalations", "escalation", "escalation"),
    ] {
        let old_ids: HashSet<String> = prev
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        if let Some(items) = current.get(field).and_then(Value::as_array) {
            for item in items {
                let is_new = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|id| !old_ids.contains(id))
                    .unwrap_or(false);
                if is_new {
                    out.push(json!({ "event": event, key: item }));
                }
            }
        }
    }

    // Decisions have no id: match newest-first entries by full key with
    // multiplicity, then emit only the unmatched prefix.
    let key_of = |v: &Value| -> String {
        let get = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
        let at = v.get("at").map(Value::to_string).unwrap_or_default();
        format!(
            "{at}|{}|{}|{}|{}",
            get("supervisor"),
            get("session"),
            get("action"),
            get("reason")
        )
    };
    let mut seen: HashMap<String, usize> = HashMap::new();
    if let Some(items) = prev.get("decisions").and_then(Value::as_array) {
        for item in items {
            *seen.entry(key_of(item)).or_insert(0) += 1;
        }
    }
    if let Some(items) = current.get("decisions").and_then(Value::as_array) {
        for item in items {
            let key = key_of(item);
            match seen.get_mut(&key) {
                Some(n) if *n > 0 => *n -= 1,
                _ => out.push(json!({ "event": "decision", "decision": item })),
            }
        }
    }

    // Session output: the only tail the daemon publishes is `lastAction`.
    // Emit when it appears or changes; never replay an unchanged value.
    let mut old_actions: HashMap<String, Option<String>> = HashMap::new();
    if let Some(items) = prev.get("sessions").and_then(Value::as_array) {
        for s in items {
            if let Some(id) = s.get("id").and_then(Value::as_str) {
                old_actions.insert(
                    id.to_string(),
                    s.get("lastAction")
                        .and_then(Value::as_str)
                        .filter(|v| !v.is_empty())
                        .map(str::to_string),
                );
            }
        }
    }
    let at = current.get("generatedAt").cloned().unwrap_or(Value::Null);
    if let Some(items) = current.get("sessions").and_then(Value::as_array) {
        for s in items {
            let Some(id) = s.get("id").and_then(Value::as_str) else {
                continue;
            };
            let new = s
                .get("lastAction")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty());
            if new.is_some() && new != old_actions.get(id).and_then(Option::as_deref) {
                out.push(json!({
                    "event": "session_output",
                    "session": id,
                    "line": new,
                    "at": at,
                }));
            }
        }
    }

    out
}

// ---------- one connection ----------

fn handle_conn(
    stream: TcpStream,
    daemon: Arc<Daemon>,
    clients: Arc<Mutex<Vec<Client>>>,
    shutdown: Arc<AtomicBool>,
    token: &str,
    expected_host: &str,
    browser_mode: bool,
) {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    // Auth happens in the handshake callback: reject before the 101 upgrade
    // and before any daemon state is touched.
    let expected_token = token.to_string();
    let expected_host = expected_host.to_string();
    // tungstenite's Callback trait fixes the error type to a by-value
    // http::Response; boxing it is not possible without forking the trait.
    #[allow(clippy::result_large_err)]
    let callback = move |req: &Request<()>, response| {
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
        let authorization = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        let host = req.headers().get("host").and_then(|v| v.to_str().ok());
        let query = req.uri().query();
        if handshake_auth_ok(
            origin,
            authorization,
            host,
            query,
            &expected_token,
            &expected_host,
            browser_mode,
        ) {
            Ok(response)
        } else {
            Err(auth_rejection())
        }
    };
    let mut ws = match accept_hdr_with_config(stream, callback, Some(config)) {
        Ok(ws) => ws,
        Err(_) => return,
    };
    // Handshake is done; now bound how long a read/write may block. TimedOut
    // is a normal "nothing to do" tick, not a fatal WS error.
    let _ = ws.get_ref().set_read_timeout(Some(READ_TIMEOUT));
    let _ = ws.get_ref().set_write_timeout(Some(WRITE_TIMEOUT));

    let snapshot_value = serde_json::to_value(daemon.snapshot()).unwrap_or(Value::Null);
    let hello = json!({
        "event": "hello",
        "version": 1,
        "pid": std::process::id(),
        "snapshot": snapshot_value.clone(),
    });
    let outbox = Arc::new(Outbox::new());
    // First fleet frame is queued behind hello on this connection only. An
    // empty daemon sends hello alone, preserving the v1 frame sequence for
    // clients that are not interested in an empty fleet. The hello bytes
    // themselves are unchanged.
    let has_units = ["supervisors", "sessions"].iter().any(|key| {
        snapshot_value
            .get(*key)
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    });
    if has_units {
        if let Some(frame) = fleet_frame(&daemon, &snapshot_value) {
            outbox.push(OutFrame::Event(frame));
        }
    }
    if send_text(&mut ws, hello).is_err() {
        return;
    }

    let id = NEXT_CLIENT_ID.fetch_add(1, Ordering::SeqCst);
    clients
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Client {
            id,
            outbox: Arc::clone(&outbox),
        });

    conn_loop(&mut ws, &outbox, &daemon, &shutdown);

    outbox.close();
    clients
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|c| c.id != id);
    let _ = ws.send(Message::Close(Some(CloseFrame {
        code: CloseCode::Away,
        reason: "".into(),
    })));
}

fn conn_loop(
    ws: &mut WebSocket<TcpStream>,
    outbox: &Outbox,
    daemon: &Daemon,
    shutdown: &AtomicBool,
) {
    let mut last_ping = Instant::now();
    let mut last_pong = Instant::now();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }

        // 1. Commands are read and answered inline — never queued, never
        // dropped, independent of how full the event outbox is.
        loop {
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let replies = handle_command(daemon, text.as_str());
                    // A mutating WS command has already called notify_change.
                    // Push it to subscribers now instead of waiting out the
                    // NDJSON socket's 100 ms debounce: this is the real-time
                    // path, and fleet frames must land in the same tick.
                    if daemon.has_pending_change() {
                        daemon.flush_subscribers_immediate();
                    }
                    for reply in replies {
                        if send_text(ws, reply).is_err() {
                            return;
                        }
                    }
                }
                Ok(Message::Ping(_)) => {
                    // tungstenite queues the pong itself; flush it now.
                    if ws.flush().is_err() {
                        return;
                    }
                }
                Ok(Message::Pong(_)) => last_pong = Instant::now(),
                Ok(Message::Close(frame)) => {
                    let code = frame.as_ref().map(|f| f.code).unwrap_or(CloseCode::Normal);
                    let _ = ws.send(Message::Close(Some(CloseFrame {
                        code,
                        reason: "".into(),
                    })));
                    return;
                }
                Ok(Message::Binary(_)) => {
                    if send_text(ws, ack("", false, json!({"error": "binary_not_supported"})))
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(Message::Frame(_)) => {}
                Err(WsError::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    break
                }
                Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => return,
                Err(_) => return,
            }
        }

        // 2. Push whatever events fit; snapshots carry the drop counter.
        while let Some((frame, lag, _is_snapshot)) = outbox.try_pop() {
            if send_text(ws, with_lag(frame, lag)).is_err() {
                return;
            }
        }

        // 3. Heartbeat: ping on an interval, drop a peer that never pongs back.
        if last_ping.elapsed() >= PING_INTERVAL {
            if ws.send(Message::Ping(Vec::new().into())).is_err() {
                return;
            }
            last_ping = Instant::now();
        }
        if last_pong.elapsed() > PONG_TIMEOUT {
            return;
        }
    }
}

fn send_text(ws: &mut WebSocket<TcpStream>, value: Value) -> Result<(), WsError> {
    ws.send(Message::Text(value.to_string().into()))
}

fn ack(cmd: &str, ok: bool, extra: Value) -> Value {
    let mut obj = Map::new();
    obj.insert("event".into(), json!("ack"));
    obj.insert("cmd".into(), json!(cmd));
    obj.insert("ok".into(), json!(ok));
    if let Some(map) = extra.as_object() {
        for (k, v) in map {
            obj.insert(k.clone(), v.clone());
        }
    }
    Value::Object(obj)
}

fn ack_from(cmd: &str, value: &impl serde::Serialize) -> Value {
    let body = serde_json::to_value(value).unwrap_or(Value::Null);
    ack(
        cmd,
        body.get("ok").and_then(Value::as_bool).unwrap_or(false),
        body,
    )
}

fn snapshot_frame(daemon: &Daemon) -> Value {
    json!({
        "event": "snapshot",
        "snapshot": serde_json::to_value(daemon.snapshot()).unwrap_or(Value::Null),
    })
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Dispatch one client command. Mirrors the NDJSON socket (bridge.md §2) so
/// both transports have identical semantics; returns one or more frames.
fn handle_command(daemon: &Daemon, text: &str) -> Vec<Value> {
    let v: Value = match serde_json::from_str(text.trim()) {
        Ok(v) => v,
        Err(_) => return vec![ack("", false, json!({"error": "invalid_json"}))],
    };
    let cmd = v.get("cmd").and_then(Value::as_str).unwrap_or("");
    let reply = match cmd {
        "" => ack("", false, json!({"error": "missing_cmd"})),
        "ping" => ack(
            "ping",
            true,
            json!({"pong": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)}),
        ),
        "subscribe" => ack("subscribe", true, json!({})),
        "snapshot" => return vec![snapshot_frame(daemon)],
        "status" => return vec![serde_json::to_value(daemon.status()).unwrap_or(Value::Null)],
        "run_plan" => {
            let path = str_field(&v, "path");
            let a = daemon.run_plan(std::path::Path::new(&path));
            if a.ok {
                ack("run_plan", true, json!({"spawned": a.spawned}))
            } else {
                ack(
                    "run_plan",
                    false,
                    json!({"error": a.error.unwrap_or_else(|| "unknown error".into())}),
                )
            }
        }
        "submit_task" => ack_from(
            "submit_task",
            &daemon.submit_task(
                str_field(&v, "task"),
                opt_field(&v, "cwd"),
                opt_field(&v, "title"),
                opt_field(&v, "definitionOfDone"),
                opt_field(&v, "model"),
                opt_field(&v, "supervisorId"),
            ),
        ),
        "message" => ack_from(
            "message",
            &daemon.message(
                &str_field(&v, "from"),
                &str_field(&v, "to"),
                &str_field(&v, "type"),
                &str_field(&v, "body"),
            ),
        ),
        "kill" => match daemon.kill(&str_field(&v, "id"), opt_field(&v, "reason")) {
            Ok(a) => ack("kill", true, json!({"id": a.id, "reason": a.reason})),
            Err(e) => ack("kill", false, json!({"error": e})),
        },
        "reprompt" => {
            let id = str_field(&v, "id");
            match daemon.reprompt(&id, &str_field(&v, "text")) {
                Ok(_) => ack("reprompt", true, json!({"id": id})),
                Err(e) => ack("reprompt", false, json!({"error": e})),
            }
        }
        "focus" => {
            let id = str_field(&v, "id");
            match daemon.focus(&id) {
                Ok(_) => ack("focus", true, json!({"id": id})),
                Err(e) => ack("focus", false, json!({"error": e})),
            }
        }
        "cleanup" => {
            let a = daemon.cleanup(opt_field(&v, "id").as_deref());
            ack("cleanup", true, json!({"removed": a.removed.unwrap_or(0)}))
        }
        "pause_supervisor" => {
            let id = str_field(&v, "id");
            match daemon.pause_supervisor(&id) {
                Ok(_) => ack("pause_supervisor", true, json!({"id": id, "paused": true})),
                Err(e) => ack("pause_supervisor", false, json!({"error": e})),
            }
        }
        "resume_supervisor" => {
            let id = str_field(&v, "id");
            match daemon.resume_supervisor(&id) {
                Ok(_) => ack(
                    "resume_supervisor",
                    true,
                    json!({"id": id, "resumed": true}),
                ),
                Err(e) => ack("resume_supervisor", false, json!({"error": e})),
            }
        }
        "kill_switch" => {
            let a = daemon.kill_switch();
            ack("kill_switch", true, json!({"closed": a.closed}))
        }
        "list_models" => ack_from("list_models", &daemon.list_models()),
        "logs" => {
            let id = str_field(&v, "id");
            let tail = v.get("tail").and_then(Value::as_u64).map(|n| n as usize);
            match daemon.logs(&id, tail) {
                Ok(lines) => return vec![json!({"event": "logs", "id": id, "lines": lines})],
                Err(e) => ack("logs", false, json!({"error": e})),
            }
        }
        "spawn" => {
            let supervisor = str_field(&v, "supervisor");
            let prompt = str_field(&v, "prompt");
            let cwd = str_field(&v, "cwd");
            let title = str_field(&v, "title");
            if supervisor.is_empty() || prompt.is_empty() || cwd.is_empty() || title.is_empty() {
                ack(
                    "spawn",
                    false,
                    json!({"error": "supervisor, prompt, cwd and title are required"}),
                )
            } else {
                let a = daemon.spawn(&supervisor, &prompt, &cwd, &title);
                if a.ok {
                    ack("spawn", true, json!({"id": a.id}))
                } else {
                    ack(
                        "spawn",
                        false,
                        json!({"error": a.error.unwrap_or_else(|| "spawn failed".into())}),
                    )
                }
            }
        }
        other => ack(other, false, json!({"error": "unknown_command"})),
    };
    vec![reply]
}
