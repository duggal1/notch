//! A4's adversarial review of A5 (HARDENING §3.6 / T9). This test was NOT
//! written by A5. It attacks the isolation claims with real git repos:
//!   - two sessions editing different files must not cross-attribute;
//!   - conflict detection must agree with an actual `git merge`;
//!   - a dirty worktree must be refused removal with its data intact;
//!   - cwd -> worktree attribution must be exact.

use notchd::worktree;
use std::path::{Path, PathBuf};
use std::process::Command;

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path =
        std::env::temp_dir().join(format!("notch-a4-review-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn git(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git must be installed for this review test");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

fn git_ok(args: &[&str], cwd: &Path) -> String {
    let (ok, out) = git(args, cwd);
    assert!(
        ok,
        "git {} failed in {}: {out}",
        args.join(" "),
        cwd.display()
    );
    out
}

fn init_repo(root: &Path) -> (PathBuf, String) {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&["init", "-q"], &repo);
    std::fs::write(repo.join("base.txt"), "base\n").unwrap();
    git_ok(&["add", "base.txt"], &repo);
    git_ok(
        &[
            "-c",
            "user.name=review",
            "-c",
            "user.email=r@x",
            "commit",
            "-qm",
            "init",
        ],
        &repo,
    );
    let base = git_ok(&["rev-parse", "HEAD"], &repo);
    (repo, base)
}

#[test]
fn a5_isolation_survives_adversarial_cross_edits_and_conflicts() {
    let root = tmp_root("isolation");
    let (repo, base) = init_repo(&root);
    let tmp = root.join("tmp");

    // Distinct files in distinct worktrees: no cross-attribution.
    let a = worktree::create(&repo, "S1-OC01", &tmp).unwrap().unwrap();
    let b = worktree::create(&repo, "S1-OC02", &tmp).unwrap().unwrap();
    assert_ne!(a.path, b.path, "each session needs its own worktree path");
    assert_eq!(a.branch, "notch/S1-OC01");
    std::fs::write(a.path.join("only-a.txt"), "a\n").unwrap();
    let sha_a = worktree::snapshot(&a.path, "S1-OC01", "kill")
        .unwrap()
        .expect("WIP committed");
    assert!(!sha_a.is_empty());
    std::fs::write(b.path.join("only-b.txt"), "b\n").unwrap();
    let _ = worktree::snapshot(&b.path, "S1-OC02", "kill")
        .unwrap()
        .expect("WIP committed");

    let files_a = worktree::sample_files(&a);
    let files_b = worktree::sample_files(&b);
    assert!(
        files_a.iter().any(|f| f.ends_with("only-a.txt")),
        "A's file must be attributed to A: {files_a:?}"
    );
    assert!(
        !files_a.iter().any(|f| f.ends_with("only-b.txt")),
        "B's file leaked into A: {files_a:?}"
    );
    assert!(
        files_b.iter().any(|f| f.ends_with("only-b.txt")),
        "B's file must be attributed to B: {files_b:?}"
    );
    assert!(
        !files_b.iter().any(|f| f.ends_with("only-a.txt")),
        "A's file leaked into B: {files_b:?}"
    );
    assert!(
        !worktree::conflicts(&repo, &base, &a.branch, &b.branch).unwrap(),
        "different files must not conflict"
    );

    // Same file, divergent edits: conflict detection must agree with a real
    // merge rather than guessing from path names.
    let c = worktree::create(&repo, "S1-OC03", &tmp).unwrap().unwrap();
    let d = worktree::create(&repo, "S1-OC04", &tmp).unwrap().unwrap();
    std::fs::write(c.path.join("shared.txt"), "left\n").unwrap();
    let _ = worktree::snapshot(&c.path, "S1-OC03", "kill")
        .unwrap()
        .unwrap();
    std::fs::write(d.path.join("shared.txt"), "right\n").unwrap();
    let _ = worktree::snapshot(&d.path, "S1-OC04", "kill")
        .unwrap()
        .unwrap();
    let predicted = worktree::conflicts(&repo, &base, &c.branch, &d.branch).unwrap();
    assert!(
        predicted,
        "same-file divergence must be reported as a conflict"
    );

    let verify = root.join("verify");
    std::fs::create_dir_all(&verify).unwrap();
    git_ok(&["init", "-q"], &verify);
    git_ok(
        &["remote", "add", "origin", repo.to_str().unwrap()],
        &verify,
    );
    git_ok(&["fetch", "-q", "origin"], &verify);
    git_ok(
        &[
            "checkout",
            "-q",
            "-b",
            "verify",
            &format!("refs/remotes/origin/{}", c.branch),
        ],
        &verify,
    );
    let merge = Command::new("git")
        .args([
            "merge",
            "--no-commit",
            &format!("refs/remotes/origin/{}", d.branch),
        ])
        .current_dir(&verify)
        .output()
        .unwrap();
    assert!(!merge.status.success(), "real git merge must also conflict");
    assert_eq!(
        predicted,
        !merge.status.success(),
        "conflicts() disagreed with the real merge"
    );

    // Dirty removal must be refused, with the uncommitted bytes left intact.
    std::fs::write(a.path.join("dirty.txt"), "do not delete\n").unwrap();
    assert!(
        worktree::remove(&a).is_err(),
        "a dirty worktree must never be removed"
    );
    assert_eq!(
        std::fs::read_to_string(a.path.join("dirty.txt")).unwrap(),
        "do not delete\n"
    );

    // cwd attribution is exact: the worktree maps to exactly its own branch.
    assert_eq!(
        worktree::meta_for_cwd(&b.path).map(|w| w.branch),
        Some("notch/S1-OC02".to_string())
    );
}

#[test]
fn attribution_preserves_exact_paths_in_dirty_and_committed_work() {
    let root = tmp_root("artifact-paths");
    let (repo, _) = init_repo(&root);
    git_ok(&["config", "status.renames", "true"], &repo);
    let work = worktree::create(&repo, "S1-OC01", &root.join("tmp"))
        .unwrap()
        .unwrap();

    let renamed = " renamed -> final.txt ";
    let nested = "artifacts/雪\nnotes.txt";
    git_ok(&["mv", "base.txt", renamed], &work.path);
    std::fs::create_dir(work.path.join("artifacts")).unwrap();
    std::fs::write(work.path.join(nested), "output\n").unwrap();
    let dirty = worktree::sample_files(&work);

    worktree::snapshot(&work.path, "S1-OC01", "test attribution")
        .unwrap()
        .expect("dirty artifacts committed");
    let committed = worktree::sample_files(&work);

    let expected = vec![renamed.to_string(), nested.to_string()];
    assert_eq!((dirty, committed), (expected.clone(), expected));
}
