#!/usr/bin/env bash
# Check Tauri declarations, registrations, and frontend call sites.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export LC_ALL=C

backend="$(grep -rhA1 '#\[tauri::command\]' "$ROOT"/app/src-tauri/src --include='*.rs' |
  grep -oE 'fn [a-z_0-9]+' | sed 's/^fn //' | sort -u)"
registered="$(sed -n '/tauri::generate_handler!\[/,/^ *\])/p' "$ROOT/app/src-tauri/src/lib.rs" |
  grep -oE 'commands::[a-z_0-9]+' | sed 's/^commands:://' | sort -u)"
frontend="$(grep -rh --include='*.ts' --include='*.tsx' . "$ROOT/ui/src" |
  sed -E '/^[[:space:]]*(\/\/|\|[[:space:]]*\{[[:space:]]*cmd:)/d' |
  grep -oE "cmd: [^,;{}]*|invoke(<[^>]*>)?\\('[a-z_0-9]+'" |
  grep -oE "'[a-z_0-9]+'" | tr -d "'" | sort -u)"

if [ -z "$backend" ] || [ -z "$registered" ] || [ -z "$frontend" ]; then
  printf 'wiring-check: declarations, registrations and callers must all be present\n' >&2
  exit 1
fi

compare() {
  diff -u --label "$1" --label "$3" <(printf '%s\n' "$2") <(printf '%s\n' "$4")
}

compare declarations "$backend" registrations "$registered" || exit 1
compare registrations "$registered" callers "$frontend" || exit 1
printf 'wiring-check: OK (%s commands)\n' "$(printf '%s\n' "$registered" | wc -l | tr -d ' ')"
