//! Signed human-approval channel for `needs_you` escalations (W2-3).
//!
//! Stolen from `agentbox/main/approvals.mts` + `shared/approval-card.mts`:
//! the answer is *signed* over the exact card the human was shown, so a worker
//! cannot forge, flip, move, backdate or replay it. The signing secret is
//! ephemeral and in-process only — it is never written to disk. Verification
//! is notch-side; the worker is never handed a verifier or a signer.
//!
//! Port deviation (honest): agentbox uses ed25519 (private key in-process,
//! public half to workers). Here both signing and verification happen inside
//! notchd, so an HMAC-SHA256 keyed by the same in-process secret gives the same
//! property with std-only code and no new dependency: a same-uid worker still
//! cannot read the key, and no public verifier needs to exist. The card is
//! bound by a SHA-256 digest of its canonical JSON (sorted keys), exactly like
//! agentbox's `cardDigest`.
//!
//! Black-box rule: this is notch-side control plane. No OpenCode internals, no
//! sub-agents, no typed answers into permission prompts.

use crate::clock::Clock;
use crate::error::NotchError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// How long an unanswered request stays answerable, measured on the monotonic
/// clock (port of agentbox `REQUEST_TTL_MS`, 16 minutes).
pub const DEFAULT_APPROVAL_TTL_MS: u64 = 16 * 60 * 1000;
/// A backdated answer is refused; a far-future one is refused too.
pub const MAX_CLOCK_SKEW_MS: u64 = 60_000;
/// Hard caps: a worker cannot make the daemon hold unbounded approval state.
pub const MAX_PENDING_APPROVALS: usize = 1024;
pub const MAX_RECORDED_ANSWERS: usize = 1024;
pub const MAX_NOTE_CHARS: usize = 1000;
pub const MAX_REASON_CHARS: usize = 240;

/// Closed vocabulary of what one answer may mean. `allow`/`deny`/`answer` are
/// verdicts for the card; the rest are standing permission grants that only a
/// human answer can create (port of agentbox `answer-modes.mts` whitelist).
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
    /// Strict: everything outside the whitelist is `None`, never a fallback.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(AnswerMode::Allow),
            "deny" => Some(AnswerMode::Deny),
            "answer" => Some(AnswerMode::Answer),
            "default" => Some(AnswerMode::Default),
            "plan" => Some(AnswerMode::Plan),
            "bypass_permissions" => Some(AnswerMode::BypassPermissions),
            _ => None,
        }
    }
    pub fn is_permission_grant(self) -> bool {
        matches!(self, AnswerMode::Default | AnswerMode::Plan | AnswerMode::BypassPermissions)
    }
}

/// The card actually displayed to the human. Its digest is what the answer
/// signs — not the request at answer time, which a worker could swap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalCard {
    pub id: String,
    pub session: String,
    pub kind: String,
    pub reason: String,
    pub at: u64,
}

/// A verified answer. `sig` is lowercase hex HMAC-SHA256.
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

/// Canonical fingerprint of a card: serde_json's `Map` is a `BTreeMap`, so
/// serializing the value sorts keys — the two sides cannot disagree on order.
/// No Unicode normalization: two byte strings that look alike are still two
/// different commands.
pub fn card_digest(card: &ApprovalCard) -> String {
    let value = serde_json::to_value(card).unwrap_or(serde_json::Value::Null);
    let canonical = value.to_string();
    hex(&sha256::digest(canonical.as_bytes()))
}

