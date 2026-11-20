//! Clock and watchdog policy regressions, without external harnesses.

use notchd::clock::{Clock, FakeClock, SystemClock};
use notchd::status::Status;
use notchd::supervisor::{decide_on_failure_with_reason, FailureAction};
use notchd::watchdog::{Liveness, Obs, Watchdog};

const T0: u64 = 1_763_520_000_000;

// ---------- Phase 0: Clock::mono_ms ----------

#[test]
fn phase0_mono_is_monotonic_and_shares_one_epoch() {
    let a = SystemClock.mono_ms();
    let b = SystemClock.mono_ms();
    let c = SystemClock.mono_ms();
    assert!(b >= a, "mono_ms must never go backwards: {a} -> {b}");
    assert!(c >= b, "mono_ms must never go backwards: {b} -> {c}");
    assert!(
        c - b < 5_000,
        "separate Clock instances share one OnceLock anchor"
    );
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
    assert_eq!(
        c.mono_ms(),
        T0,
        "mono must not follow a backwards wall jump"
    );

    c.advance_ms(5_000); // real elapsed time
    assert_eq!(
        c.mono_ms(),
        T0 + 5_000,
        "mono advances only with elapsed time"
    );
    assert_eq!(c.now_ms(), T0 - 3_600_000 + 5_000);
}

// ---------- section 3.4: classification ----------

fn obs(quiet_ms: u64) -> Obs {
    Obs {
        status: Status::Working,
        revision_changed_now: false,
        oc_busy: None,
        quiet_for_ms: quiet_ms,
    }
}

#[test]
fn classify_alive_when_not_working_or_revision_changed_or_oc_busy() {
    let w = Watchdog::new(15);
    assert_eq!(
        w.classify(obs(20_000)),
        Liveness::Suspect,
        "sanity: quiet working session"
    );
    assert_eq!(
        w.classify(Obs {
            status: Status::Idle,
            ..obs(60_000)
        }),
        Liveness::Alive
    );
    assert_eq!(
        w.classify(Obs {
            status: Status::Done,
            ..obs(60_000)
        }),
        Liveness::Alive
    );
    assert_eq!(
        w.classify(Obs {
            status: Status::Failed,
            ..obs(60_000)
        }),
        Liveness::Alive
    );
    assert_eq!(
        w.classify(Obs {
            revision_changed_now: true,
            ..obs(60_000)
        }),
        Liveness::Alive,
        "revision changed in THIS poll means the old timestamp is irrelevant"
    );
    assert_eq!(
        w.classify(Obs {
            oc_busy: Some(true),
            ..obs(60_000)
        }),
        Liveness::Alive,
        "oc_busy is a second opinion that rescues a frozen revision"
    );
}

#[test]
fn classify_suspect_at_threshold_stuck_at_double_threshold() {
    let w = Watchdog::new(15);
    assert_eq!(w.classify(obs(14_999)), Liveness::Alive);
    assert_eq!(
        w.classify(obs(15_000)),
        Liveness::Suspect,
        "threshold is inclusive"
    );
    assert_eq!(w.classify(obs(29_999)), Liveness::Suspect);
    assert_eq!(
        w.classify(obs(30_000)),
        Liveness::Stuck,
        "Stuck requires 2x threshold"
    );
    assert_eq!(
        w.classify(obs(u64::MAX)),
        Liveness::Stuck,
        "no overflow panic"
    );
}

#[test]
fn oc_busy_false_is_not_a_rescue() {
    let w = Watchdog::new(15);
    assert_eq!(
        w.classify(Obs {
            oc_busy: Some(false),
            ..obs(60_000)
        }),
        Liveness::Stuck,
        "only Some(true) counts; Some(false) and None are not evidence of life"
    );
}

#[test]
fn pane_close_defaults_on_and_only_for_stuck() {
    let mut w = Watchdog::new(15);
    assert!(
        w.kill_on_stuck(),
        "production default kills verified stuck sessions"
    );
    assert!(w.should_close_pane(Liveness::Stuck));
    assert!(
        !w.should_close_pane(Liveness::Suspect),
        "Suspect is nudge-only"
    );
    assert!(!w.should_close_pane(Liveness::Alive));
    w.set_kill_on_stuck(false);
    assert!(
        !w.should_close_pane(Liveness::Stuck),
        "operators can opt out"
    );
}

#[test]
fn stall_gap_is_strictly_greater_than_three_poll_intervals() {
    assert!(!Watchdog::is_stall_gap(6_000, 2_000));
    assert!(Watchdog::is_stall_gap(6_001, 2_000));
    assert!(
        !Watchdog::is_stall_gap(0, 0),
        "zero interval never trips the guard"
    );
}

#[test]
fn suspect_action_is_once_per_episode_and_reset_on_revision_change() {
    let mut w = Watchdog::new(15);
    assert_eq!(w.note_poll(1_000), None);
    assert_eq!(
        w.note_poll(3_000),
        Some(2_000),
        "gap reported for stall guard"
    );
    assert!(w.take_suspect_action("S1-OC01", 20_000));
    assert!(
        !w.take_suspect_action("S1-OC01", 25_000),
        "no nudge spam every tick"
    );
    w.mark_revision_change("S1-OC01", 30_000);
    assert!(
        w.take_suspect_action("S1-OC01", 50_000),
        "new episode re-arms nudge"
    );
    assert!(w.take_stuck_action("S1-OC01", 70_000));
    assert!(
        !w.take_stuck_action("S1-OC01", 75_000),
        "stuck escalation is once per episode"
    );
}

// ---------- section 3.8: blocked is waiting on a human ----------

#[test]
fn blocked_is_needs_human_on_first_and_later_failures() {
    for count in [0, 1, 7] {
        match decide_on_failure_with_reason(Some("blocked_waiting"), count, Some("tail".into())) {
            FailureAction::NeedsHuman { reason } => {
                assert!(
                    reason.contains("blocked_waiting"),
                    "reason carries the cause: {reason}"
                );
            }
            other => {
                panic!("blocked must never reprompt or plain-escalate (count {count}): {other:?}")
            }
        }
    }
}

#[test]
fn non_blocked_failures_keep_reprompt_then_escalate() {
    assert!(matches!(
        decide_on_failure_with_reason(None, 0, Some("tail".into())),
        FailureAction::Reprompt { .. }
    ));
    assert!(matches!(
        decide_on_failure_with_reason(Some("other_reason"), 0, None),
        FailureAction::Reprompt { .. }
    ));
    assert!(matches!(
        decide_on_failure_with_reason(None, 1, None),
        FailureAction::Escalate
    ));
}
