//! W2-4 — headless tests for the fleet-level start guard.
//!
//! No sleeps, no real subprocesses, no GUI, no `#[ignore]`. Every timeline is
//! monotonic and injected explicitly, so a wall-clock jump cannot influence a
//! verdict. These are the RED tests for steal #5 (agentbox start-hold /
//! storm-detection / preemption).

use notchd::supervisor::{
    FleetGuard, StartGate, BURST_STARTS, FAST_START_MS, HELD_RETRY_MS, PREEMPTION_HOLD_MS,
    START_HOLD_MS, STORM_STARTS, STORM_WINDOW_MS,
};

use std::sync::Arc;

const T0: u64 = 7_000_000_000;

#[test]
fn burst_of_twenty_starts_is_held_then_released() {
    let mut g = FleetGuard::new();
    let mut held = 0usize;
    for i in 0..20 {
        let id = format!("S1-OC{i:02}");
        match g.note_start(&id, T0) {
            StartGate::Hold { reason, until_ms, first } => {
                held += 1;
                assert!(
                    reason.contains("burst") || reason.contains("hold"),
                    "hold must carry a reason: {reason}"
                );
                assert_eq!(until_ms, T0 + START_HOLD_MS, "hold is brief and bounded");
                if first {
                    assert!(reason.contains("start burst"), "first hold names the cause");
                }
            }
            StartGate::Admit => {}
            other => panic!("distinct sessions must not storm or preempt: {other:?}"),
        }
    }
    assert_eq!(
        held,
        20 - BURST_STARTS + 1,
        "all starts after the burst threshold are held"
    );
    assert_eq!(g.held_ids().len(), held, "held set tracks every deferred start");

    // Still held just before the deadline; released exactly at it, with a
    // reason. The guard is time-based — no release call is required to unblock.
    assert!(g.admission("S1-OC19", T0 + 1).is_some());
    let due = g.release_due(T0 + START_HOLD_MS);
    assert!(!due.is_empty(), "release must report a reason");
    assert!(due[0].1.contains("released"), "release reason is explicit");
    assert!(g.held_ids().is_empty(), "release clears the held set");
    assert!(g.admission("S1-OC19", T0 + START_HOLD_MS).is_none());

    // A fresh, isolated start after the window is admitted again — the hold is
    // a burst reaction, not a permanent stop.
    assert!(matches!(
        g.note_start("S2-OC01", T0 + START_HOLD_MS + 1),
        StartGate::Admit
    ));
}

#[test]
fn hold_expires_on_time_even_without_a_release_call() {
    let mut g = FleetGuard::new();
    for i in 0..20 {
        let _ = g.note_start(&format!("S1-OC{i:02}"), T0);
    }
    assert!(g.admission("S1-OC10", T0 + START_HOLD_MS - 1).is_some());
    assert!(
        g.admission("S1-OC10", T0 + START_HOLD_MS).is_none(),
        "a brief hold must never become an infinite hold"
    );
}

#[test]
fn six_fast_starts_in_ten_minutes_detects_storm_with_reason() {
    let mut g = FleetGuard::new();
    let mut storm_reason = None;
    for i in 0..STORM_STARTS {
        let gate = g.note_start("S1-OC01", T0 + (i as u64) * 10_000);
        if i + 1 == STORM_STARTS {
            match gate {
                StartGate::Storm { reason } => {
                    assert!(reason.contains("storm"), "reason names the pattern: {reason}");
                    assert!(reason.contains("6"), "reason carries the count: {reason}");
                    storm_reason = Some(reason);
                }
                other => panic!("expected a storm on start {STORM_STARTS}, got {other:?}"),
            }
        } else {
            assert!(
                matches!(gate, StartGate::Admit | StartGate::Hold { .. }),
                "start {} must not storm early: {gate:?}",
                i + 1
            );
        }
    }
    assert!(storm_reason.is_some());
    assert_eq!(g.stormed_ids(), vec!["S1-OC01".to_string()]);
    assert_eq!(g.counters().1, 1, "one storm counted");

    // A single fast start must NEVER trip it: the pattern needs the full ring.
    let mut g2 = FleetGuard::new();
    assert!(!matches!(
        g2.note_start("S1-OC01", T0),
        StartGate::Storm { .. }
    ));
    assert!(g2.stormed_ids().is_empty());
}

#[test]
fn slow_starts_do_not_form_a_storm() {
    let mut g = FleetGuard::new();
    // Same count, but each start ends well after FAST_START_MS: not a storm.
    for i in 0..STORM_STARTS + 2 {
        let gate = g.note_start("S1-OC01", T0 + (i as u64) * (FAST_START_MS + 1));
        assert!(
            !matches!(gate, StartGate::Storm { .. }),
            "a slow start is not evidence of a dead-start loop"
        );
    }
    assert!(g.stormed_ids().is_empty());
}

