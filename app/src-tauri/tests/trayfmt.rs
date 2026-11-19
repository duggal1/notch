//! Tray title/tooltip formatting — N ∈ {0,1,7,12} × failed counts.

use notch::snapshot::{Orchestrator, Snapshot, Status, SNAPSHOT_VERSION};
use notch::trayfmt::{menu_inflight_label, status_from_snapshot, tooltip, tray_title, TrayStatus};

fn snapshot_with(statuses: &[Status]) -> Snapshot {
    let mut snap = Snapshot::empty(1_763_520_000_000);
    snap.orchestrator = Orchestrator { id: "ORCH".into(), status: Status::Working };
    snap.version = SNAPSHOT_VERSION;
    snap.sessions = statuses
        .iter()
        .enumerate()
        .map(|(i, s)| notch::snapshot::Session {
            id: format!("S1-OC{:02}", i + 1),
            sup: "SV-1".into(),
            status: *s,
            title: "t".into(),
            task: "t".into(),
            cwd: "/tmp".into(),
            pane_id: format!("w1:p{}", i + 1),
            started_at: 1_763_520_000_000,
            last_action: String::new(),
            last_revision: 0,
            latency_ms: None,
            failure_reason: None,
        })
        .collect();
    snap
}

#[test]
fn tray_title_counts() {
    for n in [0usize, 1, 7, 12] {
        let statuses = vec![Status::Working; n];
        let st = status_from_snapshot(&snapshot_with(&statuses));
        match n {
            0 => assert_eq!(tray_title(&st), None, "idle fleet must not render a count"),
            other => assert_eq!(tray_title(&st).as_deref(), Some(format!("‹{other}›").as_str())),
        }
    }
}

#[test]
fn tooltip_counts_and_failed_suffix() {
    let cases: &[(usize, usize, &str)] = &[
        (0, 0, "notch — 0 in flight"),
        (1, 0, "notch — 1 in flight"),
        (7, 0, "notch — 7 in flight"),
        (12, 0, "notch — 12 in flight"),
        (1, 1, "notch — 1 in flight · 1 failed"),
        (7, 1, "notch — 7 in flight · 1 failed"),
        (12, 3, "notch — 12 in flight · 3 failed"),
        (0, 2, "notch — 0 in flight · 2 failed"),
    ];
    for (in_flight, failed, expect) in cases {
        let mut statuses = vec![Status::Working; *in_flight];
        statuses.extend(vec![Status::Failed; *failed]);
        let st = TrayStatus { in_flight: *in_flight, failed: *failed };
        assert_eq!(status_from_snapshot(&snapshot_with(&statuses)), st);
        assert_eq!(&tooltip(&st), expect, "tooltip for {in_flight}/{failed}");
    }
}

#[test]
fn menu_inflight_label_counts() {
    let cases: &[(usize, &str)] = &[
        (0, "Nothing in flight"),
        (1, "1 in flight"),
        (7, "7 in flight"),
        (12, "12 in flight"),
    ];
    for (in_flight, expect) in cases {
        assert_eq!(menu_inflight_label(&TrayStatus { in_flight: *in_flight, failed: 0 }), *expect);
    }
}

#[test]
fn failed_only_sessions_show_zero_inflight_but_failures() {
    let statuses = vec![Status::Failed, Status::Failed, Status::Done, Status::Idle];
    let st = status_from_snapshot(&snapshot_with(&statuses));
    assert_eq!(st, TrayStatus { in_flight: 0, failed: 2 });
    assert_eq!(tray_title(&st), None);
    assert_eq!(tooltip(&st), "notch — 0 in flight · 2 failed");
}
