//! Durable JSON file writes — the persistence floor for `state.json` and
//! `registry.json` (bridge.md §5).
//!
//! Every write is:
//! 1. A time-spaced refresh of the rolling backup chain `path.bak1..3`.
//!    A tier is refreshed only when its own age reaches 60 s / 900 s /
//!    3600 s (or the file is missing), propagating content from the tier
//!    below: `live → bak1 → bak2 → bak3`. With 1 Hz state writes this yields
//!    three backups of genuinely different ages instead of three copies of
//!    the last few seconds. The deepest tier is dropped past depth 3.
//!    The live file is *copied* (not moved) into `bak1`, so there is never an
//!    instant where `path` does not exist. Backups are best-effort: a failure
//!    in the backup chain never aborts the real write.
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
use std::time::{SystemTime, UNIX_EPOCH};

/// Depth of the rolling backup chain (`path.bak1` … `path.bakN`).
const BACKUPS: usize = 3;

/// Minimum age of `bak1` / `bak2` / `bak3` before that tier is refreshed.
/// The old code rotated on *every* write, which at 1 Hz produced three
/// near-identical snapshots seconds apart (HARDENING §3.5, defect 9).
pub(crate) const BACKUP_TIERS_MS: [u64; BACKUPS] = [60_000, 900_000, 3_600_000];

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// The temp file written and synced before the atomic rename. Same
/// directory as `path` so the rename never crosses a filesystem.
pub(crate) fn tmp_path(path: &Path) -> PathBuf {
    path.with_extension("tmp")
}

/// Files considered during recovery, newest tier first. The `.tmp` file is
/// deliberately NEVER included: a torn write may leave it half-written and
/// adopting it would resurrect ids that were never committed (T5).
pub(crate) fn read_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![path.to_path_buf()];
    for n in 1..=BACKUPS {
        candidates.push(backup_path(path, n));
    }
    candidates
}

/// `path` with `.bak<n>` appended to the full file name
/// (`state.json` → `state.json.bak1`).
fn backup_path(path: &Path, n: usize) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(format!(".bak{n}"));
    PathBuf::from(os)
}

/// Refresh whichever backup tiers are due, deepest first so content moves
/// exactly one tier per write. Best-effort: every step that fails is skipped
/// so a broken backup can never take the real write down with it.
fn rotate_backups_at(path: &Path, now: u64) {
    for n in (1..=BACKUPS).rev() {
        let dest = backup_path(path, n);
        // Tier 1 pulls from live; deeper tiers pull from the tier above.
        // Copy, not move: both source and destination must stay readable.
        let source = if n == 1 {
            path.to_path_buf()
        } else {
            backup_path(path, n - 1)
        };
        if !source.is_file() {
            continue;
        }
        let tier = BACKUP_TIERS_MS[n - 1];
        let due = match mtime_ms(&dest) {
            // bak1 is established on the second write. A deeper tier is only
            // created once its source is old enough — otherwise a burst of
            // writes would fill all three slots with second-old snapshots.
            None => n == 1 || mtime_ms(&source).is_some_and(|t| now.saturating_sub(t) >= tier),
            Some(written) => now.saturating_sub(written) >= tier,
        };
        if due {
            let _ = fs::copy(&source, &dest);
        }
    }
}

/// Write `bytes` to `path` atomically and durably, refreshing due backup
/// tiers first (wall clock). Creates missing parent directories.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_at(path, bytes, wall_ms())
}

/// Same as [`write_atomic`] with an injected wall-clock value, so the tier
/// logic is testable without sleeping. `now` is milliseconds since epoch.
pub(crate) fn write_atomic_at(path: &Path, bytes: &[u8], now: u64) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    rotate_backups_at(path, now);
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

    fn age(path: &Path, secs: u64) {
        use std::time::{Duration, SystemTime};
        if path.exists() {
            set_mtime(path, SystemTime::now() - Duration::from_secs(secs));
        }
    }

    #[test]
    fn backups_rotate_in_order_and_never_exceed_depth() {
        // Rotation is driven by tier age, not by write count. Age the tiers
        // explicitly so the test exercises the same chain the 1 Hz writer gets
        // after 60 s / 900 s / 3600 s have elapsed.
        let dir = test_dir("rotate");
        let path = dir.join("state.json");
        write_atomic(&path, b"v1").unwrap();
        write_atomic(&path, b"v2").unwrap(); // bak1 = v1
        for (live, a1, a2) in [
            (b"v3".as_slice(), 61, 0),
            (b"v4", 901, 3601),
            (b"v5", 901, 3601),
            (b"v6", 901, 3601),
            (b"v7", 901, 3601),
            (b"v8", 901, 3601),
        ] {
            age(&backup_path(&path, 1), a1);
            age(&backup_path(&path, 2), a2);
            write_atomic(&path, live).unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"v8");
        assert_eq!(fs::read(backup_path(&path, 1)).unwrap(), b"v7");
        assert_eq!(fs::read(backup_path(&path, 2)).unwrap(), b"v6");
        assert_eq!(fs::read(backup_path(&path, 3)).unwrap(), b"v5");
        // Depth is capped at 3: older snapshots rolled off the end.
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
        rotate_backups_at(&path, wall_ms());
        assert_eq!(
            fs::read(&path).unwrap(),
            b"original",
            "rotation must not remove the live file"
        );
        assert_eq!(fs::read(backup_path(&path, 1)).unwrap(), b"original");
    }

    fn set_mtime(path: &Path, at: std::time::SystemTime) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(at).unwrap();
    }

    #[test]
    fn backups_are_time_spaced_not_rotated_on_every_write() {
        use std::time::{Duration, SystemTime};
        let dir = test_dir("spaced");
        let path = dir.join("state.json");

        write_atomic(&path, b"v1").unwrap();
        // A rapid burst (all inside the 60 s tier) must not churn bak2/bak3.
        // Only bak1 exists, holding the first snapshot.
        for v in [b"v2".as_slice(), b"v3", b"v4", b"v5"] {
            write_atomic(&path, v).unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"v5");
        assert_eq!(fs::read(backup_path(&path, 1)).unwrap(), b"v1");
        assert!(!backup_path(&path, 2).exists(), "bak2 waits for 900 s");
        assert!(!backup_path(&path, 3).exists(), "bak3 waits for 3600 s");

        // Age bak1 past the 60 s tier: it refreshes from live, bak2 still waits.
        set_mtime(&backup_path(&path, 1), SystemTime::now() - Duration::from_secs(61));
        write_atomic(&path, b"v6").unwrap();
        assert_eq!(fs::read(backup_path(&path, 1)).unwrap(), b"v5");
        assert!(!backup_path(&path, 2).exists(), "bak2 still waits for 900 s");

        // Age bak1 past the 900 s tier: bak2 is refreshed from bak1.
        set_mtime(&backup_path(&path, 1), SystemTime::now() - Duration::from_secs(901));
        write_atomic(&path, b"v7").unwrap();
        assert!(backup_path(&path, 2).exists(), "bak2 due at 900 s");

        // Age bak2 past the 3600 s tier: bak3 is refreshed from bak2.
        set_mtime(&backup_path(&path, 2), SystemTime::now() - Duration::from_secs(3601));
        write_atomic(&path, b"v8").unwrap();
        assert!(backup_path(&path, 3).exists(), "bak3 due at 3600 s");
        assert_eq!(fs::read(&path).unwrap(), b"v8");
        assert!(!tmp_path(&path).exists(), "no .tmp may survive a completed write");
    }
}
