# coverwave

Terminal music player in Rust. It renders the album cover as ASCII, and each cover gets its own audio-reactive motion. The full design, crate layout and milestones are in [PLAN.md](PLAN.md). Read it before making architectural changes, and update it when a decision changes.

## Status
M0 (workspace + TUI shell), M1 (local playback), M2 (static ASCII album art), M3 (audio analysis), M4 (cover analysis -> recipe JSON), M5 (recipe-driven renderer, all layer kinds) and M6 (library, search, queue, playlists, lyrics) done. M7 (system capture + Spotify + Apple Music) is built and tested on Linux. Its macOS code (Core Audio tap, AppleScript, keychain) is type-checked only and awaits the owner's Mac (LOCAL_TODO.md). M8 (Windows) is built: paths, colour and browser handling, and a CI workflow. It is unverified on real Windows because the CI is manual-only and has not run yet. All eight milestones are now built. After M8: genre metadata and recipe selection v2 (31 layer kinds, 16 scenes, genre + cover + seed scoring); see PLAN.md section 5. Remote tracks that cannot be heard move to a beat clock (`cw_core::BeatClock`) at a tempo looked up from Deezer (`cw_sources::tempo`); on iOS they replay a rhythm map (`cw_core::RhythmMap`) measured from the Deezer preview by the bundled Beat This! model (`cw_analysis::rhythm`), lined up with the song because Deezer previews start 48 s in; see PLAN.md section 9. The iOS app does not use the microphone: recording makes iOS lower the music (owner, 2026-11-25). A native iOS app (`ios/`, SwiftUI over the shared core via the UniFFI crate `crates/cw-ffi`) is written but not yet built; `ios/build-core.sh` then `xcodegen generate` in `ios/`. Hands-on checks waiting for the owner are listed in LOCAL_TODO.md. The shipped `assets/pca.bin` is provisional (87 covers); see PLAN.md section 15. Track progress against the milestone table in PLAN.md section 13. Crates are only added when their milestone starts. The workspace currently holds `cw-core` (shared types, colour maths, play queue, `Recipe`), `cw-analysis` (cover decode, seed, palette, MobileCLIP via tract, features, subject mask, PCA, recipe mapping), `cw-render` (idle scene, static base art, `prepare`/`render`/`limit_changes` for recipe motion), `cw-audio` (decode/resample/playback, cover-art lookup, FFT/onset analyser), `cw-library` (SQLite index, fuzzy search, m3u8 playlists, lyrics, saved queue), `cw-capture` (system audio: macOS process tap, cpal loopback/input elsewhere), `cw-sources` (Spotify Web API with PKCE, Apple Music via osascript, `RemoteHandle` polling threads), `cw-tui` (also config, model download, recipe cache, library browser) and `tools/pca-fit`.

## Terminal hygiene (cw-tui/src/term.rs)
- stderr is redirected to `~/.cache/coverwave/coverwave.log` while the UI runs, and frames are drawn inside synchronized updates. Never print to stdout/stderr from library code, and give child processes null stdio, or the screen gets corrupted. ctrl-l forces a full repaint.

