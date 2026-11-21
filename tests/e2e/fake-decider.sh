#!/bin/zsh
# tests/e2e/fake-decider.sh — SCRIPTED decider used ONLY for forced proofs.
#
# It is a thin NOTCH_OPENCODE_CMD wrapper: `api` and `models` go to the REAL
# opencode (so idle detection is still the real primary signal); only `run`
# (the supervisor decider/verifier call) is scripted. Every scripted call is
# logged with a millisecond timestamp so the marker-delivery hop is provable.
#
# Env:
#   FAKE_DECIDER_MODE   continue|done|stuck   (default continue)
#   FAKE_DECIDER_MARKER marker text echoed in every verdict (required)
#   FAKE_DECIDER_LOG    append-only call log (default stderr)
set -u
CMD=$1; shift
case "$CMD" in
  api|models) exec opencode "$CMD" "$@" ;;
  run) ;;
  *) exec opencode "$CMD" "$@" ;;
esac

MODE=${FAKE_DECIDER_MODE:-continue}
MARKER=${FAKE_DECIDER_MARKER:?FAKE_DECIDER_MARKER required}
LOG=${FAKE_DECIDER_LOG:-/dev/stderr}
TS=$(python3 -c 'import time;print(int(time.time()*1000))')
# last argv is the prompt
PROMPT="${@[-1]}"
if [[ "$PROMPT" == "You verify a definition of done"* ]]; then
  KIND=verifier
else
  KIND=decider
fi
printf '{"t":%s,"kind":"%s","model":"%s","prompt_head":"%s"}\n' \
  "$TS" "$KIND" "${2:-?}" "${PROMPT:0:120}" >> "$LOG"

case "$MODE:$KIND" in
  done:decider)     print -r -- "DONE: evidence present in pane tail: $MARKER" ;;
  done:verifier)    print -r -- "YES: checkable evidence $MARKER satisfies the definition of done" ;;
  stuck:*)          print -r -- "STUCK: $MARKER" ;;
  continue:*)       print -r -- "CONTINUE: $MARKER" ;;
  *)                print -r -- "CONTINUE: $MARKER" ;;
esac
