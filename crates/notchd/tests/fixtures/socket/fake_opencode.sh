#!/bin/bash
# Scripted fake opencode for the socket fuzz tests. Read-only canned answers.
# No real opencode process is ever invoked by the daemon under test.
set -u
case "${1:-}" in
  api)
    printf '{"cursor":null,"data":[]}\n'
    ;;
  models)
    printf 'opencode/hy4-flash-free\nopencode/inkling-2-small-free\n'
    ;;
  run)
    printf 'done\n'
    ;;
  *)
    : # tolerate anything else
    ;;
esac
exit 0
