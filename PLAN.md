# ASCII Album-Art Music Player: Architecture & Build Plan

Working name: `coverwave` (rename anytime).

**Design read:** a terminal music player where the album cover, drawn as ASCII, is the whole show. Each cover gets its own motion, derived from what is in the image, tinted with the cover's own palette and pulsing with the audio. Motion supports the art and never buries it.

Inspiration: an ASCII Christmas tree reel. A point cloud is shaded with density characters on near-black, with snow particles drifting behind it at 30fps.

---

## 1. Goals and non-goals

**Goals (v1)**
- Play local audio (mp3, flac, ogg, wav, m4a/aac, opus).
- Remote-control Spotify and Apple Music, with their cover art and metadata.
- Reactive visuals for *any* source by capturing system audio.
- Per-album motion that is generated, deterministic and inspectable (recipe JSON). There are no hand-written per-album rules.
- Switchable views: fullscreen art, and library/queue.
- Library browser with fuzzy search, queue, saved playlists, synced lyrics.
- Ports should stay possible: the core is platform-agnostic, and the terminal is only one frontend.

**Non-goals (v1)**
- Streaming audio *through* the player from Spotify/Apple Music. Their APIs do not allow it.
- Mobile apps. The architecture keeps the door open, but nothing for mobile ships in v1.
- Editing tags, transcoding, or managing downloads.

---

## 2. Workspace layout

A Cargo workspace. Everything below `frontends/` must stay free of terminal, OS-audio and network dependencies so that it can later compile for iOS/Android (through FFI or UniFFI).

```
coverwave/
  crates/
    cw-core/        shared types: Track, Source, PlaybackState, Recipe, Palette, CellGrid
    cw-analysis/    cover -> Recipe (CLIP, PCA, k-means, features, subject mask, seed)
    cw-render/      Recipe + base art + audio bands + t -> CellGrid (pure, deterministic)
    cw-audio/       decode + playback (local), FFT bands, onset/beat detection
    cw-capture/     system-audio capture trait + per-OS backends
    cw-sources/     local library scan, Spotify Web API, Apple Music bridge
    cw-library/     SQLite index, search, queue, playlists, lyrics loading
  frontends/
    cw-tui/         ratatui + crossterm terminal app (the only v1 frontend)
  tools/
    pca-fit/        one-off: fits the fixed 8-dim PCA over a cover corpus
  assets/
    models/         MobileCLIP ONNX (downloaded on first run, not committed)
    pca.bin         shipped PCA matrix (output of tools/pca-fit)
    scene_prompts.json
```

### Key crates
| Concern | Crate |
|---|---|
| TUI | `ratatui`, `crossterm` |
| Decode | `symphonia` |
| Output | `cpal` (directly, so playback and capture share one audio stack) |
| FFT | `rustfft` |
| Tags / embedded art / lyrics | `lofty` |
| Images | `image`, `fast_image_resize` |
| CLIP inference | `ort` (ONNX Runtime) with MobileCLIP-S0/S1 image + text encoders |
| Hashing / seed | `sha2` |
| Seeded RNG | `rand_chacha` |
| DB | `rusqlite` (bundled) |
| Fuzzy search | `nucleo` |
| Spotify | `rspotify` (OAuth PKCE) |
| Config / recipe | `serde`, `serde_json`, `toml` |
| macOS capture | Core Audio process taps (macOS 14.2+), fallback `screencapturekit` (13+) |

---

## 3. Data flow

```
             +-------------------+        +------------------+
 local file->| cw-audio decode   |--PCM-->| cpal output      |
             +---------+---------+        +------------------+
                       | PCM (tap)
 Spotify/Apple ------> | system audio via cw-capture (loopback)
                       v
             +-------------------+   bands, rms, onset (every audio block)
             | analyser (FFT)    |----------------------------+
             +-------------------+                            |
                                                              v
 cover image -> cw-analysis --(cached by SHA-256)--> Recipe -> cw-render -> CellGrid -> cw-tui
                    ^                                  ^
           MobileCLIP + PCA + k-means          base ASCII art + subject mask (precomputed per size)
```

