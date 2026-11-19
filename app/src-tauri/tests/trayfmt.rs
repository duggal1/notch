//! Tray title/tooltip formatting — N ∈ {0,1,7,12} × failed counts.
//! The counting helper reads the snapshot as JSON, exactly as the emit
//! boundary delivers it.

use serde_json::json;

use notch::trayfmt::{menu_inflight_label, status_from_value, tooltip, tray_title, TrayStatus};

fn snapshot_value(statuses: &[&str]) -> serde_json::Value {
    let sessions: Vec<_> = statuses
        .iter()
        .enumerate()
        .map(|(i, s)| json!({
            "id": format!("S1-OC{:02}", i + 1),
            "sup": "SV-1",
            "status": s,
            "title": "t",
            "task": "t",
            "cwd": "/tmp",
            "paneId": format!("w1:p{}", i + 1),
            "startedAt": 1_763_520_000_000u64,
            "lastAction": null,
            "lastRevision": null,
            "latencyMs": null,
            "failureReason": if *s == "failed" { json!("stuck") } else { serde_json::Value::Null },
        }))
        .collect();
    json!({
        "version": 1,
        "generatedAt": 1_763_520_000_000u64,
        "orchestrator": { "id": "ORCH", "status": "working" },
        "supervisors": [],
        "sessions": sessions,
        "eps": 0.0,
        "conflicts": [],
        "attribution": [],
        "warnings": [],
    })
}

#[test]
fn tray_title_counts() {
    for n in [0usize, 1, 7, 12] {
        let statuses = vec!["working"; n];
        let st = status_from_value(&snapshot_value(&statuses));
        match n {
            0 => assert_eq!(tray_title(&st), None, "idle fleet must not render a count"),
            other => assert_eq!(
                tray_title(&st).as_deref(),
                Some(format!("‹{other}›").as_str())
            ),
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
        let mut statuses = vec!["working"; *in_flight];
        statuses.extend(vec!["failed"; *failed]);
        let st = TrayStatus {
            in_flight: *in_flight,
            failed: *failed,
        };
        assert_eq!(status_from_value(&snapshot_value(&statuses)), st);
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
        assert_eq!(
            menu_inflight_label(&TrayStatus {
                in_flight: *in_flight,
                failed: 0
            }),
            *expect
        );
    }
}

#[test]
fn failed_only_sessions_show_zero_inflight_but_failures() {
    let statuses = vec!["failed", "failed", "done", "idle"];
    let st = status_from_value(&snapshot_value(&statuses));
    assert_eq!(
        st,
        TrayStatus {
            in_flight: 0,
            failed: 2
        }
    );
    assert_eq!(tray_title(&st), None);
    assert_eq!(tooltip(&st), "notch — 0 in flight · 2 failed");
}

#[test]
fn malformed_snapshot_counts_zero() {
    // Never panic on garbage at the boundary.
    assert_eq!(
        status_from_value(&json!(null)),
        TrayStatus {
            in_flight: 0,
            failed: 0
        }
    );
    assert_eq!(
        status_from_value(&json!({})),
        TrayStatus {
            in_flight: 0,
            failed: 0
        }
    );
    assert_eq!(
        status_from_value(&json!({"sessions": "not-an-array"})),
        TrayStatus {
            in_flight: 0,
            failed: 0
        }
    );
}
