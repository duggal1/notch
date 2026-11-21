#!/bin/zsh
# tests/e2e/acceptance.sh — LIVE six-step acceptance for notchd (bridge.md v1.3 §8).
#
# Real herdr + real OpenCode throughout (panes are real OpenCode sessions; the
# decider is the real `opencode run` using the operator's harness default model
# — no --model flag is forced). Forced proofs for steps that a real model may
# not reach are run separately by latency-probe.sh --mode fake-loop / fake-done
# and are labelled as scripted.
#
# Scratch is ALWAYS /private/var/folders/ct/.../e2e2-XXXXXX (never notch-*).
# Every pane this run creates is closed; the daemon started here is stopped here.
#
# Usage: tests/e2e/acceptance.sh <label>   (label: ^[A-Za-z0-9._-]+$)
set -u
setopt pipefail
umask 077
zmodload zsh/datetime

LABEL=${1:-accept-$(date +%s)}
[[ "$LABEL" =~ '^[A-Za-z0-9._-]+$' ]] || { print -u2 -- "bad label"; exit 2; }
HERE=${0:A:h}; REPO=${HERE:h:h}; CLI=$REPO/target/debug/notchd
LIB=$HERE/e2e_lib.py
ART=$REPO/.proof/agents/e2e-latency/$LABEL
mkdir -p "$ART"; : > "$ART/result.jsonl"
SCRATCH=$(mktemp -d /private/var/folders/ct/cwpf7vpn059fkp932tq9zm4m0000gn/T/e2e2-XXXXXX)
STATE=$SCRATCH/state; SOCK=$SCRATCH/notch.sock
mkdir -p "$STATE" "$SCRATCH/cwd-e2e-1" "$SCRATCH/cwd-e2e-2" "$SCRATCH/cwd-loop" "$SCRATCH/cwd-watchdog"
export NOTCH_STATE_DIR=$STATE NOTCH_SOCK=$SOCK
LOG=$SCRATCH/acceptance.log
exec > >(tee -a "$LOG") 2>&1
log() { print -r -- "[$(date +%H:%M:%S)] $*" >&2; }

emit() { # emit STEP STATUS EVIDENCE [WALL_S]
  STEP="$1" STATUS="$2" EVIDENCE="$3" WALL="${4:-0}" ARTIFACT="$ART" LABEL="$LABEL" \
  python3 -c 'import json,os,time
d={"ts":time.strftime("%Y-%m-%dT%H:%M:%S%z"),"run":os.environ["LABEL"],"step":os.environ["STEP"],"status":os.environ["STATUS"],"evidence":os.environ["EVIDENCE"],"wall_s":int(os.environ["WALL"])}
line=json.dumps(d,separators=(",",":")); print(line)
with open(os.environ["ARTIFACT"]+"/result.jsonl","a") as f: f.write(line+"\n")'
}

rq() { python3 "$LIB" req --sock "$SOCK" --json "$1"; }
snap() { rq '{"cmd":"snapshot"}' | python3 -c 'import json,sys; d=json.load(sys.stdin); print(json.dumps(d.get("snapshot",d)))'; }
sids_of() { python3 -c 'import json,sys; d=json.load(sys.stdin); print(" ".join(s.get("id","") for s in d.get("sessions",[])))'; }

DAEMON_PIDS=$SCRATCH/daemon.pid; : > "$DAEMON_PIDS"
MYPANES=$SCRATCH/my-panes.txt; : > "$MYPANES"
cleanup() {
  rq '{"cmd":"kill_switch"}' > "$ART/kill-switch.json" 2>/dev/null || true
  # Close ONLY the workspace this run created (never a shared/user pane).
  if [[ -n "${WS:-}" ]]; then
    timeout 30 herdr workspace close "$WS" > "$ART/workspace-close.json" 2>/dev/null || true
  fi
  herdr pane list > "$SCRATCH/post-panes.json" 2>/dev/null || true
  while IFS= read -r pid; do
    [[ -z "$pid" ]] && continue
    kill -0 "$pid" 2>/dev/null && kill "$pid" 2>/dev/null
  done < "$DAEMON_PIDS"
  rm -f "$SOCK"
}
trap cleanup EXIT

