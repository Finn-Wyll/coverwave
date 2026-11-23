# coverwave

Terminal music player in Rust. It renders the album cover as ASCII, and each cover gets its own audio-reactive motion. The full design, crate layout and milestones are in [PLAN.md](PLAN.md). Read it before making architectural changes, and update it when a decision changes.

## Status
M0 (workspace + TUI shell), M1 (local playback), M2 (static ASCII album art), M3 (audio analysis), M4 (cover analysis -> recipe JSON) and M5 (recipe-driven renderer, all layer kinds) done. Next up: M6 (library, search, queue, playlists, lyrics). Hands-on checks waiting for the owner are listed in LOCAL_TODO.md. The shipped `assets/pca.bin` is provisional (87 covers); see PLAN.md section 15. Track progress against the milestone table in PLAN.md section 13. Crates are only added when their milestone starts. The workspace currently holds `cw-core` (shared types, colour maths, play queue, `Recipe`), `cw-analysis` (cover decode, seed, palette, MobileCLIP via tract, features, subject mask, PCA, recipe mapping), `cw-render` (idle scene, static base art, `prepare`/`render`/`limit_changes` for recipe motion), `cw-audio` (decode/resample/playback, cover-art lookup, FFT/onset analyser), `cw-tui` (also downloads the model and caches recipes) and `tools/pca-fit`.

## Commands
- Run: `cargo run -p cw-tui --release -- <files or folders>` (`?` shows keys in the app)
- Check: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- Renderer perf: `cargo bench -p cw-render --bench render` (must stay under 4ms per frame at 240x70)
- Decode check without playing: `cargo run -p cw-audio --release --example probe -- <files or folders>`
- Analysis trace without playing: `cargo run -p cw-audio --release --example analyse -- <file>` (bands, level and onsets at 30fps as text)
- Recipe for one image: `cargo run -p cw-analysis --release --example recipe -- <vision_model.onnx> <image> [assets/pca.bin]`
- Cover-analysis tooling (`tools/pca-fit`): `prompts` (re-embed `assets/scene_prompts.json` with the text model), `fit` (refit `assets/pca.bin` from image/music folders), `scenes` (print scene weights per image). Model files come from `https://huggingface.co/Xenova/mobileclip_s0` (`onnx/vision_model.onnx`, `onnx/text_model.onnx`, `tokenizer.json`). Keep them outside the repo.
- Test audio: generate with ffmpeg (e.g. `ffmpeg -f lavfi -i "sine=frequency=440:duration=10" -metadata title=X a.flac`). Never commit audio files.

## Working without the owner's Mac (cloud or CI)
- There is usually no audio output device. `Player::new` then returns `AudioError::Device` and the TUI still runs, showing the error. No test opens a device, so `cargo test --workspace` works anywhere.
- Verify playback logic headlessly with the `probe` example and unit tests. Leave interactive checks (keys, listening for gaps) to the owner, and say so in reports.
- ffmpeg may not be installed. Tests write their own WAV fixtures, so they don't need it.
- No test needs the CLIP model or the network. The app downloads it on first run from `huggingface.co`, which redirects to `us.aws.cdn.hf.co` (both must be allowed). `--offline` skips the download, and `XDG_CACHE_HOME`/`XDG_CONFIG_HOME` point the model, recipe cache and overrides elsewhere. If you set `XDG_CONFIG_HOME` while using PulseAudio, copy `~/.config/pulse` into it or audio fails to connect.
- On Linux, cpal needs ALSA headers to build (`libasound2-dev` and `pkg-config`).
- To drive the real app headlessly (screenshots, track changes), give it a real-time fake device: install `pulseaudio libasound2-plugins`, then run `pulseaudio -D`, `pactl load-module module-null-sink sink_name=nul` and `pactl set-default-sink nul`, with `~/.asoundrc` set to `pcm.!default { type pulse }`. ALSA's own `null` device has no clock, so tracks "finish" instantly.

## Audio design notes (cw-audio)
- Four threads: the cpal callback (ring buffers + atomics only), the decoder thread (owns `Queue` and the open `TrackReader`, decodes ~0.75s ahead), the analyser thread (drains the callback's pre-volume mono tap into `Analyser`, publishes an `AudioFrame`), and the UI thread (sends `Command`s, reads a `Snapshot` and an `AudioFrame` per frame).
- Output is always stereo f32 at the device rate. `TrackReader` resamples with rubato and emits exactly `round(frames * ratio)` frames per track, which is what keeps track changes gapless.
- "What is playing" comes from `Boundary` markers in output-frame time compared with the callback's consumed-frame counter, not from the queue. The decoder is usually already decoding the next track.
- Seek, skip and jump flush the ring buffer through the callback (`flush` flag) before new audio is pushed.
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
  - At most about 15% of cells change per frame (up to 30% for high-energy covers).
- The audio callback thread never allocates or locks. It hands samples to other threads through a lock-free ring buffer (`rtrb`).

## Conventions
- Rust stable, edition 2024, and shared metadata/deps in the root `Cargo.toml` (`[workspace.package]`, `[workspace.dependencies]`). Run `cargo fmt` and `cargo clippy --workspace -- -D warnings` before committing.
- Tests: `cargo test --workspace`. Renderer changes need determinism, loop-seam and change-budget tests, plus `insta` golden frames. A perf bench (`criterion`) keeps `render()` under 4ms at 240x70.
- Errors: `thiserror` in libraries, `anyhow` in the frontend and tools.
- Model files (`*.onnx`, `assets/models/`) are downloaded on first run and never committed. MobileCLIP's weights are research/personal-use licensed: never redistribute them.
- Do not commit secrets. Spotify tokens live in the OS keychain, and the config holds only the public client ID.

## User preferences
- The project owner wants clarifying questions asked before big design decisions.
- Keep the look restrained: one accent colour taken from the cover's palette, and motion that supports the art instead of overpowering it.
