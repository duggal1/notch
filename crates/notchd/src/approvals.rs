//! RED STUB — signed human-approval channel (W2-3). Replaced in the GREEN commit.

use crate::error::NotchError;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub const DEFAULT_APPROVAL_TTL_MS: u64 = 16 * 60 * 1000;
pub const ANSWER_MODES: [&str; 6] = ["allow", "deny", "answer", "default", "plan", "bypass_permissions"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerMode {
    Allow,
    Deny,
    Answer,
    Default,
    Plan,
    BypassPermissions,
}

impl AnswerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AnswerMode::Allow => "allow",
            AnswerMode::Deny => "deny",
            AnswerMode::Answer => "answer",
            AnswerMode::Default => "default",
            AnswerMode::Plan => "plan",
            AnswerMode::BypassPermissions => "bypass_permissions",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        ANSWER_MODES.iter().find(|m| **m == s).map(|m| match *m {
            "allow" => AnswerMode::Allow,
            "deny" => AnswerMode::Deny,
            "answer" => AnswerMode::Answer,
            "default" => AnswerMode::Default,
            "plan" => AnswerMode::Plan,
            _ => AnswerMode::BypassPermissions,
        })
    }
    pub fn is_permission_grant(self) -> bool {
        matches!(self, AnswerMode::Default | AnswerMode::Plan | AnswerMode::BypassPermissions)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalCard {
    pub id: String,
    pub session: String,
    pub kind: String,
    pub reason: String,
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedAnswer {
    pub id: String,
    pub allow: bool,
    pub note: Option<String>,
    pub at: u64,
    pub card: String,
    pub mode: String,
    pub sig: String,
}

pub fn card_digest(_card: &ApprovalCard) -> String {
    todo!("RED: card digest unimplemented")
}

pub fn decision_bytes(
    _id: &str,
    _allow: bool,
    _note: Option<&str>,
    _at: u64,
    _card: &str,
    _mode: &str,
) -> Vec<u8> {
    todo!("RED: decision bytes unimplemented")
}

#[derive(Clone)]
pub struct ApprovalAuthority {
    secret: [u8; 32],
}

impl ApprovalAuthority {
    pub fn new() -> Self {
        Self { secret: [0u8; 32] }
    }
    pub fn sign(&self, _id: &str, _allow: bool, _note: Option<&str>, _at: u64, _card: &str, _mode: &str) -> String {
        todo!("RED: sign unimplemented")
    }
    pub fn verify(&self, _answer: &SignedAnswer) -> bool {
        todo!("RED: verify unimplemented")
    }
}

impl Default for ApprovalAuthority {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ApprovalInbox {
    _clock: Arc<dyn crate::clock::Clock>,
    _authority: ApprovalAuthority,
    _pending: Mutex<()>,
}

impl ApprovalInbox {
    pub fn new(clock: Arc<dyn crate::clock::Clock>) -> Self {
        Self {
            _clock: clock,
            _authority: ApprovalAuthority::new(),
            _pending: Mutex::new(()),
        }
    }
    pub fn issue(&self, _card: ApprovalCard) -> Result<String, NotchError> {
        todo!("RED: issue unimplemented")
    }
    pub fn pending_cards(&self) -> Vec<ApprovalCard> {
        todo!("RED: pending_cards unimplemented")
    }
    pub fn answer(
        &self,
        _id: &str,
        _allow: bool,
        _note: Option<String>,
        _at: u64,
        _card: &str,
        _mode: &str,
    ) -> Result<SignedAnswer, NotchError> {
        todo!("RED: answer unimplemented")
    }
    pub fn signature_valid(&self, _answer: &SignedAnswer) -> bool {
        todo!("RED: signature_valid unimplemented")
    }
    pub fn verify_answer(&self, _answer: &SignedAnswer) -> bool {
        todo!("RED: verify_answer unimplemented")
    }
    pub fn is_answered(&self, _id: &str) -> bool {
        todo!("RED: is_answered unimplemented")
    }
    pub fn take_answer(&self, _id: &str) -> Option<SignedAnswer> {
        todo!("RED: take_answer unimplemented")
    }
}
