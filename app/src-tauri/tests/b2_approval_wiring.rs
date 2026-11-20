//! B2 command-level regression: the Tauri approval commands must use the
//! engine's signed channel and the engine's real pending cards. Real in-process
//! `Daemon` with an isolated state directory.
//!
//! Written BEFORE the fix: fails against the snapshot-projection / reprompt
//! implementation (compile RED because the engine-backed helpers do not exist).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use notch::commands::{answer_approval_payload, approvals_from_engine};
use notchd::clock::SystemClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;

static N: AtomicU64 = AtomicU64::new(0);

fn config(r: &Path) -> Config {
    std::fs::create_dir_all(r.join("tmp")).unwrap();
    Config {
        state_dir: r.join("state"),
        tmp_dir: r.join("tmp"),
        stuck_after_secs: 3600,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

fn daemon() -> Daemon {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let r = std::env::temp_dir().join(format!("notch-b2wire-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&r);
    std::fs::create_dir_all(&r).unwrap();
    Daemon::open(config(&r), Herdr::from_env(), Arc::new(SystemClock)).unwrap()
}

fn issue(d: &Daemon, id: &str) -> String {
    d.issue_approval(
        id,
        "S1-OC01",
        "needs_you",
        "permission prompt: write to ~/.aws/credentials",
    )
    .expect("issue needs_you card")
}

#[test]
fn list_approvals_reads_engine_cards_with_the_real_digest() {
    let d = daemon();
    let real = issue(&d, "esc_list");
    let cards = approvals_from_engine(&d);
    assert_eq!(cards.len(), 1, "card comes from the engine inbox");
    assert_eq!(cards[0].id, "esc_list");
    assert_eq!(cards[0].session, "S1-OC01");
    assert!(cards[0].signed, "the card is genuinely signable");
    assert_eq!(
        cards[0].digest, real,
        "digest is the engine SHA-256 card digest"
    );
    assert_eq!(cards[0].digest.len(), 64, "SHA-256 hex, not an FNV hash");
}

#[test]
fn answer_approval_uses_the_signed_channel_and_consumes_once() {
    let d = daemon();
    let digest = issue(&d, "esc_answer");
    let res = answer_approval_payload(&d, "esc_answer", true, Some("ok".into()), &digest)
        .expect("honest answer succeeds");
    assert_eq!(res["signed"], true, "the command reports a signed answer");
    assert_eq!(res["source"], "engine");
    assert!(
        res["sig"].as_str().map(|s| s.len() == 64).unwrap_or(false),
        "real HMAC sig"
    );
    assert!(
        d.pending_approvals().is_empty(),
        "the card is consumed exactly once"
    );

    let replay = answer_approval_payload(&d, "esc_answer", true, Some("ok".into()), &digest);
    assert!(replay.is_err(), "replayed answer returns an honest error");
}

#[test]
fn unknown_id_returns_an_honest_error() {
    let d = daemon();
    let err =
        answer_approval_payload(&d, "esc_nope", true, None, "").expect_err("no such card -> error");
    assert!(!err.is_empty(), "error string reaches the UI");
}

#[test]
fn stale_displayed_digest_cannot_consume_the_new_card() {
    let d = daemon();
    let old = issue(&d, "esc_reissued");
    answer_approval_payload(&d, "esc_reissued", true, None, &old).unwrap();
    let new = d
        .issue_approval("esc_reissued", "S1-OC01", "needs_you", "A changed request")
        .unwrap();

    let stale = answer_approval_payload(&d, "esc_reissued", true, None, &old);
    assert!(
        stale.is_err(),
        "the old digest must not authorize the new card"
    );
    assert_eq!(approvals_from_engine(&d)[0].digest, new);
    answer_approval_payload(&d, "esc_reissued", true, None, &new).unwrap();
}