Threads:
- **Audio thread** (cpal callback): mixes output, pushes samples into a lock-free ring buffer (`rtrb`).
- **Analyser thread:** reads the ring buffer, computes FFT bands, and publishes an `AudioFrame` through an atomic snapshot (`arc-swap`).
- **Analysis worker:** runs cover analysis off the UI thread, and the cache makes it near-instant on repeat plays.
- **UI thread:** fixed 30fps tick (configurable to 60). Reads the latest `AudioFrame`, renders, and diffs to the terminal.

---

## 4. Stage 1: Analyse (once per cover, cached)

Cache key: `sha256(cover bytes)`. Cache dir: `~/.cache/coverwave/recipes/<hash>.json` plus `<hash>.features.bin`.

1. **Scene semantics.** Zero-shot MobileCLIP against `scene_prompts.json`:
   `snow, lava, very dark scene, forest, ocean, city at night, desert, sky`
   (extensible, with a version number stored in the cache). Keep the full softmax (temperature 100, as CLIP does) as `scene_weights` and never take an argmax. Text embeddings are computed once and cached.
2. **Style fingerprint.** Project the 512-dim image embedding onto the shipped `pca.bin`, which gives 8 floats. Normalise each with the corpus mean/std stored in `pca.bin`, then squash to [-1, 1] with `tanh`. This is the `style_dna`.
3. **Visual features.**
   - Palette: k-means (k=5, seeded, in Lab colour space) on a 64x64 downsample, sorted by cluster size.
   - Mean brightness, RMS contrast, mean saturation (HSV), colour temperature (estimated CCT from mean chromaticity).
   - Edge density: Sobel magnitude above a threshold, as a fraction of pixels.
   - **Subject mask:** v1 uses a heuristic. It scores saliency from colour distance to the border colour plus a centre prior plus edge density, then applies Otsu's threshold and a morphological close. v1.1 optionally swaps in U²-Netp ONNX (~4.7MB) for real salient-object masks. The mask is stored at 64x64 and resampled to the cell grid at render time.
4. **Seed.** The first 8 bytes of the SHA-256, read as a `u64`.

Analysis budget: under 300ms per new cover on an M-series Mac, and under 5ms on a cache hit.

---

## 5. Stage 2: Map to a motion recipe

### Effect layers (one per scene)
| Scene | Layer | Core parameters |
|---|---|---|
| snow | `falling_particles` | density, fall speed, drift, glyphs `. * +` |
| lava | `heat_cycle` + `embers` | noise scale, palette cycle speed, ember rise rate |
| very dark | `flicker` + `fog` | flicker rate, fog density, fog drift vector |
| forest | `row_sway` + `fireflies` | sway amplitude, wavelength, firefly count/blink |
| ocean | `ripple` | displacement amplitude, wavelength, direction |
| city at night | `neon_flicker` + `rain` | flicker cells/rate, rain angle, density |
| desert | `heat_shimmer` + `dust` | shimmer amplitude, dust drift |
| sky | `cloud_drift` | layer count, drift speed, softness |

### Rules
- `layer.intensity = scene_weight`. Drop layers with intensity below 0.10, sort by intensity and keep the top 3.
- **Style DNA → global params** (each mapping is linear and clamped, defined in one table in `cw-analysis/src/mapping.rs`):
  - dna[0] → overall speed multiplier (0.5–1.6)
  - dna[1] → wind direction (angle)
  - dna[2] → particle density
  - dna[3] → flicker rate
  - dna[4] → sway amplitude
  - dna[5] → charset: `sparse` (` .:`), `standard` (` .:-=+*#%@`), `blocks` (` ░▒▓█`)
  - dna[6], dna[7] → reserved, kept in the JSON for future tuning
