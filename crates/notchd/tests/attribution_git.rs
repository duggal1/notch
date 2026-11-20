//! Integration tests for attribution against REAL git repositories in temp dirs.
//!
//! Every fixture repo is created under a fresh temp dir (never in the project
//! tree). Fixture git commands always pass `-c core.fsmonitor=false`, except
//! the fsmonitor hang case where the hook is wired into the temp repo on
//! purpose (and that repo is deleted afterwards).
//!
//! Owner: Agent attribution-deep. src/ is never modified here.
//!
//! Capacity notes (verified against crates/notchd/src/attribution.rs + daemon.rs):
//!   PER_SESSION_CAP  = 64   (per-session tracked-file cap)
//!   SNAPSHOT_CAP     = 24   (attribution entries surfaced in the snapshot, newest first)
//!   ATTRIBUTION_KEEP = 512  (global in-memory attribution store; oldest popped past this)
//!
//! Ordering notes (verified against record_attribution_locked in daemon.rs):
//!   * record_attribution_locked processes the incoming `files` slice IN ORDER.
//!     sample_git -> parse_porcelain SORTS paths, so through the poll path the
//!     per-session cap keeps the first 64 paths in sorted (porcelain) order,
//!     not the newest 64.
//!   * inner.attribution is newest-first (insert at 0); past ATTRIBUTION_KEEP
//!     the oldest (end of vec) are dropped. Evicted (session, file) pairs are
//!     removed from attribution_seen and decrement session_tracked, so an
//!     evicted file can become attributed again later.
//!   * The snapshot surfaces the newest SNAPSHOT_CAP=24 entries only.
//!
//! Baseline-first-sample rule (verified in poll_once Pass B):
//!   the first sample of a session while it is Working becomes its baseline
//!   (files sampled then are excluded from attribution); subsequent samples
//!   attribute only paths not in the baseline. Idle sessions never sample.

use notchd::attribution;
use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ---------- helpers ----------

static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-attr-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn test_config(root: &Path, stuck_after_secs: u64) -> Config {
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("tmp"),
        stuck_after_secs,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

/// A fresh fixture starts from a valid empty checkpoint. Without this,
/// `Daemon::open` warns "state recovery failed ... starting empty" on every
/// brand-new state dir (A4's strict-recovery path); these tests are about git
/// attribution, so the state-dir warning would be noise in their assertions.
fn seed_state(root: &Path) {
    let state_dir = root.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let path = state_dir.join("state.json");
    if !path.exists() {
        std::fs::write(&path, r#"{"version":1}"#).unwrap();
    }
}

/// The directory a session ACTUALLY samples. Since per-session worktrees
/// landed, a session spawned from a git repo with a HEAD runs in its own
/// private worktree (`<tmp>/notch-<id>/worktree`), not in the cwd handed to
/// `run_plan`. Post-baseline edits must be made here to be observable.
fn session_cwd(daemon: &Daemon, session: &str) -> PathBuf {
    let snap = daemon.snapshot();
    let s = snap
        .sessions
        .iter()
        .find(|s| s.id == session)
        .unwrap_or_else(|| {
            let ids: Vec<&str> = snap.sessions.iter().map(|s| s.id.as_str()).collect();
            panic!("session {session} missing from snapshot: {ids:?}")
        });
    PathBuf::from(&s.cwd)
}

/// Simulate a post-baseline edit BY the session: write into its real worktree.
fn write_in_session(daemon: &Daemon, session: &str, rel: &str, body: &str) -> PathBuf {
    let path = session_cwd(daemon, session).join(rel);
    std::fs::write(&path, body).unwrap();
    path
}

/// Run a git command inside a session's real worktree.
fn git_in_session(daemon: &Daemon, session: &str, args: &[&str]) -> std::process::Output {
    git(&session_cwd(daemon, session), args)
}

/// A4's coalesced writer flushes at most once per second and `poll_once` only
/// marks state dirty. Force a synchronous checkpoint through the public
/// `record_attribution` API with an EMPTY file set (no attribution change), so
/// a fixture can assert persisted state deterministically — no sleep/polling.
fn flush_state(daemon: &Daemon, session: &str) {
    daemon.record_attribution(session, &[]);
}

/// Minimal scripted fake herdr (same shape as tests/acceptance.rs).
struct FakeHerdr {
    agents: Mutex<Vec<Value>>,
    revision: Mutex<u64>,
}

impl FakeHerdr {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            agents: Mutex::new(Vec::new()),
            revision: Mutex::new(1),
        })
    }

    fn set_agents(&self, agents: Vec<Value>) {
        *self.agents.lock().unwrap() = agents;
    }

    fn bump_revision(&self, rev: u64) {
        *self.revision.lock().unwrap() = rev;
    }

    fn herdr(self: &Arc<Self>) -> Herdr {
        let fake = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| fake.run(args))
    }

    fn run(&self, args: &[String]) -> Result<String, HerdrError> {
        let sub = args.get(1).cloned().unwrap_or_default();
        let sub2 = args.get(2).cloned().unwrap_or_default();
        match (sub.as_str(), sub2.as_str()) {
            ("agent", "list") => {
                let agents = self.agents.lock().unwrap().clone();
                Ok(serde_json::json!({
                    "id": "cli:agent:list",
                    "result": { "type": "agent_list", "agents": agents }
                })
                .to_string())
            }
            ("pane", "list") => Ok(serde_json::json!({
                "id": "cli:pane:list",
                "result": { "type": "pane_list", "panes": self.agents.lock().unwrap().clone() }
            })
            .to_string()),
            ("pane", "split") => {
                static SPLIT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let seq = SPLIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                Ok(serde_json::json!({
                    "id": "cli:pane:split",
                    "result": { "type": "pane_info",
                                "pane": { "pane_id": format!("w9{seq}:p{seq}"), "tab_id": "w1:t1", "workspace_id": "w1" } }
                })
                .to_string())
            }
            ("agent", "start") => Ok(serde_json::json!({
                "id": "cli:agent:start",
                "result": { "type": "agent_started", "agent": { "pane_id": "w9:p1", "agent_status": "working", "revision": 1 }, "argv": ["opencode"] }
            })
            .to_string()),
            ("agent", "read") => Ok("test line of pane output\n".to_string()),
            ("agent", "prompt") => Ok(serde_json::json!({
                "id": "cli:agent:prompt",
                "result": { "type": "agent_prompted", "agent": { "pane_id": "w9:p1", "agent_status": "working", "revision": 2 } }
            })
            .to_string()),
            ("pane", "close") => Ok(serde_json::json!({
                "id": "cli:pane:close",
                "result": { "type": "ok" }
            })
            .to_string()),
            other => Err(HerdrError::Failed {
                args: args.to_vec(),
                code: Some(2),
                stderr: format!("unexpected fake-herdr call: {other:?}"),
            }),
        }
    }
}

