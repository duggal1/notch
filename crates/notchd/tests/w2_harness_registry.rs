//! W2-1 — definition-driven harness registry (black-box acceptance tests).
//!
//! Stolen from agentbox's `main/harnesses.mts` shape: one adapter per engine,
//! capabilities declared as data, capability negotiation that beats mere
//! method presence, decline-before-side-effect, and unknown-engine rejection.
//! Everything here is headless: no window, no multiplexer, no subprocess.

#[path = "../src/harness_registry.rs"]
mod harness_registry;

use harness_registry::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn fake_def(id: &str, caps: &[HarnessOperation]) -> HarnessDefinition {
    let mut def = HarnessDefinition::new(id, format!("{id} harness"));
    for &op in caps {
        def = def.with_capability(op);
    }
    def
}

fn recording_adapter(def: HarnessDefinition, calls: Arc<AtomicUsize>) -> DelegatingHarness {
    let engine = def.id.clone();
    DelegatingHarness::new(
        def,
        Arc::new(move |req| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Response::Value {
                engine: engine.clone(),
                op: req.op,
                text: format!("{}:{}", req.target, req.text),
            })
        }),
    )
}

#[test]
fn multiple_adapter_kinds_register_and_dispatch_without_cross_talk() {
    let herdr_calls = Arc::new(AtomicUsize::new(0));
    let oc_calls = Arc::new(AtomicUsize::new(0));
    let offline_calls = Arc::new(AtomicUsize::new(0));
    let mut reg = HarnessRegistry::new();
    reg.register(recording_adapter(herdr_definition(), herdr_calls.clone()))
        .unwrap();
    reg.register(recording_adapter(opencode_definition(), oc_calls.clone()))
        .unwrap();
    reg.register(recording_adapter(
        fake_def("offline", &[HarnessOperation::Spawn]),
        offline_calls.clone(),
    ))
    .unwrap();

    let spawned = reg
        .invoke(
            "herdr",
            &Request::new(HarnessOperation::Spawn, "w1:p1", "/tmp/wt"),
        )
        .unwrap();
    assert_eq!(spawned.engine(), "herdr");
    assert_eq!(spawned.op(), HarnessOperation::Spawn);
    assert_eq!(spawned.text(), Some("w1:p1:/tmp/wt"));
    assert_eq!(herdr_calls.load(Ordering::SeqCst), 1);
    assert_eq!(oc_calls.load(Ordering::SeqCst), 0);
    assert_eq!(offline_calls.load(Ordering::SeqCst), 0);

    let models = reg
        .invoke("opencode", &Request::new(HarnessOperation::Models, "", ""))
        .unwrap();
    assert_eq!(models.engine(), "opencode");
    assert_eq!(oc_calls.load(Ordering::SeqCst), 1);
    assert_eq!(herdr_calls.load(Ordering::SeqCst), 1);
    assert_eq!(offline_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn declared_availability_beats_method_presence_and_refuses_before_side_effects() {
    let calls = Arc::new(AtomicUsize::new(0));
    // The adapter CAN invoke every operation, but this engine did not negotiate
    // Kill. Having the method is not the truth (agentbox's ACP lesson).
    let adapter = recording_adapter(herdr_definition(), calls.clone())
        .declining([HarnessOperation::Kill]);
    let mut reg = HarnessRegistry::new();
    reg.register(adapter).unwrap();

    assert!(reg.supports("herdr", HarnessOperation::Prompt));
    assert!(!reg.supports("herdr", HarnessOperation::Kill));
    let err = reg
        .invoke(
            "herdr",
            &Request::new(HarnessOperation::Kill, "w1:p1", ""),
        )
        .unwrap_err();
    assert_eq!(
        err,
        HarnessError::Unsupported {
            engine: "herdr".to_string(),
            op: HarnessOperation::Kill,
        }
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a declined operation must never reach the adapter"
    );

    reg.invoke(
        "herdr",
        &Request::new(HarnessOperation::Prompt, "s1-oc01", "continue"),
    )
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn declined_optional_read_reports_absent_not_a_fake_zero() {
    let calls = Arc::new(AtomicUsize::new(0));
    let adapter = recording_adapter(opencode_definition(), calls.clone())
        .declining([HarnessOperation::Models]);
    let mut reg = HarnessRegistry::new();
    reg.register(adapter).unwrap();

    let result = reg
        .invoke("opencode", &Request::new(HarnessOperation::Models, "", ""))
        .unwrap();
    assert!(!result.supported());
    assert_eq!(
        result,
        Response::Absent {
            engine: "opencode".to_string(),
            op: HarnessOperation::Models,
        }
    );
    assert_eq!(result.text(), None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!reg.supports("opencode", HarnessOperation::Models));
}

#[test]
fn unknown_engine_is_rejected_before_any_side_effect() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = HarnessRegistry::new();
    reg.register(recording_adapter(herdr_definition(), calls.clone()))
        .unwrap();

    for id in ["", "nope", "__proto__", "../herdr", "HERDR", "herdr "] {
        let get_err = reg.get(id).err().unwrap();
        assert_eq!(get_err, HarnessError::UnknownEngine(id.to_string()));
        let invoke_err = reg
            .invoke(id, &Request::new(HarnessOperation::Spawn, "x", "y"))
            .unwrap_err();
        assert_eq!(invoke_err, HarnessError::UnknownEngine(id.to_string()));
        assert!(!reg.supports(id, HarnessOperation::Spawn));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    assert_eq!(
        reg.get_or_default(None).unwrap().definition().id,
        "herdr"
    );
    assert_eq!(
        reg.get_or_default(Some("herdr")).unwrap().definition().id,
        "herdr"
    );
}

#[test]
fn registration_rejects_duplicates_and_invalid_definitions() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = HarnessRegistry::new();
    reg.register(recording_adapter(herdr_definition(), calls.clone()))
        .unwrap();
    assert!(matches!(
        reg.register(recording_adapter(herdr_definition(), calls.clone())),
        Err(HarnessError::DuplicateEngine(_))
    ));

    for (id, label) in [
        ("", "X"),
        ("Herdr", "X"),
        ("1herdr", "X"),
        ("has space", "X"),
        ("herdr!", "X"),
    ] {
        let def = HarnessDefinition::new(id, label).with_capability(HarnessOperation::Status);
        assert!(
            matches!(
                reg.register(recording_adapter(def, calls.clone())),
                Err(HarnessError::InvalidDefinition(_))
            ),
            "definition id {id:?} must be rejected"
        );
    }

    let empty_label =
        HarnessDefinition::new("ok", "   ").with_capability(HarnessOperation::Status);
    assert!(matches!(
        reg.register(recording_adapter(empty_label, calls.clone())),
        Err(HarnessError::InvalidDefinition(_))
    ));

    // An offered engine must declare both a spawn capability and a worker
    // command line as data (agentbox's "remoteControl requires attachInput").
    let mut bad = fake_def("bad", &[HarnessOperation::Read]);
    bad.admitted = true;
    assert!(matches!(
        reg.register(recording_adapter(bad, calls.clone())),
        Err(HarnessError::InvalidDefinition(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn registered_is_not_offered_but_still_resolvable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = HarnessRegistry::new();
    reg.register(recording_adapter(herdr_definition(), calls.clone()))
        .unwrap();
    let mut oc = opencode_definition();
    oc.admitted = false;
    reg.register(recording_adapter(oc, calls.clone())).unwrap();

    let offered: Vec<String> = reg
        .offered_engines()
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(offered, vec!["herdr".to_string()]);
    assert_eq!(reg.definitions().len(), 2);
    assert!(reg.get("opencode").is_ok(), "legacy records must still resolve");
}

#[test]
fn concurrent_dispatch_is_thread_safe() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = HarnessRegistry::new();
    reg.register(recording_adapter(herdr_definition(), calls.clone()))
        .unwrap();
    let reg = Arc::new(reg);

    let mut handles = Vec::new();
    for i in 0..32usize {
        let shared = Arc::clone(&reg);
        handles.push(std::thread::spawn(move || {
            let target = format!("s{i}");
            let req = Request::new(HarnessOperation::Status, &target, "");
            let out = shared.invoke("herdr", &req).unwrap();
            assert_eq!(out.engine(), "herdr");
            assert_eq!(out.op(), HarnessOperation::Status);
            let expected = format!("{target}:");
            assert_eq!(out.text(), Some(expected.as_str()));
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 32);
}

#[test]
fn registry_is_bounded() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = HarnessRegistry::new();
    for i in 0..MAX_ENGINES {
        let def = fake_def(&format!("e{i}"), &[HarnessOperation::Status]);
        reg.register(recording_adapter(def, calls.clone())).unwrap();
    }
    let overflow = fake_def("overflow", &[HarnessOperation::Status]);
    assert!(matches!(
        reg.register(recording_adapter(overflow, calls.clone())),
        Err(HarnessError::RegistryFull)
    ));
    assert_eq!(reg.definitions().len(), MAX_ENGINES);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
