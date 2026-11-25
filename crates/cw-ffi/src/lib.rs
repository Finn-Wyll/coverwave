//! coverwave's shared core for native apps, exported with UniFFI.
//!
//! The iOS app (`ios/`) links this as a static library, and Swift bindings
//! are generated from it (`ios/build-core.sh`). It wraps the same analysis,
//! recipe mapping, renderer and beat clock the terminal app uses, so a cover
//! moves identically on both.
//!
//! No I/O happens here. Swift fetches covers, the CLIP model and song
//! previews, and decodes audio with AVFoundation; this crate only computes.
//!
//! Threading: [`CoverEngine::analyse`] is slow (CLIP inference, around a
//! second on a phone), so call it off the main thread. Everything else is
//! cheap. All objects are `Send + Sync`.

use cw_analysis::{ClipModel, GenreFamily, Pca, ScenePrompts};
use cw_core::{ArtStyle, AudioFrame, BAND_COUNT, BeatClock, Cell, CellGrid, Recipe, Rgb};
use std::sync::{Arc, Mutex, RwLock};

uniffi::setup_scaffolding!();

const PCA_BIN: &[u8] = include_bytes!("../../../assets/pca.bin");
/// Frames over which a new cover's motion fades in (1s at 30fps).
const FADE_FRAMES: u32 = 30;
/// Loop length of the idle scene (8s at 30fps).
const IDLE_LOOP_FRAMES: u32 = 240;

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CoreError {
    #[error("cannot read the cover: {0}")]
    Cover(String),
    #[error("cover-analysis model: {0}")]
    Model(String),
}

/// The colour of a cell or a palette entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct CoverColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<Rgb> for CoverColor {
    fn from(c: Rgb) -> CoverColor {
        CoverColor {
            r: c.0,
            g: c.1,
            b: c.2,
        }
    }
}

impl From<CoverColor> for Rgb {
    fn from(c: CoverColor) -> Rgb {
        Rgb(c.r, c.g, c.b)
    }
}

/// A summary of the music for one moment (see `cw_core::AudioFrame`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AudioLevels {
    /// Six bands, sub-bass to treble, each 0..1.
    pub bands: Vec<f32>,
    pub level: f32,
    pub onset: f32,
    pub onset_count: u64,
}

impl Default for AudioLevels {
    fn default() -> Self {
        AudioFrame::default().into()
    }
}

impl From<AudioFrame> for AudioLevels {
    fn from(f: AudioFrame) -> AudioLevels {
        AudioLevels {
            bands: f.bands.to_vec(),
            level: f.level,
            onset: f.onset,
            onset_count: f.onset_count,
        }
    }
}

impl From<&AudioLevels> for AudioFrame {
    fn from(a: &AudioLevels) -> AudioFrame {
        let mut bands = [0.0; BAND_COUNT];
        for (b, v) in bands.iter_mut().zip(&a.bands) {
            *b = v.clamp(0.0, 1.0);
        }
        AudioFrame {
            bands,
            level: a.level.clamp(0.0, 1.0),
            onset: a.onset.clamp(0.0, 1.0),
            onset_count: a.onset_count,
        }
    }
}

/// How cover pixels become coloured glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum ArtMode {
    #[default]
    Palette,
    TrueColor,
    Duotone,
}

impl From<ArtMode> for ArtStyle {
    fn from(m: ArtMode) -> ArtStyle {
        match m {
            ArtMode::Palette => ArtStyle::PaletteLed,
            ArtMode::TrueColor => ArtStyle::TrueColor,
            ArtMode::Duotone => ArtStyle::Duotone,
        }
    }
}

/// One rendered frame, row-major. Colours are packed `0xRRGGBB`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CellFrame {
    pub cols: u16,
    pub rows: u16,
    /// Exactly `cols * rows` characters, no newlines.
    pub glyphs: String,
    pub fg: Vec<u32>,
    /// Background per cell; [`NO_BACKGROUND`] where the page shows through.
    pub bg: Vec<u32>,
}

