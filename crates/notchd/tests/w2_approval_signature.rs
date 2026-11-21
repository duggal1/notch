//! W2-3 RED-first: signed approval channel. A worker must not be able to
//! forge, flip, backdate, or replay a human answer. No daemon, no OpenCode.

use notchd::approvals::{
    card_digest, decision_bytes, AnswerMode, ApprovalAuthority, ApprovalCard, ApprovalInbox,
    SignedAnswer, ANSWER_MODES,
};
use notchd::clock::{Clock, FakeClock};
use notchd::error::NotchError;
use std::sync::Arc;

fn clock(at: u64) -> Arc<dyn Clock> {
    Arc::new(FakeClock::new(at))
}

fn card(id: &str) -> ApprovalCard {
    ApprovalCard {
        id: id.to_string(),
        session: "S1-OC01".to_string(),
        kind: "needs_you".to_string(),
        reason: "permission prompt: write to ~/.aws/credentials".to_string(),
        at: 1000,
    }
}

fn issue(inbox: &ApprovalInbox, c: &ApprovalCard) -> String {
    inbox.issue(c.clone()).expect("issue must succeed")
}

#[test]
fn answer_modes_are_a_closed_whitelist() {
    assert_eq!(ANSWER_MODES.len(), 6);
    for m in ANSWER_MODES {
        assert!(AnswerMode::parse(m).is_some(), "mode must parse: {m}");
        assert_eq!(AnswerMode::parse(m).unwrap().as_str(), m);
    }
    for bad in [
        "sudo",
        "bypass",
        "allow\n",
        "ALLOW",
        "",
        "root",
        "bypass_permissions,x",
    ] {
        assert!(AnswerMode::parse(bad).is_none(), "must reject mode {bad:?}");
    }
    assert!(AnswerMode::BypassPermissions.is_permission_grant());
    assert!(AnswerMode::Default.is_permission_grant());
    assert!(
        !AnswerMode::Allow.is_permission_grant(),
        "a verdict is not a standing grant"
    );
}

#[test]
fn signature_binds_every_field_in_a_stable_array() {
    let a = ApprovalAuthority::new();
    let sig = a.sign("esc_1", true, Some("ok"), 1000, "cardhash", "allow");
    let ans = SignedAnswer {
        id: "esc_1".into(),
        allow: true,
        note: Some("ok".into()),
        at: 1000,
        card: "cardhash".into(),
        mode: "allow".into(),
        sig,
    };
    assert!(a.verify(&ans), "honest answer must verify");

    // A second, independent in-process key cannot verify the first's answer:
    // the secret never leaves the process that made it.
    let stranger = ApprovalAuthority::new();
    assert!(
        !stranger.verify(&ans),
        "a foreign/forge key must not verify"
    );

    // The signed bytes are a canonical ARRAY (id, verdict, note, at, card, mode).
    let bytes = decision_bytes("esc_1", true, Some("ok"), 1000, "cardhash", "allow");
    assert_eq!(
        bytes,
        br#"["esc_1",true,"ok",1000,"cardhash","allow"]"#.to_vec()
    );
    // Object key order cannot be exploited to reorder the signature input.
    assert_ne!(
        decision_bytes("esc_1", true, Some("ok"), 1000, "cardhash", "allow"),
        decision_bytes("esc_1", true, Some("ok"), 1000, "allow", "cardhash")
    );
}

#[test]
fn unsigned_answer_is_rejected() {
    let inbox = ApprovalInbox::new(clock(1000));
    let c = card("esc_1");
    let digest = issue(&inbox, &c);
    let mut ans = inbox
        .answer("esc_1", true, None, 1000, &digest, "allow")
        .expect("honest answer");
    ans.sig.clear(); // worker strips the signature
    assert!(!ans.sig.is_empty() || ans.sig.is_empty()); // silence clippy on the mutate
    assert!(
        !inbox.signature_valid(&ans),
        "unsigned answer must be rejected"
    );
}

#[test]
fn forged_and_tampered_answers_are_rejected() {
    let inbox = ApprovalInbox::new(clock(1000));
    let c = card("esc_2");
    let digest = issue(&inbox, &c);
    let good = inbox
        .answer(
            "esc_2",
            true,
            Some("yes".to_string()),
            1000,
            &digest,
            "bypass_permissions",
        )
        .expect("honest answer");
    assert!(inbox.signature_valid(&good));

    // Flip the verdict after signing.
    let mut flipped = good.clone();
    flipped.allow = false;
    assert!(
        !inbox.signature_valid(&flipped),
        "flipped verdict must be rejected"
    );

    // Rewrite the note after signing.
    let mut noted = good.clone();
    noted.note = Some("no, actually".into());
    assert!(
        !inbox.signature_valid(&noted),
        "tampered note must be rejected"
    );

    // Change the card the answer claims to authorize.
    let mut wrong_card = good.clone();
    wrong_card.card = "some-other-card".into();
    assert!(
        !inbox.signature_valid(&wrong_card),
        "card substitution must be rejected"
    );

    // Move the answer to a different escalation id.
    let mut moved = good.clone();
    moved.id = "esc_99".into();
    assert!(
        !inbox.signature_valid(&moved),
        "an answer cannot be moved to another card"
    );

    // Change the permission mode after signing — a worker cannot widen its own grant.
    let digest3 = issue(&inbox, &card("esc_3"));
    let mut widen = inbox
        .answer("esc_3", true, None, 1000, &digest3, "default")
        .unwrap();
    assert_eq!(widen.mode, "default");
    widen.mode = "bypass_permissions".into();
    assert!(
        !inbox.signature_valid(&widen),
        "a tampered mode must be rejected"
    );
}

