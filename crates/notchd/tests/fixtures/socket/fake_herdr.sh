#!/bin/bash
# Scripted fake herdr for the socket fuzz tests. NO real panes exist.
# Reads argv exactly like the real `herdr`; answers canned JSON on stdout.
# Exit code 0 for everything the daemon is allowed to call.
set -u
sub="${1:-}"
sub2="${2:-}"
case "$sub $sub2" in
  "agent list")
    printf '{"id":"cli:agent:list","result":{"type":"agent_list","agents":[]}}\n'
    ;;
  "pane list")
    printf '{"id":"cli:pane:list","result":{"type":"pane_list","panes":[]}}\n'
    ;;
  "pane split")
    printf '{"id":"cli:pane:split","result":{"pane_id":"w90:p7"}}\n'
    ;;
  "agent read")
    printf 'last line of pane output\n'
    ;;
  "agent prompt"|"agent start"|"agent wait"|"agent focus"|"pane close")
    : # success, silent
    ;;
  *)
    : # tolerate anything else, silent success
    ;;
esac
exit 0
