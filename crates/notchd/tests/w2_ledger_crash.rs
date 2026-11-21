//! W2-2: crash consistency of the append-only ledger itself.
//!
//! A child appends whole lines forever; the parent SIGKILLs it at arbitrary
//! byte offsets. Replay must apply only whole, in-budget lines, never a torn
//! one, and a fresh writer must continue from the last committed seq. This is
//! the ledger analogue of HARDENING T5 (state/registry crash consistency).

use notchd::ledger::{read_lines, replay, Ledger, Op, Source};
use notchd::model::StateFile;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn tmp_root(tag: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notch-w2-crash-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn ledger_writer_child() {
    if std::env::var("NOTCH_W2_CHILD").ok().as_deref() != Some("1") {
        return;
    }
    let dir = PathBuf::from(std::env::var("NOTCH_W2_STATE").expect("state dir"));
    let mut ledger = Ledger::open(&dir).unwrap();
    let mut i = 0u64;
    loop {
        let id = format!("S1-OC{:04}", i % 10);
        let title = format!("value-{i}");
        ledger
            .append(
                Source::System,
                None,
                None,
                &id,
                Op::Upsert {
                    entity: "session".into(),
                    id: id.clone(),
                    value: json!({
                        "id": id, "sup": "SV-1", "status": "working",
                        "title": title, "task": title, "cwd": "/tmp"
                    }),
                },
                1_000 + i,
            )
            .unwrap();
        if i == 0 {
            println!("READY");
            std::io::stdout().flush().unwrap();
        }
        i += 1;
    }
}

#[test]
fn sigkill_during_append_never_applies_a_torn_line() {
    const ROUNDS: usize = 200;
    let dir = tmp_root("sigkill");
    let exe = std::env::current_exe().expect("current_exe");
    let mut total_lines = 0usize;

    for round in 0..ROUNDS {
        let mut child = Command::new(&exe)
            .args(["--exact", "ledger_writer_child", "--nocapture"])
            .env("NOTCH_W2_CHILD", "1")
            .env("NOTCH_W2_STATE", &dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ledger writer child");
        let stdout = child.stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);
        let mut ready = false;
        for _ in 0..16 {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if line.trim() == "READY" {
                ready = true;
                break;
            }
        }
        assert!(
            ready,
            "child failed to append its first line (round {round})"
        );
        drop(reader);
        child.kill().expect("SIGKILL ledger writer");
        let status = child.wait().expect("reap child");
        assert!(!status.success() || status.code().is_none());

        let lines = read_lines(&dir.join("ledger.jsonl")).unwrap();
        assert!(!lines.is_empty(), "at least the first line must survive");
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(
                line.seq,
                (i + 1) as u64,
                "seq must stay contiguous after a crash: {i}"
            );
            assert!(line.refused.is_none());
        }
        let rr = replay(StateFile::empty(), &lines);
        assert!(
            rr.warnings.is_empty(),
            "no torn line may reach replay: {:?}",
            rr.warnings
        );
        assert_eq!(rr.applied, lines.len());
        // A title is either the fully-written value-N or an earlier one; a
        // torn JSON line is skipped, so a half-written title never becomes state.
        for session in &rr.state.sessions {
            assert!(
                session.title.starts_with("value-"),
                "torn line became a session title: {}",
                session.title
            );
        }
        total_lines += lines.len();

        // A fresh writer must continue above the last committed seq, never
        // reuse a seq whose line was torn by the kill.
        let mut ledger = Ledger::open(&dir).unwrap();
        let last = ledger.last_seq();
        let next = ledger
            .append(Source::System, None, None, "ORCH", Op::Release, 9_999_999)
            .unwrap();
        assert!(
            next > last,
            "crash must not rewind the seq: {next} <= {last}"
        );
    }
    assert!(
        total_lines >= ROUNDS,
        "every round must commit at least one line"
    );
    println!("W2 ledger crash: {ROUNDS} SIGKILL rounds, {total_lines} whole lines replayed");
}