/// `CellFrame::bg` value for "no background".
pub const NO_BACKGROUND: u32 = 0x0100_0000;

#[uniffi::export]
pub fn no_background() -> u32 {
    NO_BACKGROUND
}

fn pack(c: Rgb) -> u32 {
    (c.0 as u32) << 16 | (c.1 as u32) << 8 | c.2 as u32
}

impl From<&CellGrid> for CellFrame {
    fn from(g: &CellGrid) -> CellFrame {
        let cells = g.cells();
        CellFrame {
            cols: g.width(),
            rows: g.height(),
            glyphs: cells.iter().map(|c: &Cell| c.ch).collect(),
            fg: cells.iter().map(|c| pack(c.fg)).collect(),
            bg: cells
                .iter()
                .map(|c| c.bg.map_or(NO_BACKGROUND, pack))
                .collect(),
        }
    }
}

/// Holds the shipped scene prompts and PCA, plus the CLIP model once the app
/// has downloaded it.
#[derive(uniffi::Object)]
pub struct CoverEngine {
    scenes: ScenePrompts,
    pca: Option<Pca>,
    clip: RwLock<Option<Arc<ClipModel>>>,
}

#[uniffi::export]
impl CoverEngine {
    #[uniffi::constructor]
    pub fn new() -> Arc<CoverEngine> {
        Arc::new(CoverEngine {
            scenes: ScenePrompts::shipped(),
            pca: Pca::from_bytes(PCA_BIN).ok(),
            clip: RwLock::new(None),
        })
    }

