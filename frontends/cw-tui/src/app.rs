use crate::browse::{Action, Browser};
use crate::cover::{CoverEntry, CoverLoader, RecipeContext, Slot};
use crate::sources::{SourceKind, Sources};
use cw_audio::{Player, Snapshot};
use cw_core::{ArtStyle, AudioFrame, CellGrid, Repeat};
use cw_library::{Library, Lyrics, ScanStats};
use cw_sources::Command as Remote;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Art,
    Library,
}

impl Mode {
    pub fn next(self) -> Mode {
        match self {
            Mode::Art => Mode::Library,
            Mode::Library => Mode::Art,
        }
    }
}

/// How colours reach the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    /// xterm 256-colour palette, for terminals without 24-bit colour (for
    /// example macOS Terminal.app before macOS 26).
    Ansi256,
}

impl ColorMode {
    /// `COLORTERM=truecolor|24bit` means 24-bit colour; anything else gets the
    /// 256-colour fallback, which every modern terminal supports.
    pub fn detect() -> ColorMode {
        Self::detect_from(cfg!(windows), &|k| std::env::var(k).ok())
    }

    /// Windows 10+ consoles (conhost and Windows Terminal) all do 24-bit
    /// colour but rarely set `COLORTERM`, so Windows defaults to truecolor.
    pub fn detect_from(windows: bool, env: &dyn Fn(&str) -> Option<String>) -> ColorMode {
        let colorterm = env("COLORTERM").unwrap_or_default();
        if colorterm.eq_ignore_ascii_case("truecolor") || colorterm.eq_ignore_ascii_case("24bit") {
            return ColorMode::TrueColor;
        }
        if windows || env("WT_SESSION").is_some() {
            return ColorMode::TrueColor;
        }
        ColorMode::Ansi256
    }

    pub fn from_name(s: &str) -> Option<ColorMode> {
        match s.to_ascii_lowercase().as_str() {
            "truecolor" | "24bit" => Some(ColorMode::TrueColor),
            "256" => Some(ColorMode::Ansi256),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ColorMode::TrueColor => "truecolor",
            ColorMode::Ansi256 => "256",
        }
    }
}

/// Startup settings from the command line.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub art_style: ArtStyle,
    pub color_mode: ColorMode,
    /// Fixed cell width/height; `None` measures the terminal.
    pub cell_aspect: Option<f32>,
    /// Never download the analysis model.
    pub offline: bool,
    /// Start with cover motion on.
    pub motion: bool,
    pub fps: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            art_style: ArtStyle::default(),
            color_mode: ColorMode::TrueColor,
            cell_aspect: None,
            offline: false,
            motion: true,
            fps: 30,
        }
    }
}

/// The prepared scene for one cover entry at one size and style, kept until
/// any of those change.
pub struct ArtCache {
    pub entry: u64,
    pub width: u16,
    pub height: u16,
    pub style: ArtStyle,
    pub cell_aspect: f32,
    /// The static art (shown with motion off).
    pub base: CellGrid,
    pub scene: cw_render::Scene,
}

/// Frames over which a new recipe's motion fades in (1s at 30fps).
pub const FADE_FRAMES: u32 = 30;

/// What the `D` overlay shows; `D` cycles through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugPage {
    Off,
    /// fps, cover, live audio analysis.
    Stats,
    /// The cover's motion recipe.
    Recipe,
}

impl DebugPage {
    pub fn next(self) -> DebugPage {
        match self {
            DebugPage::Off => DebugPage::Stats,
            DebugPage::Stats => DebugPage::Recipe,
            DebugPage::Recipe => DebugPage::Off,
        }
    }
}

/// Where the app keeps its files. `None` disables that feature.
#[derive(Debug, Clone, Default)]
pub struct Dirs {
    pub playlists: Option<PathBuf>,
    pub queue_file: Option<PathBuf>,
}

/// A track counts as played once this fraction of it has been heard (or
/// four minutes, for long tracks).
const PLAYED_FRACTION: f64 = 0.5;
const PLAYED_SECS: f64 = 240.0;
/// Frames a status message stays up.
const STATUS_FRAMES: u32 = 120;

