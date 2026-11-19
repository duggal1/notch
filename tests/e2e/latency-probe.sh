#!/bin/zsh
# tests/e2e/latency-probe.sh — re-runnable six-hop latency driver for notchd.
#
# Uses the SAME paths as acceptance.sh: notchd NDJSON socket (`NOTCH_SOCK`),
# `herdr agent read` for pane tails, `herdr agent/pane list` for pane ids, and
# the real `opencode api GET /api/session` idle signal. Nothing is mocked except
# in --mode fake-* (where ONLY the decider `run` call is scripted, labelled).
#
# Usage: tests/e2e/latency-probe.sh [label] [--mode real|fake-loop|fake-done] [--budget-ms N]
#   label   ^[A-Za-z0-9._-]+$ (default probe-<epoch>)
# Output: JSONL hops on stdout, also saved to .proof/agents/e2e-latency/<label>/hops.jsonl
#
# Idempotent: every call gets a NEW scratch dir
#   /private/var/folders/ct/.../e2e2-XXXXXX   (never a notch-* prefix),
# a fresh NOTCH_STATE_DIR and socket; it closes every pane it creates (verified
# against herdr) and stops exactly the daemon it started.
set -u
setopt pipefail
umask 077
zmodload zsh/datetime

LABEL=${1:-probe-$(date +%s)}; shift || true
MODE=real
BUDGET_MS=180000
while (( $# )); do
  case "$1" in
    --mode) MODE=$2; shift 2 ;;
    --budget-ms) BUDGET_MS=$2; shift 2 ;;
    *) print -u2 -- "unknown arg: $1"; exit 2 ;;
  esac
done
[[ "$LABEL" =~ '^[A-Za-z0-9._-]+$' ]] || { print -u2 -- "bad label"; exit 2; }
case "$MODE" in real|fake-loop|fake-done) ;; *) print -u2 -- "bad mode"; exit 2 ;; esac

HERE=${0:A:h}; REPO=${HERE:h:h}; CLI=$REPO/target/debug/notchd
ART=$REPO/.proof/agents/e2e-latency/$LABEL
mkdir -p "$ART"
SCRATCH=$(mktemp -d /private/var/folders/ct/cwpf7vpn059fkp932tq9zm4m0000gn/T/e2e2-XXXXXX)
STATE=$SCRATCH/state; SOCK=$SCRATCH/notch.sock; CWD=$SCRATCH/cwd
mkdir -p "$STATE" "$CWD"
export NOTCH_STATE_DIR=$STATE NOTCH_SOCK=$SOCK
LOG=$SCRATCH/probe.log
exec > >(tee -a "$LOG") 2>&1
MARKER="latency-marker-$(python3 -c 'import secrets;print(secrets.token_hex(6))')"
print -r -- "$SCRATCH" > "$ART/scratch-path.txt"
print -r -- "$MARKER" > "$ART/marker.txt"

DAEMON_PIDS=$SCRATCH/daemon.pid
: > "$DAEMON_PIDS"
cleanup() {
  # kill-switch only affects panes this daemon manages; then close by cwd for
  # any pane that escaped management (spawn failures), then stop our daemon.
  timeout 20 "$CLI" kill-switch > "$ART/kill-switch.json" 2>/dev/null || true
  herdr pane list > "$SCRATCH/post-panes.json" 2>/dev/null || true
  python3 - "$SCRATCH/post-panes.json" "$CWD" > "$SCRATCH/close-list.txt" 2>/dev/null <<'PY'
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: d={}
cwd=sys.argv[2]
for p in d.get("result",{}).get("panes",[]):
    if p.get("cwd")==cwd and p.get("pane_id"): print(p["pane_id"])
PY
  while IFS= read -r pane; do
    [[ -z "$pane" ]] && continue
    timeout 15 herdr pane close "$pane" >/dev/null 2>&1 || true
    print -r -- "closed-by-cwd $pane" >> "$ART/panes-closed.txt"
  done < "$SCRATCH/close-list.txt"
  while IFS= read -r pid; do
    [[ -z "$pid" ]] && continue
    kill -0 "$pid" 2>/dev/null && { kill "$pid" 2>/dev/null; print -r -- "killed daemon pid $pid" >> "$ART/daemon-stopped.txt"; }
  done < "$DAEMON_PIDS"
  rm -f "$SOCK"
}
trap cleanup EXIT

# daemon env: drop stale herdr triplet; keep HERDR_SOCKET_PATH.
export FAKE_DECIDER_MARKER=$MARKER FAKE_DECIDER_LOG=$ART/decider-calls.jsonl
: > "$ART/decider-calls.jsonl"
(
  unset HERDR_PANE_ID HERDR_TAB_ID HERDR_WORKSPACE_ID
  export NOTCH_STATE_DIR=$STATE NOTCH_SOCK=$SOCK
  [[ "$MODE" == fake-loop ]] && { export NOTCH_OPENCODE_CMD=$HERE/fake-decider.sh FAKE_DECIDER_MODE=continue; }
  [[ "$MODE" == fake-done ]] && { export NOTCH_OPENCODE_CMD=$HERE/fake-decider.sh FAKE_DECIDER_MODE=done; }
  cd "$REPO"
  nohup "$CLI" serve > "$ART/daemon.log" 2>&1 &
  echo $! >> "$DAEMON_PIDS"
)
# wait for hello on the fresh socket
for i in {1..90}; do
  [[ -S "$SOCK" ]] && break
  sleep 0.5
done
python3 - "$SOCK" <<'PY' || { echo "daemon did not come up"; exit 3; }
import socket,sys
s=socket.socket(socket.AF_UNIX); s.settimeout(2)
try:
    s.connect(sys.argv[1]); print(s.recv(4096).decode().splitlines()[0])
except Exception as e:
    raise SystemExit(f"NO: {e}")
PY

if [[ "$MODE" == fake-done ]]; then
  TASK="Create file LATENCY-DONE.md in the current directory containing exactly '$MARKER' and nothing else. Then stop and leave the pane idle; do not print explanations."
  DOD="file $CWD/LATENCY-DONE.md exists and its entire contents are exactly '$MARKER'; evidence must be visible in the pane output"
  EXPECT=marker
else
  TASK="Create file LATENCY-PROBE.md in the current directory containing exactly '$MARKER' and nothing else. Do not create any other file. Do not print explanations."
  DOD="file $CWD/LATENCY-PROBE.md exists and its entire contents are exactly '$MARKER'; no other file was created"
  EXPECT=any
fi
[[ "$MODE" == fake-loop ]] && { TASK="Create file LOOP-SEED.md in the current directory containing exactly 'seed-7' and nothing else. Do not create any other file. Do not print explanations."; DOD="the pane prints only the digit 7 forever; no other output is ever produced; no explanation is ever given (impossible by design)"; EXPECT=marker; }

python3 "$HERE/e2e_lib.py" probe \
  --sock "$SOCK" --state "$STATE" --cwd "$CWD" --task "$TASK" --dod "$DOD" \
  --marker "$MARKER" --expect "$EXPECT" --out "$ART/hops.jsonl" --budget-ms "$BUDGET_MS"
RC=$?
# post-run pane census for the report
herdr pane list > "$ART/post-panes.json" 2>/dev/null || true
print -r -- "{\"label\":\"$LABEL\",\"mode\":\"$MODE\",\"rc\":$RC,\"scratch\":\"$SCRATCH\"}" > "$ART/meta.json"
exit $RC
