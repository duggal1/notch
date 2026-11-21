//! W2-3 RED-first: controlled A2A ingress routes ONLY through the orchestrator.
//! FROM/TO session ids are preserved in the routed envelope; session ids are
//! never mailboxes, so no direct session→session channel can exist.

use notchd::bus::{A2aBridgeIngress, OrchestratorBus};
use notchd::clock::FakeClock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn setup() -> (OrchestratorBus, std::sync::mpsc::Receiver<notchd::bus::BusMessage>, std::sync::mpsc::Receiver<notchd::bus::BusMessage>, A2aBridgeIngress) {
    let bus = OrchestratorBus::new();
    let (_s1, rx1) = bus.register("SV-1");
    let (_s2, rx2) = bus.register("SV-2");
    let ingress = A2aBridgeIngress::new(Arc::new(FakeClock::new(1000)), bus.inbox_sender());
    let mut routes = HashMap::new();
    routes.insert("S1-OC01".to_string(), "SV-1".to_string());
    routes.insert("S2-OC01".to_string(), "SV-2".to_string());
    ingress.set_routes(routes);
    ingress.set_known(HashSet::from(["S1-OC01".to_string(), "S2-OC01".to_string()]));
    (bus, rx1, rx2, ingress)
}

#[test]
fn session_ids_are_not_mailboxes_no_direct_channel_exists() {
    let (bus, _rx1, _rx2, ingress) = setup();
    assert!(bus.is_registered("SV-1"));
    assert!(bus.is_registered("SV-2"));
    assert!(!bus.is_registered("S1-OC01"), "a session must never own a mailbox");
    assert!(!bus.is_registered("S2-OC01"), "a session must never own a mailbox");
    // The ingress exposes no receiver and no route table mutation for sessions;
    // only `submit` (enqueue into the orchestrator inbox) is possible.
    assert_eq!(ingress.submitted_count(), 0);
}

#[test]
fn from_to_routing_is_preserved_through_the_orchestrator() {
    let (bus, rx1, rx2, ingress) = setup();
    ingress
        .submit("a2a-1", "S1-OC01", "S2-OC01", "handoff", "tests fail at auth.rs:12")
        .expect("valid submit");
    assert_eq!(ingress.submitted_count(), 1);
    assert_eq!(bus.drain(&mut |_| {}), 1, "nothing routes before drain");

    let msg = rx2.try_recv().expect("target supervisor must receive");
    assert_eq!(msg.from, "SV-1", "FROM supervisor preserved");
    assert_eq!(msg.to, "SV-2", "TO supervisor preserved");
    assert_eq!(msg.session.as_deref(), Some("S2-OC01"), "target session preserved");
    assert!(msg.detail.contains("S1-OC01->S2-OC01"), "FROM/TO session pair preserved");
    assert!(msg.detail.contains("tests fail at auth.rs:12"), "body preserved");
    assert!(rx1.try_recv().is_err(), "source supervisor must not receive its own message");
    assert_eq!(bus.drain(&mut |_| {}), 0, "exactly-once delivery");
}

#[test]
fn ingress_rejects_wildcards_unknown_targets_bad_types_and_duplicate_ids() {
    let (bus, _rx1, _rx2, ingress) = setup();
    assert!(ingress.submit("x1", "S1-OC01", "all", "task", "x").is_err());
    assert!(ingress.submit("x2", "S1-OC01", "team:core", "task", "x").is_err());
    assert!(ingress.submit("x3", "S1-OC01", "S9-OC99", "task", "x").is_err());
    assert!(ingress.submit("x4", "S1-OC01", "S2-OC01", "INFO", "x").is_err());
    assert!(ingress.submit("x5", "S9-OC99", "S2-OC01", "task", "x").is_err(), "unknown sender refused");
    assert_eq!(ingress.submitted_count(), 0, "rejected submits never enqueue");
    assert_eq!(bus.drain(&mut |_| {}), 0);

    assert!(ingress.submit("same", "S1-OC01", "S2-OC01", "task", "first").is_ok());
    assert!(ingress.submit("same", "S1-OC01", "S2-OC01", "task", "second").is_err(), "duplicate id dropped");
    assert_eq!(ingress.submitted_count(), 1);
    let routed = bus.drain(&mut |_| {});
    assert_eq!(routed, 1);
    assert_eq!(routed, 1);
}

#[test]
fn stress_100_messages_route_fifo_and_stay_bounded() {
    let (bus, rx1, rx2, ingress) = setup();
    for i in 0..100 {
        ingress
            .submit(&format!("m{i}"), "S1-OC01", "S2-OC01", "task", &format!("job {i}"))
            .expect("submit {i}");
    }
    assert_eq!(ingress.submitted_count(), 100);
    assert_eq!(bus.drain(&mut |_| {}), 100);
    for i in 0..100 {
        let m = rx2.try_recv().expect("missing message");
        assert_eq!(m.detail, format!("S1-OC01->S2-OC01 job {i}"), "FIFO order");
    }
    assert!(rx1.try_recv().is_err());
    assert!(rx2.try_recv().is_err());
}
