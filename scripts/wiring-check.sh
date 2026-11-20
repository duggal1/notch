#!/usr/bin/env bash
# wiring-check.sh — bidirectional command wiring check. No args, no deps.
# Backend = every #[tauri::command] fn under app/src-tauri/src.
# Frontend = every cmd name in the ui/src typed command vocabulary/call sites.
# Exit 0 clean; exit 1 listing offenders. Owned by A5; A6 hooks into gate.sh.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

backend="$(
  grep -rhA1 '#\[tauri::command\]' "$ROOT"/app/src-tauri/src --include=*.rs \
  | grep -oE 'fn [a-z_0-9]+' | sed 's/^fn //' | sort -u
)"
frontend="$(
  grep -rhoE "cmd: '[a-z_0-9]+'|invoke(<[^>]*>)?\\('[a-z_0-9]+'" "$ROOT"/ui/src \
    --include=*.ts --include=*.tsx \
  | grep -oE "'[a-z_0-9]+'" | tr -d "'" | sort -u
)"

no_caller="$(comm -23 <(printf '%s\n' "$backend") <(printf '%s\n' "$frontend"))"
no_command="$(comm -13 <(printf '%s\n' "$backend") <(printf '%s\n' "$frontend"))"

if [ -z "$no_caller" ] && [ -z "$no_command" ]; then
  printf 'wiring-check: OK (%s commands)\n' "$(printf '%s\n' "$backend" | grep -c .)"
  exit 0
fi
[ -n "$no_caller" ] && printf 'backend command has no frontend caller:\n%s\n' "$no_caller"
[ -n "$no_command" ] && printf 'frontend caller has no backend command:\n%s\n' "$no_command"
exit 1
