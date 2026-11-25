#!/bin/bash
# Sets up (or removes) the launch agent that runs ios/auto-resign.sh, so
# coverwave on your iPhone never expires. The agent does not poll: it is
# scheduled for the day before the signature runs out, and only retries
# hourly from then until a re-sign works (auto-resign.sh moves it between
# the two).
#
#   ios/install-auto-resign.sh              # set up (schedules itself)
#   ios/install-auto-resign.sh --uninstall  # stop and remove
#
# Used by auto-resign.sh:  --at EPOCH (one run at that time)  |  --hourly
#
# It runs from this checkout, so keep the folder where it is (moving or
# deleting it stops the re-signing; run this again from the new place).
set -euo pipefail

label="local.coverwave.auto-resign"
plist="$HOME/Library/LaunchAgents/$label.plist"
here="$(cd "$(dirname "$0")" && pwd)"
script="$here/auto-resign.sh"
domain="gui/$(id -u)"
# What the agent is set to now ("at EPOCH" or "hourly").
current="$HOME/Library/Caches/coverwave-resign-schedule"

case "${1:-}" in
  --uninstall)
    launchctl bootout "$domain/$label" 2>/dev/null || true
    rm -f "$plist" "$current"
    echo "Removed: coverwave is no longer re-signed automatically."
    exit 0
    ;;
  "")
    chmod +x "$script" "$here/run-on-iphone.sh"
    rm -f "$current"
    # Works out when it is next needed and schedules itself.
    "$script" --schedule
    echo "Set up. Log: ~/Library/Logs/coverwave-resign.log"
    "$script" --check
    exit 0
    ;;
  --at) when="${2:?--at needs a time (seconds since 1970)}"; want="at $when" ;;
  --hourly) want="hourly" ;;
  *) echo "usage: $0 [--uninstall]" >&2; exit 2 ;;
esac

# Already set this way: nothing to do (also what stops a reload from
# starting the job again and again).
[ "$(cat "$current" 2>/dev/null)" = "$want" ] && exit 0

if [ "$want" = hourly ]; then
  timing="  <key>StartInterval</key>
  <integer>3600</integer>"
else
  timing="  <key>StartCalendarInterval</key>
  <dict>
    <key>Month</key><integer>$(date -r "$when" +%-m)</integer>
    <key>Day</key><integer>$(date -r "$when" +%-d)</integer>
    <key>Hour</key><integer>$(date -r "$when" +%-H)</integer>
    <key>Minute</key><integer>$(date -r "$when" +%-M)</integer>
  </dict>"
fi

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
$timing
  <!-- At login: catches a time missed while the Mac was off. -->
  <key>RunAtLoad</key>
  <true/>
  <!-- auto-resign.sh reschedules from inside the job. -->
  <key>AbandonProcessGroup</key>
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
echo "$want" > "$current"
launchctl bootout "$domain/$label" 2>/dev/null || true
launchctl bootstrap "$domain" "$plist"
if [ "$want" = hourly ]; then
  echo "$(date '+%Y-%m-%d %H:%M') scheduled: every hour until a re-sign works"
else
  echo "$(date '+%Y-%m-%d %H:%M') scheduled: $(date -r "$when" '+%a %e %b %H:%M') (the day before expiry)"
fi
