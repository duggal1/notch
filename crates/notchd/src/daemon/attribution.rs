//! Per-session file attribution and cross-session conflict detection.
//!
//! `record_attribution` is the public command; `record_attribution_locked`
//! does the work behind the tree mutex (also used by the poll pass).
//! Attribution never mutates the filesystem.

use super::types::{lock, Daemon, Inner, ATTRIBUTION_KEEP};
use crate::events::EventLog;
use crate::model::{AttributionEntry, ConflictEntry};
use std::collections::HashSet;

impl Daemon {
    /// Record that `session` touched `files`. Dedupes, caps per-session
    /// tracking at 64, prepends (newest first), detects cross-session file
    /// conflicts. Attribution never mutates the filesystem.
    pub fn record_attribution(&self, session: &str, files: &[String]) {
        let mut inner = lock(&self.inner);
        record_attribution_locked(&mut inner, &self.event_log, session, files);
        self.save_state(&inner);
    }
}

pub(crate) fn record_attribution_locked(
    inner: &mut Inner,
    event_log: &EventLog,
    session: &str,
    files: &[String],
) {
    if session.is_empty() {
        return;
    }
    let mut tracked = inner.session_tracked.get(session).copied().unwrap_or(0);
    for file in files {
        if tracked >= crate::attribution::per_session_cap() {
            break;
        }
        let key = (session.to_string(), file.clone());
        if !inner.attribution_seen.insert(key) {
            continue; // already tracked
        }
        inner.attribution.insert(
            0,
            AttributionEntry {
                session: session.to_string(),
                file: file.clone(),
            },
        );
        tracked += 1;
        // Conflict: same file touched by >= 2 sessions.
        let others: HashSet<String> = inner
            .attribution
            .iter()
            .filter(|a| &a.file == file && a.session != session)
            .map(|a| a.session.clone())
            .collect();
        if !others.is_empty() {
            let mut sessions: Vec<String> = others.into_iter().collect();
            sessions.push(session.to_string());
            sessions.sort();
            sessions.dedup();
            match inner.conflicts.iter_mut().find(|c| &c.file == file) {
                Some(entry) => {
                    if entry.sessions != sessions {
                        entry.sessions = sessions.clone();
                        let _ = event_log.append(
                            Some(session.to_string()),
                            "conflict",
                            &format!("{file}: {}", sessions.join(",")),
                        );
                    }
                }
                None => {
                    inner.conflicts.push(ConflictEntry {
                        file: file.clone(),
                        sessions: sessions.clone(),
                    });
                    let _ = event_log.append(
                        Some(session.to_string()),
                        "conflict",
                        &format!("{file}: {}", sessions.join(",")),
                    );
                }
            }
        }
    }
    inner.session_tracked.insert(session.to_string(), tracked);
    while inner.attribution.len() > ATTRIBUTION_KEEP {
        if let Some(dropped) = inner.attribution.pop() {
            inner
                .attribution_seen
                .remove(&(dropped.session.clone(), dropped.file.clone()));
            if let Some(n) = inner.session_tracked.get_mut(&dropped.session) {
                *n = n.saturating_sub(1);
            }
        }
    }
}
