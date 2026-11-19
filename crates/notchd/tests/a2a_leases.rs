//! TTL file leases (cw/1 `cw claim`) as an in-memory Rust registry.
//! No disk state — the notch repo never gets a `.comms/` directory.

use notchd::bus::{A2aRegistry, BusEvent, DEFAULT_LEASE_TTL_SECS, MAX_LEASE_TTL_SECS};
use notchd::clock::{Clock, FakeClock};

use std::sync::Arc;
use std::thread;

fn registry(at: u64) -> (A2aRegistry, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(at));
    let reg = A2aRegistry::new(clock.clone() as Arc<dyn Clock>);
    (reg, clock)
}

#[test]
fn lease_survives_before_expiry_and_is_released_after() {
    let (reg, clock) = registry(1_000_000);
    assert_eq!(DEFAULT_LEASE_TTL_SECS, 900);
    assert_eq!(MAX_LEASE_TTL_SECS, 3600);

    assert!(reg.claim("S1-OC01", "src/auth.rs", 2), "first claim wins");
    clock.advance_ms(1_500); // 1.5s < 2s
    assert_eq!(reg.owners("src/auth.rs"), vec!["S1-OC01".to_string()]);

    clock.advance_ms(600); // 2.1s > 2s
    assert!(reg.owners("src/auth.rs").is_empty(), "lease must expire");
    assert!(reg.claim("S2-OC01", "src/auth.rs", 2), "expired lease is reclaimable");
    assert_eq!(reg.owners("src/auth.rs"), vec!["S2-OC01".to_string()]);
}

#[test]
fn two_sessions_claiming_same_file_second_is_refused_and_logged() {
    let (reg, _clock) = registry(0);
    assert!(reg.claim("S1-OC01", "src/shared.rs", 60));
    assert!(!reg.claim("S2-OC01", "src/shared.rs", 60), "conflicting claim denied");

    let owners = reg.owners("src/shared.rs");
    assert_eq!(owners, vec!["S1-OC01".to_string()], "owner unchanged after conflict");

    let events: Vec<BusEvent> = reg.events();
    assert_eq!(events.len(), 1, "exactly one conflict event: {events:?}");
    assert_eq!(events[0].class, "conflict");
    assert!(events[0].detail.contains("S1-OC01"), "{:?}", events[0]);
    assert!(events[0].detail.contains("S2-OC01"), "{:?}", events[0]);
    assert!(events[0].detail.contains("src/shared.rs"), "{:?}", events[0]);
}

#[test]
fn only_the_owner_can_release_and_release_frees_the_path() {
    let (reg, _clock) = registry(0);
    assert!(reg.claim("S1-OC01", "src/a.rs", 60));
    assert!(!reg.release("S2-OC01", "src/a.rs"), "non-owner cannot release");
    assert_eq!(reg.owners("src/a.rs"), vec!["S1-OC01".to_string()]);
    assert!(reg.release("S1-OC01", "src/a.rs"));
    assert!(reg.owners("src/a.rs").is_empty());
    assert!(!reg.release("S1-OC01", "src/a.rs"), "double release is a no-op");
}

#[test]
fn same_session_renews_its_lease() {
    let (reg, clock) = registry(0);
    assert!(reg.claim("S1-OC01", "src/a.rs", 2));
    clock.advance_ms(1_500);
    assert!(reg.claim("S1-OC01", "src/a.rs", 2), "owner renews");
    clock.advance_ms(1_500); // 3s total, but renewal pushed expiry to 3.5s
    assert_eq!(reg.owners("src/a.rs"), vec!["S1-OC01".to_string()]);
}

#[test]
fn lease_ttl_is_bounded_to_cw_range() {
    let (reg, clock) = registry(0);
    assert!(reg.claim("S1-OC01", "src/a.rs", 99_999));
    clock.advance_ms((MAX_LEASE_TTL_SECS - 1) * 1000);
    assert_eq!(reg.owners("src/a.rs"), vec!["S1-OC01".to_string()], "clamped to max");
    clock.advance_ms(2_000);
    assert!(reg.owners("src/a.rs").is_empty());
}

#[test]
fn lease_and_event_maps_are_bounded_no_unbounded_growth() {
    let (reg, clock) = registry(0);
    // Exceed the hard cap without expiry first: oldest leases get evicted.
    for i in 0..(notchd::bus::MAX_LEASES + 500) {
        reg.claim("S1-OC01", &format!("src/cap/file{i}.rs"), 3_600);
    }
    assert!(reg.lease_count() <= notchd::bus::MAX_LEASES);
    for batch in 0..20 {
        for i in 0..1_000 {
            let p = format!("src/batch{batch}/file{i}.rs");
            reg.claim("S1-OC01", &p, 1);
        }
        // Expire this batch so the prune path is exercised too.
        clock.advance_ms(1_500);
        // Force pruning without needing a competing claim.
        let _ = reg.owners("src/batch0/file0.rs");
    }
    assert!(reg.lease_count() <= notchd::bus::MAX_LEASES, "leases must be bounded");
    assert!(reg.event_count() <= notchd::bus::MAX_BUS_EVENTS);
    // The registry is plain Mutex storage: it spawns no threads itself, so
    // there is no background worker to leak. Bounded maps are the guarantee.
}

#[test]
fn concurrent_claims_on_one_path_elect_exactly_one_owner() {
    let (reg, _clock) = registry(0);
    let reg = Arc::new(reg);
    let mut handles = Vec::new();
    for i in 0..8 {
        let r = Arc::clone(&reg);
        handles.push(thread::spawn(move || {
            r.claim(&format!("S1-OC{i:02}"), "src/race.rs", 3_600)
        }));
    }
    let wins: usize = handles.into_iter().map(|h| h.join().unwrap()).filter(|won| *won).count();
    assert_eq!(wins, 1, "exactly one successful claim: {wins}");
    assert_eq!(reg.owners("src/race.rs").len(), 1);
}
