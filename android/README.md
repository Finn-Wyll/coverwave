# coverwave for Android

The Android app: the same Rust core as the terminal and iPhone apps
(`crates/cw-ffi`, through UniFFI's Kotlin bindings), with a Jetpack Compose
UI that mirrors the iPhone app (gestures, HUD, library, queue, lyrics,
offline mode, settings).

## Install

Download `coverwave-android.apk` from the
[latest build](https://github.com/Finn-Wyll/coverwave/releases/tag/latest-build)
on the phone, open it, and allow installing from your browser or files app.
Android 8 or newer. Every build is signed with the same key
(`coverwave-debug.keystore`, committed on purpose), so new builds install over
old ones.

## Sources

| Source | How |
|---|---|
| Spotify | the Web API (PKCE login in a browser tab), like the other apps |
| Apple Music | Android's media sessions: Apple Music, or any other player that shows media controls. Needs **Notification access** for coverwave (Settings opens there; notifications themselves are ignored). Android gives no way to browse Apple Music's library from another app, so the library panel says so; the queue is the player's own when it shares one. |
| Offline | Spotify's App Remote SDK, talking to the Spotify app on the phone while it plays downloads |

### Spotify setup

In your app at developer.spotify.com:

- Redirect URI: `coverwave://spotify-callback`
- Android package: `app.coverwave`, SHA-1
  `1F:C1:54:69:E7:2A:EB:2D:2A:42:44:F8:C3:AD:0E:28:49:38:74:32`
  (the committed key's fingerprint; Offline needs it).

The client ID is built in when the repository variable `SPOTIFY_CLIENT_ID`
is set (Settings > Secrets and variables > Actions > Variables), or when
`spotify.clientId=...` is in `android/local.properties` for local builds.
Without one, paste it into the app's Settings.

## Build it yourself

Needs Rust, JDK 17 and the Android SDK with NDK 27.2.12479018.

```sh
android/build-core.sh                 # Rust core for arm64 + x86_64, Kotlin bindings
curl -L -o android/app/src/main/assets/BeatThis-small.onnx \
  https://github.com/Finn-Wyll/coverwave/releases/download/latest-build/BeatThis-small.onnx
cd android && ./gradlew :app:assembleRelease
adb install -r app/build/outputs/apk/release/app-release.apk
```

`./gradlew :app:testReleaseUnitTest` runs the Kotlin bindings against a host
build of the core (`cargo build -p cw-ffi --release` first).

## Files that are made, not committed

- `app/src/main/jniLibs/`, `app/src/main/java/uniffi/`: the Rust core and
  its bindings (`build-core.sh`).
- `app/src/main/assets/BeatThis-small.onnx`: the Beat This! beat tracker
  (MIT), from the Release workflow or `ios/tools/export_beat_model.py`.

`app/libs/spotify-app-remote-release-0.8.0.aar` is Spotify's App Remote SDK
(Apache-2.0, from github.com/spotify/android-sdk), vendored with its licence.
