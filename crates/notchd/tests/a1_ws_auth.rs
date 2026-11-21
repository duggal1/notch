//! A1 / 3.7 + T8: WebSocket auth and NDJSON cap RED tests.
//!
//! RED FIRST: these fail until ws.rs/socket.rs expose the auth primitives.
//! Run: cargo test -p notchd --test a1_ws_auth

use notchd::socket::read_line_capped;
use notchd::ws::{auth_headers_ok, constant_time_eq, load_or_create_token, ws_enabled};
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);
fn tmp_dir() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("notchd-a1-ws-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const HOST: &str = "127.0.0.1:7407";

#[test]
fn constant_time_eq_is_exact_including_length() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    assert!(!constant_time_eq(b"ab", b"abc"));
    assert!(constant_time_eq(b"", b""));
    let expected = TOKEN.as_bytes();
    let mut wrong_last = expected.to_vec();
    *wrong_last.last_mut().unwrap() ^= 1;
    assert!(!constant_time_eq(expected, &wrong_last));
}

#[test]
fn t8_auth_matrix_rejects_origin_token_and_host() {
    let bearer = format!("Bearer {TOKEN}");
    // Happy path: token + exact loopback host, no Origin.
    assert!(auth_headers_ok(None, Some(&bearer), Some(HOST), TOKEN, HOST));

    // ANY Origin is rejected — including the innocuous-looking ones.
    for origin in ["null", "http://127.0.0.1:7407", "https://example.com", "file://", ""] {
        assert!(
            !auth_headers_ok(Some(origin), Some(&bearer), Some(HOST), TOKEN, HOST),
            "Origin must be rejected: {origin:?}"
        );
    }

    // Token must be present, well-formed and exact.
    assert!(!auth_headers_ok(None, None, Some(HOST), TOKEN, HOST));
    assert!(!auth_headers_ok(None, Some(""), Some(HOST), TOKEN, HOST));
    assert!(!auth_headers_ok(None, Some("Basic abc"), Some(HOST), TOKEN, HOST));
    assert!(!auth_headers_ok(None, Some("Bearer wrong"), Some(HOST), TOKEN, HOST));
    assert!(!auth_headers_ok(None, Some("bearer "), Some(HOST), TOKEN, HOST));
    let mut wrong_last = TOKEN.to_string();
    wrong_last.pop();
    wrong_last.push('0');
    assert!(!auth_headers_ok(None, Some(&format!("Bearer {wrong_last}")), Some(HOST), TOKEN, HOST));

    // Host must be exactly 127.0.0.1:<bound port> (DNS rebinding defence).
    for host in ["localhost:7407", "127.0.0.1:7408", "127.0.0.1", "[::1]:7407", "example.com:7407", ""] {
        assert!(
            !auth_headers_ok(None, Some(&bearer), Some(host), TOKEN, HOST),
            "Host must be rejected: {host:?}"
        );
    }
}

#[test]
fn token_file_is_0600_in_0700_state_dir_and_stable() {
    let root = tmp_dir();
    let state = root.join("state");
    let a = load_or_create_token(&state).unwrap();
    let b = load_or_create_token(&state).unwrap();
    assert_eq!(a, b, "token must be stable across reads");
    assert_eq!(a.len(), 64, "32 random bytes => 64 hex chars");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "token must be hex: {a}");

    let dir_mode = std::fs::metadata(&state).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "state dir must be 0700, got {dir_mode:o}");
    let file = state.join("ws.token");
    let file_mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "token file must be 0600, got {file_mode:o}");

    // A corrupted token is regenerated rather than trusted.
    std::fs::write(&file, "short").unwrap();
    let c = load_or_create_token(&state).unwrap();
    assert_ne!(c, "short");
    assert_eq!(c.len(), 64);
}

#[test]
fn ndjson_line_cap_drops_oversize_line_and_keeps_following_lines() {
    let cap = 1 << 20; // 1 MiB
    let good = "{\"cmd\":\"ping\"}\n";
    let huge = "x".repeat(cap + 1);
    let tail = "{\"cmd\":\"status\"}\n";
    let input = format!("{good}{huge}\n{tail}");
    let mut r = Cursor::new(input.as_bytes());
    let mut line = String::new();

    assert!(read_line_capped(&mut r, &mut line, cap).unwrap());
    assert_eq!(line.trim(), "{\"cmd\":\"ping\"}");

    assert!(!read_line_capped(&mut r, &mut line, cap).unwrap(), "oversize line must be reported");
    assert!(line.len() <= cap, "no oversize buffer may be retained");

    assert!(read_line_capped(&mut r, &mut line, cap).unwrap());
    assert_eq!(line.trim(), "{\"cmd\":\"status\"}", "cap must skip only the bad line");

    // Exactly-at-cap is accepted.
    let exact = format!("{}\n", "y".repeat(cap));
    let mut r2 = Cursor::new(exact.as_bytes());
    assert!(read_line_capped(&mut r2, &mut line, cap).unwrap());
    assert_eq!(line.len(), cap);
}

#[test]
fn ws_is_wired_off_until_explicitly_enabled() {
    // The daemon must stay unwired by default; NOTCH_WS=1 opts in.
    std::env::remove_var("NOTCH_WS");
    assert!(!ws_enabled(), "WS must stay unwired unless NOTCH_WS=1");
    std::env::set_var("NOTCH_WS", "1");
    assert!(ws_enabled(), "NOTCH_WS=1 must enable WS");
    std::env::set_var("NOTCH_WS", "yes");
    assert!(!ws_enabled(), "only the exact value 1 enables WS");
    std::env::remove_var("NOTCH_WS");
}
