//! Parser fuzz/flood cases for the NDJSON socket daemon (bridge.md §2).
//! Each test spawns a real `notchd serve` child and talks real bytes.
//! Naming: socket_parser_<case>.

#[path = "fixtures/socket/harness.rs"]
mod harness;

use harness::*;
use serde_json::json;
use std::time::Duration;

#[test]
fn socket_parser_empty_lines() {
    let d = TestDaemon::start("parse-empty");
    let mut c = TestConn::connect_with_timeouts(&d.sock, Duration::from_millis(400), Duration::from_secs(5));
    let _hello = c.hello();
    // Four bare newlines: the server must skip them silently (bridge.md §2 —
    // empty lines are not frames). Any reply here would mean extra acks.
    c.send_raw(b"\n\n\n\n\n");
    // A ping on the same connection must be the FIRST response we read.
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_whitespace_only() {
    let d = TestDaemon::start("parse-ws");
    let mut c = TestConn::connect_with_timeouts(&d.sock, Duration::from_millis(400), Duration::from_secs(5));
    let _hello = c.hello();
    c.send_raw(b"   \n\t\n \t \n\r\n");
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_bare_brace() {
    let d = TestDaemon::start("parse-brace");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    c.send_line("{");
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "invalid_json");
    // Connection stays up for the next command.
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_truncated_cmd() {
    let d = TestDaemon::start("parse-trunc");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    c.send_line(r#"{"cmd":"#);
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "invalid_json");
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_split_frame_byte_trickle() {
    let d = TestDaemon::start("parse-trickle");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // One byte at a time, 2 ms apart — the server must reassemble the frame.
    for b in r#"{"cmd":"ping"}"#.bytes() {
        c.send_raw(&[b]);
        std::thread::sleep(Duration::from_millis(2));
    }
    // Terminate the frame.
    c.send_raw(b"\n");
    let ack = c.read_value();
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
    assert!(ack["pong"].as_u64().is_some());
}

#[test]
fn socket_parser_split_frame_chunked_8kb() {
    let d = TestDaemon::start("parse-8k");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // A >40 KB valid frame written in 8192-byte writes.
    let pad = "B".repeat(40_000);
    let frame = format!(r#"{{"cmd":"ping","pad":"{pad}"}}"#);
    for chunk in frame.as_bytes().chunks(8192) {
        c.send_raw(chunk);
    }
    c.send_raw(b"\n");
    let ack = c.read_value();
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_one_big_flush_large_frame() {
    let d = TestDaemon::start("parse-bigflush");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // 200 KB frame in a single write — one big flush vs the byte trickle.
    let pad = "A".repeat(200_000);
    let frame = format!(r#"{{"cmd":"ping","pad":"{pad}"}}"#);
    c.send_raw(frame.as_bytes());
    c.send_raw(b"\n");
    let ack = c.read_value();
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_single_line_5mb() {
    let d = TestDaemon::start("parse-5mb");
    let mut c = TestConn::connect_with_timeouts(&d.sock, Duration::from_secs(10), Duration::from_secs(60));
    let _hello = c.hello();
    // A single NDJSON frame just over 5 MB — well beyond the daemon's
    // MAX_NDJSON_LINE (1 MiB, socket.rs). Under the hardening the capped
    // reader drains the overflow and DROPS this connection without ever
    // buffering the whole frame. Intent preserved: fuzz robustness — no
    // crash, no wedge, and the daemon still answers a ping afterwards.
    let pad = "x".repeat(5 * 1024 * 1024);
    let frame = format!(r#"{{"cmd":"ping","pad":"{pad}"}}"#);
    assert!(frame.len() > 5 * 1024 * 1024);
    c.send_raw(frame.as_bytes());
    c.send_raw(b"\n");
    // Oversize line: no ack for it; the server closes this connection.
    let buf = c.read_to_end();
    assert!(buf.is_empty(), "over-cap line must be dropped without a reply, got {buf:?}");
    // A fresh connection still works — the daemon did not wedge or crash.
    let mut fresh = TestConn::connect(&d.sock);
    let _hello = fresh.hello();
    let ack = fresh.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
    assert!(ack["pong"].as_u64().is_some());
}

#[test]
fn socket_parser_crlf_line_endings() {
    let d = TestDaemon::start("parse-crlf");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    c.send_raw(b"{\"cmd\":\"ping\"}\r\n");
    let ack = c.read_value();
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_bom_prefix() {
    let d = TestDaemon::start("parse-bom");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // UTF-8 BOM before the frame. serde_json does not accept a BOM, so the
    // ACTUAL behavior is: ack ok:false / invalid_json, connection stays open.
    c.send_raw(b"\xEF\xBB\xBF{\"cmd\":\"ping\"}\n");
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "invalid_json");
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_nul_bytes() {
    let d = TestDaemon::start("parse-nul");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // Raw NUL (0x00, valid UTF-8) after the JSON value: serde_json rejects
    // trailing content -> invalid_json.
    c.send_raw(b"{\"cmd\":\"ping\"}\x00\n");
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "invalid_json");
    // Raw NUL inside a JSON string: JSON forbids unescaped control chars.
    c.send_raw(b"{\"cmd\":\"pi\x00ng\"}\n");
    let ack = c.read_value();
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "invalid_json");
    // The connection is still usable afterwards.
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_high_bytes_0xff() {
    let d = TestDaemon::start("parse-ff");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // 0xFF 0xFE are not valid UTF-8. Under the hardening the capped reader
    // passes bytes through String::from_utf8_lossy (socket.rs), mapping them
    // to U+FFFD: the frame stays valid NDJSON/JSON but no longer names a real
    // command. The daemon rejects it CLEANLY (ok:false / unknown_command),
    // keeps the connection open and never crashes. Intent preserved: bad bytes
    // must not be accepted as a command and must not wedge the daemon.
    c.send_raw(b"{\"cmd\":\"pi\xFF\xFEng\"}\n");
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "unknown_command");
    assert_ne!(ack["cmd"], "ping", "invalid bytes must not be accepted as a valid ping");
    // The same connection is still usable — invalid bytes did not wedge it.
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
    assert!(ack["pong"].as_u64().is_some());
    // A fresh connection still works — the daemon did not crash.
    let mut fresh = TestConn::connect(&d.sock);
    let _hello = fresh.hello();
    let ack = fresh.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_deep_nesting_100k_braces() {
    let d = TestDaemon::start("parse-deep");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // 100_000 levels of nesting. serde_json's default recursion limit (128)
    // must turn this into a parse error, never a stack overflow/crash.
    let mut line = "[".repeat(100_000);
    line.push_str(&"]".repeat(100_000));
    c.send_line(&line);
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "invalid_json");
    // Nesting inside an otherwise valid envelope.
    let mut line = String::from(r#"{"cmd":"ping","x":"#);
    line.push_str(&"[".repeat(100_000));
    line.push_str(&"]".repeat(100_000));
    line.push('}');
    c.send_line(&line);
    let ack = c.read_value();
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "invalid_json");
    // Daemon still healthy.
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_duplicate_keys() {
    let d = TestDaemon::start("parse-dup");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // serde_json keeps the LAST occurrence of a duplicate key.
    c.send_line(r#"{"cmd":"bogus_dup","cmd":"ping"}"#);
    let ack = c.read_value();
    assert_eq!(ack["cmd"], "ping");
    assert_eq!(ack["ok"], true);
    c.send_line(r#"{"cmd":"ping","cmd":"bogus_dup"}"#);
    let ack = c.read_value();
    assert_eq!(ack["cmd"], "bogus_dup");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "unknown_command");
}

#[test]
fn socket_parser_cmd_missing_or_wrong_type() {
    let d = TestDaemon::start("parse-missing");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    let cases = [
        (r#"{}"#, "missing_cmd"),
        (r#"{"cmd":null}"#, "missing_cmd"),
        (r#"{"cmd":5}"#, "missing_cmd"),
        (r#"{"cmd":true}"#, "missing_cmd"),
        (r#"{"cmd":[1,2]}"#, "missing_cmd"),
        (r#"{"cmd":{"x":1}}"#, "missing_cmd"),
        (r#"{"cmdd":"ping"}"#, "missing_cmd"),
        (r#"[1,2,3]"#, "missing_cmd"),
        (r#""just a string""#, "missing_cmd"),
        (r#"42"#, "missing_cmd"),
        (r#"null"#, "missing_cmd"),
        (r#"true"#, "missing_cmd"),
    ];
    for (line, err) in cases {
        c.send_line(line);
        let ack = c.read_value();
        assert_eq!(ack["event"], "ack", "line {line}");
        assert_eq!(ack["ok"], false, "line {line}");
        assert_eq!(ack["error"], err, "line {line}");
    }
    // Still healthy at the end.
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_cmd_unknown() {
    let d = TestDaemon::start("parse-unknown");
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    c.send_line(r#"{"cmd":"frobnicate"}"#);
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["cmd"], "frobnicate");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "unknown_command");
}

#[test]
fn socket_parser_kill_id_wrong_types() {
    let d = TestDaemon::start("parse-killtypes");
    let _ = d.run_plan();
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // kill with id of every non-string JSON type: `as_str()` fails, the id
    // becomes "", the daemon answers ok:false / unknown_session.
    let cases = [
        r#"{"cmd":"kill","id":42}"#,
        r#"{"cmd":"kill","id":true}"#,
        r#"{"cmd":"kill","id":false}"#,
        r#"{"cmd":"kill","id":{"pane":"w90:p7"}}"#,
        r#"{"cmd":"kill","id":["S1-OC01"]}"#,
        r#"{"cmd":"kill","id":null}"#,
        r#"{"cmd":"kill"}"#,
        r#"{"cmd":"kill","id":""}"#,
    ];
    for line in cases {
        c.send_line(line);
        let ack = c.read_value();
        assert_eq!(ack["cmd"], "kill", "line {line}");
        assert_eq!(ack["ok"], false, "line {line}");
        assert_eq!(ack["error"], "unknown_session", "line {line}");
    }
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_kill_id_escaped_nul() {
    let d = TestDaemon::start("parse-killnul");
    let _ = d.run_plan();
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // ESCAPED NUL: the JSON string parses fine to "S1-OC01\0" — a valid NUL in
    // a Rust String. Lookup by exact id fails -> unknown_session, never a crash.
    c.send_line(r#"{"cmd":"kill","id":"S1-OC01\u0000"}"#);
    let ack = c.read_value();
    assert_eq!(ack["cmd"], "kill");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "unknown_session");
    let ack = c.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_kill_id_raw_nul_control_char() {
    let d = TestDaemon::start("parse-killrawnul");
    let _ = d.run_plan();
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    // RAW NUL byte inside the JSON string: JSON forbids unescaped control
    // characters, serde_json rejects the frame -> invalid_json.
    c.send_raw(b"{\"cmd\":\"kill\",\"id\":\"S1-OC01\x00\"}\n");
    let ack = c.read_value();
    assert_eq!(ack["event"], "ack");
    assert_eq!(ack["ok"], false);
    assert_eq!(ack["error"], "invalid_json");
    // RAW 0xFF byte inside the id: the capped reader replaces it with U+FFFD
    // (from_utf8_lossy), so the line stays valid JSON and the id lookup simply
    // misses. No crash/wedge; a fresh connection still pings below.
    c.send_raw(b"{\"cmd\":\"kill\",\"id\":\"S1-OC01\xFF\"}\n");
    let mut fresh = TestConn::connect(&d.sock);
    let _hello = fresh.hello();
    let ack = fresh.rpc(&json!({"cmd":"ping"}));
    assert_eq!(ack["ok"], true);
}

#[test]
fn socket_parser_emoji_title() {
    let d = TestDaemon::start("parse-emoji");
    let _ = d.run_plan();
    let mut c = TestConn::connect(&d.sock);
    let _hello = c.hello();
    let title = "🚀 polish the onboarding flow 🎨 — ⌘⇧K";
    let ack = c.rpc(&json!({
        "cmd": "spawn",
        "supervisor": "SV-1",
        "prompt": "prompt with emoji 🎉 inside",
        "cwd": d.root,
        "title": title,
    }));
    assert_eq!(ack["ok"], true, "spawn failed: {ack}");
    let id = ack["id"].as_str().unwrap().to_string();
    // The snapshot must carry the emoji title verbatim.
    let snap = d.snapshot();
    let sessions = snap["snapshot"]["sessions"].as_array().unwrap().clone();
    let s = sessions.iter().find(|s| s["id"] == id).expect("spawned session in snapshot");
    assert_eq!(s["title"].as_str().unwrap(), title);
    assert_eq!(s["task"].as_str().unwrap(), "prompt with emoji 🎉 inside");
}