- **Visual features adjust the result:**
  - `energy = 0.5*saturation + 0.5*edge_density`. Energy scales speed and density.
  - Particle and glow colours come from the palette (accent = most saturated cluster above a minimum size).
  - If brightness is below 0.25, contrast-stretch (percentile 2–98) before ASCII conversion.
- **Seed** drives every random choice: particle positions, phases, flicker schedules (ChaCha8). This gives the guarantee that an identical cover produces an identical animation.

### Recipe JSON (example)
```json
{
  "version": 1,
  "cover_hash": "9f2c…",
  "seed": 1183742009128734,
  "scene_weights": { "ocean": 0.61, "sky": 0.22, "very dark scene": 0.08, "...": 0.0 },
  "style_dna": [0.31, -0.72, 0.10, -0.44, 0.58, 0.02, -0.15, 0.40],
  "features": { "brightness": 0.42, "contrast": 0.37, "saturation": 0.55,
                "temperature_k": 7400, "edge_density": 0.12, "energy": 0.34 },
  "palette": ["#0b2a3c", "#1f5a73", "#7fb3c4", "#d9e6ea", "#e8a25c"],
  "accent": "#e8a25c",
  "global": { "loop_frames": 240, "speed": 0.9, "wind_deg": 205,
              "density": 0.45, "flicker_rate": 0.2, "sway": 0.7, "charset": "standard" },
  "layers": [
    { "kind": "ripple", "intensity": 0.61, "amplitude": 1.4, "wavelength": 9, "cycles": 3, "direction_deg": 180 },
    { "kind": "cloud_drift", "intensity": 0.22, "layers": 2, "cycles": 1, "softness": 0.6 }
  ],
  "audio_mapping": { "bass": "intensity", "mid": "density", "treble": "flicker", "onset": "pulse" }
}
```

Users can override a recipe by dropping an edited copy in `~/.config/coverwave/overrides/<hash>.json`. The `R` key in the UI reloads it live.

---

## 6. Stage 3: Constraints (enforced in `cw-render`)

- **Subject protection:** cells in the subject mask, and every HUD/title cell, are never displaced or overwritten. Particles draw behind them (z-order: background art < effects < subject < HUD).
- **Seamless loops:** all motion is a function of `phase = (frame mod L) / L`, with `L = loop_frames`. Speeds are integer `cycles` per loop, sway is `sin(2π·cycles·phase + seed_phase)`, and particles wrap on a torus whose period divides `L`. Frame `L` is identical to frame `0`, which a test checks.
- **Audio never touches timing.** Audio bands only scale *amplitude* (intensity, density, brightness, pulse) and never phase or speed. The loop stays seamless and the beat is still visible.
- **Change budget:** at most about 15% of cells may change per frame. That rises linearly to 30% when `energy > 0.7`. If a frame goes over budget, the lowest-intensity layer's changes are dropped first. The diff is computed against the previous frame anyway for terminal output.

---

## 7. Rendering pipeline (`cw-render`)

