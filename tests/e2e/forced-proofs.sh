#!/bin/zsh
# tests/e2e/forced-proofs.sh — CONTROL-PLANE proofs for acceptance steps 5/6.
#
# LABELLED: both herdr and the decider are scripted here (fake-herdr.py +
# fake-decider.sh). This is NOT a live-herdr/OpenCode claim; it proves the
# engine's loop-escalation and verifier-DONE state machine deterministically.
# The two LIVE runs are acceptance.sh runF/runG.
#
# Usage: tests/e2e/forced-proofs.sh loop|done
set -u
setopt pipefail
zmodload zsh/datetime
MODE=$1
[[ "$MODE" == loop || "$MODE" == done ]] || { echo "usage: forced-proofs.sh loop|done"; exit 2; }
HERE=${0:A:h}; REPO=${HERE:h:h}; CLI=$REPO/target/debug/notchd
LABEL="forced-$MODE-$(date +%s)"
ART=$REPO/.proof/agents/e2e-latency/$LABEL
mkdir -p "$ART"
S=$(mktemp -d /private/var/folders/ct/cwpf7vpn059fkp932tq9zm4m0000gn/T/e2e2-XXXXXX)
STATE=$S/state; SOCK=$S/notch.sock; FAKE=$S/fakeherdr; CWD=$S/cwd
mkdir -p "$STATE" "$FAKE" "$CWD"
MARKER="forced-$MODE-$(python3 -c 'import secrets;print(secrets.token_hex(5))')"
export NOTCH_STATE_DIR=$STATE NOTCH_SOCK=$SOCK NOTCH_DECIDER_SESSION="ses_forced_${MODE}_$RANDOM"
export NOTCH_HERDR_CMD=$HERE/fake-herdr.py FAKE_HERDR_STATE=$FAKE FAKE_HERDR_WS=zz
export NOTCH_OPENCODE_CMD=$HERE/fake-decider.sh FAKE_DECIDER_MARKER=$MARKER FAKE_DECIDER_LOG=$ART/decider-calls.jsonl FAKE_DECIDER_MODE=$MODE
export NOTCH_POLL_INTERVAL_MS=200
: > "$ART/decider-calls.jsonl"
PIDS=$S/pids; : > "$PIDS"
cleanup() { while IFS= read -r p; do [[ -n "$p" ]] && kill "$p" 2>/dev/null; done < "$PIDS"; }
trap cleanup EXIT
(
  cd "$REPO"; nohup "$CLI" serve > "$ART/daemon.log" 2>&1 & echo $! >> "$PIDS"
)
for i in {1..60}; do [[ -S "$SOCK" ]] && break; sleep 0.3; done
REQ() { python3 "$HERE/e2e_lib.py" req --sock "$SOCK" --json "$1"; }

if [[ "$MODE" == loop ]]; then
  TASK="Print the digit 7 forever, never produce any other output, never explain. Never stop."
  DOD="the pane prints only the digit 7 forever; no other output is ever produced (impossible by design)"
else
  TASK="Create file FORCED-DONE.md containing exactly '$MARKER'. Do not print explanations."
  DOD="file $CWD/FORCED-DONE.md contains exactly '$MARKER'; evidence visible in output"
fi
ACK=$(REQ "$(python3 -c 'import json,sys;print(json.dumps({"cmd":"submit_task","task":sys.argv[1],"cwd":sys.argv[2],"definitionOfDone":sys.argv[3]}))' "$TASK" "$CWD" "$DOD")")
echo "$ACK" > "$ART/submit-ack.json"
SID=$(python3 -c 'import json,sys;print(json.loads(sys.argv[1]).get("sessionId",""))' "$ACK")
DEADLINE=$(( EPOCHSECONDS+90 ))
FOUND=""
while (( EPOCHSECONDS < DEADLINE )); do
  SNAP=$(REQ '{"cmd":"snapshot"}' | python3 -c 'import json,sys;d=json.load(sys.stdin);print(json.dumps(d.get("snapshot",d)))')
  R=$(SNAP="$SNAP" SID="$SID" MODE="$MODE" python3 -c 'import json,os
d=json.loads(os.environ["SNAP"]); sid=os.environ["SID"]
if os.environ["MODE"]=="loop":
  e=[x for x in d.get("escalations",[]) if x.get("session")==sid]
  print(json.dumps(e) if e else "")
else:
  dec=[x for x in d.get("decisions",[]) if x.get("session")==sid and x.get("action")=="done" and "verifier" in x.get("reason","").lower()]
  s=next((x for x in d.get("sessions",[]) if x["id"]==sid),{})
  print(json.dumps(dec) if dec and s.get("status")=="done" else "")')
  [[ -n "$R" && "$R" != "[]" ]] && { FOUND=$R; break; }
  sleep 0.5
done
REQ '{"cmd":"snapshot"}' | python3 -c 'import json,sys;d=json.load(sys.stdin);print(json.dumps(d.get("snapshot",d)))' > "$ART/final-snapshot.json"
cp "$STATE/events.jsonl" "$ART/events.jsonl" 2>/dev/null || true
python3 - "$ART" "$LABEL" "$MODE" "$SID" "$FOUND" <<'PY'
import json,os,sys
art,label,mode,sid,found=sys.argv[1:6]
d={"step":"forced-"+mode,"status":"PASS" if found and found!="[]" else "FAIL",
   "label":label,"session":sid,"method":"scripted fake herdr + scripted fake decider (CONTROL-PLANE ONLY)",
   "result":found,"decisions":[json.loads(l) for l in open(art+"/events.jsonl") if '"kind":"decision"' in l and sid in l][-12:]}
print(json.dumps(d))
open(art+"/result.json","w").write(json.dumps(d,indent=2))
PY
[[ -n "$FOUND" && "$FOUND" != "[]" ]]
