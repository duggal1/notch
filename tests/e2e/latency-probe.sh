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
  # kill-switch affects panes this daemon manages; then close ONLY our own
  # workspace (never a shared/user pane), then stop our daemon.
  timeout 20 "$CLI" kill-switch > "$ART/kill-switch.json" 2>/dev/null || true
  if [[ -n "${WS:-}" ]]; then
    timeout 30 herdr workspace close "$WS" > "$ART/workspace-close.json" 2>/dev/null || true
  fi
  herdr pane list > "$SCRATCH/post-panes.json" 2>/dev/null || true
  while IFS= read -r pid; do
    [[ -z "$pid" ]] && continue
    kill -0 "$pid" 2>/dev/null && { kill "$pid" 2>/dev/null; print -r -- "killed daemon pid $pid" >> "$ART/daemon-stopped.txt"; }
  done < "$DAEMON_PIDS"
  rm -f "$SOCK"
}
trap cleanup EXIT

# Own herdr workspace: every pane created by this run lives here, never in the
# shared workspace. --no-focus keeps the user's view untouched.
WS_JSON=$(timeout 30 herdr workspace create --cwd "$SCRATCH" --label "notch-e2e-$LABEL" --no-focus 2>&1)
print -r -- "$WS_JSON" > "$ART/workspace-create.json"
read -r WS TAB ROOTPANE < <(print -r -- "$WS_JSON" | python3 -c 'import json,sys
d=json.load(sys.stdin)["result"]
print(d["workspace"]["workspace_id"], d["tab"]["tab_id"], d["root_pane"]["pane_id"])')
export NOTCH_E2E_WS=$WS
export FAKE_DECIDER_MARKER=$MARKER FAKE_DECIDER_LOG=$ART/decider-calls.jsonl
: > "$ART/decider-calls.jsonl"
(
  export HERDR_WORKSPACE_ID=$WS HERDR_TAB_ID=$TAB HERDR_PANE_ID=$ROOTPANE
  export NOTCH_STATE_DIR=$STATE NOTCH_SOCK=$SOCK
  export NOTCH_DECIDER_SESSION="ses_e2e_probe_${LABEL}_$(date +%s)"
  # tighter poll catches the brief idle window before herdr reports "done"
  export NOTCH_POLL_INTERVAL_MS=200
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
# close our workspace now (trap is the safety net), then prove it is gone
timeout 20 "$CLI" kill-switch > "$ART/kill-switch.json" 2>/dev/null || true
timeout 30 herdr workspace close "$WS" > "$ART/workspace-close.json" 2>/dev/null || true
herdr pane list > "$ART/post-panes.json" 2>/dev/null || true
herdr workspace list > "$ART/post-workspaces.json" 2>/dev/null || true
PANE_PROOF=$(WS="$WS" P="$ART/post-panes.json" W="$ART/post-workspaces.json" python3 -c 'import json,os,sys
ws=os.environ["WS"]
panes=[a.get("pane_id") for a in json.load(open(os.environ["P"]))["result"].get("panes",[]) if a.get("workspace_id")==ws]
wsl=[w["workspace_id"] for w in json.load(open(os.environ["W"]))["result"]["workspaces"]]
print("OUR_PANES="+json.dumps(panes)+" WS_PRESENT="+("1" if ws in wsl else "0"))')
print -r -- "{\"label\":\"$LABEL\",\"mode\":\"$MODE\",\"rc\":$RC,\"scratch\":\"$SCRATCH\",\"workspace\":\"$WS\",\"pane_proof\":\"$PANE_PROOF\"}" > "$ART/meta.json"
exit $RC
