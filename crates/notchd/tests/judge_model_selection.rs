//! Acceptance tests for harness-default judge model + rate-limit free-model swap.
//!
//! RED until the implementation lands. Run:
//!   cargo test -p notchd --test judge_model_selection
//!
//! No real model calls: every external CLI goes through the injectable runner
//! seam, and the tool-script check runs the real script against a temp config.

use notchd::clock::FakeClock;
use notchd::config::Config;
use notchd::daemon::Daemon;
use notchd::decider::{self};
use notchd::herdr::{Herdr, HerdrError};
use notchd::opencode::Opencode;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const T0: u64 = 1_763_520_000_000;
static N: AtomicU64 = AtomicU64::new(0);

fn root(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-judge-model-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn config(r: &Path) -> Config {
    std::fs::create_dir_all(r.join("tmp")).unwrap();
    Config {
        state_dir: r.join("state"),
        tmp_dir: r.join("tmp"),
        stuck_after_secs: 3600,
        poll_interval_ms: 2000,
        opencode_decider: false,
    }
}

// ---------------------------------------------------------------------------
// R1 / R3 — default is the harness default sentinel; explicit model is intact
// ---------------------------------------------------------------------------

#[test]
fn default_model_is_harness_sentinel_not_hy4() {
    assert_eq!(
        decider::DECIDER_MODEL,
        "auto",
        "the judge default must be the harness default sentinel, not a hardcoded model"
    );
    assert!(decider::is_default_model("auto"));
    assert!(decider::is_default_model(""));
    assert!(!decider::is_default_model("opencode/hy4-flash-free"));
}

#[test]
fn auto_omits_model_flag_but_explicit_model_is_kept() {
    let calls = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let sink = Arc::clone(&calls);
    let oc = Opencode::with_runner(move |args: &[String]| {
        sink.lock().unwrap().push(args.to_vec());
        Ok("CONTINUE: keep going".to_string())
    });

    oc.run_decider_in("auto", "prompt", None).unwrap();
    oc.run_decider_session_in("auto", "ses_notch_S1", "prompt", None).unwrap();
    oc.run_decider_in("opencode/hy4-flash-free", "prompt", None).unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    for call in &calls[..2] {
        assert_eq!(call[1], "run");
        assert!(
            !call.iter().any(|a| a == "--model"),
            "harness default must let OpenCode pick the model; argv={call:?}"
        );
        assert!(call.iter().any(|a| a == "--auto"), "full-access flag kept: {call:?}");
    }
    assert!(
        calls[2].iter().any(|a| a == "opencode/hy4-flash-free"),
        "an explicitly supplied model must be used verbatim: {:?}",
        calls[2]
    );
    assert!(
        calls[1].iter().any(|a| a == "ses_notch_S1"),
        "session flag must survive dropping --model: {:?}",
        calls[1]
    );
}

// ---------------------------------------------------------------------------
// R6 — free-model selection
// ---------------------------------------------------------------------------

#[test]
fn free_model_picker_prefers_good_available_models_and_skips_dumb_ones() {
    let available: Vec<String> = [
        "opencode/tidepool-alpha-free",
        "anthropic/claude-opus-5",
        "opencode/hy4-flash-free",
        "openrouter/liquid/lfm-3-1.7b:free",
        "opencode/mimo-v2.6-flash-free",
        "opencode/deepseek-v4.1-flash-free",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    assert!(decider::is_free_model("opencode/hy4-flash-free"));
    assert!(decider::is_free_model("openrouter/google/gemma-4:free"));
    assert!(!decider::is_free_model("anthropic/claude-opus-5"));

    let picked = decider::pick_free_fallback(&available, "auto").unwrap();
    assert_eq!(
        picked, "opencode/deepseek-v4.1-flash-free",
        "preferred good free model must win over a tiny/other free model"
    );

    let picked2 = decider::pick_free_fallback(&available, &picked).unwrap();
    assert_eq!(
        picked2, "opencode/mimo-v2.6-flash-free",
        "the current model must never be re-picked on a repeat rate limit"
    );

    let dumb_only: Vec<String> = ["openrouter/liquid/lfm-3-1.7b:free", "ollama/qwen2.5:0.5b"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        decider::pick_free_fallback(&dumb_only, "auto"),
        None,
        "tiny/toy models are not good-quality fallbacks"
    );
    assert_eq!(decider::pick_free_fallback(&[], "auto"), None);
    assert_eq!(
        decider::pick_free_fallback(&available, "opencode/deepseek-v4.1-flash-free").unwrap(),
        "opencode/mimo-v2.6-flash-free"
    );
}

// ---------------------------------------------------------------------------
// R4 — decider-side 429 swaps to a free model once and persists it
// ---------------------------------------------------------------------------

struct FakeOpencode {
    calls: Mutex<Vec<Vec<String>>>,
    cv: Condvar,
}

impl FakeOpencode {
    fn new() -> Arc<Self> {
        Arc::new(Self { calls: Mutex::new(Vec::new()), cv: Condvar::new() })
    }

    fn client(self: &Arc<Self>, tail_429: bool) -> Opencode {
        let fake = Arc::clone(self);
        Opencode::with_runner(move |args: &[String]| {
            fake.calls.lock().unwrap().push(args.to_vec());
            fake.cv.notify_all();
            match args[1..].join(" ").as_str() {
                "api GET /api/session" => Ok(json!({"data": []}).to_string()),
                "models" => Ok(
                    "opencode/deepseek-v4.1-flash-free\nopencode/hy4-flash-free\nanthropic/claude-opus-5\n"
                        .to_string(),
                ),
                joined if joined.starts_with("run") => {
                    if tail_429 {
                        Ok("CONTINUE: keep working".to_string())
                    } else if fake.calls.lock().unwrap().iter().filter(|c| c.get(1).map(String::as_str) == Some("run")).count() == 1 {
                        Err(HerdrError::Failed {
                            args: args.to_vec(),
                            code: Some(1),
                            stderr: "Error: 429 Too Many Requests: rate limit exceeded".to_string(),
                        })
                    } else {
                        Ok("CONTINUE: keep working and verify with a concrete command".to_string())
                    }
                }
                other => Err(HerdrError::Failed {
                    args: args.to_vec(),
                    code: Some(2),
                    stderr: format!("unexpected opencode call: {other}"),
                }),
            }
        })
    }

    fn run_calls(&self) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.get(1).map(String::as_str) == Some("run"))
            .cloned()
            .collect()
    }

    fn wait_for_runs(&self, n: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut guard = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if guard.iter().filter(|c| c.get(1).map(String::as_str) == Some("run")).count() >= n {
                return true;
            }
            let (g, res) = self
                .cv
                .wait_timeout(guard, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| e.into_inner());
            guard = g;
            if res.timed_out() {
                return false;
            }
        }
    }
}