RUN_START=$EPOCHSECONDS
FAILS=0

# ---- step 0: binary built from source -------------------------------------
t0=$EPOCHSECONDS
( cd "$REPO" && timeout 300 cargo build -p notchd > "$SCRATCH/build.log" 2>&1 ) || {
  emit 0 FAIL "cargo build failed; see $SCRATCH/build.log" $(( EPOCHSECONDS-t0 )); exit 3; }
emit 0 PASS "cargo build -p notchd ok; CLI=$CLI; model=harness default (no --model forced)" $(( EPOCHSECONDS-t0 ))

# ---- step 1: own herdr workspace + fresh daemon + baseline panes -----------
t0=$EPOCHSECONDS
herdr agent list > "$SCRATCH/baseline-agents.json" 2>/dev/null
BASE=$(python3 -c 'import json;d=json.load(open("'"$SCRATCH"'/baseline-agents.json"));print(",".join(sorted(a["pane_id"] for a in d["result"]["agents"])))')
print -r -- "$BASE" > "$ART/baseline-panes.txt"
WS_JSON=$(timeout 30 herdr workspace create --cwd "$SCRATCH" --label "notch-e2e-$LABEL" --no-focus 2>&1)
print -r -- "$WS_JSON" > "$ART/workspace-create.json"
read -r WS TAB ROOTPANE < <(print -r -- "$WS_JSON" | python3 -c 'import json,sys
d=json.load(sys.stdin)["result"]
print(d["workspace"]["workspace_id"], d["tab"]["tab_id"], d["root_pane"]["pane_id"])')
[[ -n "${WS:-}" ]] || { emit 1 FAIL "workspace create failed: $WS_JSON" $(( EPOCHSECONDS-t0 )); exit 3; }
export NOTCH_E2E_WS=$WS
log "own workspace=$WS tab=$TAB root pane=$ROOTPANE (no-focus); baseline shared panes=$BASE"
(
  export HERDR_WORKSPACE_ID=$WS HERDR_TAB_ID=$TAB HERDR_PANE_ID=$ROOTPANE
  export NOTCH_STATE_DIR=$STATE NOTCH_SOCK=$SOCK
  # unique decider session per run: agents share the machine-wide default
  # `ses_notchd_supervisor_decider`, which collides across concurrent runs.
  export NOTCH_DECIDER_SESSION="ses_e2e_${LABEL}_$(date +%s)"
  cd "$REPO"; nohup "$CLI" serve > "$SCRATCH/daemon.log" 2>&1 &
  echo $! >> "$DAEMON_PIDS"
)
HELLO=""
for i in {1..90}; do
  HELLO=$(python3 - "$SOCK" <<'PY' 2>/dev/null
import socket,sys
s=socket.socket(socket.AF_UNIX); s.settimeout(1)
try: s.connect(sys.argv[1]); print(s.recv(4096).decode().splitlines()[0])
except Exception: pass
PY
)
  [[ "$HELLO" == *hello* ]] && break
  sleep 0.5
done
if [[ "$HELLO" != *hello* ]]; then
  tail -10 "$SCRATCH/daemon.log" > "$ART/daemon-log-tail.txt"
  emit 1 FAIL "daemon did not come up; see $ART/daemon-log-tail.txt" $(( EPOCHSECONDS-t0 )); exit 3
fi
emit 1 PASS "own herdr workspace $WS (tab $TAB, root $ROOTPANE, --no-focus); fresh NOTCH_STATE_DIR=$STATE NOTCH_SOCK=$SOCK; hello=$HELLO; baseline shared panes: $BASE" $(( EPOCHSECONDS-t0 ))

# ---- step 2: SV-1 with two sessions; six-hop latency for session 1 ---------
t0=$EPOCHSECONDS
MARKER="e2e-marker-$(python3 -c 'import secrets;print(secrets.token_hex(6))')"
T1="Create file E2E-1.md in the current directory containing exactly '$MARKER' and nothing else. Do not create any other file. Do not print explanations."
D1="file $SCRATCH/cwd-e2e-1/E2E-1.md exists and its entire contents are exactly '$MARKER'; no other file was created"
T2="Create file E2E-2.md in the current directory containing exactly 'e2e-beta' and nothing else. Do not create any other file. Do not print explanations."
D2="file $SCRATCH/cwd-e2e-2/E2E-2.md exists and its entire contents are exactly 'e2e-beta'; no other file was created"
for attempt in 1 2 3; do
  : > "$ART/hops-S1.jsonl"
  python3 "$LIB" probe --sock "$SOCK" --state "$STATE" --cwd "$SCRATCH/cwd-e2e-1" \
    --task "$T1" --dod "$D1" --marker "$MARKER" --expect any \
    --out "$ART/hops-S1.jsonl" --budget-ms 120000 > "$ART/hops-S1.stdout" 2>&1
  A1=$(python3 -c 'import json;print(json.dumps([json.loads(l) for l in open("'"$ART"'/hops-S1.jsonl") if "\"hop\":\"a_" in l][0]["ack"]))' 2>/dev/null)
  [[ "$A1" == *'"ok": false'* || "$A1" == *'"ok":false'* ]] || break
  log "step2: spawn failed (agent_pane_busy); retry $attempt/3 after 3s"
  sleep 3
done
A1=$(python3 -c 'import json;print(json.dumps([json.loads(l) for l in open("'"$ART"'/hops-S1.jsonl") if "\"hop\":\"a_" in l][0]["ack"]))' 2>/dev/null)
SID1=$(python3 -c 'import json;print(json.loads(open("'"$ART"'/hops-S1.jsonl").readline())["sid"])' 2>/dev/null)
for attempt in 1 2 3; do
  A2=$(rq "$(python3 -c 'import json,sys;print(json.dumps({"cmd":"submit_task","task":sys.argv[1],"cwd":sys.argv[2],"definitionOfDone":sys.argv[3],"supervisorId":"SV-1"}))' "$T2" "$SCRATCH/cwd-e2e-2" "$D2")")
  [[ "$A2" == *'"ok":true'* ]] && break
  log "step2: S1-OC02 spawn failed (agent_pane_busy); retry $attempt/3 after 3s"
  sleep 3
done
SID2=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("sessionId",""))' "$A2" 2>/dev/null)
SUP=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("supervisorId",""))' "$A2" 2>/dev/null)
snap > "$SCRATCH/after-submit-snap.json"
python3 -c 'import json;d=json.load(open("'"$SCRATCH"'/after-submit-snap.json"));print("\n".join(s["paneId"] for s in d.get("sessions",[]) if s.get("paneId")))' >> "$MYPANES"
sort -u "$MYPANES" -o "$MYPANES"
if [[ "$A1" == *'"ok": true'* && -n "$SID1" && -n "$SID2" && "$SUP" == "SV-1" ]]; then
  emit 2 PASS "SV-1 allocated $SID1 and $SID2; ack1=$A1; ack2=$A2; hops: $ART/hops-S1.jsonl" $(( EPOCHSECONDS-t0 ))
