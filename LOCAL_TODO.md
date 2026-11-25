# Checks to do on your Mac

Everything here needs a real terminal, speakers, your music or your eyes.
None of it could be done in the cloud sessions that built M1–M6, where there
was no sound card and no human to press keys. Tick items off as you go, and
note anything that feels wrong; it can all be tuned.

## 0. Setup

- [x] The workspace builds on macOS 27 with no warnings, all 118 tests pass,
      clippy is clean, and `cw-tui` embeds its Info.plist with both
      permission strings (checked 2026-11-24).

- [x] `cargo run -p cw-tui --release` once. It writes
      `~/.config/coverwave/config.toml` with comments.
- [x] Set `music_dirs` in that file to your music folders (default `~/Music`),
      then restart. The library title shows "scanning" and then the track
      count. (2026-11-24: the default `~/Music` was right. The scan found 128
      tracks in 52 artists and 94 albums, all with durations; 4 untagged
      files went to "Unknown artist". 10 songs appear twice because the
      Music library has "Song 1.mp3" copies.)
- [x] The first run downloads the cover-analysis model (45MB, from
      huggingface.co). (2026-11-24: downloaded 45.5MB and the SHA-256
      matches the pinned value.) Press `D` twice to see its progress. If your network
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
      neon, embers, dust, fireflies, sway, shimmer, ambient, aurora,
      starfield, digital rain, lightning).
- [ ] Beat effects on real music: shockwave from the subject, bass thump,
      glitch slices on big hits, margin glow and equaliser. Too much, too
      little, or wrong on some genre? Tuning constants are at the top of
      `crates/cw-render/src/scene.rs` (`RING_TINT`, `THUMP_CELLS`,
      `BEAT_BURST`) and in `gain_intensity`.
- [ ] Menus never get corrupted any more. If they do, press ctrl-l and
      send the contents of `~/.cache/coverwave/coverwave.log`.
- [ ] With `D`: `fps` holds at 30/30 at full screen, and `changed` stays at
      about 25% or less between beats (beats may briefly go higher).
- [x] `cargo bench -p cw-render --bench render` stays under 4ms
      (0.5ms in the cloud; on this Mac 0.42ms, 0.72ms with the change
      limiter, and 0.28ms for the new layers on a full beat, on 2026-11-24).
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
- [x] With Spotify playing, run
      `cargo run -p cw-capture --example meter -- 10 com.spotify.client`.
      macOS should ask for "audio recording" permission. Then the bars should
      move with the music. (2026-11-24: worked first time at 48000 Hz; bands
      moved and onsets counted about 2.5 per second. Bars mostly sat at 70-100%
      on loud masters. `RANGE_DB` was then tuned from 30 to 15 dB using
      `examples/range_tune.rs` on the whole library.)
- [ ] Try `com.apple.Music` with Music.app too.
- [ ] If there is no prompt and only silence: check that
      `frontends/cw-tui/build.rs` embedded the Info.plist
      (`otool -s __TEXT __info_plist target/debug/coverwave`). The meter example
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
      Audio MIDI Setup. (Stopping cleanly and leaving no device behind were
      verified 2026-11-24. Whether the sound changes still needs your ears.)
- [ ] macOS 13–14.1 has no process taps; the ScreenCaptureKit fallback is not
      written yet (only needed if you are on an older macOS).

### Spotify in the TUI
- [ ] Library: with Spotify active, `tab` shows Liked Songs, Playlists and
      Albums. Log in again with `L` once (new permissions). `enter` opens or
      plays, `/` filters, `backspace` goes back, `r` refreshes. Followed
      playlists you do not own play whole (Spotify will not list them).
- [ ] With the Spotify app closed, play something from the library:
      coverwave should open Spotify in the background and start it there.
- [ ] `S` → Spotify → "press L to log in" → `L` opens the browser → approve →
      the HUD shows what is playing, with the cover and its motion.
- [ ] Quit and restart: still logged in (refresh token in the Keychain; look
      for "coverwave" in Keychain Access).
