//! Unread A2A mail counts — the black-box equivalent of cw/1's OpenCode
//! `context` plugin banner, delivered as a one-line prompt prefix.

use notchd::bus::{A2aRegistry, MAX_UNREAD_SESSIONS};
use notchd::clock::Clock;
use notchd::clock::FakeClock;

use std::sync::Arc;

fn registry() -> A2aRegistry {
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::new(0));
    A2aRegistry::new(clock)
}

#[test]
fn unread_counts_increment_and_decrement_across_100_messages() {
    let reg = registry();
    for i in 1..=100 {
        reg.note_delivery("S1-OC01");
        assert_eq!(reg.unread_for("S1-OC01"), i, "count after {i} messages");
    }
    // A second session is independent.
    assert_eq!(reg.unread_for("S2-OC01"), 0);
    reg.note_delivery("S2-OC01");
    assert_eq!(reg.unread_for("S1-OC01"), 100);
    assert_eq!(reg.unread_for("S2-OC01"), 1);

    assert_eq!(reg.mark_read("S1-OC01"), 100);
    assert_eq!(reg.unread_for("S1-OC01"), 0);
    assert_eq!(reg.unread_for("S2-OC01"), 1, "other session untouched");
    assert_eq!(reg.mark_read("S1-OC01"), 0, "mark_read is idempotent");

    // New mail after a read starts the count again.
    reg.note_delivery("S1-OC01");
    assert_eq!(reg.unread_for("S1-OC01"), 1);
}

#[test]
fn unread_prefix_is_emitted_only_when_positive_and_is_truthful() {
    let reg = registry();
    assert_eq!(reg.unread_prefix("S1-OC01"), None, "zero unread -> no line");

    reg.note_delivery("S1-OC01");
    reg.note_delivery("S1-OC01");
    reg.note_delivery("S1-OC01");
    let prefix = reg.unread_prefix("S1-OC01").expect("3 unread -> prefix");
    assert_eq!(prefix, "you have 3 unread A2A messages");
    assert!(!prefix.contains('\n'), "must be a single line");
    assert!(prefix.len() < 80, "must be short and bounded");

    // Taking the prefix marks the mail read (done at prompt-delivery glue).
    assert_eq!(
        reg.take_unread_prefix("S1-OC01").as_deref(),
        Some(prefix.as_str())
    );
    assert_eq!(reg.unread_for("S1-OC01"), 0);
    assert_eq!(reg.take_unread_prefix("S1-OC01"), None);
}

#[test]
fn prefix_counts_all_pending_mail_not_just_the_last_message() {
    let reg = registry();
    for _ in 0..7 {
        reg.note_delivery("S1-OC01");
    }
    // A new message arrives while 7 are pending; the next prompt announces 8.
    reg.note_delivery("S1-OC01");
    assert_eq!(
        reg.take_unread_prefix("S1-OC01").as_deref(),
        Some("you have 8 unread A2A messages")
    );
}

#[test]
fn unread_map_is_bounded_over_many_sessions() {
    let reg = registry();
    for i in 0..20_000 {
        reg.note_delivery(&format!("S{i}-OC01"));
    }
    assert!(reg.unread_session_count() <= MAX_UNREAD_SESSIONS);
    // The counts that remain are still correct for a live session.
    reg.note_delivery("S1-OC01");
    assert_eq!(reg.unread_for("S1-OC01"), 1);
}

#[test]
fn prefix_body_preserves_a_verbatim_body_when_no_mail_is_pending() {
    let reg = registry();
    let body = "handoff: the tests fail at auth.rs:12";
    assert_eq!(
        reg.prefix_body("S1-OC01", body),
        body,
        "byte-verbatim at zero unread"
    );

    // Old pending mail is announced once, then cleared; the new message that
    // is being delivered becomes the single unread item for the next prompt.
    reg.note_delivery("S1-OC01");
    reg.note_delivery("S1-OC01");
    let prefixed = reg.prefix_body("S1-OC01", body);
    assert_eq!(prefixed, format!("you have 2 unread A2A messages\n{body}"));
    assert_eq!(reg.unread_for("S1-OC01"), 0);
    reg.note_delivery("S1-OC01"); // ingress step in the daemon glue
    assert_eq!(
        reg.prefix_body("S1-OC01", body),
        format!("you have 1 unread A2A messages\n{body}")
    );
}

#[test]
fn prefix_text_never_claims_zero_or_negative() {
    let reg = registry();
    reg.note_delivery("S1-OC01");
    reg.mark_read("S1-OC01");
    assert_eq!(reg.unread_prefix("S1-OC01"), None);
    // Empty session ids are ignored — no phantom counters.
    reg.note_delivery("");
    assert_eq!(reg.unread_for(""), 0);
}
