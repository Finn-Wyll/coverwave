#!/usr/bin/env bash
# Builds the Swift app, signs it with the team in Config/Local.xcconfig and
# installs it on the connected iPhone. Needs the core first: ios/fetch-core.sh
# (built on GitHub) or ios/build-core.sh (built here).
#
#   ios/run-on-iphone.sh            # the first connected iPhone or iPad
#   ios/run-on-iphone.sh <UDID>     # a specific one (xcrun devicectl list devices)
#
# Xcode's build files go to ~/Library/Caches/coverwave-ios-build (about
# 0.5 GB; safe to delete).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

if [ ! -d Frameworks/CoverwaveCore.xcframework ]; then
  echo "No Rust core yet: run ios/fetch-core.sh or ios/build-core.sh first." >&2
  exit 1
fi

device="${1:-}"
if [ -z "$device" ]; then
  # The first connected physical device.
  device="$(xcrun devicectl list devices 2>/dev/null \
    | awk '/connected/ && /physical/ { for (i = 1; i <= NF; i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$|^[0-9A-F-]{25,}$/) { print $i; exit } }')"
fi
if [ -z "$device" ]; then
  echo "No connected iPhone found. Plug it in, unlock it, and trust this Mac." >&2
  exit 1
fi

derived="$HOME/Library/Caches/coverwave-ios-build"
xcodegen generate --quiet
xcodebuild -project Coverwave.xcodeproj -scheme Coverwave -configuration Debug \
  -destination "id=$device" -derivedDataPath "$derived" \
  -allowProvisioningUpdates -quiet build
xcrun devicectl device install app --device "$device" \
  "$derived/Build/Products/Debug-iphoneos/Coverwave.app"
echo "Installed on $device."
