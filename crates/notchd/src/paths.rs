//! Filesystem locations. `NOTCH_STATE_DIR` / `NOTCH_SOCK` override the
//! defaults for tests and headless operation.

use std::path::PathBuf;

/// State dir: `$NOTCH_STATE_DIR` or `~/Library/Application Support/notch`.
pub fn default_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NOTCH_STATE_DIR") {
        let p = PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    home_dir()
        .join("Library")
        .join("Application Support")
        .join("notch")
}

/// Socket path: `$NOTCH_SOCK` or `<state dir>/notch.sock`.
pub fn default_sock_path() -> PathBuf {
    if let Some(sock) = std::env::var_os("NOTCH_SOCK") {
        let p = PathBuf::from(sock);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    default_state_dir().join("notch.sock")
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
