//! W2-1 — worker command lines are data, and no harness can create a window.
//!
//! Headless companion to `w2_harness_registry.rs`. The registry never spawns a
//! process: every command line is built from a definition's `RunFlags`, and an
//! engine that did not declare pane capability cannot reach a spawn operation.

#[path = "../src/harness_registry.rs"]
mod harness_registry;

use harness_registry::*;
use std::sync::{Arc, Mutex};

#[test]
fn absent_flags_are_omitted_and_flag_order_is_stable() {
    let run = RunFlags {
        prefix: vec!["run".to_string()],
        pre_payload: vec![
            "--model".to_string(),
            "hy4-flash-free".to_string(),
            "--auto".to_string(),
        ],
        post_payload: Vec::new(),
    };
    let req = Request::new(HarnessOperation::Spawn, "", "do work");
    assert_eq!(
        worker_args(&run, &req),
        vec!["run", "--model", "hy4-flash-free", "--auto", "do work"]
    );

    // An engine with no flags is handed a bare command line, never someone
    // else's flag names (agentbox's "absent means cannot, not default").
    let bare = RunFlags {
        prefix: vec!["pane".to_string(), "split".to_string()],
        pre_payload: Vec::new(),
        post_payload: Vec::new(),
    };
    assert_eq!(worker_args(&bare, &req), vec!["pane", "split", "do work"]);
}

#[test]
fn only_declared_multiplexers_can_build_a_worker_command() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&calls);
    let mut reg = HarnessRegistry::new();
    let def = herdr_definition();
    reg.register(DelegatingHarness::new(
        def,
        Arc::new(move |req| {
            recorder.lock().unwrap().push(req.clone());
            Ok(Response::Value {
                engine: "herdr".to_string(),
                op: req.op,
                text: String::new(),
            })
        }),
    ))
    .unwrap();
    reg.register(DelegatingHarness::new(
        opencode_definition(),
        Arc::new(|_| panic!("opencode has no pane capability; must never be invoked")),
    ))
    .unwrap();

    let req = Request::new(HarnessOperation::Spawn, "", "/tmp/wt");
    let argv = reg.worker_argv("herdr", &req).unwrap().unwrap();
    assert_eq!(
        argv,
        vec!["pane", "split", "--direction", "right", "--cwd", "/tmp/wt"]
    );
    assert!(matches!(
        reg.worker_argv("opencode", &req),
        Err(HarnessError::Unsupported { .. })
    ));
    assert!(
        calls.lock().unwrap().is_empty(),
        "building argv must never touch an adapter"
    );
}

#[test]
fn no_operation_can_create_a_window_or_foreign_multiplexer() {
    let seen: Arc<Mutex<Vec<Request>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let mut reg = HarnessRegistry::new();
    reg.register(DelegatingHarness::new(
        herdr_definition(),
        Arc::new(move |req| {
            recorder.lock().unwrap().push(req.clone());
            Ok(Response::Value {
                engine: "herdr".to_string(),
                op: req.op,
                text: String::new(),
            })
        }),
    ))
    .unwrap();

    let ops = [
        HarnessOperation::Spawn,
        HarnessOperation::Prompt,
        HarnessOperation::Read,
        HarnessOperation::Focus,
        HarnessOperation::Kill,
        HarnessOperation::Status,
        HarnessOperation::Wait,
    ];
    for &op in &ops {
        reg.invoke(
            "herdr",
            &Request::new(op, "s1-oc01", "w1:p1 --direction right"),
        )
        .unwrap();
    }

    let recorded = seen.lock().unwrap().clone();
    assert_eq!(recorded.len(), ops.len());
    let definition = herdr_definition();
    for req in &recorded {
        assert!(
            definition.supports(req.op),
            "undeclared operation reached the adapter"
        );
        assert_eq!(req.target, "s1-oc01");
        for field in [&req.target, &req.text] {
            assert!(!field.contains("osascript"), "no GUI scripting");
            assert!(!field.contains("workspace create"), "no new workspace");
            assert!(!field.contains("--window"), "no new window");
            assert!(!field.contains("open -a"), "no app launch");
        }
    }
}

#[test]
fn models_is_an_optional_read_while_spawn_is_a_required_action() {
    // An engine that declares no Models capability still answers a model query
    // with Absent (not an empty list), and an engine with no Spawn capability
    // refuses spawn before any adapter is touched.
    let mut reg = HarnessRegistry::new();
    reg.register(DelegatingHarness::new(
        herdr_definition(),
        Arc::new(|_| panic!("must not be invoked")),
    ))
    .unwrap();

    let absent = reg
        .invoke("herdr", &Request::new(HarnessOperation::Models, "", ""))
        .unwrap();
    assert_eq!(
        absent,
        Response::Absent {
            engine: "herdr".to_string(),
            op: HarnessOperation::Models,
        }
    );
    assert!(matches!(
        reg.invoke(
            "opencode",
            &Request::new(HarnessOperation::Kill, "w1:p1", "")
        ),
        Err(HarnessError::UnknownEngine(_))
    ));
}

#[test]
fn engine_ids_are_validated_as_literal_lowercase_names() {
    assert!(is_valid_engine_id("herdr"));
    assert!(is_valid_engine_id("open-code"));
    assert!(is_valid_engine_id("a"));
    for bad in [
        "",
        "Herdr",
        "1herdr",
        "-herdr",
        "herdr!",
        "has space",
        "../etc",
    ] {
        assert!(!is_valid_engine_id(bad), "{bad:?} must be rejected");
    }
    let long = "a".repeat(crate::harness_registry::MAX_ENGINE_ID_LEN + 1);
    assert!(!is_valid_engine_id(&long));
}
