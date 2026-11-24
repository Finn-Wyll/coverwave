# Checks to do on your Mac

Everything here needs a real terminal, speakers, your music or your eyes.
None of it could be done in the cloud sessions that built M1–M6, where there
was no sound card and no human to press keys. Tick items off as you go, and
note anything that feels wrong; it can all be tuned.

## 0. Setup

- [ ] `cargo run -p cw-tui --release` once. It writes
      `~/.config/coverwave/config.toml` with comments.
- [ ] Set `music_dirs` in that file to your music folders (default `~/Music`),
      then restart. The library title shows "scanning" and then the track
      count.
- [ ] The first run downloads the cover-analysis model (45MB, from
      huggingface.co). Press `D` twice to see its progress. If your network
      blocks it, download the file listed in
      `frontends/cw-tui/src/models.rs` yourself and set
      `COVERWAVE_MODEL=/path/to/vision_model.onnx`.
      Licence reminder: MobileCLIP's weights are research/personal-use only.
      Fine for you, but do not ship them.

## M1: playback (never tried by hand)

- [ ] `space` pause/resume, `n` / `p`, `←` `→` seek 5s (`shift` for 30s),
      `+` `-` volume, `s` shuffle, `r` repeat (off → all → one).
- [ ] **Gaps:** play a gapless album (a live album or a continuous mix). There
      should be no click or silence between tracks.
- [ ] Seek straight after a track starts, while paused, and right before a
      track ends. M6 fixed a race here: a late audio callback could swallow
      the first 0.75s after a seek.
- [ ] A queue mixing 44.1kHz and 48kHz files plays at the right pitch.

## M2: static ASCII art

- [ ] Covers look **square**. `D` shows `cell 0.50 measured/assumed`. If they
      look stretched, set `cell_aspect` in the config.
- [ ] `c` cycles palette → truecolor → duotone. Pick your favourite and set
      `art_style` in the config.
- [ ] Try Terminal.app (256-colour fallback) and a truecolor terminal (iTerm2,
      Ghostty, kitty). Both should be readable.

## M3: audio analysis

- [ ] With `D` open on real music, the six bands follow the music, and the
      onset counter ticks on drum hits but not on held notes. The tuning
      constants are at the top of `crates/cw-audio/src/analysis.rs`.
- [ ] Turning the volume down does **not** shrink the meters (the analyser
      listens before the volume control). Say if you would rather it followed
      the volume.

## M4: cover analysis

- [ ] **Refit the style PCA on your library.** The shipped `assets/pca.bin` is
      provisional: 87 covers from the first 100 tracks of your playlist.

      ```sh
      cargo run -p pca-fit --release -- fit \
        --vision-model ~/.cache/coverwave/models/mobileclip_s0_vision.onnx \
        ~/Music
      ```

      Commit only `assets/pca.bin`. Cached recipes rebuild themselves from
      their saved embeddings.
- [ ] Spot-check scene recognition on a spread of your covers:
      `cargo run -p pca-fit --release -- scenes --vision-model <same path> cover1.jpg cover2.jpg ...`
      and list the misfires (known: dark-background portraits read as
      "very dark scene").

## M5: motion

- [ ] Watch a handful of different covers for a full 8-second loop each.
      Does the motion support the art or compete with it? Name any layer
      that feels too busy (snow, rain, fog, ripple, heat, clouds, flicker,
      neon, embers, dust, fireflies, sway, shimmer, ambient).
- [ ] With `D`: `fps` holds at 30/30 at full screen, and `changed` stays at
      about 15% or less.
- [ ] `cargo bench -p cw-render --bench render` stays under 4ms
      (0.5ms in the cloud).
- [ ] `m` turns motion off and on. Copy a recipe from
      `~/.cache/coverwave/recipes/<hash>.json` to
      `~/.config/coverwave/overrides/`, edit a layer, and press `R`.

## M6: library, search, queue, playlists, lyrics

- [ ] First scan of your full library: how long it takes, and whether
      artists and albums group sensibly (compilations use the album-artist
      tag; untagged files land in "Unknown artist").
- [ ] `/` search feels instant on your library size. `enter` plays the
      results, `tab` adds to the queue.
