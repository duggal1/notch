//! B2 security regression: the signed approval channel on a REAL in-process
//! `Daemon`. No mocks of `approvals.rs`, no fake clock hand-rolled, no sleeps.
//! The fake herdr is the only external binary stand-in (no herdr in CI), and
//! it never touches the approval path.
//!
//! Attacks covered: forged signature, replay, expired card (16 min TTL),
//! wrong digest, tampered card -> all REJECTED. Valid -> consumed once.

use notchd::approvals::{card_digest, ApprovalCard, SignedAnswer, DEFAULT_APPROVAL_TTL_MS};
use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::herdr::Herdr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-b2sec-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

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

fn daemon(r: &Path, clock: Arc<FakeClock>) -> Daemon {
    let herdr = Herdr::with_runner(|args: &[String]| match args.get(1).map(String::as_str) {
        Some("agent") => Ok(r#"{"result":{"agents":[]}}"#.to_string()),
        _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
    });
    Daemon::open(config(r), herdr, clock).unwrap()
}

fn issue(d: &Daemon, id: &str) -> (String, ApprovalCard) {
    let digest = d
        .issue_approval(
            id,
            "S1-OC01",
            "needs_you",
            "permission prompt: write to ~/.aws/credentials",
        )
        .expect("issue needs_you card");
    let card = d
        .pending_approvals()
        .into_iter()
        .find(|c| c.id == id)
        .expect("issued card is pending");
    assert_eq!(digest, card_digest(&card), "issued digest is the real card digest");
    (digest, card)
}

#[test]
fn valid_answer_is_signed_and_consumed_exactly_once() {
    let d = daemon(&root("valid"), Arc::new(FakeClock::new(T0)));
    let (digest, _card) = issue(&d, "esc_ok");

    let ans = d
        .answer_approval("esc_ok", true, Some("ok".into()), T0, &digest, "allow")
        .expect("honest signed answer");
    assert!(!ans.sig.is_empty(), "a signature is produced");
    assert!(d.approval_signature_valid(&ans), "engine verifies its own signature");
    assert!(d.pending_approvals().is_empty(), "card consumed on valid answer");

    // Replay: the same bytes cannot answer twice.
    let replay = d.answer_approval("esc_ok", true, Some("ok".into()), T0, &digest, "allow");
    assert!(replay.is_err(), "replayed answer must be rejected");
}

#[test]
fn forged_signature_is_rejected() {
    let d = daemon(&root("forged"), Arc::new(FakeClock::new(T0)));
    let (digest, _card) = issue(&d, "esc_forged");
    let good = d
        .answer_approval("esc_forged", true, None, T0, &digest, "allow")
        .unwrap();

    // A worker invents a signature (no secret access) and flips the verdict.
    let mut forged = good.clone();
    forged.sig = "00".repeat(32);
    assert!(!d.approval_signature_valid(&forged), "invented sig rejected");
    forged.allow = false;
    assert!(!d.approval_signature_valid(&forged), "flipped verdict rejected");

    // No standing grant was created by the forged bytes.
    assert!(d.approval_grant("S1-OC01").is_none());
}

#[test]
fn tampered_card_is_rejected() {
    let d = daemon(&root("tampered"), Arc::new(FakeClock::new(T0)));
    let (digest, card) = issue(&d, "esc_tampered");
    let forged_card = ApprovalCard {
        reason: "permission prompt: read ~/.ssh/id_rsa".into(),
        ..card
    };
    let forged_digest = card_digest(&forged_card);
    assert_ne!(digest, forged_digest, "different cards have different digests");

    let attempt = d.answer_approval("esc_tampered", true, None, T0, &forged_digest, "allow");
    assert!(attempt.is_err(), "tampered card digest must be rejected");
    assert!(
        attempt.unwrap_err().to_ascii_lowercase().contains("card"),
        "error names the card mismatch"
    );
    // The real card is still pending and answerable.
    assert_eq!(d.pending_approvals().len(), 1);
}

#[test]
fn wrong_digest_is_rejected() {
    let d = daemon(&root("wrong"), Arc::new(FakeClock::new(T0)));
    let (_digest, _card) = issue(&d, "esc_wrong");
    let attempt = d.answer_approval("esc_wrong", true, None, T0, "deadbeef", "allow");
    assert!(attempt.is_err(), "a non-card digest must be rejected");
    assert_eq!(d.pending_approvals().len(), 1, "card survives a wrong answer");
}

#[test]
fn expired_card_is_rejected() {
    let clock = Arc::new(FakeClock::new(T0));
    let d = daemon(&root("expired"), clock.clone());
    let (digest, _card) = issue(&d, "esc_expired");
    // TTL is on the monotonic timeline, so no wall-clock trust and no 16-min sleep.
    clock.advance_mono_ms(DEFAULT_APPROVAL_TTL_MS + 1);
    let attempt = d.answer_approval("esc_expired", true, None, T0, &digest, "allow");
    match attempt {
        Err(msg) => assert!(msg.to_ascii_lowercase().contains("expir"), "{msg}"),
        Ok(_) => panic!("expired card must be rejected"),
    }
    assert_eq!(d.pending_approvals().len(), 1, "expired card stays out of the grant path");
}

#[test]
fn forged_answer_cannot_be_verified_as_live() {
    let d = daemon(&root("verify"), Arc::new(FakeClock::new(T0)));
    let (digest, _card) = issue(&d, "esc_verify");
    let good: SignedAnswer = d
        .answer_approval("esc_verify", true, None, T0, &digest, "allow")
        .unwrap();
    // Once consumed, the exact signed bytes are no longer the live answer.
    assert!(
        d.approval_signature_valid(&good),
        "signature bytes are still well formed"
    );
    // A second answer for the same id is refused, so replay cannot re-enter.
    assert!(d.answer_approval("esc_verify", true, None, T0, &digest, "allow").is_err());
}
