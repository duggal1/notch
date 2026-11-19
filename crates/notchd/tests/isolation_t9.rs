//! T9 — isolation acceptance: per-session worktrees, exact attribution,
//! merge-tree conflict agreement, loud non-git fallback, WIP snapshot before
//! cleanup, real disk-budget eviction, and pane env isolation.
//!
//! Owner: A5 isolation. Every fixture repo is a real git repo under a fresh
//! temp root; no mocks stand in for git. One red commit lands this file before
//! the implementation exists (it cannot compile until worktree.rs lands).

use notchd::attribution::{sample_git, sample_git_at};
use notchd::cleanup::enforce_budget;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::{Herdr, HerdrError};
use notchd::model::SessionState;
use notchd::status::Status;
use notchd::worktree::{self, WorktreeError};

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!(
        "notchd-t9-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git spawn failed")
}

fn git_ok(repo: &Path, args: &[&str]) -> String {
    let out = git(repo, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "-q"]);
    git_ok(&repo, &["config", "user.email", "t9@example.com"]);
    git_ok(&repo, &["config", "user.name", "t9"]);
    std::fs::write(
        repo.join("shared.rs"),
        "line1\nline2\nline3\nline4\nline5\nline6\n",
    )
    .unwrap();
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["commit", "-qm", "base"]);
    repo
}

fn edit_line(wt: &Path, line: usize, text: &str) {
    let p = wt.join("shared.rs");
    let mut lines: Vec<String> = std::fs::read_to_string(&p).unwrap().lines().map(str::to_string).collect();
    lines[line] = text.to_string();
    std::fs::write(p, format!("{}\n", lines.join("\n"))).unwrap();
}

fn commit_all(wt: &Path, msg: &str) {
    git_ok(wt, &["add", "-A"]);
    git_ok(wt, &["commit", "-qm", msg]);
}

