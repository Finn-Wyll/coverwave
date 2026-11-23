use crate::cover::{CoverEntry, CoverLoader, RecipeContext, Slot};
use cw_audio::{Player, Snapshot};
use cw_core::{ArtStyle, AudioFrame, CellGrid};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;
use std::sync::Arc;
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
        match std::env::var("COLORTERM") {
            Ok(v) if v.eq_ignore_ascii_case("truecolor") || v.eq_ignore_ascii_case("24bit") => {
                ColorMode::TrueColor
            }
            _ => ColorMode::Ansi256,
        }
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
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            art_style: ArtStyle::default(),
            color_mode: ColorMode::TrueColor,
            cell_aspect: None,
            offline: false,
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
    /// Highlighted row in the library list.
    pub library_cursor: usize,
    /// First visible row of the library list.
    pub library_offset: usize,
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
            library_cursor: 0,
            library_offset: 0,
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
            motion: true,
            fade_entry: 0,
            fade_start: 0,
            frame_times: VecDeque::with_capacity(64),
        }
    }

    pub fn track_count(&self) -> usize {
        self.player.as_ref().map_or(0, |p| p.tracks().len())
    }

    /// Refreshes the playback snapshot; call once before drawing a frame.
    pub fn tick(&mut self) {
        self.snapshot = self.player.as_ref().map(Player::snapshot);
        self.audio = self
            .player
            .as_ref()
            .map_or(AudioFrame::default(), Player::audio_frame);
        self.covers.poll();
        // Covers analysed before the model was ready get proper recipes now.
        let ready = self.covers.model().ready().is_some();
        if ready && !self.model_was_ready {
            self.covers.invalidate_modelless();
        }
        self.model_was_ready = ready;
        self.update_cover();
    }

    /// Requests covers for the playing track and the one after it (so a
    /// track change usually finds its art ready), and picks what Art mode
    /// shows.
    fn update_cover(&mut self) {
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
            self.covers.request(t, tracks[t].path.clone());
        }
        match self.covers.get(current) {
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
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Esc if self.show_help => self.show_help = false,
            KeyCode::Esc => self.should_quit = true,
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
        let count = self.track_count();
        if self.mode == Mode::Library && count > 0 {
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.library_cursor = self.library_cursor.saturating_sub(1);
                    return;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.library_cursor = (self.library_cursor + 1).min(count - 1);
                    return;
                }
                KeyCode::Enter => {
                    let cursor = self.library_cursor;
                    if let Some(p) = self.player.as_mut() {
                        p.jump_to(cursor);
                    }
                    return;
                }
                _ => {}
            }
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
