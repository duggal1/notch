//! Typed errors at notchd's boundaries (bridge.md §2 wire aside).
//!
//! `NotchError` is the internal error currency for the daemon. The NDJSON
//! socket and the Tauri shell cross a process edge as plain JSON strings,
//! so `Display` for every variant renders the bare payload — converting
//! `NotchError -> String` at that edge is byte-identical to the strings
//! the code emitted before this type existed. Variants exist for
//! classification, not for decoration:
//!
//! - [`NotchError::Io`] — a real I/O failure (wraps [`std::io::Error`]),
//!   carrying its original kind.
//! - [`NotchError::Config`] — a bad configuration or plan (unreadable
//!   file, invalid plan JSON, missing plan fields).
//! - [`NotchError::Herdr`] — a failure talking to herdr or the `opencode`
//!   CLI (message is the rendered `HerdrError` text, optionally with
//!   call-site context prepended).
//! - [`NotchError::State`] — in-memory state or persistence trouble.
//! - [`NotchError::NotFound`] — a referenced entity does not exist
//!   (unknown session / supervisor / missing cwd).
//! - [`NotchError::Protocol`] — a malformed request or an internal
//!   protocol violation (bad message type, oversized body).
//!
//! No `thiserror`: std only. `impl std::error::Error` by hand below.

use std::fmt;

/// The daemon's typed error. See the module docs for the variant kinds.
///
/// `Display` renders only the payload so edge conversion is wire-safe.
#[derive(Debug)]
pub enum NotchError {
    /// A real I/O failure. Preserves the original [`std::io::ErrorKind`]
    /// via the wrapped [`std::io::Error`].
    Io(std::io::Error),
    /// A bad configuration, plan, or environment.
    Config(String),
    /// A failure talking to herdr or the `opencode` CLI.
    Herdr(String),
    /// State trouble (persistence or in-memory invariants).
    State(String),
    /// The referenced entity does not exist.
    NotFound(String),
    /// A malformed request or internal protocol violation.
    Protocol(String),
}

impl fmt::Display for NotchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotchError::Io(e) => write!(f, "{e}"),
            NotchError::Config(s)
            | NotchError::Herdr(s)
            | NotchError::State(s)
            | NotchError::NotFound(s)
            | NotchError::Protocol(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for NotchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            NotchError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for NotchError {
    fn from(e: std::io::Error) -> Self {
        NotchError::Io(e)
    }
}

impl From<crate::herdr::HerdrError> for NotchError {
    fn from(e: crate::herdr::HerdrError) -> Self {
        NotchError::Herdr(e.to_string())
    }
}

/// Wire-boundary conversion: `socket ack error: String` stays byte-identical.
impl From<NotchError> for String {
    fn from(e: NotchError) -> String {
        e.to_string()
    }
}