fn agent_entry(pane_id: &str, status: &str, revision: u64) -> Value {
    serde_json::json!({
        "agent": "opencode",
        "agent_status": status,
        "agent_session": { "agent": "opencode", "kind": "id", "source": "herdr:opencode", "value": pane_id },
        "cwd": "/tmp",
        "focused": false,
        "pane_id": pane_id,
        "revision": revision,
        "terminal_title": format!("OC | {pane_id}"),
        "terminal_id": format!("term_{pane_id}"),
        "tab_id": "w1:t1",
        "workspace_id": "w1",
        "screen_detection_skipped": true
    })
}

fn spawn_daemon(root: &Path, fake: &Arc<FakeHerdr>, clock: Arc<FakeClock>) -> Daemon {
    let config = test_config(root, 900);
    std::fs::create_dir_all(&config.tmp_dir).unwrap();
    seed_state(root);
    Daemon::open(config, fake.herdr(), clock).unwrap()
}

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    let out = std::process::Command::new("git")
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git spawn failed");
    out
}

/// Create a repo at `dir` with `user.name`/`user.email` configured, committed.
fn init_repo_with_commit(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let out = git(dir, &["init", "-q"]);
    assert!(out.status.success(), "git init failed: {:?}", out);
    git(
        dir,
        &["config", "user.email", "attribution-deep@example.com"],
    );
    git(dir, &["config", "user.name", "attribution-deep"]);
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    let out = git(dir, &["commit", "-qm", message]);
    assert!(
        out.status.success(),
        "git commit failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Dirty exactly 300 paths in `dir`: 100 modified tracked `mod_*`, 100
/// untracked `new*_*`, 100 staged `staged*_*`. Prefixes are parameterized so
/// the identical fixture can be replayed inside a per-session worktree.
fn dirty_300(dir: &Path, new_prefix: &str, staged_prefix: &str, mod_body: &str) {
    for i in 0..100 {
        std::fs::write(dir.join(format!("mod_{i:03}.rs")), mod_body).unwrap();
    }
    for i in 0..100 {
        std::fs::write(dir.join(format!("{new_prefix}{i:03}.rs")), "fn new() {}\n").unwrap();
    }
    for i in 0..100 {
        std::fs::write(
            dir.join(format!("{staged_prefix}{i:03}.rs")),
            "fn staged() {}\n",
        )
        .unwrap();
    }
    let out = std::process::Command::new("git")
        .args(["-c", "core.fsmonitor=false", "add"])
        .args((0..100).map(|i| format!("{staged_prefix}{i:03}.rs")))
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git add {staged_prefix}* failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Write plan spawning one session per cwd entry, all in supervisor SV-1.
fn write_plan(root: &Path, cwds: &[&Path]) -> PathBuf {
    let sessions: Vec<Value> = cwds
        .iter()
        .enumerate()
        .map(|(i, cwd)| {
            serde_json::json!({
                "title": format!("task {i}"),
                "prompt": format!("prompt {i}"),
                "cwd": cwd,
            })
        })
        .collect();
    let plan = serde_json::json!({
        "stuck_after_secs": 900,
        "supervisors": [ { "sessions": sessions } ]
    });
    let path = root.join("plan.json");
    std::fs::write(&path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    path
}

/// Spawn daemon + plan with `cwds`, set all sessions to working @ revision 1,
/// and return (daemon, session ids in spawn order, pane ids).
fn spawn_working_sessions(
    root: &Path,
    fake: &Arc<FakeHerdr>,
    clock: Arc<FakeClock>,
    cwds: &[&Path],
) -> (Daemon, Vec<String>, Vec<String>) {
    let daemon = spawn_daemon(root, fake, clock);
    let plan = write_plan(root, cwds);
    let ack = daemon.run_plan(&plan);
    assert!(ack.ok, "run_plan failed: {:?}", ack.error);
    let snap = daemon.snapshot();
    let session_ids: Vec<String> = snap.sessions.iter().map(|s| s.id.clone()).collect();
    let panes: Vec<String> = snap.sessions.iter().map(|s| s.pane_id.clone()).collect();
    let entries: Vec<Value> = panes
        .iter()
        .map(|p| agent_entry(p, "working", *fake.revision.lock().unwrap()))
        .collect();
    fake.set_agents(entries);
    (daemon, session_ids, panes)
}

fn set_working_revision(fake: &Arc<FakeHerdr>, panes: &[String], revision: u64) {
    fake.bump_revision(revision);
    let entries: Vec<Value> = panes
        .iter()
        .map(|p| agent_entry(p, "working", revision))
        .collect();
    fake.set_agents(entries);
}

fn attributed_files(daemon: &Daemon, session: &str) -> Vec<String> {
    daemon
        .snapshot()
        .attribution
        .into_iter()
        .filter(|a| a.session == session)
        .map(|a| a.file)
        .collect()
}

/// Deterministic 300-dirty-file fixture: 100 modified tracked files
/// (`mod_000.rs`..), 100 untracked new files (`new_000.rs`..), 100 staged
/// (`staged_000.rs`..). Returns the repo path.
fn build_300_dirty_fixture(base: &Path, name: &str) -> PathBuf {
    let repo = base.join(name);
    init_repo_with_commit(&repo);
    // Tracked files to be modified.
    for i in 0..100 {
        std::fs::write(repo.join(format!("mod_{i:03}.rs")), "fn base() {}\n").unwrap();
    }
    commit_all(&repo, "mod base");
    dirty_300(&repo, "new_", "staged_", "fn base() { /* modified */ }\n");
    repo
}

fn state_json_attribution(root: &Path) -> Vec<(String, String)> {
    let raw = std::fs::read_to_string(root.join("state").join("state.json")).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    v["attribution"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["session"].as_str().unwrap().to_string(),
                e["file"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn cleanup(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

// ---------- Cap sanity (verify the exact current cap from code) ----------

#[test]
fn per_session_cap_current_value_is_64() {
    assert_eq!(attribution::per_session_cap(), 64);
    assert_eq!(attribution::snapshot_cap(), 24);
    assert_eq!(attribution::tracked_cap(), 512);
}

// ---------- 1. Unborn HEAD (git init, no commit) ----------

#[test]
fn unborn_head_attribution_signal_no_panic() {
    let root = tmp_root("unborn");
    let repo = root.join("unborn");
    std::fs::create_dir_all(&repo).unwrap();
    let out = git(&repo, &["init", "-q"]);
    assert!(out.status.success());
    git(
        &repo,
        &["config", "user.email", "attribution-deep@example.com"],
    );
    git(&repo, &["config", "user.name", "attribution-deep"]);
    std::fs::write(repo.join("untracked.rs"), "fn untracked() {}\n").unwrap();
    std::fs::write(repo.join("staged.rs"), "fn staged() {}\n").unwrap();
    git(&repo, &["add", "staged.rs"]);

    // Direct sample: unborn HEAD is a normal signal source — staged + untracked
    // files are reported. No error is returned to the caller.
    let files = attribution::sample_git(&repo);
    assert!(
        files.contains(&"staged.rs".to_string()),
        "staged file missing: {files:?}"
    );
    assert!(
        files.contains(&"untracked.rs".to_string()),
        "untracked file missing: {files:?}"
    );

    // Through the daemon, against the same unborn repo: poll must not panic
    // or error. Baseline rule applies as usual — the files that are dirty at
    // the first sample are excluded, later ones are attributed.
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[repo.as_path()]);
    daemon.poll_once(); // baseline = {staged.rs, untracked.rs}; attributes nothing
    assert_eq!(
        daemon.snapshot().attribution.len(),
        0,
        "baseline files must not be attributed"
    );
    // An unborn HEAD has no commit to branch a per-session worktree from, so
    // the engine now emits ONE loud fallback warning (HARDENING §3.6) and
    // keeps using the original cwd. It is a signal, not an error.
    let warnings = daemon.snapshot().warnings;
    assert_eq!(
        warnings.len(),
        1,
        "unborn HEAD: exactly the worktree-fallback warning expected: {warnings:?}"
    );
    assert!(
        warnings[0].contains(&session_ids[0]),
        "warning must name the session: {warnings:?}"
    );
    assert!(
        warnings[0].contains("worktree disabled"),
        "warning must explain fallback: {warnings:?}"
    );

    // A file dirtied after the baseline IS attributed, even though HEAD is unborn.
    std::fs::write(repo.join("after.rs"), "fn after() {}\n").unwrap();
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();
    let attributed = attributed_files(&daemon, &session_ids[0]);
    assert!(
        attributed.contains(&"after.rs".to_string()),
        "after.rs missing: {attributed:?}"
    );
    assert!(!attributed.contains(&"staged.rs".to_string()));
    assert!(!attributed.contains(&"untracked.rs".to_string()));

    // Completely empty unborn repo (no files at all): empty signal, no panic, no error.
    let empty_repo = root.join("empty-unborn");
    std::fs::create_dir_all(&empty_repo).unwrap();
    assert!(git(&empty_repo, &["init", "-q"]).status.success());
    let empty = attribution::sample_git(&empty_repo);
    assert!(
        empty.is_empty(),
        "empty unborn repo must yield no paths: {empty:?}"
    );

    cleanup(&root);
}

// ---------- 2. 300 dirty files: per-session cap + determinism ----------

#[test]
fn three_hundred_dirty_files_cap_and_deterministic_order() {
    let mut attributed_sets = Vec::new();
    for name in ["fixture-a", "fixture-b"] {
        let root = tmp_root("300");
        let repo = build_300_dirty_fixture(&root, name);

        // Verify the fixture is really 300 dirty paths.
        let sampled = attribution::sample_git(&repo);
        assert_eq!(
            sampled.len(),
            300,
            "expected 300 dirty paths, got {}: {:?}",
            sampled.len(),
            sampled
        );
        assert_eq!(
            sampled,
            {
                let mut s = sampled.clone();
                s.sort();
                s
            },
            "porcelain parse must be sorted"
        );

        let fake = FakeHerdr::new();
        let clock = Arc::new(FakeClock::new(1_763_520_000_000));
        let (daemon, session_ids, panes) =
            spawn_working_sessions(&root, &fake, clock, &[repo.as_path()]);
        // The worktree is created from the committed base, so replay the dirty
        // fixture INSIDE the session's own worktree before the first sample.
        dirty_300(
            &session_cwd(&daemon, &session_ids[0]),
            "new_",
            "staged_",
            "fn base() { /* replay */ }\n",
        );
        daemon.poll_once(); // fixture is dirty at first sample -> baseline absorbs all 300
                            // (baseline-first-sample rule; the cap itself is exercised below on a clean baseline)
        set_working_revision(&fake, &panes, 2);
        daemon.poll_once();
        let attributed = attributed_files(&daemon, &session_ids[0]);
        // Documented reality: because the first sample happened while the tree
        // was already dirty, the baseline-first-sample rule swallows everything.
        assert_eq!(
            attributed.len(),
            0,
            "dirty-at-first-sample must be excluded"
        );
        cleanup(&root);

        // End-to-end cap check: baseline on a CLEAN repo, then dirty 300 files.
        let root = tmp_root("300b");
        let repo = build_300_dirty_fixture(&root, &format!("{name}-clean"));
        // Reset: commit everything so the first sample sees a clean tree.
        commit_all(&repo, "fixture checkpoint");
        let fake = FakeHerdr::new();
        let clock = Arc::new(FakeClock::new(1_763_520_000_000));
        let (daemon, session_ids, panes) =
            spawn_working_sessions(&root, &fake, clock, &[repo.as_path()]);
        daemon.poll_once(); // baseline = ∅ (clean tree at first sample)
                            // Re-dirty exactly 300 files the same way, in the session's worktree
                            // (100 unstaged modified + 100 untracked + 100 staged).
        let wt = session_cwd(&daemon, &session_ids[0]);
        dirty_300(&wt, "new2_", "staged2_", "fn base() { /* v2 */ }\n");
        let sampled = attribution::sample_git(&wt);
        assert_eq!(
            sampled.len(),
            300,
            "expected 300 dirty paths at poll 2, got {}",
            sampled.len()
        );

        set_working_revision(&fake, &panes, 2);
        daemon.poll_once(); // the 300 files are now post-baseline
                            // The snapshot surfaces only the newest SNAPSHOT_CAP entries, so the
                            // full per-session cap is observed via state.json (keep = 512).
        let cap = attribution::per_session_cap();
        assert_eq!(cap, 64);
        flush_state(&daemon, &session_ids[0]);
        let tracked = state_json_attribution(&root);
        let mut mine: Vec<String> = tracked
            .into_iter()
            .filter(|(s, _)| s == &session_ids[0])
            .map(|(_, f)| f)
            .collect();
        assert_eq!(
            mine.len(),
            cap,
            "per-session cap must hold, got {}",
            mine.len()
        );
        mine.sort();
        attributed_sets.push(mine);
        // Snapshot head must be newest-first and capped.
        let snap = daemon.snapshot();
        assert_eq!(snap.attribution.len(), 24, "snapshot attribution cap");
        assert_eq!(
            snap.attribution[0].file, "mod_063.rs",
            "snapshot attribution must be newest first"
        );
        cleanup(&root);
    }
    assert_eq!(
        attributed_sets[0], attributed_sets[1],
        "attribution order/set must be deterministic across identical fixtures"
    );
    // The poll path feeds parse_porcelain output (sorted) into
    // record_attribution_locked in order, so the cap keeps the FIRST 64
    // sorted paths: mod_000..mod_063 (mod_* < new2_* < staged2_*).
    let expected: Vec<String> = (0..64).map(|i| format!("mod_{i:03}.rs")).collect();
    assert_eq!(
        attributed_sets[0], expected,
        "cap must keep first 64 in sorted order"
    );
}

#[test]
fn record_attribution_direct_cap_keeps_first_input_order() {
    let root = tmp_root("300direct");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    let files: Vec<String> = (0..300).map(|i| format!("src/file_{i:03}.rs")).collect();
    daemon.record_attribution("S1-OC01", &files);
    let tracked = state_json_attribution(&root)
        .into_iter()
        .filter(|(s, _)| s == "S1-OC01")
        .count();
    assert_eq!(
        tracked, 64,
        "per-session cap must stop tracking at 64, got {tracked}"
    );
    // Input order (not sorted, not newest) is honored by record_attribution.
    let snap = daemon.snapshot();
    let first = &snap.attribution[0];
    assert_eq!(
        first.file, "src/file_063.rs",
        "newest-first: last-recorded of the kept 64 is at head"
    );
    let tracked_files: std::collections::HashSet<String> = state_json_attribution(&root)
        .into_iter()
        .filter(|(s, _)| s == "S1-OC01")
        .map(|(_, f)| f)
        .collect();
    assert!(
        !tracked_files.contains("src/file_064.rs"),
        "path 64 must not be tracked"
    );
    assert_eq!(tracked_files.len(), 64);

    // Cap already reached: recording 10 more NEW files tracks nothing.
    let extra: Vec<String> = (300..310).map(|i| format!("src/extra_{i}.rs")).collect();
    daemon.record_attribution("S1-OC01", &extra);
    let tracked = state_json_attribution(&root)
        .into_iter()
        .filter(|(s, _)| s == "S1-OC01")
        .count();
    assert_eq!(tracked, 64);
    let tracked_files: std::collections::HashSet<String> = state_json_attribution(&root)
        .into_iter()
        .filter(|(s, _)| s == "S1-OC01")
        .map(|(_, f)| f)
        .collect();
    assert!(
        !tracked_files.iter().any(|f| f.contains("extra_")),
        "nothing new tracked past the cap"
    );
    cleanup(&root);
}

#[test]
fn global_keep_eviction_drops_oldest_keeps_newest() {
    let root = tmp_root("300global");
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let daemon = spawn_daemon(&root, &fake, clock);
    // 9 sessions x 60 files = 540 inserts > ATTRIBUTION_KEEP (512).
    for s in 0..9 {
        let session = format!("S{}-OC01", s + 1);
        let files: Vec<String> = (0..60).map(|i| format!("src/s{s}_f{i:02}.rs")).collect();
        daemon.record_attribution(&session, &files);
    }
    let tracked = state_json_attribution(&root);
    assert_eq!(
        tracked.len(),
        512,
        "global keep must trim to 512, got {}",
        tracked.len()
    );
    let present: std::collections::HashSet<String> =
        tracked.iter().map(|(_, f)| f.clone()).collect();
    // First 28 inserts globally are s0_f00..s0_f27 -> evicted (oldest).
    for i in 0..28 {
        assert!(
            !present.contains(&format!("src/s0_f{i:02}.rs")),
            "oldest must be evicted: s0_f{i:02}"
        );
    }
    assert!(
        present.contains("src/s0_f28.rs"),
        "newest of session 0 must survive"
    );
    assert!(
        present.contains("src/s8_f59.rs"),
        "most recent insert must survive"
    );
    // Newest-first order preserved at the head of the snapshot too.
    let snap = daemon.snapshot();
    assert_eq!(snap.attribution[0].file, "src/s8_f59.rs");
    cleanup(&root);
}

// ---------- 3. Rename with similarity: new path wins ----------

#[test]
fn rename_attributes_new_path_not_old() {
    let root = tmp_root("rename");
    let repo = root.join("repo");
    init_repo_with_commit(&repo);
    // Multi-line file so a rename + small edit stays above git's similarity threshold.
    let body: String = (1..=40)
        .map(|i| format!("fn line_{i}() {{}} // payload {i}\n"))
        .collect();
    std::fs::write(repo.join("oldpath.rs"), &body).unwrap();
    commit_all(&repo, "base");

    // Stage a rename with a tiny edit (git mv + edit).
    let out = git(&repo, &["mv", "oldpath.rs", "newpath.rs"]);
    assert!(out.status.success());
    std::fs::write(
        repo.join("newpath.rs"),
        body.replace("payload 7 ", "payload 7 EDITED "),
    )
    .unwrap();
    let porcelain = git(&repo, &["status", "--porcelain=v1"]);
    let porcelain = String::from_utf8_lossy(&porcelain.stdout);
    assert!(
        porcelain.contains(" -> "),
        "expected rename line in: {porcelain:?}"
    );

    let files = attribution::parse_porcelain(&porcelain);
    assert!(
        files.contains(&"newpath.rs".to_string()),
        "new path must be attributed: {files:?}"
    );
    assert!(
        !files.contains(&"oldpath.rs".to_string()),
        "old path must NOT appear: {files:?}"
    );

    // End-to-end through the daemon. The session samples its own worktree,
    // which is created from the committed base (still `oldpath.rs`) and is
    // clean at spawn, so its baseline is empty. Replay the rename there.
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[repo.as_path()]);
    daemon.poll_once(); // baseline = ∅ in the private worktree

    let out = git_in_session(
        &daemon,
        &session_ids[0],
        &["mv", "oldpath.rs", "renamed.rs"],
    );
    assert!(
        out.status.success(),
        "git mv failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let wt = session_cwd(&daemon, &session_ids[0]);
    std::fs::write(
        wt.join("renamed.rs"),
        std::fs::read_to_string(wt.join("renamed.rs"))
            .unwrap()
            .replace("payload 9 ", "payload 9 EDITED "),
    )
    .unwrap();
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();
    let attributed = attributed_files(&daemon, &session_ids[0]);
    assert!(
        attributed.contains(&"renamed.rs".to_string()),
        "renamed path missing: {attributed:?}"
    );
    assert!(
        !attributed.contains(&"newpath.rs".to_string()),
        "source of rename must not be attributed: {attributed:?}"
    );
    assert!(!attributed.contains(&"oldpath.rs".to_string()));
    // The old path is never falsely flagged as conflicted: conflicts are empty
    // for a single session even across a rename.
    assert!(
        daemon.snapshot().conflicts.is_empty(),
        "conflicts: {:?}",
        daemon.snapshot().conflicts
    );
    cleanup(&root);
}

// ---------- 4. Submodule: parent sees ` M sub`, never inner paths ----------

#[test]
fn submodule_dirty_inner_file_no_garbage_no_crash() {
    let root = tmp_root("submodule");
    let sub_src = root.join("sub-src");
    init_repo_with_commit(&sub_src);
    std::fs::write(sub_src.join("inner.rs"), "fn inner() {}\n").unwrap();
    commit_all(&sub_src, "inner base");

    let parent = root.join("parent");
    init_repo_with_commit(&parent);
    std::fs::write(parent.join("parent.rs"), "fn parent() {}\n").unwrap();
    commit_all(&parent, "parent base");
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "-c",
            "core.fsmonitor=false",
            "submodule",
            "add",
            "-q",
        ])
        .arg(&sub_src)
        .arg("sub")
        .current_dir(&parent)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "submodule add failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    commit_all(&parent, "add submodule");

    // Dirty a file INSIDE the submodule.
    std::fs::write(
        parent.join("sub").join("inner.rs"),
        "fn inner() { /* dirty */ }\n",
    )
    .unwrap();

    // Direct sample: parent porcelain shows the submodule as a single entry.
    let sampled = attribution::sample_git(&parent);
    assert_eq!(
        sampled,
        vec!["sub".to_string()],
        "expected only the submodule path, got {sampled:?}"
    );
    assert!(
        !sampled.iter().any(|f| f.contains("inner.rs")),
        "inner path must not leak: {sampled:?}"
    );

    // Reset so the daemon's first sample sees a CLEAN parent (else the
    // baseline rule vacuously absorbs `sub` and nothing is ever attributed).
    // Restore the tracked inner file instead of committing a new submodule
    // revision: the gitlink still points at `sub-src`'s "inner base" commit,
    // which a fresh worktree can fetch when it initializes the submodule.
    let out = git(&parent.join("sub"), &["checkout", "--", "inner.rs"]);
    assert!(
        out.status.success(),
        "submodule reset failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        attribution::sample_git(&parent).is_empty(),
        "parent must be clean before baseline"
    );

    // End-to-end: baseline on clean parent, then dirty inner file -> poll.
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[parent.as_path()]);
    daemon.poll_once(); // baseline = ∅

    // A fresh worktree has an EMPTY submodule checkout: populate it first,
    // then dirty the inner file inside the session's real worktree.
    let init = git_in_session(
        &daemon,
        &session_ids[0],
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "-q",
            "--",
            "sub",
        ],
    );
    assert!(
        init.status.success(),
        "worktree submodule init failed: {:?}",
        String::from_utf8_lossy(&init.stderr)
    );
    write_in_session(
        &daemon,
        &session_ids[0],
        "sub/inner.rs",
        "fn inner() { /* dirty again */ }\n",
    );
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();
    let attributed = attributed_files(&daemon, &session_ids[0]);
    assert_eq!(
        attributed,
        vec!["sub".to_string()],
        "only the submodule path may be attributed: {attributed:?}"
    );
    assert!(
        daemon.snapshot().warnings.is_empty(),
        "no warnings expected: {:?}",
        daemon.snapshot().warnings
    );
    cleanup(&root);
}

// ---------- 5. Linked worktree: session cwd = worktree, not "not a repo" ----------

#[test]
fn linked_worktree_cwd_attribution_works() {
    let root = tmp_root("worktree");
    let main_repo = root.join("main");
    init_repo_with_commit(&main_repo);
    std::fs::write(main_repo.join("base.rs"), "fn base() {}\n").unwrap();
    commit_all(&main_repo, "base");

    let wt = root.join("wt");
    let out = git(
        &main_repo,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "wt-branch",
        ],
    );
    assert!(
        out.status.success(),
        "worktree add failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The linked worktree's .git is a FILE, not a directory — attribution must
    // still recognize it as a repo (not a "not a git repo" false negative).
    assert!(
        wt.join(".git").is_file(),
        "linked worktree .git must be a file"
    );

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) = spawn_working_sessions(&root, &fake, clock, &[wt.as_path()]);
    daemon.poll_once(); // baseline = ∅

    // The session gets its OWN worktree, branched from `wt`'s HEAD; write the
    // post-baseline file there, not into the plan cwd.
    write_in_session(&daemon, &session_ids[0], "wt-only.rs", "fn wt_only() {}\n");
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();
    let attributed = attributed_files(&daemon, &session_ids[0]);
    assert!(
        attributed.contains(&"wt-only.rs".to_string()),
        "worktree file must be attributed: {attributed:?}"
    );

    // The main checkout does NOT see the worktree's dirty file.
    let main_sampled = attribution::sample_git(&main_repo);
    assert!(
        !main_sampled.contains(&"wt-only.rs".to_string()),
        "worktree change leaked into main: {main_sampled:?}"
    );
    cleanup(&root);
}

// ---------- 6. core.fsmonitor=true + hanging hook: attribution < 1s ----------

#[test]
fn hanging_fsmonitor_hook_attribution_returns_within_1s() {
    let root = tmp_root("fsmonitor");
    let repo = root.join("repo");
    init_repo_with_commit(&repo);
    std::fs::write(repo.join("base.rs"), "fn base() {}\n").unwrap();
    commit_all(&repo, "base");

    // Wire the repo's fsmonitor to a hook that sleeps 30s on every invocation.
    let hook = root.join("fsm-hook.sh");
    std::fs::write(&hook, "#!/bin/sh\nsleep 30\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook, perms).unwrap();
    }
    let out = git(&repo, &["config", "core.fsmonitor", hook.to_str().unwrap()]);
    assert!(out.status.success());
    // Read-back must be done WITHOUT the -c core.fsmonitor=false override
    // (the override would mask the configured value). `git config` is a
    // pure config read: it never executes the hook.
    let configured = std::process::Command::new("git")
        .args(["config", "core.fsmonitor"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&configured.stdout).trim(),
        hook.to_str().unwrap(),
        "fsmonitor hook must be wired into the repo config"
    );

    // Dirty a file so there is something to report.
    std::fs::write(repo.join("post.rs"), "fn post() {}\n").unwrap();

    // sample_git forces `-c core.fsmonitor=false`, so the 30s hook is never
    // executed on this path. Measure the real wall time; the hard bar is 1.2s.
    let start = std::time::Instant::now();
    let files = attribution::sample_git(&repo);
    let elapsed = start.elapsed();
    eprintln!(
        "[fsmonitor] sample_git wall time: {} ms (hook wired: sleep 30)",
        elapsed.as_millis()
    );
    assert!(
        files.contains(&"post.rs".to_string()),
        "expected post.rs: {files:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(1200),
        "attribution exceeded 1.2s with hanging fsmonitor hook: {:?}",
        elapsed
    );

    // End-to-end through the daemon (baseline clean, dirty after baseline).
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[repo.as_path()]);
    // Reset the repo to clean BEFORE the baseline poll (commit the dirty file),
    // then re-dirty so the post-baseline sample has work to do.
    commit_all(&repo, "post checkpoint");
    assert!(attribution::sample_git(&repo).is_empty());
    // Baseline poll (fsmonitor still wired): must be fast and not hang.
    let start = std::time::Instant::now();
    daemon.poll_once();
    let baseline_elapsed = start.elapsed();
    eprintln!(
        "[fsmonitor] baseline poll_once wall time: {} ms",
        baseline_elapsed.as_millis()
    );
    assert!(baseline_elapsed < std::time::Duration::from_millis(1200));

    // The fsmonitor config lives in the shared repo config, so the session's
    // worktree inherits it; `git_run` still forces core.fsmonitor=false.
    write_in_session(&daemon, &session_ids[0], "post2.rs", "fn post2() {}\n");
    set_working_revision(&fake, &panes, 2);
    let start = std::time::Instant::now();
    daemon.poll_once(); // attributes post2.rs through the hanging-hook worktree
    let poll_elapsed = start.elapsed();
    eprintln!(
        "[fsmonitor] post-baseline poll_once wall time: {} ms",
        poll_elapsed.as_millis()
    );
    assert!(poll_elapsed < std::time::Duration::from_millis(1200));
    let attributed = attributed_files(&daemon, &session_ids[0]);
    assert!(
        attributed.contains(&"post2.rs".to_string()),
        "attributed: {attributed:?}"
    );
    cleanup(&root);
}

// ---------- 7. cwd = repo toplevel (temp root) and nested repo ----------

#[test]
fn cwd_repo_toplevel_is_temp_root_and_nested_repo_returns_inner_files() {
    // (a) The repo's toplevel IS the temp scratch root of this test.
    let root = tmp_root("toplevel");
    init_repo_with_commit(&root);
    std::fs::write(root.join("topfile.rs"), "fn top() {}\n").unwrap();
    commit_all(&root, "base");
    std::fs::write(root.join("top-dirty.rs"), "fn dirty() {}\n").unwrap();
    let files = attribution::sample_git(&root);
    assert!(
        files.contains(&"top-dirty.rs".to_string()),
        "toplevel repo must yield real files: {files:?}"
    );
    let out = git(&root, &["rev-parse", "--show-toplevel"]);
    assert_eq!(
        std::fs::canonicalize(String::from_utf8_lossy(&out.stdout).trim()).unwrap(),
        std::fs::canonicalize(&root).unwrap(),
        "toplevel must be the temp root itself"
    );

    // (b) A repo nested INSIDE our temp scratch (a linked worktree of an
    // inner repo) is attributed with the INNER repo's files.
    let scratch = tmp_root("nested");
    let inner = scratch.join("inner-repo");
    init_repo_with_commit(&inner);
    std::fs::write(inner.join("inner-base.rs"), "fn base() {}\n").unwrap();
    commit_all(&inner, "base");

    let nested_wt = scratch.join("nested-wt");
    let out = git(
        &inner,
        &[
            "worktree",
            "add",
            "-q",
            nested_wt.to_str().unwrap(),
            "-b",
            "nested-wt-branch",
        ],
    );
    assert!(
        out.status.success(),
        "nested worktree add failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::write(nested_wt.join("inner-dirty.rs"), "fn inner_dirty() {}\n").unwrap();

    let files = attribution::sample_git(&nested_wt);
    assert!(
        files.contains(&"inner-dirty.rs".to_string()),
        "inner repo files expected: {files:?}"
    );
    // Paths are relative to the INNER toplevel, never prefixed by the scratch path.
    assert!(
        !files.iter().any(|f| f.contains(scratch.to_str().unwrap())),
        "scratch path leaked: {files:?}"
    );

    // And poll through the daemon with cwd = the nested worktree.
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&scratch, &fake, clock, &[nested_wt.as_path()]);
    // The session's private worktree is branched from the nested worktree's
    // HEAD (clean), so its baseline is empty; replay the post-baseline edit
    // there, not in the plan cwd.
    daemon.poll_once(); // baseline = ∅ in the private worktree
    write_in_session(&daemon, &session_ids[0], "another.rs", "fn another() {}\n");
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();
    let attributed = attributed_files(&daemon, &session_ids[0]);
    assert_eq!(attributed, vec!["another.rs".to_string()]);

    // A non-repo directory sitting next to the nested repo yields no signal.
    let plain = scratch.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("x.rs"), "fn x() {}\n").unwrap();
    assert!(attribution::sample_git(&plain).is_empty());
    cleanup(&root);
    cleanup(&scratch);
}

