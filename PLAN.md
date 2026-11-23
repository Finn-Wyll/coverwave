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
    pca.bin         shipped PCA matrix (output of tools/pca-fit)
    scene_prompts.json  scene prompts + their precomputed text embeddings
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
| CLIP inference | `tract-onnx` (pure Rust) with the MobileCLIP-S0 image encoder; text embeddings precomputed |
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
- **Analyser thread:** drains a mono tap ring buffer filled by the callback, computes FFT bands and onsets, and publishes an `AudioFrame` behind a `Mutex`. Only the analyser and UI threads touch that lock, never the callback, so `arc-swap` was not needed.
- **Analysis worker:** runs cover analysis off the UI thread, and the cache makes it near-instant on repeat plays.
- **UI thread:** fixed 30fps tick (configurable to 60). Reads the latest `AudioFrame`, renders, and diffs to the terminal.

---

## 4. Stage 1: Analyse (once per cover, cached)

Cache key: `sha256(cover bytes)`. Cache dir: `~/.cache/coverwave/recipes/<hash>.json` plus `<hash>.clip.bin` (the raw 512-dim embedding, so new prompts or a refitted PCA rebuild recipes without running the model). A cached recipe is reused only if its `version` and `analysis` block (model, prompts version, PCA hash) match the running build.

**Model (decided in M4).** MobileCLIP-S0's vision encoder, from the ONNX export at `Xenova/mobileclip_s0`, pinned to a revision and SHA-256-checked, 45.5MB fp32. It is downloaded on first run to `~/.cache/coverwave/models/` (`--offline` never downloads; `COVERWAVE_MODEL` points at a local file). It runs with **tract** (pure Rust): no native runtime to ship, so the same code works on the planned ports. It takes ~110ms per cover on a cloud x86 CPU, and its output matches onnxruntime to 1e-6 on identical input. Preprocessing: shortest edge to 256 (antialiased bilinear), centre crop, 0..1, no normalisation. **Licence:** Apple's weights are research/personal-use only (`apple-amlr`). The owner chose this for a personal player. The weights are never committed or redistributed, and a public release would need a permissively licensed model (OpenAI CLIP ViT-B/32 is MIT) plus new prompt embeddings and a PCA refit. Until the model is ready, covers get neutral recipes (no scenes, zero DNA, `ambient` layer) that are not cached, and they are re-analysed once it loads.

1. **Scene semantics.** Zero-shot MobileCLIP against `assets/scene_prompts.json`:
   `snow, lava, very dark scene, forest, ocean, city at night, desert, sky`, plus three neutral catch-alls `portrait, typography, abstract` (*added in M4*). Each scene's text embedding is the normalised mean over 3 subjects × 3 templates ("an album cover showing {}", "a photo of {}", "artwork of {}"). `pca-fit prompts` computes these once with the text encoder and ships them in the JSON, so the player never runs the text model. Keep the full softmax (temperature 100, as CLIP does) as `scene_weights` and never take an argmax. The neutral scenes have a logit bias of −3: without it, they swallowed real scenes, because nearly every cover has text or a plain area. With it, they win only on portraits, typography-led and abstract covers. Checked by eye on the playlist covers.
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
- `layer.intensity = scene_weight`. Each scene at or above 0.10 contributes its layers, all at its weight. Layers are then sorted by intensity, ties keep table order, and the top 3 are kept. The neutral scenes' combined weight (if ≥ 0.10) becomes one `ambient` layer, and `ambient` at 0.5 is also the fallback when nothing qualifies. Each scene draws its random parameters from its own ChaCha8 stream, so a scene's look doesn't change when other scenes come and go.
- **Style DNA → global params** (each mapping is linear and clamped, defined in one table in `cw-analysis/src/mapping.rs`):
  - dna[0] → overall speed multiplier (0.5–1.6)
  - dna[1] → wind direction (angle)
  - dna[2] → particle density
  - dna[3] → flicker rate
  - dna[4] → sway amplitude
  - dna[5] → charset: `sparse` (` .:`), `standard` (` .:-=+*#%@`), `dense` (a 66-step ASCII ramp). *Changed in M4:* `blocks` (` ░▒▓█`) was dropped to keep the art plain ASCII.
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

