#!/usr/bin/env bash
# Builds the shared Rust core (crates/cw-ffi) for iOS and generates its Swift
# bindings. Run on a Mac with Xcode, from anywhere:
#
#   ios/build-core.sh            # release (default; debug is too slow for CLIP)
#   PROFILE=debug ios/build-core.sh
#
# Output (all git-ignored):
#   ios/Frameworks/CoverwaveCore.xcframework   device + Apple-silicon simulator
#   ios/Coverwave/Generated/CoverwaveCore.swift
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

profile="${PROFILE:-release}"
flag=$([ "$profile" = release ] && echo --release || true)
export IPHONEOS_DEPLOYMENT_TARGET=17.0

if [ ! -f "$here/Config/Local.xcconfig" ]; then
  cp "$here/Config/Local.example.xcconfig" "$here/Config/Local.xcconfig"
  echo "Created ios/Config/Local.xcconfig: add your team, bundle ID and Spotify client ID."
fi

targets=(aarch64-apple-ios aarch64-apple-ios-sim)
rustup target add "${targets[@]}" >/dev/null
for t in "${targets[@]}"; do
  cargo build -p cw-ffi --lib $flag --target "$t"
done

# Bindings come from the host build of the same code.
cargo build -p cw-ffi --lib $flag
gen="$root/target/uniffi-swift"
rm -rf "$gen"
cargo run -q -p cw-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$root/target/$profile/libcw_ffi.dylib" \
  --language swift --out-dir "$gen"

headers="$gen/include"
mkdir -p "$headers"
cp "$gen/cw_ffiFFI.h" "$headers/"
cp "$gen/cw_ffiFFI.modulemap" "$headers/module.modulemap"

mkdir -p "$here/Coverwave/Generated" "$here/Frameworks"
cp "$gen/cw_ffi.swift" "$here/Coverwave/Generated/CoverwaveCore.swift"

out="$here/Frameworks/CoverwaveCore.xcframework"
rm -rf "$out"
xcodebuild -create-xcframework \
  -library "$root/target/aarch64-apple-ios/$profile/libcw_ffi.a" -headers "$headers" \
  -library "$root/target/aarch64-apple-ios-sim/$profile/libcw_ffi.a" -headers "$headers" \
  -output "$out"

if [ ! -f "$here/Coverwave/Resources/BeatThis-small.onnx" ]; then
  echo "Note: the beat tracker is missing, so covers fall back to genre tempos."
  echo "      Make it with ios/tools/export_beat_model.py (see the comment at its top)."
fi

echo "Core ready. Next: cd ios && xcodegen generate && open Coverwave.xcodeproj"