// ---------- 8. Non-git directory: no signal, one loud fallback warning ----------

#[test]
fn non_git_directory_zero_errors_one_loud_warning_no_signal() {
    let root = tmp_root("nongit");
    let plain = root.join("foo");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("x.rs"), "fn x() {}\n").unwrap();

    // Direct sample: empty, never an error.
    let files = attribution::sample_git(&plain);
    assert!(
        files.is_empty(),
        "non-git dir must yield no paths: {files:?}"
    );

    // Daemon poll against a non-git cwd: works, attributes nothing, and
    // surfaces the required loud "worktree disabled" fallback warning.
    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, _panes) =
        spawn_working_sessions(&root, &fake, clock, &[plain.as_path()]);
    daemon.poll_once();
    let attributed = attributed_files(&daemon, &session_ids[0]);
    assert!(attributed.is_empty());
    let snap = daemon.snapshot();
    // HARDENING §3.6: a non-git cwd spawns with a LOUD warning and falls back
    // to the shared-cwd heuristic. It is the only warning for this fixture.
    assert_eq!(
        snap.warnings.len(),
        1,
        "non-git cwd: exactly one documented fallback warning expected: {:?}",
        snap.warnings
    );
    assert!(
        snap.warnings[0].contains("not a git repository")
            && snap.warnings[0].contains("worktree disabled"),
        "warning must explain the worktree fallback: {:?}",
        snap.warnings
    );
    assert!(snap.conflicts.is_empty());
    // Missing directory: same silent non-signal, no panic.
    let missing = root.join("does-not-exist");
    assert!(attribution::sample_git(&missing).is_empty());
    cleanup(&root);
}