Users can override a recipe by dropping an edited copy in `~/.config/coverwave/overrides/<hash>.json`. The `R` key in the UI reloads it live. (`XDG_CACHE_HOME` / `XDG_CONFIG_HOME` move these directories.) The recipe also carries `analysis` (model, prompts version, PCA hash) and `subject_mask` (64 text rows of `0`/`1`, so it can be edited by hand). Directions are screen degrees: 0 = right, 90 = down. The `D` overlay's second page shows the live recipe and where it came from.

---

## 6. Stage 3: Constraints (enforced in `cw-render`)

- **Subject protection:** cells in the subject mask, and every HUD/title cell, are never displaced or overwritten. Particles draw behind them (z-order: background art < effects < subject < HUD).
- **Seamless loops:** all motion is a function of `phase = (frame mod L) / L`, with `L = loop_frames`. Speeds are integer `cycles` per loop, sway is `sin(2π·cycles·phase + seed_phase)`, and particles wrap on a torus whose period divides `L`. Frame `L` is identical to frame `0`, which a test checks.
- **Audio never touches timing.** Audio bands only scale *amplitude* (intensity, density, brightness, pulse) and never phase or speed. The loop stays seamless and the beat is still visible.
- **Change budget:** at most about 15% of cells may change per frame. That rises linearly to 30% when `energy > 0.7`. If a frame goes over budget, the lowest-intensity layer's changes are dropped first. The diff is computed against the previous frame anyway for terminal output.

---

## 7. Rendering pipeline (`cw-render`)

