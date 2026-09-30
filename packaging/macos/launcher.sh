#!/bin/bash
# CFBundleExecutable: opens ruDI.FM in a new Terminal window, since it's a TUI app
# and Finder launches .app bundles without an attached terminal.
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/rudi-fm"

if ! command -v mpv >/dev/null 2>&1; then
  osascript -e 'display alert "mpv fehlt" message "ruDI.FM braucht mpv. Installieren mit: brew install mpv" as critical'
  exit 1
fi

osascript <<EOF
tell application "Terminal"
    activate
    do script "\"$BIN\""
end tell
EOF