#[test]
fn starts_outside_the_ten_minute_window_do_not_form_a_storm() {
    let mut g = FleetGuard::new();
    // Six fast starts, but the first has aged out of STORM_WINDOW_MS by the
    // time the sixth arrives.
    g.note_start("S1-OC01", T0);
    for i in 1..STORM_STARTS {
        let gate = g.note_start("S1-OC01", T0 + STORM_WINDOW_MS + (i as u64) * 1_000);
        assert!(
            !matches!(gate, StartGate::Storm { .. }),
            "expired starts must be dropped from the ring"
        );
    }
    assert!(g.stormed_ids().is_empty());
}

#[test]
fn preemption_is_bounded_no_infinite_reprompts() {
    let mut g = FleetGuard::new();
    for i in 0..STORM_STARTS {
        let _ = g.note_start("S1-OC01", T0 + (i as u64) * 1_000);
    }
    let storm_at = T0 + (STORM_STARTS as u64 - 1) * 1_000;

    // A storming worker is never rapidly re-admitted.
    for i in 0..100 {
        match g.note_start("S1-OC01", storm_at + 1 + i) {
            StartGate::Preempt { reason, .. } => assert!(
                reason.contains("preempt") || reason.contains("held"),
                "preempt reason must explain the hold: {reason}"
            ),
            other => panic!("storming worker re-admitted too fast: {other:?}"),
        }
    }

    // Exactly three self-retries (30 min, +2 h, +8 h), then it waits for a
    // person. No fourth auto start, ever.
    let mut at = storm_at + HELD_RETRY_MS[0];
    assert!(matches!(g.note_start("S1-OC01", at), StartGate::Admit), "retry 1");
    at += HELD_RETRY_MS[1];
    assert!(matches!(g.note_start("S1-OC01", at), StartGate::Admit), "retry 2");
    at += HELD_RETRY_MS[2];
    assert!(matches!(g.note_start("S1-OC01", at), StartGate::Admit), "retry 3");

    for i in 0..100 {
        match g.note_start("S1-OC01", at + 1 + i) {
            StartGate::Preempt { reason, .. } => assert!(
                reason.contains("human") || reason.contains("needs") || reason.contains("preempt"),
                "exhausted retries must say needs-you: {reason}"
            ),
            other => panic!("infinite re-prompts after the retry budget: {other:?}"),
        }
    }
    assert!(g.preempted_ids().contains(&"S1-OC01".to_string()));
}

#[test]
fn preempt_victim_is_bounded_settles_by_call_and_by_timeout() {
    let mut g = FleetGuard::new();
    for i in 0..STORM_STARTS {
        let _ = g.note_start("S1-OC01", T0 + (i as u64) * 1_000);
    }
    let storm_at = T0 + (STORM_STARTS as u64 - 1) * 1_000;
    let (victim, reason) = g
        .preempt_victim(storm_at + 10)
        .expect("a storming worker is a preemption candidate");
    assert_eq!(victim, "S1-OC01");
    assert!(reason.contains("storm") || reason.contains("preempt"));

    g.settle(&victim);
    assert!(
        g.preempt_victim(storm_at + 10).is_none(),
        "a settled preemption must not be taken twice"
    );

    // A preemption that is never settled expires on the monotonic clock.
    let mut g2 = FleetGuard::new();
    for i in 0..STORM_STARTS {
        let _ = g2.note_start("S1-OC02", T0 + (i as u64) * 1_000);
    }
    assert!(g2.preempt_victim(storm_at + 1).is_some());
    assert!(
        g2.preempt_victim(storm_at + PREEMPTION_HOLD_MS + 1).is_none(),
        "preemption hold is bounded in time"
    );
}

#[test]
fn guard_is_keyed_by_state_dir_and_monotonic_only() {
    let dir_a = std::env::temp_dir().join(format!("notchd-w2-guard-a-{}", std::process::id()));
    let dir_b = std::env::temp_dir().join(format!("notchd-w2-guard-b-{}", std::process::id()));
    let a = notchd::supervisor::fleet_guard_for(&dir_a);
    let b = notchd::supervisor::fleet_guard_for(&dir_b);
    assert!(!Arc::ptr_eq(&a, &b), "different state dirs get independent guards");
    assert!(Arc::ptr_eq(&a, &notchd::supervisor::fleet_guard_for(&dir_a)));

    // A wall-clock jump of +/- 1h does not move a monotonic timeline at all:
    // the guard only ever sees the mono_ms the caller hands it.
    let mut g = FleetGuard::new();
    let _ = g.note_start("S1-OC01", T0);
    assert!(
        g.admission("S1-OC01", T0 + 3_600_000).is_none(),
        "a huge mono step is just elapsed time, not a wall-clock correction"
    );
}