// ---------- 9. Two sessions, same repo, baseline-first-sample rule ----------

#[test]
fn two_sessions_dirty_repo_at_first_sample_excluded_for_both() {
    let root = tmp_root("two-baseline");
    let repo = root.join("repo");
    init_repo_with_commit(&repo);
    std::fs::write(repo.join("shared_a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(repo.join("shared_b.rs"), "fn b() {}\n").unwrap();
    commit_all(&repo, "base");
    // Repo is DIRTY before either session's first sample.
    std::fs::write(repo.join("pre_dirty_a.rs"), "fn pa() {}\n").unwrap();
    std::fs::write(repo.join("pre_dirty_b.rs"), "fn pb() {}\n").unwrap();

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[repo.as_path(), repo.as_path()]);
    assert_eq!(session_ids.len(), 2);
    daemon.poll_once(); // first samples: both baselines = {pre_dirty_a.rs, pre_dirty_b.rs}

    // Files dirty at first sample are excluded for BOTH sessions.
    let mut attributed = attributed_files(&daemon, &session_ids[0]);
    attributed.extend(attributed_files(&daemon, &session_ids[1]));
    assert!(
        attributed.is_empty(),
        "pre-baseline files must be excluded for both sessions: {attributed:?}"
    );
    // And critically: NO false conflict between the two sessions sharing the repo.
    let snap = daemon.snapshot();
    assert!(
        snap.conflicts.is_empty(),
        "shared dirty repo must not yield a conflict: {:?}",
        snap.conflicts
    );

    // A second poll with no new changes changes nothing either.
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();
    let total: usize = session_ids
        .iter()
        .map(|id| attributed_files(&daemon, id).len())
        .sum();
    assert_eq!(total, 0);
    assert!(daemon.snapshot().conflicts.is_empty());
    cleanup(&root);
}

/// A5's per-session worktrees FIXED the old shared-cwd double-attribution:
/// two sessions spawned from the same cwd each sample a private worktree, so
/// a post-baseline edit made by one is visible to that one only and produces
/// no filename conflict. This test is the regression proof of that fix.
#[test]
fn two_sessions_same_cwd_after_baseline_attributed_to_editor_only() {
    let root = tmp_root("two-after");
    let repo = root.join("repo");
    init_repo_with_commit(&repo);
    std::fs::write(repo.join("base.rs"), "fn base() {}\n").unwrap();
    commit_all(&repo, "base");

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[repo.as_path(), repo.as_path()]);
    daemon.poll_once(); // both baselines = ∅ (clean worktrees)

    // Session S1-OC01 edits in ITS OWN worktree after both baselines.
    write_in_session(
        &daemon,
        &session_ids[0],
        "after_baseline.rs",
        "fn after() {}\n",
    );
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();

    let a0 = attributed_files(&daemon, &session_ids[0]);
    let a1 = attributed_files(&daemon, &session_ids[1]);
    eprintln!("[case9 same-cwd] {session_ids:?}: {a0:?} / {a1:?}");
    eprintln!(
        "[case9 same-cwd] conflicts: {:?}",
        daemon.snapshot().conflicts
    );
    assert!(
        a0.contains(&"after_baseline.rs".to_string()),
        "editor session: {a0:?}"
    );
    assert!(
        a1.is_empty(),
        "non-editor session must not attribute the edit: {a1:?}"
    );
    let conflicts = daemon.snapshot().conflicts;
    assert!(
        conflicts.is_empty(),
        "isolated worktrees: no filename conflict expected: {conflicts:?}"
    );
    cleanup(&root);
}

