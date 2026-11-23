# coverwave

Terminal music player in Rust. It renders the album cover as ASCII, and each cover gets its own audio-reactive motion. The full design, crate layout and milestones are in [PLAN.md](PLAN.md). Read it before making architectural changes, and update it when a decision changes.

## Status
M0 done (workspace + TUI shell with an idle scene). Next up: M1 (local playback). Track progress against the milestone table in PLAN.md section 13. Crates are only added when their milestone starts. The workspace currently holds `cw-core`, `cw-render` and `cw-tui`.

## Commands
- Run: `cargo run -p cw-tui --release` (`tab` switches mode, `D` toggles debug, `q` quits)
- Check: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

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