Per terminal size (recomputed only on resize):
1. Fit the **whole** cover to the cell grid, centred, correcting for the cell aspect ratio. Covers are never cropped, and the near-black margins beside them are where effects can live later. The cell aspect is measured from the terminal's pixel size when it reports one, otherwise 0.5, and `--cell-aspect` overrides it.
2. Precompute the **base art** (`cw_render::base_art`, decided in M2):
   - The cover is area-averaged in linear light to one sample per cell.
   - Density comes from CIE L*, contrast-stretched per cover (2nd to 98th percentile, never to less than 45 L* units so flat covers don't turn into noise), and mapped onto the ASCII ramp `" .:-=+*#%@"`.
   - Glyph colour depends on the **art style**, a user setting (`--art-style`, cycled with `c`):
     - `palette` (default): the nearest of the 5 palette colours, blended 35% back toward the true colour.
     - `truecolor`: the pixel's own colour.
     - `duotone`: a ramp from a dark tint of the darkest palette colour to the accent.

     `palette` and `truecolor` raise dark colours (L* → 30 + 0.7·L*) so sparse glyphs stay readable.
   - Accent: the most colourful palette cluster (chroma × weight^0.35), with its lightness clamped to L* 62–84 and its chroma reduced until it is in gamut. Greyscale covers get a light neutral.
   - Colour reaches the terminal as 24-bit when `COLORTERM` is `truecolor`/`24bit`, otherwise as the nearest xterm-256 index (`--color` overrides). The frontend converts the whole frame, so the grid stays RGB.
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
- The callback pushes a mono mix of what it is playing into a tap (`rtrb`, dropping samples if full), so analysis is in sync with the speakers rather than with the decoder, which runs ~0.75s ahead. While paused it pushes silence, so levels fall to zero. *Decided in M3:* the tap is **pre-volume**, so turning the volume down doesn't shrink the visuals or make the loudness tracker re-adapt.

**Analysis**
- 2048-point FFT with a Hann window and hop 512, folded into bands: sub-bass 20–60, bass 60–250, low-mid 250–500, mid 500–2k, high-mid 2k–4k, treble 4k–16k Hz.
- Spectral-flux onset detection. Flux is the log-magnitude rise averaged **per band, then across bands**, so a kick in a few bass bins counts as much as a cymbal. The adaptive threshold is `2 × median(last 0.5s) + 0.05`. It uses the median, not mean + std, because with a mean the previous beat of a 120 BPM track sits in the window and masks the next one. Peak picking costs one hop (~11ms), and onsets are at least 80ms apart. `onset` is a pulse that jumps to 1 and decays over ~150ms, and `onset_count` counts every one.
- Values are normalised with slow loudness tracking (`cw_audio::Analyser`), so quiet tracks still move. Each band and the overall level have a ceiling with an instant rise and a 4 dB/s fall, and 30 dB below the ceiling maps to 0..1. A band's ceiling stays within 24 dB of the overall ceiling, so near-empty bands don't amplify noise. Below −70 dB counts as silence. The tracking runs continuously across tracks rather than resetting on each one, which avoids a jump at every track change.
- Outputs are smoothed with a 12ms attack and a 120ms release.
- `cargo run -p cw-audio --release --example analyse -- FILE` prints the analysis at 30fps as text meters, for tuning without a speaker.

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
| `D` | debug overlay, cycling: stats (fps, cells changed, cover, live audio) → recipe (source, scenes, layers, globals, DNA) → off |
| `q` | quit |

**States to handle:** empty library (first-run screen asking for a music folder), missing cover (generated pattern from the palette of the artist name hash), analysis in progress (show static ASCII art immediately, fade motion in when the recipe arrives), capture permission denied (inline message plus how to fix it), Spotify not authenticated, a tiny terminal (below 40x12, show text only).

Terminal handling: an alternate screen, raw mode, hidden cursor, restored on panic through a panic hook. Synchronized output (`?2026`) is used where supported to avoid tearing.

---

## 12. Config

`~/.config/coverwave/config.toml`
```toml
music_dirs = ["~/Music"]
fps = 30
color = "auto"          # auto | truecolor | 256 | mono  (M2: --color flag)
art_style = "palette"   # palette | truecolor | duotone  (M2: --art-style flag, `c` key)
cell_aspect = "auto"    # auto (measured) | number  (M2: --cell-aspect flag)
charset = "auto"        # auto (from style DNA) | sparse | standard | dense
offline = false         # never download the analysis model (M4: --offline flag)
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
| M2 | Static ASCII art | Embedded/folder art rendered with aspect correction, palette (k-means), truecolor + 256 fallback. **Done.** `cw-analysis` (decode, seed, palette) started early for this. Art lookup lives in `cw-audio::cover_art` until `cw-sources` exists. |
| M3 | Audio analysis | Live bands + onsets shown in the `D` overlay and visibly tracking the music. **Done.** |
| M4 | Analysis pipeline | MobileCLIP + PCA + features + mask + seed produce a cached recipe JSON; `tools/pca-fit` produces `pca.bin`. **Done.** The shipped `pca.bin` is provisional (see risks). |
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

- **PCA corpus:** `tools/pca-fit` needs a few thousand covers to fit a meaningful projection. The shipped `pca.bin` (version `d66d5b7d`) is **provisional**: it was fitted on the 87 distinct covers of the first 100 tracks of the owner's liked-songs playlist, which is all Spotify's public embed exposes without API credentials. Refit before freezing, either on the owner's library (`pca-fit fit --vision-model … ~/Music`, which reads embedded and folder art) or on the full playlist through the Spotify Web API. A refit changes the PCA hash, so cached recipes rebuild themselves from their cached embeddings.
- **MobileCLIP licensing:** resolved for personal use, not for distribution (see section 4).
- **Scene prompt coverage:** portraits, typography and abstract covers used to land on arbitrary scenes. M4 added neutral scenes feeding the `ambient` layer. Remaining misses: dark-background portraits still read as "very dark scene", and some indoor photos as "desert". More prompts or a bigger model would help, and any prompt change bumps `version` in `scene_prompts.json`.
- **Subject mask quality:** the heuristic masks will be wrong on busy covers. U²-Netp is the planned upgrade.
- **Cell aspect ratio** varies by font. The config gets an override, and the `D` overlay shows a calibration square.
- **Spotify Premium** is required for control, and without it the player can only show now-playing. Spotify has also deprecated its audio-features endpoint for new apps, so the plan does not rely on it.
- **macOS permissions:** capture prompts need a clear first-run explanation.