/// The signed bytes are an ARRAY, not an object: object key order would let a
/// re-serialization change what the signature covers. Mirrors agentbox's
/// `decisionBytes(id, !!allow, note ?? null, at ?? null, card ?? null)`.
pub fn decision_bytes(
    id: &str,
    allow: bool,
    note: Option<&str>,
    at: u64,
    card: &str,
    mode: &str,
) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!([
        id,
        allow,
        note,
        at,
        card,
        mode,
    ]))
    .unwrap_or_default()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Length-independent byte comparison; every byte is examined.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let n = a.len().max(b.len());
    let mut diff = 0u8;
    for i in 0..n {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0 && a.len() == b.len()
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Ephemeral signing authority. The secret exists only in this process's
/// memory; it is never serialized, persisted, or handed to a worker.
#[derive(Clone)]
pub struct ApprovalAuthority {
    secret: [u8; 32],
}

impl ApprovalAuthority {
    pub fn new() -> Self {
        use std::io::Read;
        use std::time::{SystemTime, UNIX_EPOCH};
        static NONCE: AtomicU64 = AtomicU64::new(1);
        let mut bytes = [0u8; 32];
        let from_urandom = std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .is_ok();
        if !from_urandom {
            let t = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let mix = t.wrapping_add(
                NONCE.fetch_add(1, Ordering::SeqCst).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            );
            bytes[..8].copy_from_slice(&mix.to_le_bytes());
            bytes[8..16].copy_from_slice(&mix.wrapping_mul(0x9E37_79B9_7F4A_7C15).to_le_bytes());
        }
        Self { secret: bytes }
    }

    pub fn sign(
        &self,
        id: &str,
        allow: bool,
        note: Option<&str>,
        at: u64,
        card: &str,
        mode: &str,
    ) -> String {
        let bytes = decision_bytes(id, allow, note, at, card, mode);
        hex(&sha256::hmac(&self.secret, &bytes))
    }

    /// Constant-time verification of the signature over the answer's own
    /// fields. A worker that knows the card, id and verdict still cannot
    /// produce a valid `sig`.
    pub fn verify(&self, answer: &SignedAnswer) -> bool {
        let bytes = decision_bytes(
            &answer.id,
            answer.allow,
            answer.note.as_deref(),
            answer.at,
            &answer.card,
            &answer.mode,
        );
        let expected = hex(&sha256::hmac(&self.secret, &bytes));
        ct_eq(expected.as_bytes(), answer.sig.as_bytes())
    }
}

impl Default for ApprovalAuthority {
    fn default() -> Self {
        Self::new()
    }
}

struct PendingApproval {
    card: ApprovalCard,
    digest: String,
    issued_mono: u64,
    issued_at_ms: u64,
}

#[derive(Default)]
struct Answered {
    map: HashMap<String, SignedAnswer>,
    order: VecDeque<String>,
}

impl Answered {
    fn insert(&mut self, id: String, answer: SignedAnswer) {
        if !self.map.contains_key(&id) {
            self.order.push_back(id.clone());
        }
        self.map.insert(id, answer);
        while self.map.len() > MAX_RECORDED_ANSWERS {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
    fn take(&mut self, id: &str) -> Option<SignedAnswer> {
        let out = self.map.remove(id);
        if out.is_some() {
            self.order.retain(|x| x != id);
        }
        out
    }
}

/// The approval inbox. One instance per daemon state dir (self-contained, so
/// no `model.rs` / `state.json` schema change is needed). All mutations are
/// behind one mutex; expiry uses the monotonic clock.
pub struct ApprovalInbox {
    clock: Arc<dyn Clock>,
    authority: ApprovalAuthority,
    pending: Mutex<HashMap<String, PendingApproval>>,
    pending_order: Mutex<VecDeque<String>>,
    answered: Mutex<Answered>,
}

impl ApprovalInbox {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            authority: ApprovalAuthority::new(),
            pending: Mutex::new(HashMap::new()),
            pending_order: Mutex::new(VecDeque::new()),
            answered: Mutex::new(Answered::default()),
        }
    }

    /// Register the card the human is about to see. Returns its digest; only
    /// that exact digest can be signed in a later answer.
    pub fn issue(&self, card: ApprovalCard) -> Result<String, NotchError> {
        if !valid_id(&card.id) {
            return Err(NotchError::Protocol("invalid approval id".into()));
        }
        if !valid_id(&card.session) {
            return Err(NotchError::Protocol("invalid approval session".into()));
        }
        if card.kind != "needs_you" {
            return Err(NotchError::Protocol("only needs_you escalations are answerable".into()));
        }
        let reason = card.reason.trim();
        if reason.is_empty()
            || reason.chars().count() > MAX_REASON_CHARS
            || reason.chars().any(|c| c == '\n' || c == '\r' || c.is_control())
        {
            return Err(NotchError::Protocol("invalid approval reason".into()));
        }
        if card.at == 0 {
            return Err(NotchError::Protocol("approval card needs a timestamp".into()));
        }
        {
            let answered = self.answered.lock().unwrap_or_else(|e| e.into_inner());
            if answered.map.contains_key(&card.id) {
                return Err(NotchError::Protocol("approval already answered".into()));
            }
        }
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.contains_key(&card.id) {
            return Err(NotchError::Protocol("approval already pending".into()));
        }
        if pending.len() >= MAX_PENDING_APPROVALS {
            let mut order = self.pending_order.lock().unwrap_or_else(|e| e.into_inner());
            while pending.len() >= MAX_PENDING_APPROVALS {
                if let Some(old) = order.pop_front() {
                    pending.remove(&old);
                } else {
                    break;
                }
            }
        }
        let digest = card_digest(&card);
        let id = card.id.clone();
        pending.insert(
            id.clone(),
            PendingApproval {
                card,
                digest: digest.clone(),
                issued_mono: self.clock.mono_ms(),
                issued_at_ms: self.clock.now_ms(),
            },
        );
        self.pending_order
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(id);
        Ok(digest)
    }

    /// Cards still waiting for a human, oldest issue first.
    pub fn pending_cards(&self) -> Vec<ApprovalCard> {
        let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let order = self.pending_order.lock().unwrap_or_else(|e| e.into_inner());
        order
            .iter()
            .filter_map(|id| pending.get(id).map(|p| p.card.clone()))
            .collect()
    }

    /// Sign an answer for a pending card. This is the ONLY signing entry point
    /// and is called from the daemon's authenticated control path, never from
    /// a worker tool.
    pub fn answer(
        &self,
        id: &str,
        allow: bool,
        note: Option<String>,
        at: u64,
        card: &str,
        mode: &str,
    ) -> Result<SignedAnswer, NotchError> {
        if !valid_id(id) {
            return Err(NotchError::Protocol("invalid approval id".into()));
        }
        if AnswerMode::parse(mode).is_none() {
            return Err(NotchError::Protocol(format!("mode must be one of {}", ANSWER_MODES.join("|"))));
        }
        if let Some(ref n) = note {
            if n.chars().count() > MAX_NOTE_CHARS || n.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
                return Err(NotchError::Protocol("invalid approval note".into()));
            }
        }
        let pending = {
            let map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            map.get(id).map(|p| (p.digest.clone(), p.issued_mono, p.issued_at_ms))
        };
        let Some((digest, issued_mono, issued_at_ms)) = pending else {
            return Err(NotchError::NotFound(format!("no pending approval `{id}`")));
        };
        let mono = self.clock.mono_ms();
        if mono.saturating_sub(issued_mono) > DEFAULT_APPROVAL_TTL_MS {
            return Err(NotchError::Protocol(format!("approval `{id}` expired")));
        }
        if card != digest {
            return Err(NotchError::Protocol("answer does not match the card shown".into()));
        }
        let now = self.clock.now_ms();
        if at < issued_at_ms || at > now.saturating_add(MAX_CLOCK_SKEW_MS) {
            return Err(NotchError::Protocol("answer timestamp is backdated or in the future".into()));
        }
        let sig = self.authority.sign(id, allow, note.as_deref(), at, card, mode);
        let answer = SignedAnswer {
            id: id.to_string(),
            allow,
            note,
            at,
            card: card.to_string(),
            mode: mode.to_string(),
            sig,
        };
        {
            let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            map.remove(id);
            self.pending_order
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|x| x != id);
            self.answered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.to_string(), answer.clone());
        }
        Ok(answer)
    }

    /// Signature-only check. False for anything a worker writes.
    pub fn signature_valid(&self, answer: &SignedAnswer) -> bool {
        !answer.sig.is_empty()
            && valid_id(&answer.id)
            && AnswerMode::parse(&answer.mode).is_some()
            && self.authority.verify(answer)
    }

    /// Full check: a genuine signature for an answer that is still the live,
    /// unconsumed answer for this request. A replay after `take_answer` fails.
    pub fn verify_answer(&self, answer: &SignedAnswer) -> bool {
        self.signature_valid(answer)
            && self
                .answered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .map
                .get(&answer.id)
                == Some(answer)
    }

    pub fn is_answered(&self, id: &str) -> bool {
        self.answered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map
            .contains_key(id)
    }

    /// Take the answer exactly once. The engine calls this when it applies the
    /// human's decision; a second take (replay) returns `None`.
    pub fn take_answer(&self, id: &str) -> Option<SignedAnswer> {
        self.answered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take(id)
    }
}

