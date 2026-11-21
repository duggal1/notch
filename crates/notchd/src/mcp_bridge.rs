//! Narrow read-only worker MCP bridge for A2A (W2-3).
//!
//! Stolen from `agentbox/mcp/stdio.mts` + `mcp/tools.mts`: a worker can call a
//! tiny tool protocol instead of having every message injected as a prompt.
//! The surface is deliberately narrowed (the bayesian report's reject list):
//!   - `list_peers` / `list_messages` are READ-ONLY — they cannot mutate state.
//!   - `send_message` is the only mutating tool and it can only ENQUEUE into
//!     the orchestrator bus. No filesystem writes, no kill/spawn/reprompt, no
//!     approval signing, no permission grants, no shell.
//!   - FROM is derived from the run id held by the bridge, never from a tool
//!     argument (a worker cannot speak as another session).
//!
//! JSON-RPC 2.0 over an injected Read/Write seam would be the transport; here
//! `handle` is the pure protocol core so headless tests exercise the real
//! dispatch without a subprocess. Stdout belongs to the protocol: this module
//! never prints — all diagnostics go to the caller as error frames.

use crate::bus::{is_valid_message_type, is_wildcard_target, A2aBridgeIngress};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

pub const TOOL_NAMES: [&str; 3] = ["list_peers", "list_messages", "send_message"];
const MAX_SEEN_IDS: usize = 8192;
const MAX_BODY_CHARS: usize = 4000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: String,
    pub body: String,
    pub at: u64,
}

/// The only capabilities a worker gets. Deliberately no mutators besides
/// `enqueue`, which lands in the orchestrator inbox, never a mailbox.
pub trait BridgeBackend: Send + Sync {
    fn peers(&self) -> Vec<String>;
    fn messages(&self) -> Vec<BridgeMessage>;
    fn enqueue(&self, id: &str, from: &str, to: &str, kind: &str, body: &str)
        -> Result<(), String>;
}

#[derive(Default)]
struct SeenIds {
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl SeenIds {
    fn insert(&mut self, id: &str) -> bool {
        if id.is_empty() || self.set.contains(id) {
            return false;
        }
        if self.order.len() >= MAX_SEEN_IDS {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        self.order.push_back(id.to_string());
        self.set.insert(id.to_string());
        true
    }
}

pub struct McpBridge {
    backend: Arc<dyn BridgeBackend>,
    self_id: String,
    seen: Mutex<SeenIds>,
}

impl McpBridge {
    pub fn new(backend: Arc<dyn BridgeBackend>, self_id: impl Into<String>) -> Self {
        Self {
            backend,
            self_id: self_id.into(),
            seen: Mutex::new(SeenIds::default()),
        }
    }

    pub fn tool_names() -> &'static [&'static str] {
        &TOOL_NAMES
    }

    /// Tool declarations sent to the client. Descriptions say what a tool is
    /// for and what it may not do — the calling model reads them every run.
    pub fn tool_defs() -> Vec<Value> {
        vec![
            json!({
                "name": "list_peers",
                "description": "Read-only. List the notch session ids currently known to the orchestrator. Use this before send_message to pick a real recipient. No side effects.",
                "inputSchema": {"type": "object", "properties": {}},
            }),
            json!({
                "name": "list_messages",
                "description": "Read-only. Show the recent A2A messages routed through the orchestrator, newest first. No side effects.",
                "inputSchema": {"type": "object", "properties": {}},
            }),
            json!({
                "name": "send_message",
                "description": "Ask the orchestrator to route one message to another session. FROM is your own run id and cannot be changed. This can only enqueue into the orchestrator bus — it cannot edit files, kill or spawn sessions, or answer approvals. TO must be a single known session id; no broadcast.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "to": {"type": "string"},
                        "type": {"type": "string", "enum": ["task", "result", "question", "handoff", "escalate"]},
                        "body": {"type": "string"},
                    },
                    "required": ["id", "to", "type", "body"],
                },
            }),
        ]
    }

    /// One JSON-RPC request frame in, one response frame out. Never panics and
    /// never writes to stdout; malformed input gets a protocol error frame.
    pub fn handle(&self, line: &str) -> String {
        let v: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => return rpc_error(Value::Null, -32700, "invalid_json"),
        };
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        let method = v.get("method").and_then(Value::as_str).unwrap_or("");
        let outcome = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "notch-a2a", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            })),
            // Content-wrapped so every result on this narrow bridge has one
            // uniform shape for the client to read.
            "tools/list" => Ok(tool_text(
                json!({ "tools": Self::tool_defs() }).to_string(),
                false,
            )),
            "tools/call" => self.call_tool(&v),
            _ => return rpc_error(id, -32601, "method not found"),
        };
        match outcome {
            Ok(result) => rpc_result(id, result),
            Err(msg) => rpc_error(id, -32602, &msg),
        }
    }

    fn call_tool(&self, req: &Value) -> Result<Value, String> {
        let params = req.get("params").unwrap_or(&Value::Null);
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match name {
            "list_peers" => Ok(tool_text(
                json!({ "peers": self.backend.peers() }).to_string(),
                false,
            )),
            "list_messages" => Ok(tool_text(
                json!({ "messages": self.backend.messages() }).to_string(),
                false,
            )),
            "send_message" => self.send(&args),
            other => Err(format!("unknown tool `{other}`")),
        }
    }

    fn send(&self, args: &Value) -> Result<Value, String> {
        // Identity is a property of the run, never an argument. An explicit
        // `from` is an impersonation attempt and is refused outright.
        if args.get("from").is_some() {
            return Ok(tool_text(
                json!({"ok": false, "error": "FROM is your run id and cannot be supplied"})
                    .to_string(),
                true,
            ));
        }
        let field = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let id = field("id");
        let to = field("to");
        let kind = field("type");
        let body = field("body");

        let invalid = |msg: &str| {
            Ok(tool_text(
                json!({"ok": false, "error": msg}).to_string(),
                true,
            ))
        };
        if id.is_empty() || id.len() > 128 {
            return invalid("id must be 1..128 chars");
        }
        if to.is_empty() || to.len() > 128 || is_wildcard_target(&to) {
            return invalid("TO must be a single known session id; no broadcast");
        }
        if !is_valid_message_type(&kind) {
            return invalid("type must be task|result|question|handoff|escalate");
        }
        if body.trim().is_empty() || body.chars().count() > MAX_BODY_CHARS {
            return invalid("body must be non-empty and <=4000 chars");
        }
        if !self
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(&id)
        {
            return invalid("duplicate message id");
        }
        match self.backend.enqueue(&id, &self.self_id, &to, &kind, &body) {
            Ok(()) => Ok(tool_text(
                json!({"ok": true, "id": id, "from": self.self_id, "to": to}).to_string(),
                false,
            )),
            Err(e) => invalid(&format!("delivery refused: {e}")),
        }
    }
}

