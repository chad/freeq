#!/bin/bash
# Automated UI screenshot sweep for the freeq macOS client.
#
# Requires an UNLOCKED, active GUI session (the SwiftUI view lifecycle and the
# window server only run when the session is unlocked). Launches the app as a
# guest via the FREEQ_TEST_NICK env hook + the file-driven DebugBridge, then
# drives every feature and screenshots each step into OUT.
#
# Usage: freeq-macos/scripts/ui-sweep.sh [nick]
set -uo pipefail

NICK="${1:-chadsweep}"
OUT="${OUT:-/tmp/freeq-sweep}"
CMD="/tmp/freeq-cmd"
APP="$HOME/Library/Developer/Xcode/DerivedData/freeq-macos-ceipvkbzleecbidnnlkaquiqlawo/Build/Products/Debug/freeq.app"
BIN="$APP/Contents/MacOS/freeq"

mkdir -p "$OUT"
: > "$CMD"
caffeinate -d -t 1800 &
CAFF=$!

shot() { # shot <name>
  osascript -e 'tell application "System Events" to tell process "freeq" to set frontmost to true' >/dev/null 2>&1
  sleep 0.4
  # Capture freeq's window by CGWindowID — robust even if another window
  # (a permission dialog, the user's terminal) is on top.
  local id; id=$(/tmp/winid 2>/dev/null)
  if [ -n "$id" ]; then
    screencapture -x -o -l"$id" "$OUT/$1.png"
  else
    screencapture -x "$OUT/$1.png"
  fi
  echo "  shot $1.png (win ${id:-none})"
}
cmd() { echo "$1" >> "$CMD"; sleep "${2:-1.3}"; }

echo "==> launching freeq as guest '$NICK'"
pkill -x freeq 2>/dev/null; sleep 1
FREEQ_TEST_NICK="$NICK" FREEQ_CMD_FILE="$CMD" "$BIN" >/tmp/freeq-run.log 2>&1 &
sleep 8
# enlarge window
osascript -e 'tell application "System Events" to tell process "freeq" to set position of window 1 to {100,60}' \
          -e 'tell application "System Events" to tell process "freeq" to set size of window 1 to {1500,940}' 2>/dev/null
sleep 1
shot 00-connected

echo "==> channels & messaging"
# A fresh guest-owned channel (guests can't post to gated channels like #freeq).
cmd "#join #chadsweep-demo"
shot 01-join-channel
cmd "hello from the macOS client — automated sweep"
cmd "**bold** _italic_ \`code\` and a link https://bsky.app"
shot 02-messages
cmd "/me waves at the channel"
shot 03-action
cmd "/topic macOS feature-parity sweep in progress"; shot 04-topic

echo "==> media"
cmd "here is an image https://cdn.bsky.app/img/feed_thumbnail/plain/did:plc:z72i7hdynmk6r22z27h6tvur/bafkreib.jpg"
cmd "and a video https://example.com/sample.mp4 and audio https://example.com/clip.mp3"
shot 05-media

echo "==> reactions / reply / edit / delete"
cmd "/react 🔥"
shot 06-react
cmd "/reply this is a threaded reply"
shot 07-reply
cmd "a message I will edit"
cmd "/edit a message I just edited"
shot 08-edit
cmd "a message I will delete"
cmd "/delete"
shot 09-delete

echo "==> pins / search"
cmd "/pin"; shot 10-pin
cmd "/search hello"; shot 11-search

echo "==> panels & navigation"
cmd "#detail off"; shot 12-detail-off
cmd "#detail on"; shot 13-detail-on
cmd "#quickswitch on"; shot 14-quickswitch
cmd "#quickswitch off"
cmd "#channellist on"; shot 15-channellist
cmd "#channellist off"
cmd "#bookmarks on"; shot 16-bookmarks
cmd "#bookmarks off"
cmd "#search on"; shot 17-searchbar
cmd "#search off"

echo "==> voice/video call"
cmd "/av start" 2.5
shot 18-call
cmd "/av camera" 2.0
shot 19-call-camera
cmd "/av leave" 1.5
shot 20-call-left

echo "==> help + member list"
cmd "/help"; shot 21-help
cmd "#settings" ; shot 22-settings

echo "==> done. screenshots in $OUT"
kill $CAFF 2>/dev/null
ls -1 "$OUT"
