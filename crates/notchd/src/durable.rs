//! Durable JSON file writes — the persistence floor for `state.json` and
//! `registry.json` (bridge.md §5).
//!
//! Every write is:
//! 1. A rotation of the rolling backup chain: `path` → `path.bak1`,
//!    `path.bak1` → `path.bak2`, `path.bak2` → `path.bak3`. The previous
//!    `path.bak3` is dropped. The live file is *copied* (not moved) into
//!    `bak1`, so there is never an instant where `path` does not exist.
//!    Backups are best-effort: a failure in the backup chain never aborts
//!    the real write.
//! 2. Write of the new bytes to `path.tmp` in the same directory, followed
//!    by `fsync` of that file.
//! 3. An atomic `rename(path.tmp, path)`.
//! 4. `fsync` of the parent directory, so the rename itself is durable.
//!    This is also best-effort (a directory fsync can fail on exotic
//!    mounts); the write still reports success.
//!
//! Nothing here serializes — callers hand in bytes. No `unsafe`; no new
//! dependencies.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Depth of the rolling backup chain (`path.bak1` … `path.bakN`).
const BACKUPS: usize = 3;

/// The temp file written and synced before the atomic rename. Same
/// directory as `path` so the rename never crosses a filesystem.
pub(crate) fn tmp_path(path: &Path) -> PathBuf {
    path.with_extension("tmp")
}

/// `path` with `.bak<n>` appended to the full file name
/// (`state.json` → `state.json.bak1`).
fn backup_path(path: &Path, n: usize) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(format!(".bak{n}"));
    PathBuf::from(os)
}

/// Shift the rolling backup chain. Best-effort: every step that fails is
/// skipped so a broken backup can never take the real write down with it.
fn rotate_backups(path: &Path) {
    let deepest = backup_path(path, BACKUPS);
    if deepest.is_file() {
        let _ = fs::remove_file(&deepest);
    }
    for n in (1..BACKUPS).rev() {
        let from = backup_path(path, n);
        if from.is_file() {
            let _ = fs::rename(&from, backup_path(path, n + 1));
        }
    }
    if path.is_file() {
        // Copy, not move: `path` must exist at every instant.
        let _ = fs::copy(path, backup_path(path, 1));
    }
}

/// Write `bytes` to `path` atomically and durably, rotating the backup
/// chain first. Creates missing parent directories.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    rotate_backups(path);
    let tmp = tmp_path(path);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = fs::File::open(parent) {
            // Directory fsync is best-effort; some filesystems reject it.
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! New-file coverage for the durable write path (the `tests/` directory
    //! is frozen for this agent). Semantics verified: atomic replace,
    //! backup rotation order, depth cap, no backups on first write, stale
    //! temp removal, missing parents, binary bytes.
    use super::*;

    fn test_dir(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notch-durable-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_write_creates_no_backups() {
        let dir = test_dir("first");
        let path = dir.join("state.json");
        write_atomic(&path, b"one").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        for n in 1..=BACKUPS {
            assert!(
                !backup_path(&path, n).exists(),
                "bak{n} must not exist after first write"
            );
        }
    }

    #[test]
    fn backups_rotate_in_order_and_never_exceed_depth() {
        let dir = test_dir("rotate");
        let path = dir.join("state.json");
        for v in [b"v1".as_slice(), b"v2", b"v3", b"v4", b"v5"] {
            write_atomic(&path, v).unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"v5");
        assert_eq!(fs::read(backup_path(&path, 1)).unwrap(), b"v4");
        assert_eq!(fs::read(backup_path(&path, 2)).unwrap(), b"v3");
        assert_eq!(fs::read(backup_path(&path, 3)).unwrap(), b"v2");
        // v1 has rolled off the end of the chain.
        assert!(!backup_path(&path, 4).exists());
    }

    #[test]
    fn stale_tmp_file_from_a_crash_is_overwritten() {
        let dir = test_dir("stale");
        let path = dir.join("registry.json");
        fs::write(tmp_path(&path), b"{{{{garbage from a crashed write").unwrap();
        write_atomic(&path, b"fresh").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"fresh");
        assert!(
            !tmp_path(&path).exists(),
            "the temp file must be renamed away"
        );
    }

    #[test]
    fn missing_parent_directories_are_created() {
        let dir = test_dir("parents");
        let path = dir.join("a").join("b").join("c").join("state.json");
        write_atomic(&path, b"nested").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"nested");
    }

    #[test]
    fn binary_bytes_round_trip_exactly() {
        let dir = test_dir("binary");
        let path = dir.join("state.json");
        let payload: Vec<u8> = (0u8..=255).collect();
        write_atomic(&path, &payload).unwrap();
        assert_eq!(fs::read(&path).unwrap(), payload);
    }

    #[test]
    fn rotation_keeps_live_file_present_at_every_step() {
        // The live file is copied (not moved) into bak1, so even a failed
        // rename would leave a readable `path` behind at all times.
        let dir = test_dir("live");
        let path = dir.join("state.json");
        write_atomic(&path, b"original").unwrap();
        rotate_backups(&path);
        assert_eq!(
            fs::read(&path).unwrap(),
            b"original",
            "rotation must not remove the live file"
        );
        assert_eq!(fs::read(backup_path(&path, 1)).unwrap(), b"original");
    }
}