/// Production backend: the worker's only state-changing call lands in the
/// orchestrator ingress, which routes through `OrchestratorBus::drain`.
/// Peer/message reads come from injected projections of the daemon snapshot.
pub struct OrchestratorBackend {
    ingress: Arc<A2aBridgeIngress>,
    peers: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    messages: Arc<dyn Fn() -> Vec<BridgeMessage> + Send + Sync>,
}

impl OrchestratorBackend {
    pub fn new(
        ingress: Arc<A2aBridgeIngress>,
        peers: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        messages: Arc<dyn Fn() -> Vec<BridgeMessage> + Send + Sync>,
    ) -> Self {
        Self {
            ingress,
            peers,
            messages,
        }
    }
}

impl BridgeBackend for OrchestratorBackend {
    fn peers(&self) -> Vec<String> {
        (self.peers)()
    }
    fn messages(&self) -> Vec<BridgeMessage> {
        (self.messages)()
    }
    fn enqueue(
        &self,
        id: &str,
        from: &str,
        to: &str,
        kind: &str,
        body: &str,
    ) -> Result<(), String> {
        self.ingress
            .submit(id, from, to, kind, body)
            .map_err(String::from)
    }
}

impl crate::daemon::Daemon {
    /// Build the narrow MCP bridge for one managed session. FROM is fixed to
    /// that session id; no worker can supply or change it. Peers and message
    /// reads are projections of the live daemon tree, never a second registry.
    pub fn worker_mcp_bridge(self: &Arc<Self>, session_id: &str) -> McpBridge {
        let ingress = Arc::new(A2aBridgeIngress::new(
            self.clock.clone(),
            self.bus.inbox_sender(),
        ));
        {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let known = inner
                .sessions
                .iter()
                .map(|s| s.id.clone())
                .collect::<std::collections::HashSet<_>>();
            let routes = inner
                .sessions
                .iter()
                .map(|s| (s.id.clone(), s.sup.clone()))
                .collect::<std::collections::HashMap<_, _>>();
            ingress.set_known(known);
            ingress.set_routes(routes);
        }
        let daemon = Arc::clone(self);
        let peers: Arc<dyn Fn() -> Vec<String> + Send + Sync> = Arc::new(move || {
            let inner = daemon.inner.lock().unwrap_or_else(|e| e.into_inner());
            let mut ids: Vec<String> = inner.sessions.iter().map(|s| s.id.clone()).collect();
            ids.sort();
            ids
        });
        let daemon = Arc::clone(self);
        let messages: Arc<dyn Fn() -> Vec<BridgeMessage> + Send + Sync> = Arc::new(move || {
            let inner = daemon.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner
                .bus_messages
                .iter()
                .map(|e| BridgeMessage {
                    id: e.id.clone(),
                    from: e.from.clone(),
                    to: e.to.clone(),
                    kind: e.kind.clone(),
                    body: e.preview.clone(),
                    at: e.at,
                })
                .collect()
        });
        McpBridge::new(
            Arc::new(OrchestratorBackend::new(ingress, peers, messages)),
            session_id,
        )
    }
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    })
}

fn rpc_result(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn rpc_error(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}
