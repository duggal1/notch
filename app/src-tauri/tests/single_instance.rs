//! Single-instance guard: the pure decision fn is what matters; the AppKit
//! activation call site is thin. (NEVER calls focus_existing_instance here —
//! it activates another process.)

use notch::macos::{decide_single_instance, SingleInstanceDecision};

#[test]
fn no_other_instance_proceeds() {
    assert_eq!(
        decide_single_instance(111, &[111]),
        SingleInstanceDecision::Proceed
    );
    assert_eq!(
        decide_single_instance(111, &[]),
        SingleInstanceDecision::Proceed
    );
}

#[test]
fn another_instance_is_focused() {
    assert_eq!(
        decide_single_instance(111, &[111, 222]),
        SingleInstanceDecision::FocusExisting(222)
    );
    // ordering does not matter
    assert_eq!(
        decide_single_instance(111, &[222, 111]),
        SingleInstanceDecision::FocusExisting(222)
    );
    // the other instance alone — e.g. LaunchServices registered it before us
    assert_eq!(
        decide_single_instance(111, &[222]),
        SingleInstanceDecision::FocusExisting(222)
    );
}

#[test]
fn multiple_others_picks_one_deterministically() {
    // first non-self pid in the list — stable given the same input
    assert_eq!(
        decide_single_instance(111, &[111, 222, 333]),
        SingleInstanceDecision::FocusExisting(222)
    );
}