// ---------------------------------------------------------------------------
// SHA-256 + HMAC-SHA256, std only. Small, auditable, no new dependency.
// ---------------------------------------------------------------------------
mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    pub fn digest(data: &[u8]) -> [u8; 32] {
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
            0x1f83d9ab, 0x5be0cd19,
        ];
        let bit_len = (data.len() as u64).wrapping_mul(8);
        let mut padded = data.to_vec();
        padded.push(0x80);
        while padded.len() % 64 != 56 {
            padded.push(0);
        }
        padded.extend_from_slice(&bit_len.to_be_bytes());
        for chunk in padded.as_chunks::<64>().0 {
            let mut w = [0u32; 64];
            for (i, word) in w[..16].iter_mut().enumerate() {
                let j = i * 4;
                *word = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
                (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let t1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
            h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g);
            h[7] = h[7].wrapping_add(hh);
        }
        let mut out = [0u8; 32];
        for (i, word) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    pub fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
        const BLOCK: usize = 64;
        let mut k = [0u8; BLOCK];
        if key.len() > BLOCK {
            k[..32].copy_from_slice(&digest(key));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut inner = Vec::with_capacity(BLOCK + data.len());
        inner.extend(k.iter().map(|b| b ^ 0x36));
        inner.extend_from_slice(data);
        let inner_hash = digest(&inner);
        let mut outer = Vec::with_capacity(BLOCK + 32);
        outer.extend(k.iter().map(|b| b ^ 0x5c));
        outer.extend_from_slice(&inner_hash);
        digest(&outer)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn hx(bytes: &[u8]) -> String {
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        }
        #[test]
        fn sha256_known_vectors() {
            assert_eq!(
                hx(&digest(b"")),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            );
            assert_eq!(
                hx(&digest(b"abc")),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            );
        }
        #[test]
        fn hmac_known_vector_rfc4231() {
            // RFC 4231 case 1: key = 0x0b * 20, data = "Hi There".
            assert_eq!(
                hx(&hmac(&[0x0b; 20], b"Hi There")),
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
            );
        }
    }
}
