//! Unknown-command / garbage storm: one socket, 10 000 mixed random garbage
//! lines. The daemon must error-ack each (behavior for valid-UTF-8 garbage),
//! keep its memory sane, and serve a valid ping afterwards.
//! Naming: socket_storm_<case>.

#[path = "fixtures/socket/harness.rs"]
mod harness;

use harness::*;
use serde_json::json;
use std::time::Instant;

/// Deterministic xorshift64 so the storm is reproducible byte-for-byte.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn alnum(&mut self, len: usize) -> String {
        const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        (0..len)
            .map(|_| A[self.next() as usize % 36] as char)
            .collect()
    }
    fn digits(&mut self, len: usize) -> String {
        (0..len)
            .map(|_| (b'0' + (self.next() % 10) as u8) as char)
            .collect()
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn len_between(&mut self, base: usize, extra: u64) -> usize {
        base + self.below(extra) as usize
    }
}

/// Garbage templates — EVERY one is guaranteed to produce ack ok:false from
/// this server (verified against the socket command dispatcher):
///   unknown cmd / missing cmd / invalid JSON / wrong-typed cmd values.
fn garbage_line(rng: &mut Rng) -> String {
    match rng.below(12) {
        // 1. unknown (but well-formed) command
        0 => {
            let n = rng.len_between(4, 9);
            format!(r#"{{"cmd":"zz{}"}}"#, rng.alnum(n))
        }
        // 2. junk braces, not JSON at all
        1 => {
            let n = rng.len_between(1, 21);
            format!("{{{}}}", rng.alnum(n))
        }
        // 3. bare array of numbers -> missing_cmd
        2 => {
            let n = 1 + rng.below(5);
            let mut items = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let d = rng.len_between(1, 7);
                items.push(rng.digits(d));
            }
            format!("[{}]", items.join(","))
        }
        // 4. bare number -> missing_cmd
        3 => {
            let n = rng.len_between(1, 13);
            rng.digits(n)
        }
        // 5. bare unquoted word -> invalid_json
        4 => {
            let n = rng.len_between(1, 21);
            rng.alnum(n)
        }
        // 6. cmd with numeric value -> missing_cmd
        5 => {
            let n = rng.len_between(1, 7);
            format!(r#"{{"cmd":{}}}"#, rng.digits(n))
        }
        // 7. valid JSON string literal -> missing_cmd
        6 => {
            let n = rng.len_between(1, 16);
            format!(r#""{}""#, rng.alnum(n))
        }
        // 8. well-formed JSON with unknown cmd and a nested junk value
        7 => {
            let n = rng.len_between(4, 7);
            format!(r#"{{"cmd":"zz{}","n":[[[1]]]}}"#, rng.alnum(n))
        }
        // 9. case-mangled cmd key -> missing_cmd (parsing is case-sensitive)
        8 => r#"{"CMD":"ping"}"#.to_string(),
        // 10. cmd key with trailing space -> missing_cmd
        9 => r#"{"cmd ":"ping"}"#.to_string(),
        // 11. deeply nested arrays -> missing_cmd (well under the 128 limit)
        10 => {
            let depth = rng.len_between(1, 7);
            format!("{}1{}", "[".repeat(depth), "]".repeat(depth))
        }
        // 12. JSON booleans/null literal -> missing_cmd
        11 => ["null", "true", "false"][rng.below(3) as usize].to_string(),
        _ => unreachable!(),
    }
}

#[test]
fn socket_storm_10k_garbage() {
    let d = TestDaemon::start("storm-10k");
    let mut c = TestConn::connect_with_timeouts(
        &d.sock,
        std::time::Duration::from_secs(15),
        std::time::Duration::from_secs(60),
    );
    let _hello = c.hello();

    let rss_before = d.rss_kb();

    // Build the whole storm (~10k small lines) as one payload.
    let mut rng = Rng(0x5EED_CAFE_1234_5678);
    let mut payload = String::with_capacity(1 << 20);
    for _ in 0..10_000 {
        payload.push_str(&garbage_line(&mut rng));
        payload.push('\n');
    }

    let start = Instant::now();
    c.send_raw(payload.as_bytes());
    let mut acks = 0u64;
    let mut ok_true = 0u64;
    let mut ok_false = 0u64;
    let mut errors: Vec<String> = Vec::new();
    for _ in 0..10_000 {
        match c.read_line_result() {
            Ok(l) => {
                acks += 1;
                match serde_json::from_str::<serde_json::Value>(l.trim_end()) {
                    Ok(v) => {
                        if v["ok"].as_bool() == Some(true) {
                            ok_true += 1;
                            errors.push(format!("unexpected ok:true ack: {}", l.trim_end()));
                        } else {
                            ok_false += 1;
                        }
                    }
                    Err(_) => errors.push(format!("ack line was not JSON: {l:?}")),
                }
            }
            Err(e) => {
                panic!(
                    "server stopped answering after {acks} acks ({e}); the connection terminated early on valid-UTF-8 garbage — this is the failure mode the task says to report, not hide"
                );
            }
        }
    }
    let wall = start.elapsed();

    let rss_before = rss_before.unwrap_or(0);
    let rss_after = d.rss_kb().unwrap_or(0);
    let rss_growth = rss_after.saturating_sub(rss_before);
    eprintln!(
        "socket_storm_10k_garbage: 10_000 garbage lines — {ok_false} error-acks, {ok_true} unexpected ok:true, wall {wall:?}, RSS {rss_before} KiB -> {rss_after} KiB (growth {rss_growth} KiB)"
    );
    assert_eq!(acks, 10_000, "must receive one ack per garbage line");
    assert_eq!(ok_true, 0, "no garbage line may be accepted: {errors:?}");
    assert_eq!(ok_false, 10_000);
    assert!(errors.is_empty(), "malformed ack lines: {errors:?}");
    assert!(
        rss_growth < 65_536,
        "daemon RSS grew by {rss_growth} KiB during the storm — memory is not sane"
    );
    assert!(
        wall < std::time::Duration::from_secs(30),
        "storm took {wall:?}"
    );

    // A valid ping still works afterwards.
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
    assert!(ack["pong"].as_u64().is_some());
    // And the daemon still serves a snapshot over a fresh connection.
    let snap = d.snapshot();
    assert_eq!(snap["event"], "snapshot");
    assert_eq!(snap["snapshot"]["version"], 1);
}
