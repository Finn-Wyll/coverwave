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
| snow | `falling_particles` + `aurora` | density, fall speed, drift, glyphs `. * +`; curtain count |
| lava | `heat_cycle` + `embers` | noise scale, palette cycle speed, ember rise rate |
| very dark | `starfield` + `lightning` + `flicker` + `fog` | star count; strike chance; flicker rate; fog density and drift |
| forest | `row_sway` + `fireflies` | sway amplitude, wavelength, firefly count/blink |
| ocean | `ripple` + `lightning` | displacement amplitude, wavelength, direction; strike chance |
| city at night | `neon_flicker` + `digital_rain` + `rain` | flicker cells/rate; column density; rain angle, density |
| desert | `heat_shimmer` + `dust` | shimmer amplitude, dust drift |
| sky | `cloud_drift` + `aurora` + `starfield` | layer count, drift speed, softness; curtains; stars |

### Selection v2 (recipe version 3, after M8)

The owner reported getting "the same animation every song". The cause: one fixed effect per scene, and most covers read as "very dark" or neutral. On the 87 playlist covers, about two-thirds got `flicker + fog`, `ambient`, or both. v2 changes three things:

- **More kinds.** 17 new layer kinds in four families, 31 in total, all held to the same constraints (seam, protection and budget tests run per kind, plus a perf bench case):
  - **Beat & pulse:** `pulse_rings`, `subject_glow` (a highlight travelling round the subject), `eq_bars`.
  - **Electronic & glitch:** `scanlines`, `row_glitch`, `digital_rain`, `chroma_shift`. (The owner's local branch added `aurora`, `starfield`, `digital_rain` and `lightning` at the same time; the merge kept those four implementations, and the selection pools use them.)
  - **Nature & weather:** `falling_leaves`, `bubbles`, `lightning`, `aurora`, `smoke`, `sun_rays`.
  - **Space & light:** `starfield`, `twinkle` (dark cells only), `orbit`, `light_sweep`.
- **More scenes.** `space`, `storm`, `garden`, `party` and `underwater` (prompts version 2). Each scene now offers a **pool** of kinds with affinities (`mapping::pool`).
- **Genre.** It comes from file tags, Apple Music (`genre of current track`) and Spotify (the first artist's `genres`, looked up once per artist). `cw_analysis::GenreFamily::of_any` folds it into 18 families, each with favoured kinds, an energy bias and a typical tempo.
- **Score per kind:** `0.75 * sum(scene weight * pool affinity) + 0.45 * genre affinity + seed jitter (0..0.12)`.
- **Picking layers:**
  - The best 3 are kept, with at most 1 displacement and at most 2 large-area tints.
  - A known genre is guaranteed one of its top-3 kinds, replacing the weakest pick of the same class.
  - Intensity is 0.85 for the strongest and relative for the others.
- **Caching:** recipes are cached per cover and genre family (`<hash>.<family>.json`), with the embedding shared, so a new genre rebuilds without running the model.
- **Measured on the 87 covers:** 38 distinct mixes without genre and 73 with mixed genres, and the most common mix appears 4 times (it was the majority before).

### Rules (v1, superseded by v2 above for layer choice)
- `layer.intensity = scene_weight`. Each scene at or above 0.10 contributes its layers, all at its weight. Layers are then sorted by intensity, ties keep table order, and the top 3 are kept. The neutral scenes' combined weight (if ≥ 0.10) becomes one `ambient` layer, and `ambient` at 0.5 is also the fallback when nothing qualifies. Each scene draws its random parameters from its own ChaCha8 stream, so a scene's look doesn't change when other scenes come and go.
- **Style DNA → global params** (each mapping is linear and clamped, defined in one table in `cw-analysis/src/mapping.rs`):
  - dna[0] → overall speed multiplier (0.5–1.6)
  - dna[1] → wind direction (angle)
  - dna[2] → particle density
  - dna[3] → flicker rate
  - dna[4] → sway amplitude
  - dna[5] → charset: `sparse` (` .:`), `standard` (` .:-=+*#%@`), `dense` (` .`,:;~-=+*!?#%&$@`, 18 symbols; an earlier 66-step ramp with letters read as text soup). *Changed in M4:* `blocks` (` ░▒▓█`) was dropped to keep the art plain ASCII.
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
- **Change budget (soft):** about 25% of cells may change per frame, rising linearly to 40% when `energy > 0.7`. On a beat the budget widens by up to 35 points (capped at 75%) and shrinks back as the onset decays, so hits read clearly. If a frame goes over budget, the lowest-intensity layer's changes are dropped first; the beat ring outranks every layer. Originally 15%/30%; raised on 2026-11-24 because the owner wanted more motion and beat response. The diff is computed against the previous frame anyway for terminal output.
- **Beat shockwave:** each onset sends a ring of the palette's lightest colour outward from the subject's edge across the cover. Its crest brightens glyphs and packs them one or two steps denser. Its radius follows the decaying onset envelope, so it is pure and never affects the loop.
- **Stage effects** (every cover, strength `0.6 + 0.6 * energy`, added 2026-11-24 when the owner asked for more and cooler motion):
  - *Ambient glow*: the margins beside the cover fill with a stippled bloom of the cover's edge colours, reaching further with level and on beats (quantised so it does not churn).
  - *Bass thump*: on kicks the background is pushed up to two cells outward from the subject, like a speaker cone.
  - *Margin equaliser*: six bands rise from the bottom of each side margin (bass next to the cover) when margins are at least 13 cells wide.
  - *Glitch slices*: on the hardest hits (onset > 0.8, level > 0.45) of energetic covers, two or three slices shift sideways in the accent for a couple of frames; which hits glitch comes from the seed and beat count.
- **Owners in the limiter:** beat effects (shockwave, thump, glitch) outrank every layer; glow and equaliser are dropped first.
  *As built in M5:* layers are designed to stay within budget on their own under steady audio (a test checks every frame of the loop for 12 random recipes, at silence and at full level). Changes that are only colour are quantised (fog levels, heat palette steps, flicker on/off), so only boundaries move. `limit_changes(scene, prev, next)` is the safety net for sudden audio jumps. It is a pure function of the previous output, and each cell carries the id of the layer that drew it, so the weakest layer is reverted first. `render` itself never sees history, which keeps `render(L) == render(0)` exact.

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

*As built in M5:* this is split in two. `prepare(recipe, base_art, art_rect, ramp) -> Scene` runs once per cover, size, style or recipe change: it resamples the mask and makes every seeded random choice (ChaCha8 per layer). `render(&Scene, Input { frame, audio, fade }) -> Output { grid, owner }` runs per frame. Draw order is displacement (ripple, row_sway, heat_shimmer resample the base) → fields (fog, heat_cycle, cloud_drift, flicker, neon_flicker, ambient) → particles (snow, rain, embers, dust, fireflies). Weaker layers draw first, so stronger ones win. Particles move on a torus with integer wraps per loop, and may cross the cover's margins; displacement and most fields stay inside the cover. Audio gains: bass scales intensity (0.7–1.3×), mid scales particle density (0.6–1.4×), treble scales flicker depth, and onset pulses particles and neon toward the accent. Silence still moves, gently. The frontend fades a new recipe in over 30 frames and `m` turns motion off. Measured: 0.5ms per frame at 240x70 with 4 heavy layers, 0.8ms with the limiter (`cargo bench -p cw-render --bench render`).

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

*As built in M7:* `Capture::start(&Target, Producer<f32>) -> Capture` (dropping it stops). `Target::Apps(bundle IDs)` or `Target::System`, mono into a ring buffer, analysed by the same `AnalyserHandle` the local player uses.
- **macOS:** a process tap (`CATapDescription` stereo mixdown of the processes whose bundle ID starts with `com.spotify.client` / `com.apple.Music`, private and unmuted) inside a private aggregate device clocked by the default output, read with an IO block. `frontends/cw-tui/build.rs` embeds `macos/Info.plist` (`NSAudioCaptureUsageDescription`) so the permission prompt can appear.
- **Not done:** the ScreenCaptureKit fallback for macOS 13–14.1.
- **Windows:** cpal loopback (input stream on the default output); whole-system mix.
- **Linux:** cpal's default input. Point it at a monitor (`pactl set-default-source <sink>.monitor`), as the cloud tests do.
- **Capture retries:** every 3s while the app has no audio yet, and restarts after 8s of silence while the source says it is playing (the app restarted).
- **Testing:** the macOS code is UNTESTED. It is type-checked and clippy-clean for `aarch64-apple-darwin` from Linux; see LOCAL_TODO.md. `cargo run -p cw-capture --example meter -- 5 com.spotify.client` checks capture without the TUI.

---

## 9. Sources (`cw-sources`)

- **Local:** scan configured folders (`walkdir`), read tags and embedded art with `lofty`, and fall back to `cover.jpg`/`folder.png` in the same folder. Watch for changes with `notify`.
- **Spotify:** Web API with OAuth PKCE (client ID in the config, no secret). It provides now-playing, play/pause/next/prev/seek/volume and cover URLs. Playback control needs a **Premium** account. Tokens are stored in the OS keychain (`keyring`).
  - **Library (added 2026-11-24):** with Spotify active, Library mode browses Liked Songs, Playlists and Albums (`/me/tracks`, `/me/playlists`, `/me/albums`, `/playlists/{id}/items`, `/albums/{id}/tracks`; up to 1000 rows per listing, cached 5 minutes) and plays through `PUT /me/player/play` with a context URI, an offset URI, or a list of up to 100 track URIs. Uses the February 2026 Development Mode API: playlist entries carry `item` (old `track` is still read), and only playlists the user owns or collaborates on can be listed, so followed playlists play whole instead of opening.
  - **Device pickup:** when Spotify answers 404 (no active device), coverwave lists devices, prefers the active one, then a computer, then any; on macOS it opens the Spotify app in the background if there are none and waits up to 10s. Playback moves there and the command is retried once.
  - Scopes: playback ones plus `playlist-read-private playlist-read-collaborative user-library-read`. Older logins get a "press L to log in again" message on library calls.
- **Apple Music (macOS):** controls Music.app via `osascript`/AppleScript (state, track, artwork export, transport). The MusicKit web API for catalogue/library access is optional and later, since it needs an Apple Developer token.
- A shared `Source` trait means the UI never cares which source is active.

*As built in M7:*
- **Sources:** Spotify and Apple Music are `cw_sources::Backend`s, each polled on its own thread (`RemoteHandle`) about once a second. The UI reads a `RemoteState` (now playing, auth, last error) and extrapolates the position between polls. Local files stay with the richer `cw-audio` player, not this trait.
- **Switching:** `S` cycles the sources that are available: Spotify when `[spotify] client_id` is set, Apple Music on macOS. Following a remote source pauses local playback, and picking music in the library switches back to Local. Transport keys go to the active source.
- **Remote covers:** they go through the same analysis and recipe pipeline, keyed by `<source>:<track id>`.
- **Spotify:**
  - Login is PKCE with a one-shot loopback listener on `http://127.0.0.1:<redirect_port>/callback` (default 8898; register exactly that URI). `L` opens the browser, honouring `$BROWSER`.
  - The refresh token is stored in the keychain via `keyring` 3 (apple-native / windows-native; in-memory on Linux) and rotated tokens are saved.
  - The client retries a 401 after refreshing, backs off on 429 using `Retry-After`, and maps 403 to "Premium needed" and 404 to "no active device". Podcast episodes are supported.
  - `COVERWAVE_SPOTIFY_API` / `COVERWAVE_SPOTIFY_ACCOUNTS` point it at a mock server for tests.
- **Apple Music:** `osascript` with a tab-separated state line (decimal commas handled), and artwork exported to a temp file once per track. Automation permission errors (-1743) get a hint. UNTESTED on a Mac.
- **Folder watching** (`notify`): not yet.

*Beat clock (after M8, for audio that cannot be heard):*
- When capture is not running, a remote track's motion comes from `cw_core::BeatClock`. It is a pure function of tempo, a `BeatPattern` and the playback position. The pattern comes from the genre family: four-on-the-floor for electronic, half-time for hip-hop and R&B, one-drop for reggae, swells for ambient, classical and soundtracks, and a backbeat for everything else. Kicks drive the bass bands and the onsets, snares the mids, and hats the treble. It follows seeks because it reads the position.
- **Tempo:** `cw_sources::tempo` looks the track up on Deezer's public API, which needs no key. It searches by ISRC, or else by artist and title. Deezer's `bpm` is used when it is set, but it is usually 0. Otherwise the 30-second preview MP3 is downloaded and analysed with `cw_audio::estimate_tempo`:
  - The method is spectral flux, autocorrelation over 60–200 BPM and a prior centred on 120 BPM.
  - Estimates with confidence below 0.3 are dropped.
  - Live checks: One More Time 122.9 and Enter Sandman 124.5 (both correct). Some tracks come out at double or half time.
- The lookup runs once per track on a background thread. Without a tempo, the genre's typical BPM is used. `[tempo] online_lookup = false` turns the lookup off; it only ever sends artist, title or ISRC.
- **Rhythm maps (iOS, 2026-11-24):** tempo alone did not match songs well enough, and the microphone heard everything in the room. Now each remote track gets a `cw_core::RhythmMap` measured from its Deezer preview by the Beat This! beat tracker (Foscarin et al., ISMIR 2024; MIT, code and weights; the 8 MB "small0" model, exported to ONNX by `ios/tools/export_beat_model.py`, bundled in the app, run with tract in `cw_analysis::rhythm`):
  - Log-mel features exactly as PyTorch computes them (checked to 0.0002 with `cargo run -p cw-analysis --release --example beat_check`), the model's beat and downbeat logits, "minimal" peak picking, then a least-squares beat grid (beats numbered gap by gap; grids over 175 BPM folded to half speed), the metre and bar phase from the downbeats, and the groove: what each of the six bands does at each sixteenth of the bar, averaged over the clip.
  - On 14 library clips (30 s from 60 s in), 13 gave a map (e.g. Smells Like Teen Spirit 117.9 BPM, Saccharine 80.9). The miss switches between quarter and eighth notes; it falls back to the genre beat clock.
  - The map replays the song's own groove against the playback position (`RhythmMap::frame`), replacing the genre pattern. Maps are cached per track as JSON.
  - Bar phase without listening: Deezer cuts every preview from 48 s into the track. Measured by cross-correlating previews with full library files: 47.97–48.02 s for every track whose recording matched (Master of Puppets, R U Mine?, Wait and Bleed, INVISIBLE, If You Can't Hang, ...). So the preview's grid extends to the whole song: downbeat = 48 s + the preview's first downbeat (`RhythmMap::clip_start`). It is used only when Deezer's track is at least 78 s and within 3 s of the playing track's length (else a different edit; the phase is then a guess, shown as "bar start estimated").
  - A microphone lock (fitting heard onsets to the groove) was tried and dropped on 2026-11-25: recording makes iOS lower the music.
  - Apple's iTunes previews are not used (their terms allow only promotional use).
*Offline Spotify (2026-11-25):* Spotify's downloaded songs are encrypted and readable only by the Spotify app, and the Web API needs the internet, so offline coverwave follows the Spotify app itself and fills in the rest from a saved copy.
- **Desktop:** `cw_sources::spotify_app` drives the Spotify desktop app through AppleScript (state line with the artwork URL, playpause, next/previous, seek, volume, `play track <uri> in context <uri>`), which answers offline. The `Spotify` backend switches to it when a Web API call fails at the transport level (`SourceError::Offline`; then for 30 s before retrying), or always with `[spotify] offline = "always"` (offline mode, settings row "spotify offline"). Devices and Spotify's queue need the internet.
- **Saved copy (`cw_sources::offline::OfflineStore`, `<data>/offline`):** every listing browsed online is saved (`library/<hash>.json`) with its tracks' details (`tracks.json`: title, artist, album, ISRC, cover URL, genre, tempo, which lookups were tried) and covers by URL (`covers/`). Offline, Library mode shows the saved listings and search looks through saved tracks. Played tracks' covers come from the saved file, so a cover's bytes (and its seed and motion) are the same online and off.
- **Save ahead** (`[spotify] save_ahead = "off" | "liked" | "all"`, default liked): the backend lists liked songs (and every listable playlist with `all`) one listing per spare poll (`Backend::background`), every 12 hours; then `cw-tui`'s saver thread (`offline_saver.rs`) fetches each track's cover, a genre (Deezer album genres when Spotify has none), tempo and lyrics, one track at a time, only while the internet answers. Lyrics misses are cached only when LRCLIB actually answered (`lrclib::fetch_checked`).
- **iOS:** a third start-screen choice, **Offline**, follows the Spotify app on the phone through Spotify's iOS SDK (App Remote, `SpotifyAppLink`) and looks nothing up (Spotify itself still needs the internet to accept the connection; see below). In the Spotify mode the same happens by itself when the path monitor reports no network or a Web API call fails offline. App Remote connects with the Web API login's access token (the PKCE login now asks for `app-remote-control`; the last token is kept in the Keychain) or, failing that, authorizes through the Spotify app. Spotify must be running. `OfflineStore` (Application Support/offline) keeps listings, track records and covers; lyrics and rhythm maps moved there from Caches. `OfflineSaver` saves ahead (Settings > Offline: Liked Songs by default, Wi-Fi only unless mobile data is allowed): listings, then each song's cover (and its CLIP embedding), genre, lyrics and rhythm map. Offline, playing a playlist from a track uses App Remote's content API (`playItem(skipToTrackIndex:)`); liked-song rows play the song and queue the next 20.
- Offline login (2026-11-25, tested on the phone): **Spotify approves every App Remote connection with its servers.** With no internet the Spotify app answers `wamp.error.authorization_failed`, even for a token it accepted minutes earlier, so following Spotify on iPhone with no internet at all is impossible (and iOS shares no other app's now playing). The owner chose to keep App Remote anyway: once connected (online), the downloads play as usual and coverwave looks nothing up, so the Offline mode means "Spotify's downloads, saved data only", not "no internet". coverwave keeps the token the Spotify app last accepted and renews it quietly while online; it opens Spotify's login only when Spotify's servers can be reached (not on Wi-Fi without internet), and every attempt times out with Spotify's own reason. The desktop is unaffected: the Spotify desktop app answers AppleScript offline.

- Why not something else: Spotify's audio-features and audio-analysis endpoints return 403 for apps created after Nov 2024. AcousticBrainz is frozen. The iTunes preview terms forbid non-promotional use. On iOS, other apps' audio cannot be captured, so this is also the iOS motion source.

## 10. Library, queue, playlists, lyrics (`cw-library`)

- SQLite at `~/.local/share/coverwave/library.db`: tracks, albums, artists, cover_hash, play counts.
- Fuzzy search across title/artist/album with `nucleo`, ranked with a recency boost.
- The queue lives in memory and is persisted on quit. Playlists are saved as `.m3u8` in `~/.local/share/coverwave/playlists/` so other players can read them.
- **Lyrics**, in priority order:
  1. A sidecar `.lrc` file.
  2. Embedded SYLT/USLT tags.
  3. LRCLIB lookup (lrclib.net, free, keyless; on by default since 2026-11-25), cached in `<cache>/lyrics/` (misses for a week). Works for remote tracks too.
- Rendered as a centred line plus a dimmer line above and below, drawn as HUD cells (protected), and timed to playback position with an offset key.

*As built in M6:*
- **Index:** one table of tracks. Artists and albums are derived by grouping on album artist, falling back to artist, then "Unknown artist"/"Unknown album". There is no `cover_hash` column: covers stay keyed by their own hash in the recipe cache. The UI holds one connection and a background scan thread holds another (WAL, 5s busy timeout). Scans are incremental (mtime + size) and remove vanished files. The folders come from `music_dirs` in the config (default `~/Music`), scanned at every start.
- **Watching folders with `notify` is not done yet:** restart to pick up changes.
- **Browser:** a drill-down stack (Library → Artists → Albums → tracks, plus Albums, Playlists, All tracks and Search).
  - `enter` opens an entry, or plays its list from the selected track. That replaces the queue, keeping shuffle and repeat.
  - `a` appends the selected track, or everything under the selected entry.
  - `backspace`/`esc` go back.
- **Search:** live as you type; `enter` plays the results from the selection, `tab` appends. Ranking is the nucleo score × (1 + up to 0.35 for play count + up to 0.15 for being played in the last weeks).
- **Player queue:** the player now accepts a whole new list (`replace`) or a track at the end (`append`). Snapshots carry a generation counter so the UI never pairs an old index with a new list. Covers are keyed by file path for the same reason.
- **Saved queue:** saved on quit to `~/.local/share/coverwave/queue.json` (paths, current track, position, shuffle, repeat, volume). It is restored, paused, when coverwave starts without file arguments.
- **Play counts:** a play counts when half the track (or 4 minutes) has been heard.
- **Lyrics:** sidecar `.lrc` (matched case-insensitively), then embedded lyrics text (USLT, or FLAC/MP4 lyrics fields, parsed as LRC when they carry timestamps). LRC `[offset:]` is honoured. Unsynced lyrics scroll evenly through the song.
  - Not supported yet: ID3 `SYLT` (lofty exposes it only as raw binary).
  - **Lyrics view (2026-11-25):** `l` (desktop) or a two-finger swipe (iOS) replaces the cover with its lyrics, in a square the cover's size where the cover was (`cw_render::lyrics::overlay`, shared by both apps). The recipe keeps running: the frontends prepare a second scene over a base whose square is blank (`blank_square`, no subject mask), so particles drift behind the words and the stage effects continue around them. Styles: the sung line in large letters (default), all text, or big only. Large letters are Space Mono Bold (SIL OFL, `assets/fonts/`) rasterised with `fontdue` and sampled into the grid by ink coverage (dense glyphs in the core, light ones on the edges); their height is a share of the square set by Lyric size (small/medium/large), so a finer grid (iOS Detail, a smaller terminal font) draws the same-sized letters in more detail. Words too wide for the square shrink to fit; wide characters (CJK, emoji) count two cells (`WIDE_CONTINUATION` marks the second).
  - Every effect reaches the lyrics: each frame the words are drawn into the cover's square (`lyric_base`) and the cover's own recipe is prepared over that base with nothing protected (`lyric_scene`, about 0.07ms for a usual recipe at 240x70) and rendered without `limit_changes` (it would hold back scrolling). Tints, flicker, sweeps, rays and glow light the letters; sway, ripple and shimmer bend them; glitches slice them; particles cross them. `cargo run -p cw-render --release --example lyric_effects` measures each layer kind against the same frame without it: 29 of 31 change letter cells, aurora and orbit work around the words.
  - Fixes found that way (2026-11-25): subject glow, with no subject mask, haloes whatever is drawn densely (the letters); row glitch also slips a seeded half of strong beats, three times taller, at a seeded row (`hash(seed, onset_count)`); lightning lights the cells beside its bolt (local light, not a flash).
  - Colours: the accent (default) or the cover itself ("cover"): each letter shows the moving cover's colour where it stands, lifted when too dark, like windows onto the art. Lines scroll up (like Spotify, default) or down, gliding 0.35s between lines as a pure function of position. Colours: the cover's accent for the sung line, its lightest palette colour fading with distance for the rest; the sung line brightens slightly on beats.
  - `[`/`]` shift lyrics by 0.25s for the session.
- **Not done:** the small ASCII thumbnail of the selected album in the browser.

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
| `/` | fuzzy search (`enter` plays, `tab` adds) |
| `enter` | open, or play from the selected track / `a` add to queue / `backspace` back |
| `P` | save the queue as a playlist |
| `l` | lyrics instead of the cover, `[` `]` shift lyric offset |
| `S` | source picker (local, Spotify, Apple Music, with status) |
| `,` | settings menu (lyrics, art, Ghostty font size, sources, tempo); applies at once and saves to `config.toml` keeping its comments (`toml_edit`) |
| `S` | switch source (Local / Spotify / Apple Music) / `L` log in to Spotify |
| `R` | reload recipe (picks up override edits) |
| `D` | debug overlay, cycling: stats (fps, cells changed, cover, live audio) → recipe (source, scenes, layers, globals, DNA) → off |
| `m` | motion on/off (static art) |
| `c` | cover colour style: palette, truecolor, duotone |
| `q` | quit |

**States to handle:** empty library (first-run screen asking for a music folder), missing cover (generated pattern from the palette of the artist name hash), analysis in progress (show static ASCII art immediately, fade motion in when the recipe arrives), capture permission denied (inline message plus how to fix it), Spotify not authenticated, a tiny terminal (below 40x12, show text only).

Terminal handling: an alternate screen, raw mode, hidden cursor, restored on panic through a panic hook. Synchronized output (`?2026`) is used where supported to avoid tearing.

---

## 12. Config

`~/.config/coverwave/config.toml` (Windows: `%APPDATA%\coverwave\config.toml`; every location is listed in `frontends/cw-tui/src/paths.rs`) (*M6:* implemented and written with comments on first run. Command-line flags override it. `music_dirs`, `fps`, `color`, `art_style`, `cell_aspect`, `offline`, `motion` work; `change_budget` and `charset` are accepted but not used yet.)
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
[tempo]
online_lookup = true    # beat-clock tempo from Deezer for remote tracks
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
| M5 | Renderer + layers | All 8 scene layers, constraints (subject protection, seamless loop, change budget), audio modulation. **Done** (all 14 layer kinds). |
| M6 | Library, search, queue, playlists, lyrics | SQLite index, fuzzy search, m3u8 playlists, .lrc/embedded lyrics synced. **Done** (no folder watching, SYLT, LRCLIB or browser thumbnails yet). |
| M7 | System capture + Spotify + Apple Music | macOS tap/SCK capture, Spotify PKCE control, Music.app control, source switching. **Built**: tested end-to-end on Linux against a mock Spotify server with pulse-monitor capture. The macOS tap and AppleScript paths are type-checked only, and ScreenCaptureKit is not done. Needs the Mac checks in LOCAL_TODO.md. |
| M8 | Windows | WASAPI loopback, path/config differences, CI build on Windows. **Built, not yet run on Windows.** Loopback via cpal (it sets `AUDCLNT_STREAMFLAGS_LOOPBACK` on output devices). Paths: `%APPDATA%\coverwave` (config) and `%LOCALAPPDATA%\coverwave` (data, `cache\`), with `USERPROFILE` as home. Truecolor by default on Windows. The browser opens via `rundll32 url.dll` (cmd's `start` would split URLs at `&`). `.gitattributes` forces LF. CI (`.github/workflows/ci.yml`: fmt, clippy, tests and a release build on Linux, Windows and macOS) is **manual-only**, to avoid using the owner's Actions minutes, and has not been run yet. |

*After M8, Android (2026-11-25):* `android/` mirrors the iPhone app in Jetpack Compose on the same `cw-ffi` core (UniFFI Kotlin bindings over JNA). Sources: Spotify through the Web API (PKCE in a Custom Tab), Offline through Spotify's App Remote SDK, and Apple Music, or any player, through `MediaSessionManager` with a notification listener, since Android has no MusicKit. Android offers no way to browse Apple Music's library from another app. The art is drawn with `drawPosText` per colour, like Core Text on iOS. The Release workflow builds the APK, signed with a debug key committed in the repo (the owner's choice, so updates install in place), and publishes it as `coverwave-android.apk`.

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