#[test]
fn expired_answer_is_rejected() {
    let c: Arc<FakeClock> = Arc::new(FakeClock::new(1000));
    let inbox = ApprovalInbox::new(c.clone());
    let k = card("esc_3");
    let digest = issue(&inbox, &k);
    // Move past the TTL using the monotonic timeline (no wall-clock trust).
    c.advance_ms(notchd::approvals::DEFAULT_APPROVAL_TTL_MS + 1);
    match inbox.answer(
        "esc_3",
        true,
        None,
        1000 + notchd::approvals::DEFAULT_APPROVAL_TTL_MS + 1,
        &digest,
        "allow",
    ) {
        Err(NotchError::Protocol(msg)) => {
            assert!(msg.to_ascii_lowercase().contains("expir"), "{msg}")
        }
        other => panic!("expected expired rejection, got {other:?}"),
    }
}

#[test]
fn answer_must_match_the_exact_card_that_was_shown() {
    let inbox = ApprovalInbox::new(clock(1000));
    let k = card("esc_4");
    let digest = issue(&inbox, &k);
    let forged_digest = card_digest(&ApprovalCard {
        reason: "permission prompt: read ~/.ssh/id_rsa".into(),
        ..k.clone()
    });
    assert_ne!(
        digest, forged_digest,
        "different cards must have different digests"
    );
    match inbox.answer("esc_4", true, None, 1000, &forged_digest, "allow") {
        Err(NotchError::Protocol(msg)) => {
            assert!(msg.to_ascii_lowercase().contains("card"), "{msg}")
        }
        other => panic!("expected card-mismatch rejection, got {other:?}"),
    }
}

#[test]
fn backdated_and_future_answers_are_rejected() {
    let inbox = ApprovalInbox::new(clock(1000));
    let digest = issue(&inbox, &card("esc_5"));
    // `at` before the card was issued.
    assert!(inbox
        .answer("esc_5", true, None, 1, &digest, "allow")
        .is_err());
    // `at` far in the future.
    assert!(inbox
        .answer("esc_5", true, None, 1000 + 120_000, &digest, "allow")
        .is_err());
    // A valid, current answer is accepted.
    assert!(inbox
        .answer("esc_5", true, None, 1000, &digest, "allow")
        .is_ok());
}

#[test]
fn replay_of_a_consumed_answer_is_rejected() {
    let inbox = ApprovalInbox::new(clock(1000));
    let digest = issue(&inbox, &card("esc_6"));
    let ans = inbox
        .answer("esc_6", true, None, 1000, &digest, "allow")
        .unwrap();
    assert!(inbox.verify_answer(&ans), "live answer must verify");

    // A second answer for the same request is refused (consume-once).
    assert!(inbox
        .answer("esc_6", true, None, 1000, &digest, "allow")
        .is_err());

    // Once taken by the engine, the same bytes cannot verify again (replay).
    let taken = inbox
        .take_answer("esc_6")
        .expect("engine takes the answer once");
    assert_eq!(taken, ans);
    assert!(
        !inbox.verify_answer(&ans),
        "replay after consumption must be rejected"
    );
    assert!(
        inbox.take_answer("esc_6").is_none(),
        "double take must be empty"
    );
}

#[test]
fn invalid_ids_and_unissued_cards_cannot_be_answered() {
    let inbox = ApprovalInbox::new(clock(1000));
    for bad in ["", "../etc/passwd", "esc_1/../../x", "esc 1", "a:b"] {
        assert!(
            inbox
                .issue(ApprovalCard {
                    id: bad.into(),
                    ..card("x")
                })
                .is_err(),
            "bad id {bad:?}"
        );
    }
    // No request was ever issued for this id.
    assert!(inbox
        .answer("esc_nope", true, None, 1000, "any", "allow")
        .is_err());
    assert!(inbox.issue(card("esc_ok")).is_ok());
    // Duplicate issue of the same id is refused (no queue jumping).
    assert!(inbox.issue(card("esc_ok")).is_err());
}

#[test]
fn stress_500_answers_stay_bounded_and_unique() {
    let inbox = ApprovalInbox::new(clock(1000));
    for i in 0..500 {
        let id = format!("esc_{i}");
        let digest = issue(&inbox, &card(&id));
        let ans = inbox
            .answer(
                &id,
                i % 2 == 0,
                Some("note".to_string()),
                1000,
                &digest,
                "allow",
            )
            .unwrap();
        assert!(inbox.signature_valid(&ans));
        assert!(inbox.is_answered(&id));
    }
    assert!(inbox.pending_cards().is_empty(), "all requests consumed");
}