    /// Loads MobileCLIP-S0's vision model (`vision_model.onnx`). Slow (a
    /// second or two): call off the main thread.
    pub fn load_model(&self, onnx: Vec<u8>) -> Result<(), CoreError> {
        let model =
            ClipModel::from_onnx_bytes(&onnx).map_err(|e| CoreError::Model(e.to_string()))?;
        *self.clip.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(model));
        Ok(())
    }

    pub fn has_model(&self) -> bool {
        self.clip
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Analyses a cover and builds its recipe for `genre` (a raw genre tag
    /// such as "Alternative Rock", or none).
    ///
    /// Pass a cached `embedding` (from [`AnalysedCover::embedding`]) to skip
    /// the CLIP model. Without either, the recipe uses neutral scenes and
    /// should not be cached ([`AnalysedCover::is_complete`]).
    pub fn analyse(
        &self,
        image: Vec<u8>,
        genre: Option<String>,
        embedding: Option<Vec<f32>>,
    ) -> Result<Arc<AnalysedCover>, CoreError> {
        let cover = cw_analysis::analyse(&image).map_err(|e| CoreError::Cover(e.to_string()))?;
        let clip = self.clip.read().unwrap_or_else(|e| e.into_inner()).clone();
        let embedding = match (embedding, &clip) {
            (Some(e), _) => Some(e),
            (None, Some(m)) => Some(
                m.embed(&cover.image)
                    .map_err(|e| CoreError::Model(e.to_string()))?,
            ),
            (None, None) => None,
        };
        let genre = genre
            .as_deref()
            .map_or(GenreFamily::Unknown, GenreFamily::of_any);
        let models = cw_analysis::Models {
            clip: None,
            scenes: &self.scenes,
            pca: self.pca.as_ref(),
            genre,
        };
        let recipe = cw_analysis::recipe_with_embedding(&cover, embedding.as_deref(), models);
        Ok(Arc::new(AnalysedCover {
            cover,
            recipe,
            embedding,
            genre,
        }))
    }
}

/// A cover with its recipe.
#[derive(uniffi::Object)]
pub struct AnalysedCover {
    cover: cw_analysis::Cover,
    recipe: Recipe,
    embedding: Option<Vec<f32>>,
    genre: GenreFamily,
}

#[uniffi::export]
impl AnalysedCover {
    /// Hex SHA-256 of the cover bytes: a cache key.
    pub fn hash(&self) -> String {
        cw_analysis::hash_hex(&self.cover.hash)
    }

    pub fn embedding(&self) -> Option<Vec<f32>> {
        self.embedding.clone()
    }

    /// Built with a CLIP embedding (worth caching).
    pub fn is_complete(&self) -> bool {
        self.embedding.is_some()
    }

    /// The one UI accent colour, from the cover's palette.
    pub fn accent(&self) -> CoverColor {
        self.cover.palette.accent.into()
    }

    pub fn palette(&self) -> Vec<CoverColor> {
        self.cover
            .palette
            .colors
            .iter()
            .map(|&c| c.into())
            .collect()
    }

    /// The genre family the recipe was built for ("unknown" if none).
    pub fn genre_family(&self) -> String {
        self.genre.name().to_string()
    }

    /// The motion layers, strongest first (e.g. "pulse_rings").
    pub fn layer_kinds(&self) -> Vec<String> {
        self.recipe
            .layers
            .iter()
            .map(|l| l.kind().to_string())
            .collect()
    }

    /// Typical tempo for the genre, used until a real tempo is known.
    pub fn typical_bpm(&self) -> f32 {
        self.genre.typical_bpm()
    }

    pub fn recipe_json(&self) -> String {
        serde_json::to_string_pretty(&self.recipe).unwrap_or_default()
    }
}

struct AnimState {
    scene: cw_render::Scene,
    base: CellGrid,
    prev: Option<cw_render::Output>,
    first_frame: Option<u32>,
}

/// Renders one cover at one grid size, frame by frame. Make a new one when
/// the cover, the size or the art mode changes.
#[derive(uniffi::Object)]
pub struct Animator {
    state: Mutex<AnimState>,
    loop_frames: u32,
    /// The cover's colours for lyrics.
    lyric_colors: cw_render::lyrics::LyricColors,
    /// The cover's square on the grid, where lyrics go.
    art: cw_render::Rect,
    cell_aspect: f32,
    recipe: Recipe,
    ramp: &'static [char],
}

#[uniffi::export]
impl Animator {
    /// `cell_aspect` is a cell's width divided by its height (about 0.6 for
    /// SF Mono at line height 1.0). `background` fills every cell.
    #[uniffi::constructor]
    pub fn new(
        cover: Arc<AnalysedCover>,
        cols: u16,
        rows: u16,
        cell_aspect: f32,
        mode: ArtMode,
        background: CoverColor,
    ) -> Arc<Animator> {
        let recipe = &cover.recipe;
        let ramp = recipe.global.charset.ramp();
        let aspect = if cell_aspect.is_finite() {
            cell_aspect.clamp(0.2, 2.0)
        } else {
            cw_render::DEFAULT_CELL_ASPECT
        };
        let params = cw_render::ArtParams {
            cell_aspect: aspect,
            style: mode.into(),
            background: background.into(),
            ramp,
        };
        let image = &cover.cover.image;
        let base = cw_render::base_art(image, &cover.cover.palette, cols, rows, params);
        let art =
            cw_render::art_rect(image.width, image.height, cols, rows, aspect).unwrap_or_default();
        let scene = cw_render::prepare(recipe, base.clone(), art, ramp);
        let palette = &cover.cover.palette;
        // The lightest palette colour, eased toward plain text so it reads.
        let light = palette
            .colors
            .iter()
            .max_by_key(|c| c.0 as u32 + c.1 as u32 + c.2 as u32)
            .copied()
            .unwrap_or(Rgb(196, 204, 214));
        let lyric_colors = cw_render::lyrics::LyricColors {
            accent: palette.accent,
            text: light.lerp(Rgb(222, 226, 232), 0.5),
            background: background.into(),
        };
        Arc::new(Animator {
            lyric_colors,
            state: Mutex::new(AnimState {
                scene,
                base,
                prev: None,
                first_frame: None,
            }),
            art,
            cell_aspect: aspect,
            recipe: recipe.clone(),
            ramp,
            loop_frames: recipe.global.loop_frames,
        })
    }

    /// The still cover, for when motion is off (or Reduce Motion is on).
    pub fn still(&self) -> CellFrame {
        CellFrame::from(&self.state.lock().unwrap_or_else(|e| e.into_inner()).base)
    }

    /// CellFrame number `frame` of a 30fps clock. Motion fades in over the first
    /// second after the animator's first frame, and never changes more cells
    /// per frame than the recipe's budget.
    pub fn frame(&self, frame: u32, audio: AudioLevels) -> CellFrame {
        CellFrame::from(&self.render(frame, &AudioFrame::from(&audio)))
    }

    /// Lyrics in place of the cover: a square the cover's size, centred
    /// where it was, at `position` seconds into the track. The cover's
    /// motion keeps running behind and around the words.
    #[allow(clippy::too_many_arguments)]
    pub fn frame_with_lyrics(
        &self,
        frame: u32,
        audio: AudioLevels,
        lyrics: Arc<LyricSheet>,
        position: f64,
        duration: Option<f64>,
        style: LyricStyleKind,
        scroll: LyricScrollKind,
        size: LyricSizeKind,
        paint: LyricPaintKind,
        animate: bool,
    ) -> CellFrame {
        let audio = AudioFrame::from(&audio);
        let cover_base = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .base
            .clone();
        let view = cw_render::lyrics::LyricView {
            style: style.into(),
            scroll: scroll.into(),
            size: size.into(),
            paint: paint.into(),
        };
        // The words drawn into the cover's square, then the cover's own
        // recipe run over them: every effect acts on the letters.
        let base = cw_render::lyrics::lyric_base(
            &cover_base,
            self.art,
            self.cell_aspect,
            &lyrics.lyrics,
            position,
            duration,
            view,
            self.lyric_colors,
            &audio,
        );
        if !animate {
            return CellFrame::from(&base);
        }
        let scene = cw_render::lyrics::lyric_scene(&self.recipe, base, self.art, self.ramp);
        let out = cw_render::render(
            &scene,
            cw_render::Input {
                frame,
                audio: &audio,
                fade: 1.0,
            },
        );
        CellFrame::from(&out.grid)
    }

    /// Frames in one seamless loop of this cover's motion.
    pub fn loop_frames(&self) -> u32 {
        self.loop_frames
    }
}

impl Animator {
    /// The cover's frame `frame`, with the fade-in and change budget.
    fn render(&self, frame: u32, audio: &AudioFrame) -> CellGrid {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let start = *s.first_frame.get_or_insert(frame);
        let fade = (frame.wrapping_sub(start) as f32 / FADE_FRAMES as f32).min(1.0);
        let out = cw_render::render(&s.scene, cw_render::Input { frame, audio, fade });
        let out = match s.prev.take() {
            Some(prev) => cw_render::limit_changes(&s.scene, &prev, out),
            None => out,
        };
        let grid = out.grid.clone();
        s.prev = Some(out);
        grid
    }
}

// MARK: Lyrics

/// How lyric lines look (see `cw_render::lyrics::LyricStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum LyricStyleKind {
    /// The sung line in big letters, the others as text.
    #[default]
    BigCurrent,
    Text,
    BigOnly,
}