## Commands
- Run: `cargo run -p cw-tui --release -- <files or folders>` (`?` shows keys in the app). The binary is named `coverwave`.
- Install the `coverwave` command (to `~/.cargo/bin`, which the owner's `~/.zshrc` puts on PATH): `cargo install --path frontends/cw-tui`. Re-run it after pulling to update; `cargo uninstall cw-tui` removes it.
- Check: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- Renderer perf: `cargo bench -p cw-render --bench render` (must stay under 4ms per frame at 240x70)
- Capture check without the TUI: `cargo run -p cw-capture --example meter -- [SECONDS] [BUNDLE_ID ...]`
- Type-check the platform code from Linux (`rustup target add aarch64-apple-darwin x86_64-pc-windows-msvc` once): `cargo clippy -p cw-capture -p cw-sources --no-default-features --target aarch64-apple-darwin -- -D warnings`, and the same with `--target x86_64-pc-windows-msvc`. Crates that compile C (bundled SQLite, ring for TLS) cannot be checked this way, which is why `cw-sources` has an `http` default feature to switch off for the check.
- CI: `.github/workflows/ci.yml` (fmt, clippy, tests and a release build on Linux, Windows and macOS) and `.github/workflows/release.yml` (builds the iOS core, the beat model and `cw-tui` binaries, and publishes them as the rolling `latest-build` pre-release). Both run on pushes to main only while the repo is public (free Actions minutes); on a private repo they run only by hand, by the owner's choice (2026-11-24). Keep that guard (`!github.event.repository.private`) on any new automatic trigger, and never start runs without asking.
- iPhone without building Rust locally: `ios/fetch-core.sh` (downloads the core and beat model from `latest-build`), then `ios/run-on-iphone.sh` (builds the Swift app, signs it with the team in `ios/Config/Local.xcconfig`, installs it on the connected iPhone). Signing stays on the Mac: the owner's free Apple account cannot sign on GitHub, and TestFlight needs a paid one.
- Disk: `target/` only grows (Cargo keeps every old build). Debug builds carry line tables only (`[profile.dev]` in Cargo.toml). `cargo clean` frees it all.
- Beat tracker check against PyTorch, or rhythm maps of real clips: `cargo run -p cw-analysis --release --example beat_check -- <BeatThis-small.onnx> <reference dir> | --wav <clip.wav>...`
- Tempo of a remote track: `cargo run -p cw-sources --release --example tempo_lookup -- "ARTIST" "TITLE"`; of a file: `cargo run -p cw-audio --release --example tempo -- <file>`.
- Decode check without playing: `cargo run -p cw-audio --release --example probe -- <files or folders>`
- Analysis trace without playing: `cargo run -p cw-audio --release --example analyse -- <file>` (bands, level and onsets at 30fps as text)
- Recipe for one image: `cargo run -p cw-analysis --release --example recipe -- <vision_model.onnx> <image> [assets/pca.bin]`
- Cover-analysis tooling (`tools/pca-fit`): `prompts` (re-embed `assets/scene_prompts.json` with the text model), `fit` (refit `assets/pca.bin` from image/music folders), `scenes` (print scene weights per image). Model files come from `https://huggingface.co/Xenova/mobileclip_s0` (`onnx/vision_model.onnx`, `onnx/text_model.onnx`, `tokenizer.json`). Keep them outside the repo.
- Test audio: generate with ffmpeg (e.g. `ffmpeg -f lavfi -i "sine=frequency=440:duration=10" -metadata title=X a.flac`). Never commit audio files.

## Working without the owner's Mac (cloud or CI)
- There is usually no audio output device. `Player::new` then returns `AudioError::Device` and the TUI still runs, showing the error. No test opens a device, so `cargo test --workspace` works anywhere.
- Verify playback logic headlessly with the `probe` example and unit tests. Leave interactive checks (keys, listening for gaps) to the owner, and add them to LOCAL_TODO.md.
- ffmpeg may not be installed. Tests write their own WAV fixtures, so they don't need it.
- File locations are resolved in `frontends/cw-tui/src/paths.rs` (XDG layout on macOS/Linux, `%APPDATA%`/`%LOCALAPPDATA%` on Windows). The app's files live under `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and `XDG_DATA_HOME` (`config.toml` and overrides, model and recipes, library.db/playlists/queue.json). Point all three at a scratch folder for headless runs.
- No test needs the CLIP model or the network. The app downloads it on first run from `huggingface.co`, which redirects to `us.aws.cdn.hf.co` (both must be allowed). The tempo lookup uses `api.deezer.com` and its preview CDN (`cdnt-preview.dzcdn.net`); tests never touch them. `--offline` skips the download, and `XDG_CACHE_HOME`/`XDG_CONFIG_HOME` point the model, recipe cache and overrides elsewhere. If you set `XDG_CONFIG_HOME` while using PulseAudio, copy `~/.config/pulse` into it or audio fails to connect.
- On Linux, cpal needs ALSA headers to build (`libasound2-dev` and `pkg-config`).
- macOS: if `D` shows `capture on` but the meters stay flat with Spotify or Apple Music playing, the process tap is being fed silence because the audio-recording permission is missing. Grant it in System Settings > Privacy & Security > Screen & System Audio Recording (for the terminal app running coverwave). This is not a code bug; the standalone `cw-capture` meter can work while coverwave does not.
- To test remote sources headlessly:
  - Capture: with the PulseAudio null sink below, run `pactl set-default-source nul.monitor` and play audio with `paplay`. Capture then hears it.
  - Spotify: run a mock server and point the app at it with `COVERWAVE_SPOTIFY_API=http://127.0.0.1:PORT/v1` and `COVERWAVE_SPOTIFY_ACCOUNTS=http://127.0.0.1:PORT`. Set `BROWSER=<script>`, where the script receives the authorize URL and calls `http://127.0.0.1:8898/callback?code=x&state=<state from the URL>`.
  - The macOS keyring is not available on Linux, so logins there only last for the session.
- To drive the real app headlessly (screenshots, track changes), give it a real-time fake device: install `pulseaudio libasound2-plugins`, then run `pulseaudio -D`, `pactl load-module module-null-sink sink_name=nul` and `pactl set-default-sink nul`, with `~/.asoundrc` set to `pcm.!default { type pulse }`. ALSA's own `null` device has no clock, so tracks "finish" instantly.

## Audio design notes (cw-audio)
- Four threads: the cpal callback (ring buffers + atomics only), the decoder thread (owns `Queue` and the open `TrackReader`, decodes ~0.75s ahead), the analyser thread (drains the callback's pre-volume mono tap into `Analyser`, publishes an `AudioFrame`), and the UI thread (sends `Command`s, reads a `Snapshot` and an `AudioFrame` per frame).
- Output is always stereo f32 at the device rate. `TrackReader` resamples with rubato and emits exactly `round(frames * ratio)` frames per track, which is what keeps track changes gapless.
- "What is playing" comes from `Boundary` markers in output-frame time compared with the callback's consumed-frame counter, not from the queue. The decoder is usually already decoding the next track.
- Seek, skip and jump flush the ring buffer through the callback (`flush` flag) before new audio is pushed. The callback claims the flag with a compare-exchange and never touches `consumed` during a flush. The decoder sets `consumed = pushed` and cancels the flag if the callback did not answer within 250ms. (Before M6 a late callback could drain the next track's first 0.75s.)
- The player's track list can change: `replace(tracks, start, secs)` and `append(track)`. Snapshots from an older list (generation mismatch) read as stopped.
- Library dependency versions are newer than most docs and examples online (cpal 0.18, symphonia 0.6, rubato 5). Check the sources in `~/.cargo/registry` before guessing an API.

## Architecture rules (do not break)
- Cargo workspace: `crates/cw-*` libraries, `frontends/cw-tui` (the only v1 frontend), `tools/pca-fit`.
- `cw-core`, `cw-analysis` and `cw-render` must stay platform-agnostic: no terminal, OS-audio, network or filesystem-path assumptions. They will later be shared with Windows/iOS/Android ports.
- `cw-render` is pure: `render(recipe, base, mask, audio_frame, frame_idx) -> CellGrid`. No clocks, no global RNG, no I/O.
- All randomness comes from the cover seed (first 8 bytes of SHA-256 of the cover) through `rand_chacha`. The same cover must always give a byte-identical recipe and identical frames.
- Album motion is driven only by the recipe JSON. Never hard-code per-album or per-artist behaviour.
- Motion constraints:
  - Subject-mask cells and HUD cells never change.
  - Every animation loops seamlessly over `loop_frames` (integer `cycles`, phase-based).
  - Audio only modulates amplitude, never phase or speed.
  - A soft change budget keeps motion from turning into noise: about 25% of cells per frame steady (40% for high-energy covers), and beats may burst well past it (up to 75%), decaying with the beat. These are tuning values, not design rules: the owner wants lively, beat-driven motion (2026-11-24).
  - Beats show as a shockwave that starts at the subject's edge and expands as the hit decays (`beat_ring` in `cw-render/src/scene.rs`), plus a bass thump, glitch slices on the hardest hits, and lightning on some beats for dark/ocean covers. Keep beat effects spatial and anchored to the art, not full-screen flashes (photosensitivity).
  - Stage effects (margin glow, equaliser, thump, glitch) apply to every cover with strength from the cover's energy; per-cover character still comes only from recipe layers. Recipe version 2 added `aurora`, `starfield`, `digital_rain` and `lightning`; version 3 merged them with the genre-aware selection's other 13 kinds (31 in all).
  - Event effects (lightning, glitch) choose which beats fire from `hash(seed, onset_count)`, so the renderer stays pure.
  - Motion eases to still while playback is paused and back on resume (`App::motion_level`).
- The audio callback thread never allocates or locks. It hands samples to other threads through a lock-free ring buffer (`rtrb`).

## Conventions
- Rust stable, edition 2024, and shared metadata/deps in the root `Cargo.toml` (`[workspace.package]`, `[workspace.dependencies]`). Run `cargo fmt` and `cargo clippy --workspace -- -D warnings` before committing.
- Tests: `cargo test --workspace`. Renderer changes need determinism, loop-seam and change-budget tests, plus `insta` golden frames. A perf bench (`criterion`) keeps `render()` under 4ms at 240x70.
- Errors: `thiserror` in libraries, `anyhow` in the frontend and tools.
- Model files (`*.onnx`, `assets/models/`) are downloaded on first run and never committed. The iOS beat tracker (`ios/Coverwave/Resources/BeatThis-small.onnx`, MIT) is bundled in the app but still not committed: make it with `ios/tools/export_beat_model.py` (needs a Python 3.12 venv with PyTorch; see the script's header). Without it the app falls back to genre tempos. MobileCLIP's weights are research/personal-use licensed: never redistribute them.
- Do not commit secrets. Spotify tokens live in the OS keychain, and the config holds only the public client ID.

## User preferences
- The project owner wants clarifying questions asked before big design decisions.
- Keep the look restrained: one accent colour taken from the cover's palette, and motion that supports the art instead of overpowering it.