/// Spawn-brief case 9: "a file modified AFTER both baselines by one session
/// shows as attributed to that one only" for two sessions sharing one repo
/// cwd. Previously filed as an engine limitation (shared cwd); A5's
/// per-session worktrees now satisfy it, so this runs un-ignored.
#[test]
fn two_sessions_same_cwd_after_baseline_attributed_to_one_only() {
    let root = tmp_root("two-spec");
    let repo = root.join("repo");
    init_repo_with_commit(&repo);
    std::fs::write(repo.join("base.rs"), "fn base() {}\n").unwrap();
    commit_all(&repo, "base");

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[repo.as_path(), repo.as_path()]);
    daemon.poll_once(); // both baselines = ∅

    write_in_session(
        &daemon,
        &session_ids[0],
        "after_baseline.rs",
        "fn after() {}\n",
    );
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();

    let counts = session_ids
        .iter()
        .filter(|id| attributed_files(&daemon, id).contains(&"after_baseline.rs".to_string()))
        .count();
    assert_eq!(
        counts, 1,
        "spec expectation: exactly one session attributes the file"
    );
    cleanup(&root);
}

/// Same repository, separate worktrees: per-worktree git status DOES isolate
/// the editor, so the modifying session attributes the file and the other
/// does not. This satisfies the spec under the "same repository" reading.
#[test]
fn two_sessions_same_repository_worktrees_after_baseline_attributed_to_one_only() {
    let root = tmp_root("two-wt");
    let main_repo = root.join("main");
    init_repo_with_commit(&main_repo);
    std::fs::write(main_repo.join("base.rs"), "fn base() {}\n").unwrap();
    commit_all(&main_repo, "base");

    let wt = root.join("wt");
    let out = git(
        &main_repo,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "wt-two",
        ],
    );
    assert!(out.status.success());

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[main_repo.as_path(), wt.as_path()]);
    daemon.poll_once(); // both baselines = ∅

    // Only the first session "edits" after both baselines, inside its OWN
    // private worktree (not the plan cwd).
    write_in_session(
        &daemon,
        &session_ids[0],
        "session_one_work.rs",
        "fn work() {}\n",
    );
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();

    let a0 = attributed_files(&daemon, &session_ids[0]);
    let a1 = attributed_files(&daemon, &session_ids[1]);
    assert!(
        a0.contains(&"session_one_work.rs".to_string()),
        "worktree A session must see its file: {a0:?}"
    );
    assert!(
        a1.is_empty(),
        "worktree B session must not see A's file: {a1:?}"
    );
    assert!(daemon.snapshot().conflicts.is_empty());
    cleanup(&root);
}