fn idle_herdr(tail: Arc<Mutex<String>>) -> Herdr {
    Herdr::with_runner(move |args: &[String]| {
        let a = args.get(1).map(String::as_str).unwrap_or("");
        let b = args.get(2).map(String::as_str).unwrap_or("");
        match (a, b) {
            ("pane", "split") => Ok(json!({"result":{"pane":{"pane_id":"wJ:p1"}}}).to_string()),
            ("agent", "list") => Ok(json!({"result":{"agents":[{
                "agent":"opencode", "agent_status":"idle",
                "pane_id":"wJ:p1", "revision":1,
                "terminal_title":"OC | j", "focused":false, "name":"j"
            }]}}).to_string()),
            ("agent", "read") => Ok(tail.lock().unwrap().clone()),
            _ => Ok(r#"{"result":{"type":"ok"}}"#.to_string()),
        }
    })
}

fn submit(r: &Path) -> (Daemon, String) {
    let daemon = Daemon::open(config(r), idle_herdr(Arc::new(Mutex::new("working\n".into()))), Arc::new(FakeClock::new(T0))).unwrap();
    let ack = daemon.submit_task(
        "do the work".into(),
        Some(r.join("work").to_string_lossy().into_owned()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok, "submit failed: {:?}", ack.error);
    (daemon, ack.session_id.unwrap())
}

#[test]
fn decider_rate_limit_swaps_once_to_a_good_free_model_and_persists_it() {
    let r = root("decider-429");
    std::fs::create_dir_all(r.join("work")).unwrap();
    let (daemon, sid) = submit(&r);
    let oc = FakeOpencode::new();
    daemon.set_opencode(oc.client(false));

    daemon.poll_once();
    assert!(oc.wait_for_runs(2), "expected the 429 attempt + one free-model retry");
    daemon.poll_once();

    let calls = oc.run_calls();
    assert_eq!(calls.len(), 2, "exactly one bounded retry after a rate limit: {calls:?}");
    assert!(
        calls[1].iter().any(|a| a == "opencode/deepseek-v4.1-flash-free"),
        "retry must use an available good free model: {:?}",
        calls[1]
    );

    let snap = daemon.snapshot();
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(
        s.model.as_deref(),
        Some("opencode/deepseek-v4.1-flash-free"),
        "the chosen fallback must be persisted so later judgments stop hammering the limited model"
    );
    assert!(
        snap.warnings.iter().any(|w| w.contains("model") || w.contains("429") || w.contains("swap")),
        "a swap must be reported, not silent: {:?}",
        snap.warnings
    );
}

#[test]
fn pane_tail_rate_limit_backs_off_and_swaps_model_without_calling_judge() {
    let r = root("tail-429");
    std::fs::create_dir_all(r.join("work")).unwrap();
    let tail = Arc::new(Mutex::new("working\n".to_string()));
    let daemon = Daemon::open(config(&r), idle_herdr(Arc::clone(&tail)), Arc::new(FakeClock::new(T0))).unwrap();
    let ack = daemon.submit_task(
        "do the work".into(),
        Some(r.join("work").to_string_lossy().into_owned()),
        None,
        None,
        None,
        None,
    );
    assert!(ack.ok, "submit failed: {:?}", ack.error);
    let sid = ack.session_id.unwrap();
    let oc = FakeOpencode::new();
    daemon.set_opencode(oc.client(true));

    *tail.lock().unwrap() = "Error: 429 Too Many Requests: rate limit exceeded\n".to_string();
    daemon.poll_once();

    let snap = daemon.snapshot();
    let s = snap.sessions.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(
        s.model.as_deref(),
        Some("opencode/deepseek-v4.1-flash-free"),
        "a pane-tail 429 must also swap to a good free model: model={:?}",
        s.model
    );
    assert!(oc.run_calls().is_empty(), "rate-limit backoff must not call the decider");
    assert!(
        snap.warnings.iter().any(|w| w.contains("backoff") || w.contains("429")),
        "backoff must be logged: {:?}",
        snap.warnings
    );
}

// ---------------------------------------------------------------------------
// R2 — the tool must never rewrite the operator's OpenCode default
// ---------------------------------------------------------------------------

#[test]
fn default_model_tool_never_rewrites_the_operator_config() {
    let r = root("tool");
    let cfg = r.join("opencode.json");
    let original = "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"model\": \"anthropic/claude-opus-5\"\n}\n";
    std::fs::write(&cfg, original).unwrap();

    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/opencode-default-model.sh");
    let out = std::process::Command::new("sh")
        .arg(script)
        .env("NOTCH_OC_CONFIG", &cfg)
        .env("NOTCH_OC_EXPECT", "skip")
        .output()
        .unwrap();
    assert!(out.status.success(), "tool failed: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        original,
        "notch must never force hy4-flash-free (or any model) into the operator's config"
    );
}