pub struct App {
    pub mode: Mode,
    pub fps: u32,
    pub frame: u32,
    pub debug_page: DebugPage,
    pub show_help: bool,
    pub should_quit: bool,
    /// Seed for the idle scene. Fixed so the idle animation is the same every
    /// launch; real covers derive theirs from the image hash (M4).
    pub idle_seed: u64,
    /// Percentage of art cells that changed on the last frame (debug overlay).
    pub last_change_pct: f32,
    /// Previous art frame, used to measure how many cells changed.
    pub prev_art: Option<cw_core::CellGrid>,
    pub player: Option<Player>,
    /// Set when the audio device could not be opened.
    pub audio_error: Option<String>,
    /// Snapshot taken once per frame so every widget sees the same state.
    pub snapshot: Option<Snapshot>,
    /// Music analysis, read once per frame alongside the snapshot.
    pub audio: AudioFrame,
    /// The UI's connection to the library (a scan thread has its own).
    pub library: Option<Library>,
    pub browser: Browser,
    /// Finished background scans.
    pub scan_rx: Option<Receiver<Result<ScanStats, String>>>,
    pub scan_status: String,
    pub dirs: Dirs,
    /// Playlist-name input, when `P` is waiting for one.
    pub prompt: Option<String>,
    /// A short message under the HUD (saved playlist, config problems).
    pub status: Option<(String, u32)>,
    pub show_lyrics: bool,
    /// Seconds added to the playback position when timing lyrics.
    pub lyrics_offset: f64,
    /// Lyrics of the track being heard, loaded when it starts.
    pub lyrics: Option<(PathBuf, Option<Lyrics>)>,
    /// The track being heard and the furthest point reached, for play counts.
    heard: Option<(PathBuf, f64, Option<f64>)>,
    /// Local, Spotify or Apple Music (`S` cycles).
    pub sources: Sources,
    pub art_style: ArtStyle,
    pub color_mode: ColorMode,
    /// Cell width / height used to keep covers in proportion.
    pub cell_aspect: f32,
    /// Whether `cell_aspect` was measured from the terminal (debug overlay).
    pub cell_aspect_measured: bool,
    cell_aspect_fixed: bool,
    pub covers: CoverLoader,
    /// The cover shown in Art mode. Kept while the next track's cover loads,
    /// so a track change does not flash the idle scene.
    pub shown_cover: Option<Arc<CoverEntry>>,
    /// Whether the model was ready last frame, to notice it becoming ready.
    model_was_ready: bool,
    pub art_cache: Option<ArtCache>,
    /// The last rendered frame, for the change limiter.
    pub prev_out: Option<cw_render::Output>,
    /// Motion on (`m` toggles).
    pub motion: bool,
    /// The cover entry whose motion is fading in, and the frame it started.
    pub fade_entry: u64,
    pub fade_start: u32,
    frame_times: VecDeque<Instant>,
}

const SEEK_SMALL: f64 = 5.0;
const SEEK_LARGE: f64 = 30.0;
const VOLUME_STEP: u8 = 5;

impl App {
    pub fn new(
        fps: u32,
        player: Option<Player>,
        audio_error: Option<String>,
        settings: Settings,
        recipes: RecipeContext,
    ) -> Self {
        App {
            mode: Mode::Art,
            fps,
            frame: 0,
            debug_page: DebugPage::Off,
            show_help: false,
            should_quit: false,
            idle_seed: 0x00C0_7E2A_7A7E,
            last_change_pct: 0.0,
            prev_art: None,
            player,
            audio_error,
            snapshot: None,
            audio: AudioFrame::default(),
            library: None,
            browser: Browser::new(None, None),
            scan_rx: None,
            scan_status: String::new(),
            dirs: Dirs::default(),
            prompt: None,
            status: None,
            show_lyrics: true,
            lyrics_offset: 0.0,
            lyrics: None,
            heard: None,
            sources: Sources::new(&crate::config::Config::default()),
            art_style: settings.art_style,
            color_mode: settings.color_mode,
            cell_aspect: settings
                .cell_aspect
                .unwrap_or(cw_render::DEFAULT_CELL_ASPECT),
            cell_aspect_measured: false,
            cell_aspect_fixed: settings.cell_aspect.is_some(),
            covers: CoverLoader::new(recipes),
            shown_cover: None,
            model_was_ready: false,
            art_cache: None,
            prev_out: None,
            motion: settings.motion,
            fade_entry: 0,
            fade_start: 0,
            frame_times: VecDeque::with_capacity(64),
        }
    }

