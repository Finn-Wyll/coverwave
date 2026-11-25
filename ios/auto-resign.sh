#!/bin/bash
# Re-signs and reinstalls coverwave on your iPhone before its free-account
# signature runs out (free Apple accounts sign apps for 7 days).
#
# Run by the launch agent that ios/install-auto-resign.sh sets up. The
# agent does not poll: it wakes the day before the signature runs out. Then,
# if the iPhone is reachable (on the cable, or paired and on the same
# Wi-Fi), this removes coverwave's own expiring signing profile (so Xcode
# makes a fresh 7-day one), runs ios/run-on-iphone.sh and posts a
# notification, and schedules the next run for the day before the new
# expiry. If the phone is not reachable, or signing fails, it retries every
# hour until it works.
#
#   ios/auto-resign.sh             # what the agent runs
#   ios/auto-resign.sh --check     # only say what it would do
#   ios/auto-resign.sh --force     # re-sign now if the phone is reachable
#   ios/auto-resign.sh --schedule  # only set the agent's next run
#
# Log: ~/Library/Logs/coverwave-resign.log
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
app="$HOME/Library/Caches/coverwave-ios-build/Build/Products/Debug-iphoneos/Coverwave.app"
profiles="$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"
state="$HOME/Library/Caches/coverwave-resign-failed"
# Re-sign this long before expiry (hours).
hours="${COVERWAVE_RESIGN_HOURS:-24}"
mode="${1:-}"

say() { echo "$(date '+%Y-%m-%d %H:%M') $*"; }
notify() {
  osascript -e "display notification \"$1\" with title \"coverwave\"" >/dev/null 2>&1 || true
}
# A profile's expiry, as Unix seconds (empty if unreadable).
expiry_of() {
  local when
  when="$(security cms -D -i "$1" 2>/dev/null | plutil -extract ExpirationDate raw - 2>/dev/null)" || return 0
  date -j -u -f "%Y-%m-%dT%H:%M:%SZ" "$when" +%s 2>/dev/null || true
}

# Moves the agent to "--at EPOCH" or "--hourly", after this run has ended
# (the agent is reloaded, which would otherwise stop this script).
reschedule() {
  [ "$mode" = "--check" ] && return 0
  if [ "$mode" = "--schedule" ]; then
    "$here/install-auto-resign.sh" "$@"
  else
    ( sleep 3; "$here/install-auto-resign.sh" "$@" ) </dev/null &
  fi
}

now="$(date +%s)"
expires=""
[ -f "$app/embedded.mobileprovision" ] && expires="$(expiry_of "$app/embedded.mobileprovision")"
if [ -n "$expires" ]; then
  left=$(((expires - now) / 3600))
  status="signature has ${left}h left"
else
  left=-1
  status="no signed build found"
fi
due=$((left < hours))
[ "$mode" = "--force" ] && due=1

if [ "$due" -eq 0 ]; then
  next=$((expires - hours * 3600))
  [ "$mode" = "--check" ] && say "not due: $status; next run $(date -r "$next" '+%a %e %b %H:%M')"
  reschedule --at "$next"
  exit 0
fi

# The iPhone: plugged in first, else paired and reachable over Wi-Fi.
udid='{ for (i = 1; i <= NF; i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$|^[0-9A-F-]{25,}$/) { print $i; exit } }'
devices="$(xcrun devicectl list devices 2>/dev/null | grep -E 'iPhone|iPad' | grep physical || true)"
device="$(echo "$devices" | grep connected | awk "$udid" | head -1)"
[ -n "$device" ] || device="$(echo "$devices" | grep available | awk "$udid" | head -1)"
if [ -z "$device" ] || ! xcrun devicectl device info details --device "$device" >/dev/null 2>&1; then
  say "due ($status) but the iPhone is not reachable; trying again every hour"
  reschedule --hourly
  exit 0
fi
how="$(xcrun devicectl device info details --device "$device" 2>/dev/null | awk -F': ' '/Transport Type/ { print $2; exit }')"

if [ "$mode" = "--check" ]; then
  say "due ($status); would re-sign now on $device (${how:-unknown link})"
  exit 0
fi
if [ "$mode" = "--schedule" ]; then
  # Due now: the agent runs at load and every hour until it works.
  reschedule --hourly
  exit 0
fi

say "re-signing ($status) on $device (${how:-unknown link})"
bundle="$(awk -F'= *' '/^COVERWAVE_BUNDLE_ID/ { gsub(/ /, "", $2); print $2 }' "$here/Config/Local.xcconfig" 2>/dev/null)"
# Remove only coverwave's own profiles that are close to expiry.
if [ -n "$bundle" ] && [ -d "$profiles" ]; then
  for p in "$profiles"/*.mobileprovision; do
    [ -f "$p" ] || continue
    id="$(security cms -D -i "$p" 2>/dev/null | plutil -extract Entitlements.application-identifier raw - 2>/dev/null)"
    case "$id" in
      *".$bundle")
        e="$(expiry_of "$p")"
        if [ -z "$e" ] || [ $((e - now)) -lt $((hours * 3600)) ]; then
          say "removing expiring profile $(basename "$p")"
          rm -f "$p"
        fi
        ;;
    esac
  done
fi

if "$here/run-on-iphone.sh" "$device"; then
  rm -f "$state"
  new="$(expiry_of "$app/embedded.mobileprovision")"
  until_text="$([ -n "$new" ] && date -r "$new" '+%a %e %b %H:%M' || echo 'a week')"
  say "done: signed until $until_text"
  notify "Re-signed on your iPhone. Good until $until_text."
  [ -n "$new" ] && reschedule --at $((new - hours * 3600))
else
  say "re-signing failed (see above); trying again every hour"
  reschedule --hourly
  # One failure notification a day at most.
  if [ ! -f "$state" ] || [ $((now - $(stat -f %m "$state"))) -gt 86400 ]; then
    touch "$state"
    notify "Couldn't re-sign coverwave on your iPhone. See ~/Library/Logs/coverwave-resign.log"
  fi
  exit 1
fi
