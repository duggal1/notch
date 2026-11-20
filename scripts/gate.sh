#!/usr/bin/env bash
# gate.sh — one command that proves the repo is healthy.
# Runs every check, prints passed/failed/ignored test counts, and exits non-zero
# if ANY step fails. Logs are kept only for the duration of the run.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

LOGDIR="$(mktemp -d "${TMPDIR:-/tmp}/notch-gate.XXXXXX")"
trap 'rm -rf "$LOGDIR"' EXIT

FAILS=0
STEPS=0

run_step() {
  local name="$1" cmd="$2" log="$LOGDIR/$1.log" rc
  STEPS=$((STEPS + 1))
  printf '\n===== [%d] %s =====\n' "$STEPS" "$name"
  bash -c "$cmd" 2>&1 | tee "$log"
  rc=${PIPESTATUS[0]}
  if [ "$rc" -eq 0 ]; then
    printf 'PASS: %s\n' "$name"
  else
    printf 'FAIL: %s (exit %d)\n' "$name" "$rc"
    FAILS=$((FAILS + 1))
  fi
}

# Sum the "test result:" lines emitted by every cargo test binary.
test_counts() {
  awk '
    /test result:/ {
      for (i = 1; i <= NF; i++) {
        if ($i == "passed;")  p += $(i-1)
        if ($i == "failed;")  f += $(i-1)
        if ($i == "ignored;") ig += $(i-1)
      }
    }
    END { printf "%d %d %d\n", p, f, ig }
  ' "$@"
}

# At baseline the app is its own workspace. While that is true, run its manifest
# separately so its 9 test files / 35 tests are not silently dropped. Once B5
# folds it into the root workspace, the workspace run covers it (no double count).
APP_SEPARATE=0
grep -q '^\[workspace\]' app/src-tauri/Cargo.toml && APP_SEPARATE=1

# The Tauri build macro needs ui/dist to exist at compile time, so build the UI
# before clippy/test touch the app manifest.
run_step "tsc"        "cd '$REPO/ui' && npx tsc --noEmit"
run_step "vite-build" "cd '$REPO/ui' && npm run build"
run_step "test-ui"    "cd '$REPO/ui' && npm test"

run_step "fmt-workspace"    "cargo fmt --all -- --check"
run_step "fmt-app"          "cargo fmt --manifest-path app/src-tauri/Cargo.toml -- --check"
run_step "clippy-workspace" "cargo clippy --workspace --all-targets -- -D warnings"
run_step "clippy-app"       "cargo clippy --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings"
run_step "test-workspace"   "cargo test --workspace --no-fail-fast"
if [ "$APP_SEPARATE" -eq 1 ]; then
  run_step "test-app" "cargo test --manifest-path app/src-tauri/Cargo.toml --no-fail-fast"
fi
if [ -f "$REPO/scripts/wiring-check.sh" ]; then
  run_step "wiring-check" "bash '$REPO/scripts/wiring-check.sh'"
else
  printf '\nNOTE: scripts/wiring-check.sh not present yet (A5) — skipped.\n'
fi

read -r PASSED FAILED IGNORED < <(test_counts "$LOGDIR"/test-*.log)
printf '\n===== SUMMARY =====\n'
printf 'steps: %d, failed steps: %d\n' "$STEPS" "$FAILS"
printf 'tests: %d passed, %d failed, %d ignored\n' "$PASSED" "$FAILED" "$IGNORED"

[ "$FAILS" -eq 0 ] && [ "$FAILED" -eq 0 ] || exit 1
exit 0
