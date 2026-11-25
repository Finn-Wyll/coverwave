# coverwave for iPhone

The SwiftUI app, built on the same Rust core as the terminal app
(`crates/cw-ffi`, through UniFFI).

## Run it on your iPhone

1. `cp Config/Local.example.xcconfig Config/Local.xcconfig` and fill in your
   Apple team, a bundle ID and your Spotify client ID (the scripts do the
   copy for you the first time).
2. Get the Rust core and the beat model, either:
   - **from GitHub** (no Rust build here): `ios/fetch-core.sh`. It downloads
     what the Release workflow built (`latest-build` release) and warns if
     your Rust code is newer than that build. Needs `gh auth login`.
   - **or build them here**: `ios/build-core.sh` (Rust toolchain, a few
     minutes, several GB in `target/`). The beat model comes from
     `tools/export_beat_model.py`.
3. `ios/run-on-iphone.sh`: generates the Xcode project (XcodeGen), builds,
   signs with your team and installs on the connected iPhone.

Apps signed with a free Apple account stop opening after 7 days; run step 3
again, or let the Mac do it: `ios/install-auto-resign.sh` sets up a launch
agent that checks every hour and, when the app expires within 2 days and
your iPhone is reachable (cable, or paired on the same Wi-Fi), re-signs and
reinstalls it and posts a notification. `ios/auto-resign.sh --check` says
what it would do; `--uninstall` on the installer removes it. It runs from
this checkout, so keep the folder. The Mac must be awake and logged in.
TestFlight needs a paid developer account.

## Files that are made, not committed

- `Frameworks/CoverwaveCore.xcframework`, `Coverwave/Generated/`: the Rust
  core and its Swift bindings.
- `Coverwave/Resources/BeatThis-small.onnx`: the Beat This! beat tracker
  (MIT, see `Coverwave/Resources/BeatThis-LICENSE.txt`).
- `Coverwave.xcodeproj`, `Coverwave/Info.plist`: generated from
  `project.yml`.