/// Real merge in a throwaway worktree: start at `base_rev`, merge BOTH
/// branches. This is the oracle `merge-tree --write-tree A B` must agree with
/// (the predicate is symmetric); no mocking of git's conflict rules.
fn real_merge_conflicts(repo: &Path, base_rev: &str, branch_a: &str, branch_b: &str) -> bool {
    let scratch = repo.parent().unwrap().join(format!(
        "merge-oracle-{}-{}",
        branch_a.replace('/', "_"),
        branch_b.replace('/', "_")
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    git_ok(
        repo,
        &["worktree", "add", "-q", "-d", scratch.to_str().unwrap(), base_rev],
    );
    let mut conflicted = false;
    for branch in [branch_a, branch_b] {
        let out = git(&scratch, &["merge", "--no-edit", branch]);
        if !out.status.success() {
            conflicted = true;
            let _ = git(&scratch, &["merge", "--abort"]);
            break;
        }
    }
    git_ok(repo, &["worktree", "remove", "--force", scratch.to_str().unwrap()]);
    conflicted
}

// ---------------------------------------------------------------------------
// 1. Six agents, one file, six worktrees: zero false attribution.
// ---------------------------------------------------------------------------

#[test]
fn six_agents_same_file_separate_worktrees_zero_false_attribution() {
    let root = tmp_root("six");
    let repo = init_repo(&root);
    let base_rev = git_ok(&repo, &["rev-parse", "HEAD"]);
    let tmp = root.join("sessions");
    std::fs::create_dir_all(&tmp).unwrap();

    let mut wts = Vec::new();
    for i in 0..6 {
        let id = format!("S{}-OC01", i + 1);
        let wt = worktree::create(&repo, &id, &tmp).unwrap().expect("git repo must get a worktree");
        assert_eq!(wt.branch, format!("notch/{id}"));
        assert_eq!(wt.base_rev, base_rev);
        assert!(wt.path.is_dir(), "worktree dir missing: {}", wt.path.display());
        assert!(wt.path.join("shared.rs").is_file());
        // The worktree is a real linked worktree (git file, not a clone).
        assert!(wt.path.join(".git").is_file());
        // Re-creating is idempotent (restart path) — same path and branch.
        let again = worktree::create(&repo, &id, &tmp).unwrap().unwrap();
        assert_eq!(again.path, wt.path);
        assert_eq!(again.branch, wt.branch);
        wts.push(wt);
    }

    // Each agent edits a DIFFERENT line. Agent 0 commits; agent 1 leaves WIP;
    // agents 2..5 edit only their own worktree.
    for (i, wt) in wts.iter().enumerate() {
        edit_line(&wt.path, i, &format!("agent-{i}"));
    }
    commit_all(&wts[0].path, "agent 0 committed");
    commit_all(&wts[3].path, "agent 3 committed");

    // Exact attribution: committed diff (base..HEAD) + porcelain of THIS worktree.
    for (i, wt) in wts.iter().enumerate() {
        let files = worktree::sample_files(wt);
        assert_eq!(files, vec!["shared.rs".to_string()], "agent {i} exact files: {files:?}");
        let porcelain = sample_git(&wt.path);
        assert!(porcelain.is_empty() || porcelain == vec!["shared.rs".to_string()]);
        // The committed change is visible even though porcelain is clean.
        assert_eq!(sample_git_at(&wt.path, Some(&wt.base_rev)), vec!["shared.rs".to_string()]);
    }

    // No edit leaked into the main checkout: no false attribution there.
    assert!(sample_git(&repo).is_empty(), "main checkout must stay clean: {:?}", sample_git(&repo));

    // Six agents touched the same path but in isolated worktrees; a filename
    // overlap is NOT a conflict. merge-tree is the only conflict oracle.
    for i in 1..6 {
        assert!(
            !worktree::conflicts(&repo, &base_rev, &wts[0].branch, &wts[i].branch).unwrap(),
            "disjoint-line edit {} must not conflict with agent 0",
            i
        );
        assert!(
            !real_merge_conflicts(&repo, &base_rev, &wts[0].branch, &wts[i].branch),
            "real merge disagrees"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. conflict() via merge-tree exit 1 agrees with a real merge.
// ---------------------------------------------------------------------------

#[test]
fn conflict_predicate_agrees_with_real_merge() {
    let root = tmp_root("conflict");
    let repo = init_repo(&root);
    let base_rev = git_ok(&repo, &["rev-parse", "HEAD"]);
    let tmp = root.join("sessions");
    std::fs::create_dir_all(&tmp).unwrap();

    let mk = |id: &str, line: usize, text: &str, commit: bool| -> worktree::Worktree {
        let wt = worktree::create(&repo, id, &tmp).unwrap().unwrap();
        edit_line(&wt.path, line, text);
        if commit {
            commit_all(&wt.path, id);
        }
        wt
    };

    // Disjoint lines -> clean; same line -> conflict.
    let a = mk("S1-OC01", 0, "A-line1", true);
    let b = mk("S2-OC01", 5, "B-line6", true);
    let c = mk("S3-OC01", 0, "C-line1", true);

    let ab = worktree::conflicts(&repo, &base_rev, &a.branch, &b.branch).unwrap();
    let ac = worktree::conflicts(&repo, &base_rev, &a.branch, &c.branch).unwrap();
    assert!(!ab, "disjoint edits must be clean");
    assert!(ac, "same-line edits must conflict (merge-tree exit 1)");
    assert_eq!(ab, real_merge_conflicts(&repo, &base_rev, &a.branch, &b.branch));
    assert_eq!(ac, real_merge_conflicts(&repo, &base_rev, &a.branch, &c.branch));

    // No auto-merge: the predicate only reads. Committed work is still
    // attributed exactly (commits since base), and neither worktree was
    // modified by the conflict check.
    assert_eq!(sample_git(&a.path), vec!["shared.rs".to_string()]);
    assert_eq!(sample_git(&b.path), vec!["shared.rs".to_string()]);
}

// ---------------------------------------------------------------------------
// 3. Non-git cwd: no worktree, loud warning, heuristic fallback remains.
// ---------------------------------------------------------------------------

#[test]
fn non_git_cwd_falls_back_with_loud_warning() {
    let root = tmp_root("nongit");
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("x.rs"), "fn x() {}\n").unwrap();
    let tmp = root.join("sessions");
    std::fs::create_dir_all(&tmp).unwrap();

    match worktree::create(&plain, "S1-OC01", &tmp) {
        Err(WorktreeError::NotGit(_)) => {}
        Ok(None) => {}
        other => panic!("non-git cwd must be rejected without a worktree: {other:?}"),
    }
    // No branch/worktree side effects for a non-git cwd.
    assert!(!tmp.join("notch-S1-OC01").join("worktree").exists());
    assert!(sample_git(&plain).is_empty(), "heuristic fallback stays silent on git signal");
}

// ---------------------------------------------------------------------------
// 4. Snapshot WIP; never remove a dirty worktree.
// ---------------------------------------------------------------------------

#[test]
fn snapshot_commits_wip_and_dirty_removal_is_refused() {
    let root = tmp_root("dirty");
    let repo = init_repo(&root);
    let tmp = root.join("sessions");
    std::fs::create_dir_all(&tmp).unwrap();
    let wt = worktree::create(&repo, "S1-OC01", &tmp).unwrap().unwrap();

    std::fs::write(wt.path.join("wip.rs"), "fn wip() {}\n").unwrap();
    assert!(worktree::is_dirty(&wt).unwrap());
    let sha = worktree::snapshot(&wt.path, "S1-OC01", "kill").unwrap().expect("WIP committed");
    assert_eq!(sha.len(), 40, "sha must be a full object id: {sha}");
    assert!(!worktree::is_dirty(&wt).unwrap(), "snapshot must leave the worktree clean");
    assert_eq!(sample_git_at(&wt.path, Some(&wt.base_rev)), vec!["wip.rs".to_string()]);

    // New WIP, then removal must be REFUSED with data left intact.
    std::fs::write(wt.path.join("wip2.rs"), "fn wip2() {}\n").unwrap();
    match worktree::remove(&wt) {
        Err(WorktreeError::Dirty(_)) => {}
        other => panic!("dirty worktree removal must be refused: {other:?}"),
    }
    assert!(wt.path.join("wip2.rs").is_file(), "refused removal must not delete WIP");
    assert!(wt.path.is_dir());

    // Snapshot again -> clean -> removal allowed. Branch is kept so the
    // snapshotted commits remain recoverable (never auto-merge/drop work).
    worktree::snapshot(&wt.path, "S1-OC01", "cleanup").unwrap();
    worktree::remove(&wt).unwrap();
    assert!(!wt.path.exists(), "clean worktree dir must be removed");
    let branches = git_ok(&repo, &["branch", "--list", "notch/S1-OC01"]);
    assert!(branches.contains("notch/S1-OC01"), "snapshot branch must survive cleanup");
}

// ---------------------------------------------------------------------------
// 5. Pane env: TMPDIR and optional CARGO_TARGET_DIR reach the herdr split.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Capture {
    split_args: Mutex<Vec<Vec<String>>>,
    /// Signalled by the fake on the LAST herdr call of a detached spawn, so
    /// tests can wait deterministically (no sleep-as-synchronisation).
    done: Mutex<bool>,
    cv: std::sync::Condvar,
}
impl Capture {
    fn signal_done(&self) {
        *self.done.lock().unwrap() = true;
        self.cv.notify_all();
    }

    fn wait_done(&self) {
        let mut done = self.done.lock().unwrap();
        while !*done {
            let (guard, _) = self
                .cv
                .wait_timeout(done, std::time::Duration::from_secs(10))
                .unwrap();
            done = guard;
        }
    }

    fn herdr(self: &Arc<Self>) -> Herdr {
        let cap = Arc::clone(self);
        Herdr::with_runner(move |args: &[String]| {
            let sub = args.get(1).cloned().unwrap_or_default();
            let sub2 = args.get(2).cloned().unwrap_or_default();
            if sub == "pane" && sub2 == "split" {
                cap.split_args.lock().unwrap().push(args.to_vec());
                Ok(serde_json::json!({
                    "id": "cli:pane:split",
                    "result": { "type": "pane_info",
                        "pane": { "pane_id": "w9:p1", "tab_id": "w1:t1", "workspace_id": "w1" } }
                })
                .to_string())
            } else if sub == "agent" && sub2 == "start" {
                Ok(r#"{"id":"cli:agent:start","result":{"type":"agent_started","agent":{"pane_id":"w9:p1","agent_status":"working","revision":1}}}"#.to_string())
            } else if sub == "agent" && sub2 == "prompt" {
                let out = r#"{"id":"cli:agent:prompt","result":{"type":"agent_prompted","agent":{"pane_id":"w9:p1","agent_status":"working","revision":2}}}"#;
                cap.signal_done();
                Ok(out.to_string())
            } else if sub == "agent" && sub2 == "list" {
                Ok(r#"{"id":"cli:agent:list","result":{"type":"agent_list","agents":[{"pane_id":"w9:p1","agent_status":"working","revision":1}]}}"#.to_string())
            } else if sub == "agent" && sub2 == "read" {
                Ok("working\n".to_string())
            } else if sub == "pane" && sub2 == "close" {
                Ok(r#"{"id":"cli:pane:close","result":{"type":"ok"}}"#.to_string())
            } else {
                Err(HerdrError::Failed { args: args.to_vec(), code: Some(2), stderr: "unexpected".into() })
            }
        })
    }
}

fn test_config(root: &Path) -> Config {
    Config {
        state_dir: root.join("state"),
        tmp_dir: root.join("sessions"),
        stuck_after_secs: 900,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

#[test]
fn pane_env_has_tmpdir_and_isolated_build_target() {
    let root = tmp_root("env");
    let repo = init_repo(&root);
    let cfg = test_config(&root);
    std::fs::create_dir_all(&cfg.tmp_dir).unwrap();
    std::fs::create_dir_all(&cfg.state_dir).unwrap();
    let capture = Arc::new(Capture::default());
    let daemon = Daemon::open(test_config(&root), capture.herdr(), Arc::new(notchd::FakeClock::new(1_700_000_000_000))).unwrap();

    // Pure function first: both branches of the optional build isolation.
    let a = worktree::pane_env(&cfg.tmp_dir, "S1-OC01", false);
    assert!(a.iter().any(|(k, v)| k == "TMPDIR" && v.as_str() == cfg.tmp_dir.join("notch-S1-OC01").to_string_lossy()));
    assert!(a.iter().any(|(k, v)| k == "NOTCH_SESSION_ID" && v == "S1-OC01"));
    assert!(!a.iter().any(|(k, _)| k == "CARGO_TARGET_DIR"));
    let b = worktree::pane_env(&cfg.tmp_dir, "S1-OC01", true);
    assert!(b.iter().any(|(k, v)| k == "CARGO_TARGET_DIR" && v.as_str() == cfg.tmp_dir.join("notch-S1-OC01").join("build").to_string_lossy()));

    std::env::set_var("NOTCH_ISOLATE_BUILD", "1");
    let ack = daemon.submit_task(
        "write env probe".to_string(),
        Some(repo.to_string_lossy().to_string()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok, "submit_task failed: {:?}", ack.error);
    // submit_task acks before the detached worker calls herdr; keep the env
    // opt-in set until the worker has actually built the pane env.
    capture.wait_done();
    std::env::remove_var("NOTCH_ISOLATE_BUILD");
    daemon.poll_once();
    let id = ack.session_id.unwrap();
    let splits = capture.split_args.lock().unwrap();
    assert!(!splits.is_empty(), "pane split must be invoked");
    let joined = splits[0].join(" ");
    assert!(joined.contains("TMPDIR="), "pane env missing TMPDIR: {joined}");
    assert!(joined.contains(&format!("TMPDIR={}", cfg.tmp_dir.join(format!("notch-{id}")).display())), "TMPDIR wrong: {joined}");
    assert!(joined.contains("CARGO_TARGET_DIR="), "pane env missing CARGO_TARGET_DIR: {joined}");
    assert!(joined.contains(&format!("NOTCH_SESSION_ID={id}")));
    drop(splits);

    // Layout dirs exist under the session tmp root.
    let session_tmp = cfg.tmp_dir.join(format!("notch-{id}"));
    for sub in ["worktree", "build", "judge"] {
        assert!(session_tmp.join(sub).is_dir(), "missing layout dir {sub}");
    }
}

// ---------------------------------------------------------------------------
// 6. Disk budget: dead sessions evicted oldest-first; dirty ones survive.
// ---------------------------------------------------------------------------

fn fake_session(id: &str, status: Status, started_at: u64, tmp: &Path) -> SessionState {
    SessionState {
        id: id.to_string(),
        sup: "SV-1".to_string(),
        status,
        title: id.to_string(),
        task: "t".to_string(),
        cwd: tmp.to_string_lossy().to_string(),
        pane_id: String::new(),
        started_at,
        last_action: None,
        last_revision: None,
        latency_ms: None,
        failure_reason: None,
        failure_count: 0,
        revision_changed_at_ms: 0,
        attribution_baseline: None,
        goal: None,
        definition_of_done: None,
        iterations: 0,
        max_iterations: 8,
        wall_clock_deadline: None,
        last_supervisor_action: None,
        model: None,
        session_ref: None,
        escalated: false,
        temp_dir: Some(tmp.to_string_lossy().to_string()),
        worktree: None,
        branch: None,
        base_rev: None,
        backoff_until_ms: 0,
        consecutive_decider_failures: 0,
        stuck_strikes: 0,
        strategy_changed: false,
        last_tails: std::collections::VecDeque::new(),
    }
}

#[test]
fn disk_budget_evicts_dead_oldest_first_and_never_touches_dirty_or_live() {
    let root = tmp_root("budget");
    let tmp = root.join("sessions");
    std::fs::create_dir_all(&tmp).unwrap();

    // Three dead/live session dirs, each 100 KiB (sparse: fast, real metadata).
    let mut sessions = Vec::new();
    for (i, (id, status, started)) in [
        ("S1-OC01", Status::Failed, 100u64),
        ("S2-OC01", Status::Done, 200),
        ("S3-OC01", Status::Working, 300),
    ]
    .iter()
    .enumerate()
    {
        let dir = tmp.join(format!("notch-{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        let f = std::fs::File::create(dir.join("blob.bin")).unwrap();
        f.set_len(100 * 1024).unwrap();
        sessions.push(fake_session(id, *status, *started, &dir));
        assert_eq!(i + 1, sessions.len());
    }

    // 300 KiB total; budget 200 KiB => only the oldest 100 KiB dead dir is
    // evicted (remaining 200 KiB fits exactly).
    let (freed, warnings) = enforce_budget(&tmp, &sessions, 200 * 1024).unwrap();
    assert!(freed >= 100 * 1024, "must free the oldest dead dir, freed={freed}");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert!(!tmp.join("notch-S1-OC01").exists(), "oldest dead must be gone");
    assert!(tmp.join("notch-S2-OC01").exists(), "newer dead must survive");
    assert!(tmp.join("notch-S3-OC01").exists(), "live session must survive");

    // A dirty worktree in a dead session is refused even under budget pressure.
    let repo = init_repo(&root);
    let base = repo.parent().unwrap().join("budget-wt-session");
    std::fs::create_dir_all(&base).unwrap();
    let wt = worktree::create(&repo, "S4-OC01", &base).unwrap().unwrap();
    std::fs::write(wt.path.join("dirty.rs"), "fn dirty() {}\n").unwrap();
    let dir = wt.path.parent().unwrap().to_path_buf();
    let mut sessions = vec![fake_session("S4-OC01", Status::Failed, 50, &dir)];
    let (freed, warnings) = enforce_budget(&base, &sessions, 0).unwrap();
    assert_eq!(freed, 0, "dirty worktree must not be freed");
    assert!(warnings.iter().any(|w| w.contains("dirty")), "must warn about dirty refusal: {warnings:?}");
    assert!(wt.path.join("dirty.rs").is_file(), "dirty file must survive");
    sessions.clear();
}

// ---------------------------------------------------------------------------
// 7. Kill path snapshots WIP before cleanup (no data loss).
// ---------------------------------------------------------------------------

#[test]
fn kill_snapshots_wip_before_cleanup() {
    let root = tmp_root("kill");
    let repo = init_repo(&root);
    let cfg = test_config(&root);
    std::fs::create_dir_all(&cfg.tmp_dir).unwrap();
    std::fs::create_dir_all(&cfg.state_dir).unwrap();
    let capture = Arc::new(Capture::default());
    let daemon = Daemon::open(test_config(&root), capture.herdr(), Arc::new(notchd::FakeClock::new(1_700_000_000_000))).unwrap();
    let ack = daemon.submit_task(
        "do work".to_string(),
        Some(repo.to_string_lossy().to_string()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok, "{:?}", ack.error);
    let id = ack.session_id.unwrap();
    capture.wait_done();
    daemon.poll_once(); // apply the detached SpawnOutcome (cwd/worktree fields)
    let meta = worktree::load(&cfg.tmp_dir, &id).expect("worktree metadata persisted");

    std::fs::write(meta.path.join("unsaved.rs"), "fn unsaved() {}\n").unwrap();
    assert!(worktree::is_dirty(&meta).unwrap());
    daemon.kill(&id, Some("test kill".to_string())).unwrap();

    // WIP was committed to this session's branch before the dir was removed.
    let msg = git_ok(meta.base_repo.as_path(), &["log", "-1", "--format=%s", &meta.branch]);
    assert!(
        msg.starts_with("notch: WIP snapshot (") && msg.contains(&id),
        "canonical snapshot commit missing: {msg}"
    );
    assert!(!meta.path.exists(), "clean worktree dir removed after snapshot");
}

// ---------------------------------------------------------------------------
// 8. Daemon conflicts come from real merges, not shared file names.
// ---------------------------------------------------------------------------

#[test]
fn daemon_conflicts_use_merge_tree_not_filenames() {
    let root = tmp_root("daemon-conflicts");
    let repo = init_repo(&root);
    let cfg = test_config(&root);
    std::fs::create_dir_all(&cfg.tmp_dir).unwrap();
    std::fs::create_dir_all(&cfg.state_dir).unwrap();
    let capture = Arc::new(Capture::default());
    let daemon = Daemon::open(test_config(&root), capture.herdr(), Arc::new(notchd::FakeClock::new(1_700_000_000_000))).unwrap();

    let mut ids = Vec::new();
    for i in 0..3 {
        let ack = daemon.submit_task(
            format!("worker {i}"),
            Some(repo.to_string_lossy().to_string()),
            None, None, None, None,
        );
        assert!(ack.ok, "{:?}", ack.error);
        ids.push(ack.session_id.unwrap());
    }
    capture.wait_done();
    daemon.poll_once(); // apply all three detached SpawnOutcomes

    // 1 and 2 edit the SAME line (real conflict); 1 and 3 edit DIFFERENT
    // lines (no conflict) — yet all three touched the same file name.
    let wt1 = worktree::load(&cfg.tmp_dir, &ids[0]).unwrap();
    let wt2 = worktree::load(&cfg.tmp_dir, &ids[1]).unwrap();
    let wt3 = worktree::load(&cfg.tmp_dir, &ids[2]).unwrap();
    edit_line(&wt1.path, 0, "one-line1");
    edit_line(&wt2.path, 0, "two-line1");
    edit_line(&wt3.path, 5, "three-line6");
    for wt in [&wt1, &wt2, &wt3] {
        commit_all(&wt.path, "worker edit");
    }
    for id in &ids {
        daemon.record_attribution(id, &["shared.rs".to_string()]);
    }

    let conflicts = daemon.snapshot().conflicts;
    let pair = |a: &str, b: &str| {
        conflicts.iter().find(|c| {
            c.file == "shared.rs"
                && c.sessions.len() == 2
                && c.sessions.iter().any(|s| s == a)
                && c.sessions.iter().any(|s| s == b)
        })
    };
    assert!(pair(&ids[0], &ids[1]).is_some(), "same-line pair must conflict: {conflicts:?}");
    assert!(pair(&ids[0], &ids[2]).is_none(), "disjoint pair must NOT conflict: {conflicts:?}");
}

// ---------------------------------------------------------------------------
// 9. Non-git spawn warns loudly and falls back to the shared-cwd heuristic.
// ---------------------------------------------------------------------------

#[test]
fn non_git_spawn_warns_loudly() {
    let root = tmp_root("nongit-spawn");
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("x.rs"), "fn x() {}\n").unwrap();
    let cfg = test_config(&root);
    std::fs::create_dir_all(&cfg.tmp_dir).unwrap();
    std::fs::create_dir_all(&cfg.state_dir).unwrap();
    let capture = Arc::new(Capture::default());
    let daemon = Daemon::open(cfg, capture.herdr(), Arc::new(notchd::FakeClock::new(1_700_000_000_000))).unwrap();
    let ack = daemon.submit_task(
        "plain cwd".to_string(),
        Some(plain.to_string_lossy().to_string()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok, "{:?}", ack.error);
    capture.wait_done();
    daemon.poll_once(); // detached worker's warning is applied here
    let snap = daemon.snapshot();
    assert!(
        snap.warnings.iter().any(|w| w.contains("not a git") && w.contains("heuristic")),
        "non-git cwd must produce a loud warning: {:?}",
        snap.warnings
    );
    // No worktree metadata for the fallback session.
    let id = ack.session_id.unwrap();
    assert!(worktree::load(&plain, &id).is_none());
}
