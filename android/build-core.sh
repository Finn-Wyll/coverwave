#!/usr/bin/env bash
# Builds the shared Rust core (crates/cw-ffi) for Android and generates its
# Kotlin bindings. Needs the Android NDK (r27) and Rust:
#
#   android/build-core.sh              # arm64 phones + x86_64 emulators
#   ABIS="arm64-v8a" android/build-core.sh
#
# Output (git-ignored):
#   android/app/src/main/jniLibs/<abi>/libcw_ffi.so
#   android/app/src/main/java/uniffi/cw_ffi/cw_ffi.kt
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
ndk="${ANDROID_NDK_HOME:-$sdk/ndk/27.2.12479018}"
case "$(uname -s)" in
  Darwin) host=darwin-x86_64 ;;
  *) host=linux-x86_64 ;;
esac
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"
[ -d "$bin" ] || { echo "No NDK at $ndk (set ANDROID_NDK_HOME)"; exit 1; }
api=26

for abi in ${ABIS:-arm64-v8a x86_64}; do
  case "$abi" in
    arm64-v8a) target=aarch64-linux-android ;;
    x86_64) target=x86_64-linux-android ;;
    *) echo "unknown ABI $abi"; exit 1 ;;
  esac
  env_target="${target//-/_}"
  upper="$(echo "$env_target" | tr '[:lower:]' '[:upper:]')"
  rustup target add "$target" >/dev/null
  export "CC_$env_target=$bin/$target$api-clang"
  export "CXX_$env_target=$bin/$target$api-clang++"
  export "AR_$env_target=$bin/llvm-ar"
  export "CARGO_TARGET_${upper}_LINKER=$bin/$target$api-clang"
  cargo build -p cw-ffi --lib --release --target "$target"
  out="$here/app/src/main/jniLibs/$abi"
  mkdir -p "$out"
  "$bin/llvm-strip" -o "$out/libcw_ffi.so" "target/$target/release/libcw_ffi.so"
  first="${first:-target/$target/release/libcw_ffi.so}"
done

rm -rf "$here/app/src/main/java/uniffi"
cargo run -q -p cw-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$first" --language kotlin \
  --out-dir "$here/app/src/main/java"

echo "Core ready: android/app/src/main/jniLibs and uniffi/cw_ffi/cw_ffi.kt"