impl From<LyricStyleKind> for cw_render::lyrics::LyricStyle {
    fn from(k: LyricStyleKind) -> Self {
        match k {
            LyricStyleKind::BigCurrent => Self::BigCurrent,
            LyricStyleKind::Text => Self::Text,
            LyricStyleKind::BigOnly => Self::BigOnly,
        }
    }
}

/// How tall the large lyric letters are (a share of the square, the same
/// at every detail level).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum LyricSizeKind {
    Small,
    #[default]
    Medium,
    Large,
}

impl From<LyricSizeKind> for cw_render::lyrics::LyricSize {
    fn from(k: LyricSizeKind) -> Self {
        match k {
            LyricSizeKind::Small => Self::Small,
            LyricSizeKind::Medium => Self::Medium,
            LyricSizeKind::Large => Self::Large,
        }
    }
}

/// Where lyric colours come from: the accent, or the cover itself (the
/// letters as windows onto the art).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum LyricPaintKind {
    #[default]
    Accent,
    Cover,
}

impl From<LyricPaintKind> for cw_render::lyrics::LyricPaint {
    fn from(k: LyricPaintKind) -> Self {
        match k {
            LyricPaintKind::Accent => Self::Accent,
            LyricPaintKind::Cover => Self::Cover,
        }
    }
}