else
  FAILS=$(( FAILS+1 )); emit 2 FAIL "submit failed: ack1=$A1 ack2=$A2 SID1=$SID1 SID2=$SID2 sup=$SUP" $(( EPOCHSECONDS-t0 ))
fi

# ---- step 4: handoff through the bus, delivered to the pane tail -----------
t0=$EPOCHSECONDS
H="handoff-marker-$(python3 -c 'import secrets;print(secrets.token_hex(6))')"
ACK4=$(rq "$(python3 -c 'import json,sys;print(json.dumps({"cmd":"message","from":sys.argv[1],"to":sys.argv[2],"type":"handoff","body":sys.argv[3]}))' "$SID1" "$SID2" "$H")")
S4=$(snap)
V4=$(SNAP="$S4" H="$H" SID1="$SID1" SID2="$SID2" python3 -c 'import json,os,subprocess
d=json.loads(os.environ["SNAP"]); h=os.environ["H"]
m=[x for x in d.get("busMessages",[]) if x.get("from")==os.environ["SID1"] and x.get("to")==os.environ["SID2"] and h in (x.get("preview") or "")]
tail=""
for t in (os.environ["SID2"].lower(),):
  try: tail=subprocess.run(["herdr","agent","read",t,"--lines","200"],capture_output=True,text=True,timeout=10).stdout
  except Exception: pass
print("CONTAINED" if m else "MISSING", "tail_marker=" + ("1" if h in tail else "0"), json.dumps(m[:1]))' 2>/dev/null)
if [[ "$V4" == CONTAINED* ]]; then
  emit 4 PASS "busMessages $V4; ack=$ACK4" $(( EPOCHSECONDS-t0 ))
else
  FAILS=$(( FAILS+1 )); emit 4 FAIL "bus handoff $V4; ack=$ACK4" $(( EPOCHSECONDS-t0 ))
fi

# ---- step 3: both sessions reach a terminal state ---------------------------
t0=$EPOCHSECONDS
DEADLINE=$(( EPOCHSECONDS+150 ))
TERM=""
while (( EPOCHSECONDS < DEADLINE )); do
  S=$(snap 2>/dev/null)
  R=$(SNAP="$S" SID1="$SID1" SID2="$SID2" python3 -c 'import json,os
d=json.loads(os.environ["SNAP"]); sess={s["id"]:s for s in d.get("sessions",[])}
out=[]
for sid in (os.environ["SID1"],os.environ["SID2"]):
  s=sess.get(sid); out.append((sid,s.get("status") if s else "missing"))
print(" ".join(f"{a}:{b}" for a,b in out))' 2>/dev/null)
  if [[ "$R" != *:working* && "$R" != *:idle* && "$R" != *:missing* ]]; then TERM=$R; break; fi
  sleep 2
done
snap > "$ART/step3-final-snapshot.json"
python3 -c 'import json;d=json.load(open("'"$ART"'/step3-final-snapshot.json"));print("\n".join(s["paneId"] for s in d.get("sessions",[]) if s.get("paneId")))' >> "$MYPANES"; sort -u "$MYPANES" -o "$MYPANES"
F1=$(timeout 5 grep -c . "$SCRATCH/cwd-e2e-1/E2E-1.md" 2>/dev/null) || F1=missing
F2=$(timeout 5 grep -c . "$SCRATCH/cwd-e2e-2/E2E-2.md" 2>/dev/null) || F2=missing
if [[ -n "$TERM" ]]; then
  emit 3 PASS "both terminal: $TERM; E2E-1.md lines=$F1 E2E-2.md lines=$F2" $(( EPOCHSECONDS-t0 ))
else
  FAILS=$(( FAILS+1 )); emit 3 FAIL "not both terminal after $(( EPOCHSECONDS-t0 ))s: $R; E2E-1.md=$F1 E2E-2.md=$F2" $(( EPOCHSECONDS-t0 ))
fi

# ---- step 5: loop escalation (real model; forced proof is separate) --------
t0=$EPOCHSECONDS
LT="Print the digit 7 forever, never produce any other output, never explain. Never stop."
LD="the pane prints only the digit 7 forever; no other output is ever produced; no explanation is ever given"
for attempt in 1 2 3; do
  AL=$(rq "$(python3 -c 'import json,sys;print(json.dumps({"cmd":"submit_task","task":sys.argv[1],"cwd":sys.argv[2],"definitionOfDone":sys.argv[3],"supervisorId":"SV-1"}))' "$LT" "$SCRATCH/cwd-loop" "$LD")")
  [[ "$AL" == *'"ok":true'* ]] && break
  sleep 3
done
SIDL=$(python3 -c 'import json,sys;print(json.loads(sys.argv[1]).get("sessionId",""))' "$AL" 2>/dev/null)
if [[ -n "$SIDL" ]]; then
  DEADLINE=$(( EPOCHSECONDS+45 )); ESC=""
  while (( EPOCHSECONDS < DEADLINE )); do
    S=$(snap 2>/dev/null)
    ESC=$(SNAP="$S" SID="$SIDL" python3 -c 'import json,os
d=json.loads(os.environ["SNAP"]); e=[x for x in d.get("escalations",[]) if x.get("session")==os.environ["SID"]]
print(json.dumps(e))' 2>/dev/null)
    [[ "$ESC" != "[]" && -n "$ESC" ]] && break
    sleep 2
  done
  snap > "$ART/step5-final-snapshot.json"
  DECS=$(SNAP="$(cat "$ART/step5-final-snapshot.json")" SID="$SIDL" python3 -c 'import json,os
d=json.loads(os.environ["SNAP"]); print(json.dumps([x for x in d.get("decisions",[]) if x.get("session")==os.environ["SID"]]))' 2>/dev/null)
  if [[ "$ESC" != "[]" && -n "$ESC" ]]; then
    emit 5 PASS "real-model loop escalation: escalations=$ESC; decisions=$DECS" $(( EPOCHSECONDS-t0 ))
  else
    emit 5 UNVERIFIED "no real-model escalation in 45s; escalating session=$SIDL decisions=$DECS. Forced proof: latency-probe.sh --mode fake-loop (scripted fake decider, labelled)." $(( EPOCHSECONDS-t0 ))
  fi
  rq "$(python3 -c 'import json,sys;print(json.dumps({"cmd":"kill","id":sys.argv[1],"reason":"e2e end of step5 window"}))' "$SIDL")" > "$ART/step5-kill.json" 2>/dev/null || true
else
  FAILS=$(( FAILS+1 )); emit 5 FAIL "loop submit returned no session id; ack=$AL" $(( EPOCHSECONDS-t0 ))
fi

# ---- step 6: DONE only after verifier-backed decision ----------------------
t0=$EPOCHSECONDS
cp "$STATE/events.jsonl" "$ART/events-step6.jsonl" 2>/dev/null
V6=$(SID="$SID1" EVENTS="$STATE/events.jsonl" SNAP="$(snap 2>/dev/null)" python3 - <<'PY'
import json,os
sid=os.environ["SID"]
dec=[]
for l in open(os.environ["EVENTS"]):
    try: d=json.loads(l)
    except Exception: continue
    if d.get("kind")=="decision" and d.get("session")==sid: dec.append(d)
s=next((x for x in json.loads(os.environ["SNAP"]).get("sessions",[]) if x.get("id")==sid),{})
st=s.get("status")
ver=[x for x in dec if x.get("detail","").lower().startswith("done:") and "verifier" in x.get("detail","").lower()]
if st=="done" and ver: print("VERIFIED")
elif st=="done": print("BYPASSED")
else: print("NOTDONE:"+str(st))
print("DECS="+json.dumps(dec))
PY
)
case "$V6" in
  VERIFIED*) emit 6 PASS "verifier-backed DONE: $V6" $(( EPOCHSECONDS-t0 )) ;;
  BYPASSED*) FAILS=$(( FAILS+1 )); emit 6 FAILED-bypassed "DONE without verifier decision: $V6 (no spin)" $(( EPOCHSECONDS-t0 )) ;;
  *) FAILS=$(( FAILS+1 )); emit 6 FAIL "step6: $V6" $(( EPOCHSECONDS-t0 )) ;;
esac

# ---- step 8: cleanup assertions (step 7 watchdog is a separate probe) ------
t0=$EPOCHSECONDS
snap > "$ART/step8-pre-cleanup-snapshot.json"
python3 -c 'import json;d=json.load(open("'"$ART"'/step8-pre-cleanup-snapshot.json"));print("\n".join(s["paneId"] for s in d.get("sessions",[]) if s.get("paneId")))' >> "$MYPANES"; sort -u "$MYPANES" -o "$MYPANES"
rq '{"cmd":"kill_switch"}' > "$ART/kill-switch.json" 2>/dev/null || true
rq '{"cmd":"cleanup"}' > "$ART/cleanup.json" 2>/dev/null || true
timeout 30 herdr workspace close "$WS" > "$ART/workspace-close.json" 2>/dev/null || true
herdr agent list > "$SCRATCH/post-agents.json" 2>/dev/null
herdr workspace list > "$SCRATCH/post-workspaces.json" 2>/dev/null
cp "$SCRATCH/post-agents.json" "$ART/post-agents.json"
CHECK=$(MYPANES="$MYPANES" POST="$SCRATCH/post-agents.json" BASE="$SCRATCH/baseline-agents.json" WS="$WS" WSLIST="$SCRATCH/post-workspaces.json" python3 - <<'PY'
import json,os
post={a.get("pane_id") for a in json.load(open(os.environ["POST"]))["result"]["agents"]}
base={a["pane_id"] for a in json.load(open(os.environ["BASE"]))["result"]["agents"]}
mine={l.strip() for l in open(os.environ["MYPANES"]) if l.strip()}
ws=os.environ["WS"]
wslist=[w["workspace_id"] for w in json.load(open(os.environ["WSLIST"]))["result"]["workspaces"]]
our_left=[a.get("pane_id") for a in json.load(open(os.environ["POST"]))["result"]["agents"] if a.get("workspace_id")==ws]
rem=sorted(mine&post); miss=sorted(base-post)
print("REMAIN="+json.dumps(rem)+" OUR_WS_PANES="+json.dumps(our_left)+" WS_PRESENT="+("1" if ws in wslist else "0")+" BASELINE_DRIFT="+json.dumps(miss)+" CHECKED="+str(len(mine)))
PY
)
# stop the daemon we started (the EXIT trap is a second safety net)
while IFS= read -r pid; do
  [[ -z "$pid" ]] && continue
  kill "$pid" 2>/dev/null || true
done < "$DAEMON_PIDS"
sleep 1
DAEMON_OK=KILLED
while IFS= read -r pid; do
  [[ -z "$pid" ]] && continue
  # a killed background job can linger as a zombie until reaped; that is stopped.
  STATE_PS=$(ps -p "$pid" -o state= 2>/dev/null | tr -d ' ')
  [[ -n "$STATE_PS" && "$STATE_PS" != Z* ]] && DAEMON_OK=KILL_FAILED
done < "$DAEMON_PIDS"
cp "$STATE/events.jsonl" "$ART/events-final.jsonl" 2>/dev/null
cp "$STATE/state.json" "$ART/state-final.json" 2>/dev/null
cp "$SCRATCH/my-panes.txt" "$ART/my-panes.txt"
cp "$SCRATCH/post-agents.json" "$ART/post-agents.json"
if [[ "$CHECK" == *'REMAIN=[]'* && "$CHECK" == *'OUR_WS_PANES=[]'* && "$CHECK" == *WS_PRESENT=0* && "$DAEMON_OK" == KILLED ]]; then
  emit 8 PASS "pane census: $CHECK; daemon=$DAEMON_OK; total=$(( EPOCHSECONDS-RUN_START ))s" $(( EPOCHSECONDS-t0 ))
else
  FAILS=$(( FAILS+1 )); emit 8 FAIL "pane census: $CHECK; daemon=$DAEMON_OK" $(( EPOCHSECONDS-t0 ))
fi

python3 - "$ART/result.jsonl" "$ART" "$SCRATCH" "$RUN_START" <<'PY'
import json,sys
p,art,scr,start=sys.argv[1:5]
import time
recs=[json.loads(l) for l in open(p) if l.strip()]
c={}
for r in recs: c[r["status"]]=c.get(r["status"],0)+1
fails=sum(n for k,n in c.items() if k.startswith("FAIL") or k.startswith("UNVERIFIED-not"))
status="PASS" if fails==0 else "FAIL"
line=json.dumps({"step":"summary","status":status,"counts":c,"scratch":scr,"artifacts":art,"total_s":int(time.time())-int(start)},separators=(",",":"))
print(line)
with open(p,"a") as fh: fh.write(line+"\n")
PY
exit 0
