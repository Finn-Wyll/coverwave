#!/bin/bash
# Sets up (or removes) the launch agent that runs ios/auto-resign.sh every
# hour while you are logged in, so coverwave on your iPhone never expires.
#
#   ios/install-auto-resign.sh              # set up and start
#   ios/install-auto-resign.sh --uninstall  # stop and remove
#
# It runs from this checkout, so keep the folder where it is (moving or
# deleting it stops the re-signing; run this again from the new place).
set -euo pipefail

label="local.coverwave.auto-resign"
plist="$HOME/Library/LaunchAgents/$label.plist"
script="$(cd "$(dirname "$0")" && pwd)/auto-resign.sh"
domain="gui/$(id -u)"

launchctl bootout "$domain/$label" 2>/dev/null || true
if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$plist"
  echo "Removed: coverwave is no longer re-signed automatically."
  exit 0
fi

chmod +x "$script" "$(dirname "$script")/run-on-iphone.sh"
mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$script</string>
  </array>
  <key>StartInterval</key>
  <integer>3600</integer>
  <key>RunAtLoad</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>$HOME/Library/Logs/coverwave-resign.log</string>
  <key>StandardErrorPath</key>
  <string>$HOME/Library/Logs/coverwave-resign.log</string>
</dict>
</plist>
EOF
launchctl bootstrap "$domain" "$plist"
echo "Set up: every hour, coverwave is re-signed if it expires within 2 days"
echo "and your iPhone is reachable. Log: ~/Library/Logs/coverwave-resign.log"
"$script" --check