// ---------- 10. Merge conflict state (unmerged paths in index) ----------

#[test]
fn merge_conflict_unmerged_paths_attribution_does_not_choke() {
    let root = tmp_root("conflict");
    let repo = root.join("repo");
    init_repo_with_commit(&repo);
    std::fs::write(repo.join("c.rs"), "line1\nline2\nline3\n").unwrap();
    commit_all(&repo, "base");
    // Capture the initial branch name instead of assuming master/main.
    let branch = git(&repo, &["symbolic-ref", "--short", "HEAD"]);
    let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
    let out = git(&repo, &["checkout", "-qb", "side"]);
    assert!(out.status.success());
    std::fs::write(repo.join("c.rs"), "line1\nside-change\nline3\n").unwrap();
    commit_all(&repo, "side");
    git(&repo, &["checkout", "-q", &branch]);
    std::fs::write(repo.join("c.rs"), "line1\nmain-change\nline3\n").unwrap();
    commit_all(&repo, "main");

    let fake = FakeHerdr::new();
    let clock = Arc::new(FakeClock::new(1_763_520_000_000));
    let (daemon, session_ids, panes) =
        spawn_working_sessions(&root, &fake, clock, &[repo.as_path()]);
    daemon.poll_once(); // baseline = ∅ (clean tree before the merge)

    // Create the conflict.
    let merge = git(&repo, &["merge", "side"]);
    assert!(
        !merge.status.success(),
        "merge must conflict: {:?}",
        String::from_utf8_lossy(&merge.stdout)
    );
    let status = git(&repo, &["status", "--porcelain=v1"]);
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(
        status.contains("UU c.rs"),
        "expected unmerged path UU c.rs, got: {status}"
    );

    // sample_git must not panic and must surface the unmerged path as a
    // plain relative path (the engine has no special "unmerged" handling).
    let files = attribution::sample_git(&repo);
    assert!(
        files.contains(&"c.rs".to_string()),
        "unmerged path expected: {files:?}"
    );

    // The daemon samples its private worktree, which was branched from the
    // pre-merge main commit. The `side` ref is shared across worktrees, so
    // replay the conflict there for the poll path.
    let wt_merge = git_in_session(&daemon, &session_ids[0], &["merge", "side"]);
    assert!(
        !wt_merge.status.success(),
        "worktree merge must conflict: {:?}",
        String::from_utf8_lossy(&wt_merge.stdout)
    );
    let wt_status = git_in_session(&daemon, &session_ids[0], &["status", "--porcelain=v1"]);
    let wt_status = String::from_utf8_lossy(&wt_status.stdout);
    assert!(
        wt_status.contains("UU c.rs"),
        "worktree must show unmerged c.rs: {wt_status}"
    );

    // Poll through the daemon: attributed like any other dirty path, no choke.
    set_working_revision(&fake, &panes, 2);
    daemon.poll_once();
    let attributed = attributed_files(&daemon, &session_ids[0]);
    assert!(
        attributed.contains(&"c.rs".to_string()),
        "c.rs attributed like any dirty path: {attributed:?}"
    );
    assert!(
        daemon.snapshot().warnings.is_empty(),
        "no warnings expected: {:?}",
        daemon.snapshot().warnings
    );
    let snap = daemon.snapshot();
    assert!(
        snap.conflicts.is_empty(),
        "single session: no cross-session conflict: {:?}",
        snap.conflicts
    );

    // And record via the parser: UU line parses as the plain path.
    assert_eq!(
        attribution::parse_porcelain("UU c.rs\n"),
        vec!["c.rs".to_string()]
    );
    cleanup(&root);
}