- [ ] Browse: `enter` opens or plays from a track, `a` adds, and
      `backspace`/`esc` go back.
- [ ] `P` saves the queue as a playlist. Open the `.m3u8` from
      `~/.local/share/coverwave/playlists/` in another player (VLC, Music).
- [ ] Quit, then start with no arguments: the queue comes back **paused** at
      the same place.
- [ ] Lyrics: one track with a `.lrc` next to it and one with lyrics in its
      tags. Check the timing, `[` / `]` to nudge it, and `l` to hide it.
- [ ] Play something more than halfway twice, then search for it: it should
      rank above similar matches.

## Decisions you may want to revisit

- The analyser listens before the volume control (PLAN.md said after it).
- The neutral scenes (portrait, typography, abstract) have a −3 logit bias.
- The `dense` charset is 18 symbols; no letters, which read as text.
- The browser has no album-art thumbnail yet; ID3 `SYLT` lyrics, LRCLIB
  lookup and folder watching are not done.

## M7: Spotify, Apple Music, system audio

The macOS parts were written and type-checked in the cloud but have
**never run on a Mac**. Expect to fix things here, ideally in a local Claude
Code session that can run them. Linux runs worked end to end against a mock
Spotify server.

### Setup
- [ ] Create a Spotify app at developer.spotify.com (Dashboard > Create app,
      Web API). Add the redirect URI `http://127.0.0.1:8898/callback`
      exactly. Copy the **Client ID**; you never need the secret.
- [ ] Put it in `~/.config/coverwave/config.toml`:
      ```toml
      [spotify]
      client_id = "..."
      redirect_port = 8898
      ```
      Development-mode Spotify apps only work for accounts you list under
      "User Management", so add yourself.

### System-audio capture (riskiest: `crates/cw-capture/src/macos.rs`)
- [ ] With Spotify playing, run
      `cargo run -p cw-capture --example meter -- 10 com.spotify.client`.
      macOS should ask for "audio recording" permission. Then the bars should
      move with the music. Try `com.apple.Music` with Music.app too.
- [ ] If there is no prompt and only silence: check that
      `frontends/cw-tui/build.rs` embedded the Info.plist
      (`otool -s __TEXT __info_plist target/debug/cw-tui`). The meter example
      has no Info.plist, so try the TUI too. Also check System Settings >
      Privacy & Security > Screen & System Audio Recording.
- [ ] If tap creation fails, print the OSStatus. Suspects, in order:
  - the aggregate-device dictionary keys
  - the tap format being non-interleaved
  - Spotify's audio coming from a helper process whose bundle ID does not
    start with `com.spotify.client` (list processes with
    `kAudioProcessPropertyBundleID`)
- [ ] Capture must not change what you hear (the tap is "unmuted"), and must
      stop cleanly on quit. No "coverwave capture" device should be left in
      Audio MIDI Setup.
- [ ] macOS 13–14.1 has no process taps; the ScreenCaptureKit fallback is not
      written yet (only needed if you are on an older macOS).

### Spotify in the TUI
- [ ] `S` → Spotify → "press L to log in" → `L` opens the browser → approve →
      the HUD shows what is playing, with the cover and its motion.
- [ ] Quit and restart: still logged in (refresh token in the Keychain; look
      for "coverwave" in Keychain Access).
- [ ] `space`, `n` / `p`, `←` / `→`, `+` / `-` control Spotify (Premium). If
      nothing is playing anywhere, you get "no active Spotify device".
- [ ] The progress bar moves smoothly between polls and corrects itself
      after a seek.
- [ ] The cover reacts to the music (captured), not only gentle default
      motion. `D` shows `capture on, 48000 Hz`.

### Apple Music (`crates/cw-sources/src/apple_music.rs`)
- [ ] `S` → Apple Music: macOS asks whether your terminal may control Music
      (Automation). Allow it. The track, cover, progress and keys work.
- [ ] A track without artwork shows the idle scene; a radio stream does not
      crash (`missing value` fields).
- [ ] Your locale's decimal separator (comma or dot) gives the right
      position and duration.

### Switching
- [ ] Switching to a remote source pauses local playback. Playing something
      from the library switches back to Local.
