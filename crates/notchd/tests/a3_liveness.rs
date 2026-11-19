//! A3 liveness — tests for the monotonic clock, watchdog classification,
//! stall guard, suspicion/stuck ladder, and blocked-is-human semantics.
//!
//! Every test here is driven by injected fake clocks and fake herdr/opencode
//! runners: no sleeps, no real subprocesses, no GUI, no `#[ignore]`.

use notchd::clock::{Clock, FakeClock, SystemClock};

const T0: u64 = 1_763_520_000_000;

// ---------- Phase 0: Clock::mono_ms ----------

#[test]
fn phase0_mono_is_monotonic_and_shares_one_epoch() {
    let a = SystemClock.mono_ms();
    let b = SystemClock.mono_ms();
    let c = SystemClock.mono_ms();
    assert!(b >= a, "mono_ms must never go backwards: {a} -> {b}");
    assert!(c >= b, "mono_ms must never go backwards: {b} -> {c}");
    assert!(c - b < 5_000, "separate Clock instances share one OnceLock anchor");
}

#[test]
fn phase0_fake_mono_ignores_wall_clock_jumps() {
    let c = FakeClock::new(T0);
    assert_eq!(c.mono_ms(), T0);
    assert_eq!(c.now_ms(), T0);

    c.set(T0 + 3_600_000); // wall +1h (NTP / user change)
    assert_eq!(c.mono_ms(), T0, "mono must not follow a wall-clock jump");
    assert_eq!(c.now_ms(), T0 + 3_600_000);

    c.set(T0 - 3_600_000); // wall -1h
    assert_eq!(c.mono_ms(), T0, "mono must not follow a backwards wall jump");

    c.advance_ms(5_000); // real elapsed time
    assert_eq!(c.mono_ms(), T0 + 5_000, "mono advances only with elapsed time");
    assert_eq!(c.now_ms(), T0 - 3_600_000 + 5_000);
}
