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

## Before M7 (Spotify / Apple Music / system audio)

- [ ] Create a Spotify developer app (developer.spotify.com). The config only
      ever holds the public client ID; tokens go to the macOS keychain.
      Playback control needs Spotify Premium.
- [ ] System-audio capture prompts for permission on first use (macOS 14.2+
      process tap, or Screen Recording on 13.x). Plan to click through that
      once.
