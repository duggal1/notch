//! A2A routing policy ported from cw/1 (id-only, no broadcast) into the
//! in-process bus. Tests first — they define the contract the bus must meet.

use notchd::bus::{
    is_valid_message_type, is_wildcard_target, validate_target, A2aRegistry, OrchestratorBus,
    MESSAGE_TYPES,
};
use notchd::clock::{Clock, FakeClock};
use notchd::error::NotchError;

use std::collections::HashSet;
use std::sync::Arc;

fn registry(at: u64) -> A2aRegistry {
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::new(at));
    A2aRegistry::new(clock)
}

fn known(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

#[test]
fn bus_routes_fifo_and_delivers_each_message_exactly_once() {
    let bus = OrchestratorBus::new();
    let (_s1, _rx1) = bus.register("SV-1");
    let (s2, rx2) = bus.register("SV-2");

    for i in 0..5 {
        s2.send_to("SV-2", None, "status", &format!("m{i}"), 1000 + i)
            .unwrap();
    }
    // Nothing is delivered until the orchestrator drains.
    assert!(rx2.try_recv().is_err());
    let routed = bus.drain(&mut |_| {});
    assert_eq!(routed, 5);

    let got: Vec<String> = (0..5)
        .map(|_| rx2.try_recv().expect("missing message").detail)
        .collect();
    assert_eq!(got, vec!["m0", "m1", "m2", "m3", "m4"], "FIFO order");
    // Exactly once: a second drain yields nothing and the mailbox is empty.
    assert_eq!(bus.drain(&mut |_| {}), 0);
    assert!(rx2.try_recv().is_err(), "duplicate delivery");
}

#[test]
fn sequence_numbers_increase_in_drain_order() {
    let bus = OrchestratorBus::new();
    let (_s1, _rx1) = bus.register("SV-1");
    let (s2, rx2) = bus.register("SV-2");
    for i in 0..3 {
        s2.send_to("SV-2", None, "status", &format!("m{i}"), 1)
            .unwrap();
    }
    bus.drain(&mut |_| {});
    let seqs: Vec<u64> = (0..3).map(|_| rx2.try_recv().unwrap().seq).collect();
    assert_eq!(seqs.len(), 3);
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "seq must increase: {seqs:?}"
    );
}

#[test]
fn wildcard_recipients_are_rejected_at_the_bus() {
    for bad in ["all", "ALL", "*", "team:core", "a,b,c", "cw-1234", "", "  "] {
        assert!(is_wildcard_target(bad), "should be wildcard: {bad:?}");
    }
    for good in ["S1-OC01", "S2-OC04", "SV-1", "ORCH"] {
        assert!(!is_wildcard_target(good), "should be a plain id: {good:?}");
    }

    let bus = OrchestratorBus::new();
    let (_s1, _rx1) = bus.register("SV-1");
    let (s2, _rx2) = bus.register("SV-2");
    for bad in ["all", "*", "team:core", "a,b,c"] {
        let r = s2.send_to(bad, None, "status", "hello", 1);
        assert!(r.is_err(), "wildcard `{bad}` must be rejected: {r:?}");
    }
    assert_eq!(bus.drain(&mut |_| {}), 0, "rejected sends must not queue");
}

#[test]
fn unknown_target_is_a_clean_error_not_a_silent_drop() {
    let ids = known(&["S1-OC01", "S2-OC01"]);
    match validate_target("S9-OC99", &ids) {
        Err(NotchError::NotFound(msg)) => assert!(msg.contains("S9-OC99"), "{msg}"),
        other => panic!("expected NotFound, got {other:?}"),
    }
    match validate_target("all", &ids) {
        Err(NotchError::Protocol(msg)) => {
            assert!(msg.to_ascii_lowercase().contains("wildcard"), "{msg}")
        }
        other => panic!("expected Protocol wildcard, got {other:?}"),
    }
    // Exact id matching survives.
    assert!(validate_target("S1-OC01", &ids).is_ok());
}

#[test]
fn message_types_are_restricted_to_the_v1_3_set() {
    assert_eq!(MESSAGE_TYPES.len(), 5);
    for k in ["task", "result", "question", "handoff", "escalate"] {
        assert!(is_valid_message_type(k));
    }
    // cw/1's vocabulary (INFO/ALERT/REQUEST/REPLY) is NOT ours.
    for k in ["INFO", "ALERT", "REQUEST", "REPLY", "status", "", "taskx"] {
        assert!(!is_valid_message_type(k), "must reject kind {k:?}");
    }
}

#[test]
fn validation_logs_rejected_broadcasts_unknown_and_bad_types() {
    let reg = registry(1);
    let ids = known(&["S1-OC01", "S2-OC01"]);

    assert!(reg.validate("question", "all", &ids).is_err());
    assert!(reg.validate("handoff", "*", &ids).is_err());
    assert!(reg.validate("task", "S9-OC99", &ids).is_err());
    assert!(reg.validate("bogus", "S1-OC01", &ids).is_err());
    assert!(reg.validate("handoff", "S2-OC01", &ids).is_ok());

    let events = reg.events();
    assert_eq!(events.len(), 4, "only failures are logged: {events:?}");
    assert!(events.iter().all(|e| e.class == "reject"), "{events:?}");
    let joined = events
        .iter()
        .map(|e| e.detail.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(joined.contains("all"), "{joined}");
    assert!(joined.contains("*"), "{joined}");
    assert!(joined.contains("S9-OC99"), "{joined}");
    assert!(joined.contains("bogus"), "{joined}");

    // Event log is bounded even under spam.
    for _ in 0..1000 {
        let _ = reg.validate("bad", "all", &ids);
    }
    assert!(reg.event_count() <= notchd::bus::MAX_BUS_EVENTS);

    // Draining is destructive: events are appended to events.jsonl once, not
    // replayed on every bus_drain() call.
    let drained = reg.drain_events();
    assert!(!drained.is_empty());
    assert_eq!(reg.event_count(), 0);
    assert!(reg.drain_events().is_empty());
}

#[test]
fn dedupe_drops_a_second_delivery_with_the_same_id() {
    let reg = registry(1);
    let bus = OrchestratorBus::new();
    let (_s1, _rx1) = bus.register("SV-1");
    let (s2, rx2) = bus.register("SV-2");

    let deliver_once = |id: &str, detail: &str| {
        if reg.is_duplicate(id) {
            return false;
        }
        s2.send_to("SV-2", None, "status", detail, 1).unwrap();
        true
    };

    assert!(deliver_once("msg_1", "first"));
    assert!(
        !deliver_once("msg_1", "duplicate"),
        "same id must be dropped"
    );
    assert!(deliver_once("msg_2", "second"));
    assert_eq!(bus.drain(&mut |_| {}), 1 + 1);
    let a = rx2.try_recv().unwrap().detail;
    let b = rx2.try_recv().unwrap().detail;
    assert_eq!((a.as_str(), b.as_str()), ("first", "second"));
    assert!(rx2.try_recv().is_err());

    // Bounded dedupe ring: 20k distinct ids never grow the map past the cap.
    for i in 0..20_000 {
        reg.is_duplicate(&format!("msg_{i}"));
    }
    assert!(reg.seen_count() <= notchd::bus::MAX_SEEN_IDS);
}