Per terminal size (recomputed only on resize):
1. Fit the cover to the cell grid, correcting for the cell aspect ratio (about 1:2; the terminal's ratio is read when possible).
2. Precompute the **base art**: luminance → charset index, colour → nearest palette entry blended toward the true colour (truecolor), with a 256-colour fallback.
3. Resample the subject mask to cells.

Per frame:
1. Start from the base art.
2. Apply displacement layers (ripple, sway, shimmer) to background cells only.
3. Composite particle and flicker layers behind subject cells.
4. Apply the audio modulation: `bass → layer intensity`, `mid → particle density`, `treble → flicker`, `onset → brief brightness pulse on the accent`, each smoothed with attack/release.
5. Enforce the change budget, then hand off the `CellGrid` (`char`, fg, bg).

Performance target: under 4ms per frame at 240x70 cells in release builds.

`cw-render` is pure: `render(recipe, base, mask, audio_frame, frame_idx) -> CellGrid`. That makes golden tests and mobile ports simple.

---

## 8. Audio

**Local playback (`cw-audio`)**
- symphonia decodes into a cpal output stream, with resampling (`rubato`) when the device rate differs.
- Gapless playback within the queue where the format allows it; seek, volume, shuffle, repeat (off/all/one).
- A tap of the post-volume PCM goes to the analyser, so visuals match what you hear.

**Analysis**
- 2048-point FFT with a Hann window and hop 512, folded into bands (sub-bass, bass, low-mid, mid, high-mid, treble).
- Spectral-flux onset detection with an adaptive threshold for `onset`.
- Values are normalised per track with a slow-moving loudness tracker, so quiet tracks still move.

**System capture (`cw-capture`)**, used when the source is Spotify or Apple Music:
```rust
trait SystemCapture { fn start(&mut self, sink: Producer<f32>) -> Result<()>; fn stop(&mut self); }
```
| Platform | Backend | Notes |
|---|---|---|
| macOS 14.2+ | Core Audio process tap | Audio only, no screen-recording permission. Can target only the Spotify/Music process. |
| macOS 13–14.1 | ScreenCaptureKit audio | Needs the Screen Recording permission. |
| Windows | WASAPI loopback via cpal | No extra driver. |
| Linux | PipeWire/Pulse monitor source | Stretch goal. |
| iOS / Android | Not possible | The OS sandbox blocks this, so ports fall back to local-file audio only. |

---

## 9. Sources (`cw-sources`)

- **Local:** scan configured folders (`walkdir`), read tags and embedded art with `lofty`, and fall back to `cover.jpg`/`folder.png` in the same folder. Watch for changes with `notify`.
- **Spotify:** Web API with OAuth PKCE (client ID in the config, no secret). It provides now-playing, play/pause/next/prev/seek/volume and cover URLs. Playback control needs a **Premium** account. Tokens are stored in the OS keychain (`keyring`).
- **Apple Music (macOS):** controls Music.app via `osascript`/AppleScript (state, track, artwork export, transport). The MusicKit web API for catalogue/library access is optional and later, since it needs an Apple Developer token.
- A shared `Source` trait means the UI never cares which source is active.

---

## 10. Library, queue, playlists, lyrics (`cw-library`)

- SQLite at `~/.local/share/coverwave/library.db`: tracks, albums, artists, cover_hash, play counts.
- Fuzzy search across title/artist/album with `nucleo`, ranked with a recency boost.
- The queue lives in memory and is persisted on quit. Playlists are saved as `.m3u8` in `~/.local/share/coverwave/playlists/` so other players can read them.
- **Lyrics**, in priority order:
  1. A sidecar `.lrc` file.
  2. Embedded SYLT/USLT tags.
  3. Optional LRCLIB lookup, off by default and cached.
- Rendered as a centred line plus a dimmer line above and below, drawn as HUD cells (protected), and timed to playback position with an offset key.

---

## 11. UI (`cw-tui`)

**Modes** (`Tab` cycles between them)
1. **Art:** the cover fills the terminal, with one HUD line at the bottom showing title, artist, a progress bar of ASCII ticks, time and source.
2. **Library/Queue:** a two-pane browser (artists/albums/tracks on the left, the queue on the right), with a small static ASCII thumbnail of the selected album.

**Keys**
| Key | Action |
|---|---|
| `space` | play/pause |
| `n` / `p` | next / previous |
| `←` `→` | seek ±5s (`shift` for ±30s) |
| `+` `-` | volume |
| `s` / `r` | shuffle / repeat cycle |
| `/` | fuzzy search |
| `enter` | play selection / `a` add to queue |
| `P` | save the queue as a playlist |
| `l` | toggle lyrics, `[` `]` shift lyric offset |
| `S` | switch source (Local / Spotify / Apple Music) |
| `R` | reload recipe (picks up override edits) |
| `D` | debug overlay: scene weights, active layers, fps, cells changed % |
| `q` | quit |

**States to handle:** empty library (first-run screen asking for a music folder), missing cover (generated pattern from the palette of the artist name hash), analysis in progress (show static ASCII art immediately, fade motion in when the recipe arrives), capture permission denied (inline message plus how to fix it), Spotify not authenticated, a tiny terminal (below 40x12, show text only).

Terminal handling: an alternate screen, raw mode, hidden cursor, restored on panic through a panic hook. Synchronized output (`?2026`) is used where supported to avoid tearing.

---

## 12. Config

`~/.config/coverwave/config.toml`
```toml
music_dirs = ["~/Music"]
fps = 30
color = "auto"          # auto | truecolor | 256 | mono
charset = "auto"        # auto (from style DNA) | sparse | standard | blocks
change_budget = 0.15
[spotify]
client_id = ""
[lyrics]
online_lookup = false
```

---

## 13. Milestones

Each milestone ends with something runnable.

| # | Milestone | Done when |
|---|---|---|
| M0 | Workspace + TUI shell | `cargo run` opens a 30fps ratatui loop with mode switching and clean exit/panic restore |
| M1 | Local playback | Plays a folder of files with transport keys, seek, volume, gapless queue |
| M2 | Static ASCII art | Embedded/folder art rendered with aspect correction, palette (k-means), truecolor + 256 fallback |
| M3 | Audio analysis | Live bands + onsets shown in the `D` overlay and visibly tracking the music |
| M4 | Analysis pipeline | MobileCLIP + PCA + features + mask + seed produce a cached recipe JSON; `tools/pca-fit` produces `pca.bin` |
| M5 | Renderer + layers | All 8 scene layers, constraints (subject protection, seamless loop, change budget), audio modulation |
| M6 | Library, search, queue, playlists, lyrics | SQLite index, fuzzy search, m3u8 playlists, .lrc/embedded lyrics synced |
| M7 | System capture + Spotify + Apple Music | macOS tap/SCK capture, Spotify PKCE control, Music.app control, source switching |
| M8 | Windows | WASAPI loopback, path/config differences, CI build on Windows |

M4 and M5 are the heart of the project, so they get the most time. Building M5 against a hand-written recipe first, before M4 is done, removes the dependency between them.

---

## 14. Testing

- **Determinism:** the same cover bytes give a byte-identical recipe JSON and identical frames 0..L.
- **Loop seam:** `render(frame=L) == render(frame=0)` for every layer kind.
- **Change budget:** a property test over random recipes checks that changed cells per frame stay at or under the budget.
- **Subject protection:** no masked cell ever changes across a full loop.
- **Golden frames:** snapshot tests (`insta`) of rendered grids for a small fixture set of open-licence covers.
- **Perf:** a `criterion` bench on `render()` at 240x70, which fails CI if it goes over 4ms.
- Audio decode and FFT get unit tests with synthetic sine and click signals.

---

## 15. Risks and open questions

- **PCA corpus:** `tools/pca-fit` needs a few thousand covers to fit a meaningful projection. The plan is to use your own library plus an open dataset. It is fitted once and shipped, then never changes, so style DNA stays stable.
- **MobileCLIP licensing and size:** confirm the model licence allows redistribution, or download it on first run from the official source. It is about 100–200MB.
- **Scene prompt coverage:** the 8 prompts will miss some covers (abstract art, portraits). The softmax then spreads thin, and every layer could fall below 0.10. Fallback: a subtle `ambient` layer (slow palette breathing) is used when no layer qualifies.
- **Subject mask quality:** the heuristic masks will be wrong on busy covers. U²-Netp is the planned upgrade.
- **Cell aspect ratio** varies by font. The config gets an override, and the `D` overlay shows a calibration square.
- **Spotify Premium** is required for control, and without it the player can only show now-playing. Spotify has also deprecated its audio-features endpoint for new apps, so the plan does not rely on it.
- **macOS permissions:** capture prompts need a clear first-run explanation.