/// Which way lyric lines move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum LyricScrollKind {
    #[default]
    Up,
    Down,
}

impl From<LyricScrollKind> for cw_render::lyrics::LyricScroll {
    fn from(k: LyricScrollKind) -> Self {
        match k {
            LyricScrollKind::Up => Self::Up,
            LyricScrollKind::Down => Self::Down,
        }
    }
}

/// A song's lyrics, parsed from LRC (synced) or plain text.
#[derive(uniffi::Object)]
pub struct LyricSheet {
    lyrics: cw_core::lyrics::Lyrics,
}

#[uniffi::export]
impl LyricSheet {
    pub fn is_synced(&self) -> bool {
        self.lyrics.synced
    }

    pub fn line_count(&self) -> u32 {
        self.lyrics.lines.len() as u32
    }
}

/// Parses LRC or plain lyrics text; `None` if it holds no lines.
#[uniffi::export]
pub fn parse_lyrics(text: String) -> Option<Arc<LyricSheet>> {
    cw_core::lyrics::parse_lrc(&text, "lrclib").map(|lyrics| Arc::new(LyricSheet { lyrics }))
}

/// The idle scene (drifting particles behind a wordmark) for when nothing
/// is playing.
#[uniffi::export]
pub fn idle_frame(
    cols: u16,
    rows: u16,
    frame: u32,
    seed: u64,
    background: CoverColor,
) -> CellFrame {
    let style = cw_render::IdleStyle {
        background: background.into(),
        ..Default::default()
    };
    CellFrame::from(&cw_render::idle(
        cols,
        rows,
        frame,
        IDLE_LOOP_FRAMES,
        seed,
        style,
    ))
}

/// Synthetic audio for music the app cannot hear (Spotify and Apple Music
/// on iOS): a beat clock at `bpm` with the genre's rhythm, following the
/// playback position. `bpm` of 0 uses the genre's typical tempo.
#[uniffi::export]
pub fn beat_levels(
    bpm: f32,
    genre: Option<String>,
    position_secs: f64,
    playing: bool,
) -> AudioLevels {
    let g = genre
        .as_deref()
        .map_or(GenreFamily::Unknown, GenreFamily::of_any);
    let bpm = if bpm > 0.0 { bpm } else { g.typical_bpm() };
    let energy = (0.5 + g.energy_bias()).clamp(0.0, 1.0);
    BeatClock::new(bpm, g.beat_pattern(), energy)
        .frame(position_secs, playing)
        .into()
}

/// The genre family a raw genre tag falls in ("unknown" if none matches).
#[uniffi::export]
pub fn genre_family(genre: String) -> String {
    GenreFamily::of_any(&genre).name().to_string()
}

// MARK: Rhythm maps

/// Errors from measuring or reading a rhythm map.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum RhythmFailure {
    #[error("{0}")]
    Failed(String),
}

/// The Beat This! beat tracker (bundled with the app, MIT licence). Measures
/// a song's rhythm map from a short clip (its 30-second preview). Slow
/// (about a second on a phone): call off the main thread.
#[derive(uniffi::Object)]
pub struct RhythmEngine {
    model: cw_analysis::BeatModel,
}

