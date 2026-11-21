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
        {
            let mut inner = lock(&self.inner);
            record_attribution_locked(&mut inner, &self.event_log, session, files);
            self.save_now(&inner);
        }
        self.refresh_conflicts();
    }

    /// Rebuild cross-session conflicts from real git merges. A shared FILE
    /// NAME is not a conflict when sessions work in isolated worktrees; only
    /// `git merge-tree --write-tree` exit 1 is. Bounded to 32 candidate pairs
    /// so a large tree cannot spawn unbounded git calls.
    pub fn refresh_conflicts(&self) {
        let (pairs, worktrees): (
            Vec<(String, String)>,
            std::collections::HashMap<String, crate::worktree::Worktree>,
        ) = {
            let inner = lock(&self.inner);
            let mut by_file: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for entry in &inner.attribution {
                by_file
                    .entry(entry.file.clone())
                    .or_default()
                    .push(entry.session.clone());
            }
            let mut pairs = std::collections::HashSet::new();
            for sessions in by_file.values() {
                if sessions.len() < 2 {
                    continue;
                }
                let mut unique = sessions.clone();
                unique.sort();
                unique.dedup();
                for (i, a) in unique.iter().enumerate() {
                    for b in unique.iter().skip(i + 1) {
                        pairs.insert((a.clone(), b.clone()));
                    }
                }
            }
            let worktrees = inner
                .sessions
                .iter()
                .filter_map(|s| {
                    crate::worktree::meta_for_cwd(std::path::Path::new(&s.cwd))
                        .map(|w| (s.id.clone(), w))
                })
                .collect();
            (pairs.into_iter().collect(), worktrees)
        };

        let mut fresh: Vec<crate::model::ConflictEntry> = Vec::new();
        let mut checked = 0usize;
        for (a, b) in pairs {
            if checked >= 32 {
                break;
            }
            let (Some(wa), Some(wb)) = (worktrees.get(&a), worktrees.get(&b)) else {
                continue; // legacy shared-cwd sessions keep the filename rule
            };
            if wa.base_repo != wb.base_repo || wa.base_rev != wb.base_rev {
                continue;
            }
            checked += 1;
            let args = [
                "merge-tree",
                "--write-tree",
                "--merge-base",
                wa.base_rev.as_str(),
                wa.branch.as_str(),
                wb.branch.as_str(),
            ];
            let out = match crate::worktree::git_run(&args, Some(&wa.base_repo)) {
                Ok(out) => out,
                Err(e) => {
                    let _ = self.event_log.append(
                        Some(a.clone()),
                        "status",
                        &format!("conflict check failed: {e}"),
                    );
                    continue;
                }
            };
            if out.code != Some(1) {
                continue; // clean merge, or a real git error logged above
            }
            // `proc::run` reports a nonzero exit via stderr only, so the
            // conflicting paths are the intersection of the two sessions'
            // exact file sets; merge-tree's exit 1 is the conflict oracle.
            let set_a: std::collections::HashSet<String> =
                crate::worktree::sample_files(wa).into_iter().collect();
            let paths: Vec<String> = crate::worktree::sample_files(wb)
                .into_iter()
                .filter(|f| set_a.contains(f))
                .collect();
            if paths.is_empty() {
                fresh.push(crate::model::ConflictEntry {
                    file: "<merge conflict>".to_string(),
                    sessions: vec![a.clone(), b.clone()],
                });
            }
            for file in paths {
                fresh.push(crate::model::ConflictEntry {
                    file,
                    sessions: vec![a.clone(), b.clone()],
                });
            }
        }

        {
            let mut inner = lock(&self.inner);
            // Drop only conflicts whose BOTH sessions are isolated (those are
            // now decided by merge-tree). Legacy filename conflicts survive.
            inner
                .conflicts
                .retain(|c| c.sessions.iter().any(|s| !worktrees.contains_key(s)));
            for entry in fresh {
                if !inner
                    .conflicts
                    .iter()
                    .any(|c| c.file == entry.file && c.sessions == entry.sessions)
                {
                    inner.conflicts.push(entry);
                }
            }
            self.save_now(&inner);
        }
    }
}

/// Does this session run in a private worktree (rather than a shared cwd)?
fn has_worktree(inner: &Inner, session: &str) -> bool {
    inner
        .sessions
        .iter()
        .find(|s| s.id == session)
        .map(|s| {
            s.worktree.is_some()
                || crate::worktree::meta_for_cwd(std::path::Path::new(&s.cwd)).is_some()
        })
        .unwrap_or(false)
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
        // Isolated worktrees make a shared FILE NAME a non-signal: two agents
        // may touch `src/x.rs` in their own worktree without any real merge
        // conflict. `Daemon::refresh_conflicts` decides those via merge-tree.
        let isolated =
            has_worktree(inner, session) && others.iter().all(|s| has_worktree(inner, s));
        if !others.is_empty() && !isolated {
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
