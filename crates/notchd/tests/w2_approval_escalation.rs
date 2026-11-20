//! W2-3 RED-first: only `needs_you` escalations are answerable, and only via
//! the signed answer channel. Engine-side API, headless, no OpenCode process.

use notchd::approvals::AnswerMode;
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
    let p = std::env::temp_dir().join(format!("notchd-w2appr-{tag}-{}-{n}", std::process::id()));
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

fn daemon(r: &Path) -> Daemon {
    let herdr = Herdr::with_runner(|args: &[String]| match args.get(1).map(String::as_str) {
        Some("agent") => Ok(r#"{"result":{"agents":[]}}"#.to_string()),
        _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
    });
    Daemon::open(config(r), herdr, Arc::new(FakeClock::new(T0))).unwrap()
}

#[test]
fn only_needs_you_escalations_create_an_answerable_card() {
    let r = root("kinds");
    let d = daemon(&r);

    for kind in [
        "stuck",
        "loop_detected",
        "budget_exceeded",
        "blocked",
        "stuck_suspect",
    ] {
        assert!(
            d.issue_approval(&format!("esc_{kind}"), "S1-OC01", kind, "reason")
                .is_err(),
            "kind `{kind}` must not be answerable by a human"
        );
    }
    assert!(
        d.pending_approvals().is_empty(),
        "no approvals issued for non-needs_you kinds"
    );

    let digest = d
        .issue_approval("esc_1", "S1-OC01", "needs_you", "permission prompt")
        .expect("needs_you must be answerable");
    assert!(!digest.is_empty(), "card digest must be non-empty");
    let pending = d.pending_approvals();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "esc_1");
    assert_eq!(pending[0].kind, "needs_you");
    assert_eq!(pending[0].session, "S1-OC01");
}

#[test]
fn answering_a_needs_you_card_records_a_signed_grant_only_for_allowed_modes() {
    let r = root("modes");
    let d = daemon(&r);
    let digest = d
        .issue_approval("esc_2", "S1-OC01", "needs_you", "permission prompt")
        .unwrap();

    // A mode outside the whitelist is refused and grants nothing.
    assert!(d
        .answer_approval("esc_2", true, None, T0, &digest, "sudo")
        .is_err());
    assert!(
        d.approval_grant("S1-OC01").is_none(),
        "invalid mode must not grant"
    );

    // A valid signed verdict that is not a permission grant leaves no standing grant.
    let ans = d
        .answer_approval("esc_2", true, None, T0, &digest, "allow")
        .unwrap();
    assert_eq!(ans.id, "esc_2");
    assert!(
        d.approval_grant("S1-OC01").is_none(),
        "a verdict is not a standing grant"
    );

    // A signed permission-mode grant is recorded for the session only after a
    // valid answer for a needs_you card.
    let digest3 = d
        .issue_approval("esc_3", "S2-OC01", "needs_you", "permission prompt")
        .unwrap();
    let grant = d
        .answer_approval(
            "esc_3",
            true,
            Some("ok".to_string()),
            T0,
            &digest3,
            "bypass_permissions",
        )
        .unwrap();
    assert_eq!(grant.mode, "bypass_permissions");
    assert_eq!(
        d.approval_grant("S2-OC01"),
        Some(AnswerMode::BypassPermissions)
    );
    // A different session has no grant: grants are per-session, never global.
    assert_eq!(d.approval_grant("S1-OC01"), None);
}

#[test]
fn a_needs_you_escalation_is_never_answered_with_typed_prompt_text() {
    let r = root("no-prompt");
    let d = daemon(&r);
    let digest = d
        .issue_approval("esc_4", "S1-OC01", "needs_you", "blocked_waiting")
        .unwrap();
    // Deny is a valid human answer; it is recorded, not typed into the pane.
    let ans = d
        .answer_approval(
            "esc_4",
            false,
            Some("ask me first".to_string()),
            T0,
            &digest,
            "deny",
        )
        .unwrap();
    assert!(!ans.allow, "deny must be preserved");
    assert_eq!(ans.note.as_deref(), Some("ask me first"));
    // The card is consumed: a second answer cannot flip it.
    assert!(d
        .answer_approval("esc_4", true, None, T0, &digest, "allow")
        .is_err());
}

#[test]
fn malformed_cards_cannot_be_injected() {
    let r = root("inject");
    let d = daemon(&r);
    for (id, session, reason) in [
        ("esc_/../../x", "S1-OC01", "x"),
        ("", "S1-OC01", "x"),
        ("esc_1", "", "x"),
        ("esc_1", "S1-OC01", "x\ninjected instruction"),
    ] {
        assert!(
            d.issue_approval(id, session, "needs_you", reason).is_err(),
            "bad card id={id:?} session={session:?} reason={reason:?}"
        );
    }
    assert!(d.pending_approvals().is_empty());
}
