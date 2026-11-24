#!/usr/bin/env bash
# Downloads the iOS Rust core, its Swift bindings and the beat model that
# GitHub built (the "latest-build" release, see .github/workflows/release.yml),
# instead of building them here. Needs the GitHub CLI, logged in (`gh auth
# login`).
#
#   ios/fetch-core.sh
#
# Warns if the build is older than your checkout's Rust code; then either
# push and wait for the Release workflow, or build locally with
# ios/build-core.sh.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

gh release download latest-build --dir "$tmp" \
  --pattern 'CoverwaveCore.xcframework.zip' \
  --pattern 'CoverwaveCore.swift' \
  --pattern 'BeatThis-small.onnx' \
  --pattern 'build-info.txt'

built="$(awk '/^commit/ {print $2}' "$tmp/build-info.txt")"
echo "Core built from ${built:0:7} ($(awk '/^built/ {print $2}' "$tmp/build-info.txt"))."
if ! git cat-file -e "$built^{commit}" 2>/dev/null; then
  echo "Warning: that commit is not in your checkout. Run git pull."
elif ! git diff --quiet "$built" HEAD -- crates Cargo.toml Cargo.lock assets; then
  echo "Warning: your Rust code has changed since that build. Push and wait for"
  echo "         the Release workflow, or run ios/build-core.sh to build it here."
fi

mkdir -p "$here/Frameworks" "$here/Coverwave/Generated" "$here/Coverwave/Resources"
rm -rf "$here/Frameworks/CoverwaveCore.xcframework"
unzip -q "$tmp/CoverwaveCore.xcframework.zip" -d "$here/Frameworks"
cp "$tmp/CoverwaveCore.swift" "$here/Coverwave/Generated/CoverwaveCore.swift"
cp "$tmp/BeatThis-small.onnx" "$here/Coverwave/Resources/BeatThis-small.onnx"

if [ ! -f "$here/Config/Local.xcconfig" ]; then
  cp "$here/Config/Local.example.xcconfig" "$here/Config/Local.xcconfig"
  echo "Created ios/Config/Local.xcconfig: add your team, bundle ID and Spotify client ID."
fi

echo "Core ready. Next: ios/run-on-iphone.sh (or: cd ios && xcodegen generate && open Coverwave.xcodeproj)"