#[uniffi::export]
impl RhythmEngine {
    #[uniffi::constructor]
    pub fn new(onnx: Vec<u8>) -> Result<Arc<RhythmEngine>, RhythmFailure> {
        let model = cw_analysis::BeatModel::from_onnx_bytes(&onnx)
            .map_err(|e| RhythmFailure::Failed(e.to_string()))?;
        Ok(Arc::new(RhythmEngine { model }))
    }

    /// The rhythm map of a mono clip at any sample rate. `clip_start`:
    /// where the clip starts in the song, if known (48s for Deezer
    /// previews); it sets the bar phase.
    pub fn measure(
        &self,
        samples: Vec<f32>,
        sample_rate: u32,
        clip_start: Option<f64>,
    ) -> Result<Arc<Rhythm>, RhythmFailure> {
        let mut map = self
            .model
            .map(&samples, sample_rate)
            .map_err(|e| RhythmFailure::Failed(e.to_string()))?;
        map.clip_start = clip_start.map(|s| s as f32);
        Ok(Arc::new(Rhythm { map }))
    }
}

/// A song's rhythm map (see `cw_core::rhythm`).
#[derive(uniffi::Object)]
pub struct Rhythm {
    map: cw_core::RhythmMap,
}

#[uniffi::export]
impl Rhythm {
    /// A map saved with [`Rhythm::to_json`]; fails for other versions.
    #[uniffi::constructor]
    pub fn from_json(json: String) -> Result<Arc<Rhythm>, RhythmFailure> {
        let map: cw_core::RhythmMap =
            serde_json::from_str(&json).map_err(|e| RhythmFailure::Failed(e.to_string()))?;
        if !map.is_valid() {
            return Err(RhythmFailure::Failed("outdated rhythm map".into()));
        }
        Ok(Arc::new(Rhythm { map }))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.map).unwrap_or_default()
    }

    pub fn bpm(&self) -> f32 {
        self.map.bpm
    }

    pub fn beats_per_bar(&self) -> u8 {
        self.map.beats_per_bar
    }

    /// How regular the clip's beats were (0..1).
    pub fn confidence(&self) -> f32 {
        self.map.confidence
    }

    /// A downbeat time in the song, for [`Rhythm::levels`].
    pub fn default_downbeat(&self) -> f64 {
        self.map.downbeat()
    }

    /// Whether the bar phase is known (the clip's place in the song was).
    pub fn is_aligned(&self) -> bool {
        self.map.is_aligned()
    }

    /// The song's groove at `position_secs`, with a downbeat at
    /// `downbeat_secs` into the track.
    pub fn levels(&self, position_secs: f64, playing: bool, downbeat_secs: f64) -> AudioLevels {
        self.map.frame(position_secs, playing, downbeat_secs).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small PNG with a bright square on a dark ground.
    fn cover_png() -> Vec<u8> {
        let img = image::RgbImage::from_fn(64, 64, |x, y| {
            if (20..44).contains(&x) && (20..44).contains(&y) {
                image::Rgb([230, 120, 40])
            } else {
                image::Rgb([20, 24, 40])
            }
        });
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn same_cover_same_frames() {
        let engine = CoverEngine::new();
        assert!(!engine.has_model());
        let png = cover_png();
        let a = engine
            .analyse(png.clone(), Some("House".into()), None)
            .unwrap();
        let b = engine.analyse(png, Some("House".into()), None).unwrap();
        assert_eq!(a.recipe_json(), b.recipe_json());
        assert_eq!(a.genre_family(), "electronic");
        assert!(!a.is_complete());
        let bg = CoverColor { r: 0, g: 0, b: 0 };
        let anim_a = Animator::new(a, 60, 30, 0.6, ArtMode::Palette, bg);
        let anim_b = Animator::new(b, 60, 30, 0.6, ArtMode::Palette, bg);
        for f in 0..40 {
            let lv = beat_levels(124.0, Some("House".into()), f as f64 / 30.0, true);
            let fa = anim_a.frame(f, lv.clone());
            assert_eq!(fa, anim_b.frame(f, lv));
            assert_eq!(fa.glyphs.chars().count(), 60 * 30);
            assert_eq!(fa.fg.len(), 60 * 30);
        }
        assert_eq!(anim_a.still().cols, 60);
    }

    #[test]
    fn lyrics_draw_over_the_moving_cover() {
        let engine = CoverEngine::new();
        let cover = engine.analyse(cover_png(), None, None).unwrap();
        let bg = CoverColor { r: 0, g: 0, b: 0 };
        let anim = Animator::new(cover, 60, 30, 0.6, ArtMode::Palette, bg);
        let sheet = parse_lyrics("[00:01.00]hello there\n[00:04.00]second".into()).unwrap();
        assert!(sheet.is_synced() && sheet.line_count() == 2);
        assert!(parse_lyrics("  \n".into()).is_none());
        let lv = beat_levels(120.0, None, 2.0, true);
        let plain = anim.frame(5, lv.clone());
        let with = anim.frame_with_lyrics(
            6,
            lv.clone(),
            sheet.clone(),
            2.0,
            Some(30.0),
            LyricStyleKind::Text,
            LyricScrollKind::Up,
            LyricSizeKind::Medium,
            LyricPaintKind::Accent,
            true,
        );
        assert_ne!(plain.glyphs, with.glyphs);
        // Motion off: exactly the words (the effects, when on, act on them).
        let still = anim.frame_with_lyrics(
            7,
            lv.clone(),
            sheet.clone(),
            2.0,
            None,
            LyricStyleKind::Text,
            LyricScrollKind::Up,
            LyricSizeKind::Medium,
            LyricPaintKind::Accent,
            false,
        );
        assert!(still.glyphs.contains("hello there"));
        let window = anim.frame_with_lyrics(
            8,
            lv,
            sheet,
            2.0,
            None,
            LyricStyleKind::BigOnly,
            LyricScrollKind::Down,
            LyricSizeKind::Large,
            LyricPaintKind::Cover,
            false,
        );
        assert_eq!(window.cols, 60);
    }

    #[test]
    fn bad_cover_is_an_error() {
        let engine = CoverEngine::new();
        assert!(matches!(
            engine.analyse(vec![1, 2, 3], None, None),
            Err(CoreError::Cover(_))
        ));
    }

    #[test]
    fn rhythm_round_trips_with_its_clip_start() {
        let n = 16;
        let mut accents = vec![0.0; n];
        accents[0] = 1.0;
        accents[8] = 0.8;
        let map = cw_core::RhythmMap {
            version: cw_core::rhythm::RHYTHM_VERSION,
            bpm: 120.0,
            beats_per_bar: 4,
            first_downbeat: 0.4,
            clip_start: Some(48.0),
            hits: vec![[0.5; BAND_COUNT]; n],
            levels: vec![[0.2; BAND_COUNT]; n],
            accents,
            confidence: 0.9,
        };
        let r = Arc::new(Rhythm { map });
        let back = Rhythm::from_json(r.to_json()).unwrap();
        assert_eq!(back.bpm(), 120.0);
        assert!((back.default_downbeat() - 48.4).abs() < 1e-5 && back.is_aligned());
        assert!(back.levels(0.41, true, 0.4).onset > 0.5);
        assert!(Rhythm::from_json("{}".into()).is_err());
    }

    #[test]
    fn beat_levels_genre_and_idle() {
        let paused = beat_levels(0.0, None, 3.0, false);
        assert_eq!(paused, AudioLevels::default());
        let lv = beat_levels(0.0, Some("ambient".into()), 1.0, true);
        assert_eq!(lv.bands.len(), BAND_COUNT);
        assert_eq!(genre_family("Death Metal".into()), "metal");
        let idle = idle_frame(40, 20, 0, 7, CoverColor { r: 0, g: 0, b: 0 });
        assert_eq!(idle.glyphs.chars().count(), 800);
    }
}