    /// Refreshes the playback snapshot; call once before drawing a frame.
    pub fn tick(&mut self) {
        self.snapshot = self.player.as_ref().map(Player::snapshot);
        self.sources.tick();
        self.audio = if self.sources.is_remote() {
            // A remote player's audio comes from system capture (silence,
            // and gentle default motion, until capture is running).
            self.sources.audio_frame().unwrap_or_default()
        } else {
            self.player
                .as_ref()
                .map_or(AudioFrame::default(), Player::audio_frame)
        };
        self.covers.poll();
        // Covers analysed before the model was ready get proper recipes now.
        let ready = self.covers.model().ready().is_some();
        if ready && !self.model_was_ready {
            self.covers.invalidate_modelless();
        }
        self.model_was_ready = ready;
        self.update_cover();
        self.poll_scan();
        self.track_listening();
    }

    /// Attaches the library, its file locations and a running scan.
    pub fn attach_library(
        &mut self,
        library: Option<Library>,
        dirs: Dirs,
        scan_rx: Option<Receiver<Result<ScanStats, String>>>,
    ) {
        self.browser = Browser::new(library.as_ref(), dirs.playlists.clone());
        self.library = library;
        self.dirs = dirs;
        if scan_rx.is_some() {
            self.scan_status = "scanning".into();
        }
        self.scan_rx = scan_rx;
    }

    fn poll_scan(&mut self) {
        let Some(rx) = &self.scan_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.scan_rx = None;
        self.scan_status = match result {
            Ok(s) => {
                let changed = s.added + s.updated + s.removed;
                if changed > 0 {
                    self.browser.refresh(self.library.as_ref());
                }
                format!("{} tracks", s.total)
            }
            Err(e) => format!("scan failed: {e}"),
        };
    }

