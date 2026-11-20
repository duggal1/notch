//! RED STUB — narrow read-only worker MCP bridge (W2-3). Replaced in GREEN commit.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const TOOL_NAMES: [&str; 3] = ["list_peers", "list_messages", "send_message"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: String,
    pub body: String,
    pub at: u64,
}

pub trait BridgeBackend: Send + Sync {
    fn peers(&self) -> Vec<String>;
    fn messages(&self) -> Vec<BridgeMessage>;
    fn enqueue(&self, id: &str, from: &str, to: &str, kind: &str, body: &str) -> Result<(), String>;
}

pub struct McpBridge {
    _backend: Arc<dyn BridgeBackend>,
    _self_id: String,
}

impl McpBridge {
    pub fn new(backend: Arc<dyn BridgeBackend>, self_id: impl Into<String>) -> Self {
        Self { _backend: backend, _self_id: self_id.into() }
    }
    pub fn handle(&self, _line: &str) -> String {
        todo!("RED: handle unimplemented")
    }
    pub fn tool_names() -> &'static [&'static str] {
        &TOOL_NAMES
    }
}
