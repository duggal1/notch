#!/bin/zsh
# window-shot.sh <path-to-.app> [out.png] — hidden launch, capture own window, quit, JSON out.
# Never activates, never raises, never moves the user's cursor.
set -euo pipefail
APP="${1:?usage: window-shot.sh <App.app> [out.png]}"
OUT="${2:-.proof/app-hidden-launch.png}"
mkdir -p "$(dirname "$OUT")"

/usr/bin/open -g -j "$APP"
sleep 2

PID=$(pgrep -f "$APP" | head -1 || true)
if [[ -z "$PID" ]]; then
  print -r -- "{\"ok\":false,\"error\":\"process did not launch\"}"
  exit 1
fi

# Find the on-screen window belonging to our pid (top-most layer first).
WID=$(osascript -l JavaScript -e '
  ObjC.import("CoreGraphics");
  const arr = $.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, $.kCGNullWindowID);
  const pid = '\"$PID\"' | 0;
  const n = arr ? arr.count : 0;
  for (let i = 0; i < n; i++) {
    const w = arr.objectAtIndex(i);
    if (w.objectForKey("kCGWindowOwnerPID") === pid) {
      const id = w.objectForKey("kCGWindowNumber");
      id.toString();
      id;
    }
  }' 2>/dev/null | tail -1 || true)

if [[ -z "$WID" || "$WID" == "undefined" ]]; then
  osascript -e 'tell application "'"$(/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$APP/Contents/Info.plist" 2>/dev/null)"'" to quit' >/dev/null 2>&1 || kill "$PID" 2>/dev/null || true
  print -r -- "{\"ok\":false,\"error\":\"no capturable window for pid $PID (app may be hidden-by-policy or capture permission missing)\"}"
  exit 3
fi

/usr/sbin/screencapture -x -l "$WID" "$OUT" || { echo "{\"ok\":false,\"error\":\"screencapture failed — grant Screen Recording to this terminal/host app\"}"; exit 3; }
kill "$PID" 2>/dev/null || true
BYTES=$(stat -f%z "$OUT" 2>/dev/null || echo 0)
print -r -- "{\"ok\":true,\"path\":\"$OUT\",\"bytes\":$BYTES,\"pid\":$PID}"
