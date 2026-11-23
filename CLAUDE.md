# coverwave

Terminal music player in Rust. It renders the album cover as ASCII, and each cover gets its own audio-reactive motion. The full design, crate layout and milestones are in [PLAN.md](PLAN.md). Read it before making architectural changes, and update it when a decision changes.

## Status
M0 (workspace + TUI shell) and M1 (local playback) done. Next up: M2 (static ASCII album art). Track progress against the milestone table in PLAN.md section 13. Crates are only added when their milestone starts. The workspace currently holds `cw-core` (shared types + play queue), `cw-render`, `cw-audio` (decode/resample/playback) and `cw-tui`.

## Commands
- Run: `cargo run -p cw-tui --release -- <files or folders>` (`?` shows keys in the app)
- Check: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- Decode check without playing: `cargo run -p cw-audio --release --example probe -- <files or folders>`
- Test audio: generate with ffmpeg (e.g. `ffmpeg -f lavfi -i "sine=frequency=440:duration=10" -metadata title=X a.flac`). Never commit audio files.

## Audio design notes (cw-audio)
- Three threads: the cpal callback (ring buffer + atomics only), the decoder thread (owns `Queue` and the open `TrackReader`, decodes ~0.75s ahead), and the UI thread (sends `Command`s, reads a `Snapshot` per frame).
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
- Model files (`*.onnx`, `assets/models/`) are downloaded on first run and never committed.
- Do not commit secrets. Spotify tokens live in the OS keychain, and the config holds only the public client ID.

## User preferences
- The project owner wants clarifying questions asked before big design decisions.
- Keep the look restrained: one accent colour taken from the cover's palette, and motion that supports the art instead of overpowering it.