    pub fn say(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), self.frame));
    }

    /// The status message, while it is still fresh.
    pub fn status_text(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, at)| self.frame.wrapping_sub(*at) < STATUS_FRAMES)
            .map(|(m, _)| m.as_str())
    }

    /// Path of the track being heard.
    pub fn current_path(&self) -> Option<PathBuf> {
        if self.sources.is_remote() {
            return None;
        }
        let (snap, player) = (self.snapshot.as_ref()?, self.player.as_ref()?);
        snap.track.map(|t| player.tracks()[t].path.clone())
    }

    /// Loads lyrics when the track changes, and counts plays.
    fn track_listening(&mut self) {
        let current = self.current_path();
        let pos = self.snapshot.as_ref().map_or(0.0, |s| s.position_secs);
        let dur = self.snapshot.as_ref().and_then(|s| s.duration_secs);
        let same = matches!((&self.heard, &current), (Some((p, _, _)), Some(c)) if p == c);
        if same {
            if let Some(h) = self.heard.as_mut() {
                h.1 = h.1.max(pos);
            }
        } else {
            if let Some((path, furthest, duration)) = self.heard.take() {
                let enough = duration.map_or(furthest >= PLAYED_SECS, |d| {
                    furthest >= (d * PLAYED_FRACTION).min(PLAYED_SECS)
                });
                if enough && let Some(lib) = &self.library {
                    let _ = lib.record_play(&path, cw_library::now_secs());
                }
            }
            self.heard = current.clone().map(|p| (p, pos, dur));
        }
        let lyrics_stale = match (&self.lyrics, &current) {
            (Some((p, _)), Some(c)) => p != c,
            (None, Some(_)) => true,
            _ => false,
        };
        if lyrics_stale && let Some(c) = current {
            let l = cw_library::lyrics::load(&c);
            self.lyrics = Some((c, l));
        }
    }

    /// Lyrics of the current track, if any.
    pub fn current_lyrics(&self) -> Option<&Lyrics> {
        let current = self.current_path()?;
        match &self.lyrics {
            Some((p, l)) if *p == current => l.as_ref(),
            _ => None,
        }
    }

    /// Saves the queue for the next start.
    pub fn save_queue(&self) {
        let (Some(path), Some(player), Some(snap)) =
            (&self.dirs.queue_file, &self.player, &self.snapshot)
        else {
            return;
        };
        let tracks = player.tracks();
        if tracks.is_empty() {
            return;
        }
        let q = cw_library::state::SavedQueue {
            paths: tracks.iter().map(|t| t.path.clone()).collect(),
            current: snap.track.unwrap_or(0),
            position_secs: snap.position_secs,
            shuffle: snap.shuffle,
            repeat: match snap.repeat {
                Repeat::Off => "off",
                Repeat::All => "all",
                Repeat::One => "one",
            }
            .into(),
            volume: snap.volume,
        };
        let _ = q.save(path);
    }

    fn apply(&mut self, action: Action) {
        if action != Action::None && self.sources.is_remote() {
            // Picking local music switches back to local playback.
            self.sources.select(SourceKind::Local);
            self.say("source: Local");
        }
        let Some(p) = self.player.as_mut() else {
            if action != Action::None {
                self.say("no audio device: cannot play");
            }
            return;
        };
        match action {
            Action::None => {}
            Action::Play { tracks, start } => p.replace(tracks, start, 0.0),
            Action::Append(tracks) => {
                let n = tracks.len();
                for t in tracks {
                    p.append(t);
                }
                self.say(format!(
                    "added {n} track{} to the queue",
                    if n == 1 { "" } else { "s" }
                ));
            }
        }
    }

    fn save_playlist(&mut self, name: &str) {
        let (Some(dir), Some(player)) = (self.dirs.playlists.clone(), self.player.as_ref()) else {
            self.say("nothing to save");
            return;
        };
        // Queue in play order.
        let order = self
            .snapshot
            .as_ref()
            .map(|s| s.order.clone())
            .unwrap_or_default();
        let tracks: Vec<cw_audio::Track> =
            order.iter().map(|&i| player.tracks()[i].clone()).collect();
        if tracks.is_empty() {
            self.say("the queue is empty");
            return;
        }
        match cw_library::playlist::save(&dir, name, &tracks) {
            Ok(path) => {
                let shown = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.say(format!("saved playlist {shown} ({} tracks)", tracks.len()));
                self.browser.refresh(self.library.as_ref());
            }
            Err(e) => self.say(format!("cannot save playlist: {e}")),
        }
    }

    /// Requests covers for the playing track and the one after it (so a
    /// track change usually finds its art ready), and picks what Art mode
    /// shows.
    fn update_cover(&mut self) {
        if self.sources.is_remote() {
            let Some(now) = self.sources.state.as_ref().and_then(|s| s.now.clone()) else {
                self.shown_cover = None;
                return;
            };
            let key = format!("{}:{}", self.sources.active.name(), now.track_id);
            let art = now.art.clone();
            self.covers
                .request_with(&key, move || cw_sources::fetch_art(&art));
            match self.covers.get_key(&key) {
                Some(Slot::Ready(c)) => self.shown_cover = Some(c.clone()),
                Some(Slot::Missing(_)) => self.shown_cover = None,
                Some(Slot::Loading) | None => {}
            }
            return;
        }
        let (Some(snap), Some(player)) = (self.snapshot.as_ref(), self.player.as_ref()) else {
            return;
        };
        let Some(current) = snap.track else {
            self.shown_cover = None;
            return;
        };
        let tracks = player.tracks();
        let next = snap
            .order
            .iter()
            .position(|&o| o == current)
            .and_then(|i| snap.order.get(i + 1))
            .copied();
        for t in std::iter::once(current).chain(next) {
            self.covers.request(&tracks[t].path);
        }
        match self.covers.get(&tracks[current].path) {
            Some(Slot::Ready(c)) => self.shown_cover = Some(c.clone()),
            Some(Slot::Missing(_)) => self.shown_cover = None,
            Some(Slot::Loading) | None => {} // keep the previous cover a moment
        }
    }

    /// Re-reads the shown cover's recipe (picks up override edits).
    pub fn reload_recipe(&mut self) {
        if let Some(seed) = self.shown_cover.as_ref().map(|c| c.seed) {
            self.covers.invalidate(|e| e.cover.seed == seed);
        }
    }

    /// Measures the cell aspect ratio from the terminal's pixel size, when
    /// the terminal reports one and no fixed value was given.
    pub fn measure_cell_aspect(&mut self) {
        if self.cell_aspect_fixed {
            return;
        }
        if let Ok(ws) = ratatui::crossterm::terminal::window_size()
            && ws.columns > 0
            && ws.rows > 0
            && ws.width > 0
            && ws.height > 0
        {
            let cell_w = ws.width as f32 / ws.columns as f32;
            let cell_h = ws.height as f32 / ws.rows as f32;
            let aspect = cell_w / cell_h;
            if (0.25..=1.0).contains(&aspect) {
                self.cell_aspect = aspect;
                self.cell_aspect_measured = true;
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        // Text input first: the playlist-name prompt, then search.
        if let Some(text) = self.prompt.as_mut() {
            match key.code {
                KeyCode::Esc => self.prompt = None,
                KeyCode::Enter => {
                    let name = std::mem::take(text);
                    self.prompt = None;
                    if !name.trim().is_empty() {
                        self.save_playlist(&name);
                    }
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) => text.push(c),
                _ => {}
            }
            return;
        }
        if self.mode == Mode::Library && self.browser.in_search() {
            let lib = self.library.as_ref();
            match key.code {
                KeyCode::Esc => {
                    self.browser.back();
                }
                KeyCode::Backspace => {
                    if !self.browser.delete_char(lib) {
                        self.browser.back();
                    }
                }
                KeyCode::Up => self.browser.move_by(-1),
                KeyCode::Down => self.browser.move_by(1),
                KeyCode::PageUp => self.browser.move_by(-10),
                KeyCode::PageDown => self.browser.move_by(10),
                KeyCode::Enter => {
                    let a = self.browser.enter(lib);
                    self.apply(a);
                }
                KeyCode::Tab => {
                    let a = self.browser.add(lib);
                    self.apply(a);
                }
                KeyCode::Char(c) => self.browser.type_char(c, lib),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc if self.show_help => self.show_help = false,
            KeyCode::Esc if self.mode == Mode::Library && self.browser.levels.len() > 1 => {
                self.browser.back();
            }
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('/') => {
                self.mode = Mode::Library;
                self.browser
                    .open(crate::browse::ViewKind::Search, self.library.as_ref());
            }
            KeyCode::Char('P') => self.prompt = Some(String::new()),
            KeyCode::Char('S') => self.switch_source(),
            KeyCode::Char('L') if self.sources.active == SourceKind::Spotify => {
                self.sources.send(Remote::Login);
                self.say("opening Spotify login in your browser");
            }
            KeyCode::Char('l') => self.show_lyrics = !self.show_lyrics,
            KeyCode::Char('[') => self.lyrics_offset -= 0.25,
            KeyCode::Char(']') => self.lyrics_offset += 0.25,
            KeyCode::Tab => self.mode = self.mode.next(),
            KeyCode::Char('D') => self.debug_page = self.debug_page.next(),
            KeyCode::Char('R') => self.reload_recipe(),
            KeyCode::Char('?') => self.show_help = !self.show_help,
            KeyCode::Char('c') => self.art_style = self.art_style.next(),
            KeyCode::Char('m') => self.motion = !self.motion,
            _ => self.on_playback_key(key.code, shift),
        }
    }

    fn on_playback_key(&mut self, code: KeyCode, shift: bool) {
        if self.mode == Mode::Library {
            let lib = self.library.as_ref();
            let handled = match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.browser.move_by(-1);
                    true
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.browser.move_by(1);
                    true
                }
                KeyCode::PageUp => {
                    self.browser.move_by(-10);
                    true
                }
                KeyCode::PageDown => {
                    self.browser.move_by(10);
                    true
                }
                KeyCode::Home => {
                    self.browser.move_by(isize::MIN / 2);
                    true
                }
                KeyCode::End => {
                    self.browser.move_by(isize::MAX / 2);
                    true
                }
                KeyCode::Backspace => {
                    self.browser.back();
                    true
                }
                KeyCode::Enter => {
                    let a = self.browser.enter(lib);
                    self.apply(a);
                    true
                }
                KeyCode::Char('a') => {
                    let a = self.browser.add(lib);
                    self.apply(a);
                    true
                }
                _ => false,
            };
            if handled {
                return;
            }
        }
        if self.sources.is_remote() {
            self.remote_key(code, shift);
            return;
        }
        let Some(p) = self.player.as_mut() else {
            return;
        };
        match code {
            KeyCode::Char(' ') => p.toggle_pause(),
            KeyCode::Char('n') => p.next(),
            KeyCode::Char('p') => p.prev(),
            KeyCode::Right => p.seek_by(if shift { SEEK_LARGE } else { SEEK_SMALL }),
            KeyCode::Left => p.seek_by(-(if shift { SEEK_LARGE } else { SEEK_SMALL })),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let v = p.volume().saturating_add(VOLUME_STEP);
                p.set_volume(v);
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                let v = p.volume().saturating_sub(VOLUME_STEP);
                p.set_volume(v);
            }
            KeyCode::Char('s') => p.toggle_shuffle(),
            KeyCode::Char('r') => p.cycle_repeat(),
            _ => {}
        }
    }

    /// `S`: the next source. Local playback pauses while a remote source is
    /// followed.
    fn switch_source(&mut self) {
        if let Some(hint) = self.sources.hint() {
            self.say(hint);
            return;
        }
        let kind = self.sources.cycle();
        if kind != SourceKind::Local
            && let Some(p) = self.player.as_mut()
        {
            p.set_paused(true);
        }
        self.say(format!("source: {}", kind.name()));
    }

    /// Transport keys while a remote source is active.
    fn remote_key(&mut self, code: KeyCode, shift: bool) {
        let state = self.sources.state.clone();
        let now = state.as_ref().and_then(|s| s.now.clone());
        let pos = state
            .as_ref()
            .map_or(0.0, |s| s.position_now(Instant::now()));
        let step = if shift { SEEK_LARGE } else { SEEK_SMALL };
        let volume = now.as_ref().and_then(|n| n.volume).unwrap_or(50);
        let cmd = match code {
            KeyCode::Char(' ') => Remote::PlayPause,
            KeyCode::Char('n') => Remote::Next,
            KeyCode::Char('p') => Remote::Previous,
            KeyCode::Right => Remote::Seek((pos + step).max(0.0) as u64),
            KeyCode::Left => Remote::Seek((pos - step).max(0.0) as u64),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                Remote::Volume(volume.saturating_add(VOLUME_STEP).min(100))
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                Remote::Volume(volume.saturating_sub(VOLUME_STEP))
            }
            _ => return,
        };
        self.sources.send(cmd);
    }

    pub fn on_frame_drawn(&mut self, at: Instant) {
        self.frame = self.frame.wrapping_add(1);
        if self.frame_times.len() == 60 {
            self.frame_times.pop_front();
        }
        self.frame_times.push_back(at);
    }

    /// Measured frames per second over the last ~2 seconds.
    pub fn measured_fps(&self) -> f32 {
        match (self.frame_times.front(), self.frame_times.back()) {
            (Some(first), Some(last)) if self.frame_times.len() > 1 => {
                let secs = last.duration_since(*first).as_secs_f32();
                if secs > 0.0 {
                    (self.frame_times.len() - 1) as f32 / secs
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;

    fn test_context() -> RecipeContext {
        RecipeContext {
            store: crate::recipes::Store {
                cache: None,
                overrides: None,
            },
            scenes: cw_analysis::ScenePrompts::shipped(),
            pca: None,
            model: crate::models::ModelHandle::unavailable("test"),
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn colour_detection() {
        let none = |_: &str| None;
        assert_eq!(ColorMode::detect_from(false, &none), ColorMode::Ansi256);
        assert_eq!(ColorMode::detect_from(true, &none), ColorMode::TrueColor);
        let ct = |k: &str| (k == "COLORTERM").then(|| "truecolor".to_string());
        assert_eq!(ColorMode::detect_from(false, &ct), ColorMode::TrueColor);
        let wt = |k: &str| (k == "WT_SESSION").then(|| "x".to_string());
        assert_eq!(ColorMode::detect_from(false, &wt), ColorMode::TrueColor);
    }

    #[test]
    fn tab_cycles_modes_and_q_quits() {
        let mut app = App::new(30, None, None, Settings::default(), test_context());
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.mode, Mode::Library);
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.mode, Mode::Art);
        app.on_key(press(KeyCode::Char('D')));
        assert_eq!(app.debug_page, DebugPage::Stats);
        app.on_key(press(KeyCode::Char('D')));
        assert_eq!(app.debug_page, DebugPage::Recipe);
        app.on_key(press(KeyCode::Char('R'))); // nothing shown: no-op
        app.on_key(press(KeyCode::Char('m')));
        assert!(!app.motion);
        app.on_key(press(KeyCode::Char('c')));
        assert_eq!(app.art_style, ArtStyle::TrueColor);
        // Playback keys without a player are ignored, not a crash.
        app.on_key(press(KeyCode::Char(' ')));
        app.on_key(press(KeyCode::Right));
        app.on_key(press(KeyCode::Char('?')));
        assert!(app.show_help);
        app.on_key(press(KeyCode::Esc));
        assert!(!app.show_help && !app.should_quit);
        app.on_key(press(KeyCode::Char('q')));
        assert!(app.should_quit);
    }
}
