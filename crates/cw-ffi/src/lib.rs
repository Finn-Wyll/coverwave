//! coverwave's shared core for native apps, exported with UniFFI.
//!
//! The iOS app (`ios/`) links this as a static library, and Swift bindings
//! are generated from it (`ios/build-core.sh`). It wraps the same analysis,
//! recipe mapping, renderer and beat clock the terminal app uses, so a cover
//! moves identically on both.
//!
//! No I/O happens here. Swift fetches covers, the CLIP model and Deezer
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
        Arc::new(Animator {
            state: Mutex::new(AnimState {
                scene,
                base,
                prev: None,
                first_frame: None,
            }),
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
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let start = *s.first_frame.get_or_insert(frame);
        let fade = (frame.wrapping_sub(start) as f32 / FADE_FRAMES as f32).min(1.0);
        let audio = AudioFrame::from(&audio);
        let out = cw_render::render(
            &s.scene,
            cw_render::Input {
                frame,
                audio: &audio,
                fade,
            },
        );
        let out = match s.prev.take() {
            Some(prev) => cw_render::limit_changes(&s.scene, &prev, out),
            None => out,
        };
        let result = CellFrame::from(&out.grid);
        s.prev = Some(out);
        result
    }

    /// Frames in one seamless loop of this cover's motion.
    pub fn loop_frames(&self) -> u32 {
        self.loop_frames
    }
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

/// A tempo estimate (see `cw_audio::estimate_tempo`).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Tempo {
    pub bpm: f32,
    pub confidence: f32,
}

/// Minimum confidence worth trusting; below it use the genre's tempo.
#[uniffi::export]
pub fn min_tempo_confidence() -> f32 {
    0.3
}

/// Estimates the tempo of a mono clip (e.g. a decoded 30-second preview).
#[uniffi::export]
pub fn estimate_tempo(samples: Vec<f32>, sample_rate: u32) -> Option<Tempo> {
    cw_audio::estimate_tempo(&samples, sample_rate).map(|t| Tempo {
        bpm: t.bpm,
        confidence: t.confidence,
    })
}

/// Live analysis of microphone audio: push mono samples as they arrive,
/// read levels once per frame.
#[derive(uniffi::Object)]
pub struct LiveAnalyser {
    inner: Mutex<cw_audio::Analyser>,
}

#[uniffi::export]
impl LiveAnalyser {
    #[uniffi::constructor]
    pub fn new(sample_rate: u32) -> Arc<LiveAnalyser> {
        Arc::new(LiveAnalyser {
            inner: Mutex::new(cw_audio::Analyser::new(sample_rate.max(8000))),
        })
    }

    pub fn push(&self, samples: Vec<f32>) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(&samples);
    }

    pub fn levels(&self) -> AudioLevels {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frame()
            .into()
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
    fn bad_cover_is_an_error() {
        let engine = CoverEngine::new();
        assert!(matches!(
            engine.analyse(vec![1, 2, 3], None, None),
            Err(CoreError::Cover(_))
        ));
    }

    #[test]
    fn beat_levels_and_live_analyser() {
        let paused = beat_levels(0.0, None, 3.0, false);
        assert_eq!(paused, AudioLevels::default());
        let lv = beat_levels(0.0, Some("ambient".into()), 1.0, true);
        assert_eq!(lv.bands.len(), BAND_COUNT);
        let live = LiveAnalyser::new(48_000);
        let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        live.push(tone);
        assert_eq!(live.levels().bands.len(), BAND_COUNT);
        assert_eq!(genre_family("Death Metal".into()), "metal");
        let idle = idle_frame(40, 20, 0, 7, CoverColor { r: 0, g: 0, b: 0 });
        assert_eq!(idle.glyphs.chars().count(), 800);
    }
}