- [ ] `space`, `n` / `p`, `←` / `→`, `+` / `-` control Spotify (Premium). If
      nothing is playing anywhere, you get "no active Spotify device".
- [ ] The progress bar moves smoothly between polls and corrects itself
      after a seek.
- [x] The cover reacts to the music (captured), not only gentle default
      motion. `D` shows `capture on, 48000 Hz`. (2026-11-24: capture ran but
      heard silence until the owner granted the missing permission in System
      Settings > Privacy & Security > Screen & System Audio Recording.)

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

## Variety (genre + 31 animations)

- [ ] Play a mix of albums. Covers should now differ clearly, and songs of
      one genre should feel related. `D` twice shows `genre` and the
      layers. Your library re-reads tags once after this update, to pick up
      genres.
- [ ] Any new effect too busy for your taste? Name it. They are all tunable
      in `crates/cw-render/src/scene/extra.rs`.
- [ ] Spotify: genre comes from the artist; Apple Music: from the track.
      Check that the recipe page shows a genre for both.
- [ ] **Beat clock:** set `[capture] enabled = false`, play Spotify and
      press `D`. `tempo` shows the looked-up BPM, and the cover pulses in
      time with the song. Tell me about any songs where it is clearly off,
      for example at double or half speed. Quick check without the app:
      `cargo run -p cw-sources --release --example tempo_lookup -- "Artist" "Title"`.

## M8: Windows and CI (optional)

Nothing here has run on Windows yet.

- [ ] **Make the repo public** (GitHub > Settings > General > Danger
      Zone) if you want free builds: CI and Release then run on every push.
      Commits so far show your university email as author; for new ones,
      use GitHub's noreply address (Settings > Emails) with
      `git config user.email <id>+<user>@users.noreply.github.com`.
- [ ] After the first Release run: `ios/fetch-core.sh` then
      `ios/run-on-iphone.sh` installs the app without building Rust here.
- [ ] **On a Windows PC (if you have one):** `cargo run -p cw-tui --release`
      in Windows Terminal. Check colours, keys (each key acts once), the
      config at `%APPDATA%\coverwave\config.toml`, the library under
      `%LOCALAPPDATA%\coverwave`, playback, and that `S` → Spotify captures
      audio (whole-system loopback) and opens the login in your browser.

## iPhone app (2026-11-24 additions)

- [ ] **Rhythm maps:** play a few Spotify or Apple Music songs. Settings >
      This Cover > Rhythm should say "N BPM in 4/4, measured from the
      song" within a couple of seconds of a new track. The motion should
      follow the song's own kick/snare/hat pattern, not a generic one.
- [ ] **Beat lines up without the microphone:** the hits should land on
      the beat you hear from the first seconds of a song. If a song's
      Rhythm line says "bar start estimated", Deezer has a different edit
      of it; tell me which songs feel off.
