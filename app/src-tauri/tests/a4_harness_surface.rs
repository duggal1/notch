//! A4 REAL regression test for B3: `list_harnesses` must report the registry's
//! truth — herdr is the sole admitted/spawn-capable engine; opencode declares
//! Status + Models only and cannot create a managed pane.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll};

fn block_on<F: Future>(f: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut f = pin!(f);
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("future unexpectedly pending"),
    }
}

fn supported(h: &serde_json::Value, name: &str) -> bool {
    h["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["name"] == name && c["supported"] == true)
}

#[test]
fn list_harnesses_reports_registry_truth() {
    let out = block_on(notch::commands::list_harnesses()).unwrap();
    assert_eq!(out["source"], "registry");
    let hs = out["harnesses"].as_array().unwrap();

    let herdr = hs
        .iter()
        .find(|h| h["id"] == "herdr")
        .expect("herdr must be listed as the spawn engine");
    let opencode = hs
        .iter()
        .find(|h| h["id"] == "opencode")
        .expect("opencode stays resolvable for legacy records");

    assert!(
        supported(herdr, "spawn"),
        "herdr is the sole spawn-capable engine"
    );
    assert!(
        !supported(herdr, "models"),
        "herdr has no optional Models read"
    );
    assert!(
        !supported(opencode, "spawn"),
        "opencode cannot create a managed pane"
    );
    assert!(supported(opencode, "status") && supported(opencode, "models"));
    for action in ["read", "prompt", "kill", "focus"] {
        assert!(!supported(opencode, action), "opencode has no {action}");
    }
}