- [ ] **Spotify stays logged in:** log in once more (the iPhone app now
      uses Spotify's web login). Then pause a song, close coverwave, wait a
      while, and open it again: it should show the paused song with no
      login. Settings > This Cover > Genre should no longer say unknown.
- [ ] **Spotify devices:** with music playing on the Mac, the cover's
      text says "on <your mac>". Swipe down: the devices list at the top
      moves playback (tap "this iPhone" to bring it to the phone).
- [ ] **Lyrics:** desktop `l`, iPhone two-finger sideways swipe. Spotify
      songs should now have lyrics (from LRCLIB) with the cover still
      moving behind and around them, in a square where the cover was. Try
      the styles and Lyric Size (Settings > Lyrics on the phone;
      `[lyrics] style = "big" | "text" | "big_only"`,
      `scroll = "up" | "down"`, `size = "small" | "medium" | "large"` and
      `colors = "accent" | "cover"` in the desktop config, or `,`).
      "Cover" colours make the letters windows onto the art. Effects that
      light the cover (sweeps, flicker, glow, rays) should light the
      letters too. Detail (phone) or a smaller terminal font
      should make the big letters finer, not smaller. `[` `]` nudge the
      timing on the desktop.
- [ ] **Desktop settings:** `,` opens the settings menu (lyrics, art,
      Ghostty font size, sources, tempo). Changes apply at once and are
      saved to `config.toml` with your comments kept.
- [ ] **Desktop devices:** Spotify library (`tab`) > Devices > `enter`
      moves playback; the HUD and the right panel show "on <device>".
- [ ] **Ghostty zoom:** run `coverwave` in Ghostty: its window drops to a
      9 pt font (more, finer characters) and goes back when you quit with
      `q`. Change the size with `font_size` in
      `~/.config/coverwave/config.toml` (0 turns it off).
- [ ] **Art detail:** Settings > Detail: Standard (64 columns), Fine
      (96, the new default) and Finest (128). Check Finest still moves
      smoothly on your iPhone 13; if it stutters, say so.
- [ ] **Search everywhere:** iPhone library > `/` search finds Spotify
      catalogue songs (not just your playlists) and Apple Music catalogue
      songs below your own library matches. On the Mac, Spotify library:
      `/`, type, `enter` searches all of Spotify.

## Offline Spotify (2026-11-25)

- [ ] iPhone: in Settings > Offline, tap **Log In Again for Offline** if it
      shows (the login now asks Spotify for `app-remote-control`).
- [ ] iPhone, while online: leave coverwave open on Wi-Fi for a while and
      watch Settings > Offline > Saved count up. Check the size is sensible.
- [x] iPhone, offline: following Spotify with no internet fails with
      `wamp.error.authorization_failed`, even with a token minutes old
      (2026-11-25). Spotify approves each connection online; kept anyway.
- [ ] iPhone, online with Spotify playing a downloaded song: pick offline.
      It should connect and show the saved cover, lyrics and rhythm.
- [ ] iPhone offline: library panel opens saved liked songs and playlists;
      playing a playlist row starts at that song; search finds saved songs.
- [ ] Spotify dashboard: App Remote needs the iOS bundle ID
      (com.finnleywyllie.coverwave) in your Spotify app's settings; it was
      there when App Remote was used before.
- [ ] Mac: with Spotify (the desktop app) playing a downloaded song, turn
      Wi-Fi off. Within ~30 s coverwave should follow the Spotify app
      (macOS asks once to let your terminal control Spotify). Transport keys,
      the saved library and the cover should still work.
- [ ] Mac: `,` > offline > spotify offline = always, then check nothing goes
      online; save ahead fills `~/.local/share/coverwave/offline/`.

## Android app (2026-11-25)

Built and compiled in the cloud, with the Kotlin bindings tested against
the core, but **never run on a phone** (no emulator there). Expect to fix
things on the first run.

- [ ] Install `coverwave-android.apk` from the latest build. It opens on the
      connect screen; the cover-analysis model downloads (top-left status).
- [ ] Spotify: add the client ID (repository variable `SPOTIFY_CLIENT_ID`,
      or paste it in Settings), and in the Spotify dashboard add the redirect
      URI `coverwave://spotify-callback` and the Android package
      `app.coverwave` with SHA-1
      `1F:C1:54:69:E7:2A:EB:2D:2A:42:44:F8:C3:AD:0E:28:49:38:74:32`.
      Then log in, play something, and check the cover moves to the beat.
- [ ] Apple Music: tap it, allow Notification access for coverwave, go back.
      The song, cover and controls should follow Apple Music. Also try
      another player (YouTube Music...): it should be picked up when Apple
      Music is not playing.
- [ ] Gestures: tap, hold + slide to scrub, swipes (next/previous, library
      up, queue down), two-finger tap (settings), two-finger swipe (lyrics).
      The panels should follow the finger and be thrown by a flick.
- [ ] Offline: with Spotify downloads, pick Offline. Spotify asks once to
      allow coverwave (needs the internet that first time).
- [ ] Smoothness: the art at Fine and Finest detail, in True colour, should
      hold 30fps. If it stutters, say which setting.
