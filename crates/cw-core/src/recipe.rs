//! The motion recipe: everything the renderer needs to animate one cover,
//! as plain data. Produced by `cw-analysis`, cached and user-overridable as
//! JSON (PLAN.md section 5), consumed by `cw-render` (M5).
//!
//! Conventions:
//! - Every motion is phase-based over `global.loop_frames`; `cycles` fields
//!   are integer repeats per loop, so animations loop seamlessly.
//! - Directions are in degrees on screen: 0 = right, 90 = down.
//! - `intensity` (0..1) is the layer's share of the scene; audio scales it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bump when the recipe's meaning changes; cached recipes with another
/// version are rebuilt.
/// 2: aurora, starfield, digital rain and lightning layers (2026-11-24).
/// 3: genre-aware selection, 13 more layers (merged with 2).
pub const RECIPE_VERSION: u32 = 3;

/// Number of style-DNA components.
pub const DNA_DIMS: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub version: u32,
    /// Hex SHA-256 of the cover bytes.
    pub cover_hash: String,
    /// First 8 bytes of the hash: the root of all randomness.
    pub seed: u64,
    pub analysis: AnalysisInfo,
    /// Softmax over the scene prompts (sums to 1, or empty without a model).
    pub scene_weights: BTreeMap<String, f32>,
    /// PCA of the cover's CLIP embedding, squashed to -1..1.
    pub style_dna: [f32; DNA_DIMS],
    pub features: Features,
    /// Palette as `#rrggbb`, largest cluster first.
    pub palette: Vec<String>,
    pub accent: String,
    pub global: Global,
    /// At most three, strongest first.
    pub layers: Vec<Layer>,
    pub audio_mapping: AudioMapping,
    pub subject_mask: Mask,
}

/// What produced the recipe, so caches can tell when it is stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisInfo {
    /// Image model used for scenes and DNA, or "none".
    pub model: String,
    pub prompts_version: u32,
    /// Short hash of the PCA file, or "none".
    pub pca: String,
    /// Genre family that biased the layer choice ("unknown" if none).
    #[serde(default)]
    pub genre: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Features {
    /// Mean L* / 100.
    pub brightness: f32,
    /// RMS contrast: standard deviation of L* / 100.
    pub contrast: f32,
    /// Mean HSV saturation.
    pub saturation: f32,
    /// Correlated colour temperature of the mean colour (McCamy).
    pub temperature_k: f32,
    /// Fraction of pixels on a Sobel edge.
    pub edge_density: f32,
    /// 0.5 * saturation + 0.5 * edge_density.
    pub energy: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Global {
    pub loop_frames: u32,
    /// Overall speed multiplier (0.5..1.6 before energy).
    pub speed: f32,
    /// Direction drifting things move in.
    pub wind_deg: f32,
    /// Particle density, 0..1.
    pub density: f32,
    /// Flicker rate, 0..1.
    pub flicker_rate: f32,
    /// Sway amplitude in cells.
    pub sway: f32,
    pub charset: Charset,
}

/// Density ramp for the base art. All plain ASCII.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Charset {
    /// ` .:`
    Sparse,
    /// ` .:-=+*#%@`
    Standard,
    /// An 18-step symbol ramp for finer tonal detail.
    Dense,
}

impl Charset {
    pub fn ramp(self) -> &'static [char] {
        match self {
            Charset::Sparse => &[' ', '.', ':'],
            Charset::Standard => &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'],
            // Symbols only: letters in a ramp read as text, not tone.
            Charset::Dense => &[
                ' ', '.', '`', ',', ':', ';', '~', '-', '=', '+', '*', '!', '?', '#', '%', '&',
                '$', '@',
            ],
        }
    }
}

/// One effect layer. Parameters are tuned by the mapping in `cw-analysis`;
/// the renderer (M5) defines exactly how each is drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Layer {
    /// snow
    FallingParticles {
        intensity: f32,
        density: f32,
        fall_cycles: u32,
        drift_deg: f32,
        glyphs: String,
    },
    /// lava: slow palette cycling through warm noise.
    HeatCycle {
        intensity: f32,
        noise_scale: f32,
        cycles: u32,
    },
    /// lava: sparks rising.
    Embers {
        intensity: f32,
        density: f32,
        rise_cycles: u32,
    },
    /// very dark scene
    Flicker {
        intensity: f32,
        rate: f32,
        cells: f32,
    },
    /// very dark scene
    Fog {
        intensity: f32,
        density: f32,
        drift_deg: f32,
        cycles: u32,
    },
    /// forest: rows shift sideways.
    RowSway {
        intensity: f32,
        amplitude: f32,
        wavelength: f32,
        cycles: u32,
    },
    /// forest
    Fireflies {
        intensity: f32,
        count: u32,
        blink_cycles: u32,
    },
    /// ocean
    Ripple {
        intensity: f32,
        amplitude: f32,
        wavelength: f32,
        cycles: u32,
        direction_deg: f32,
    },
    /// city at night
    NeonFlicker {
        intensity: f32,
        cells: f32,
        rate: f32,
    },
    /// city at night
    Rain {
        intensity: f32,
        density: f32,
        angle_deg: f32,
        fall_cycles: u32,
    },
    /// desert
    HeatShimmer {
        intensity: f32,
        amplitude: f32,
        cycles: u32,
    },
    /// desert
    Dust {
        intensity: f32,
        density: f32,
        drift_deg: f32,
        cycles: u32,
    },
    /// sky
    CloudDrift {
        intensity: f32,
        layers: u32,
        cycles: u32,
        softness: f32,
    },
    /// Beat & pulse: rings spreading from the subject.
    PulseRings {
        intensity: f32,
        rings: u32,
        cycles: u32,
        width: f32,
    },
    /// Beat & pulse: a soft halo just outside the subject.
    SubjectGlow {
        intensity: f32,
        width: u32,
        cycles: u32,
    },
    /// Beat & pulse: level bars along the bottom edge.
    EqBars {
        intensity: f32,
        bars: u32,
        height: f32,
        cycles: u32,
    },
    /// Electronic: a bright band rolling down the screen.
    Scanlines {
        intensity: f32,
        cycles: u32,
        thickness: f32,
    },
    /// Electronic: brief horizontal slips of a few rows.
    RowGlitch {
        intensity: f32,
        events: u32,
        max_shift: u32,
    },
    /// Electronic: a VHS-style ghost of the art in a moving band.
    ChromaShift {
        intensity: f32,
        cycles: u32,
        offset: u32,
    },
    /// Nature: leaves or petals tumbling down.
    FallingLeaves {
        intensity: f32,
        density: f32,
        fall_cycles: u32,
        drift_deg: f32,
    },
    /// Nature: bubbles rising.
    Bubbles {
        intensity: f32,
        density: f32,
        rise_cycles: u32,
    },
    /// Nature: wisps of smoke drifting up.
    Smoke {
        intensity: f32,
        density: f32,
        rise_cycles: u32,
    },
    /// Nature: slow rays of light from a top corner.
    SunRays {
        intensity: f32,
        rays: u32,
        cycles: u32,
    },
    /// Space: stars twinkling in dark areas.
    Twinkle {
        intensity: f32,
        density: f32,
        cycles: u32,
    },
    /// Space: particles orbiting the subject.
    Orbit {
        intensity: f32,
        count: u32,
        cycles: u32,
        radius: f32,
    },
    /// Space: a highlight sweeping across the art.
    LightSweep {
        intensity: f32,
        cycles: u32,
        width: f32,
    },
    /// Fallback when no scene is strong enough: slow palette breathing.
    Ambient { intensity: f32, cycles: u32 },
    /// sky, snow: shimmering curtains across the upper background.
    Aurora {
        intensity: f32,
        /// Curtains, 1..=3.
        bands: u32,
        cycles: u32,
    },
    /// very dark scene, sky: stars streaming outward from the subject.
    /// Streaks lengthen with loudness.
    Starfield {
        intensity: f32,
        count: u32,
        cycles: u32,
    },
    /// city at night: cascading columns of the cover's own glyphs.
    DigitalRain {
        intensity: f32,
        density: f32,
        fall_cycles: u32,
    },
    /// very dark scene, ocean: a bolt toward the subject on some strong
    /// beats (which ones is decided by the seed and the beat count).
    Lightning {
        intensity: f32,
        /// Share of strong beats that strike, 0..1.
        chance: f32,
    },
}

/// Every layer kind, as written in JSON.
pub const LAYER_KINDS: &[&str] = &[
    "falling_particles",
    "heat_cycle",
    "embers",
    "flicker",
    "fog",
    "row_sway",
    "fireflies",
    "ripple",
    "neon_flicker",
    "rain",
    "heat_shimmer",
    "dust",
    "cloud_drift",
    "pulse_rings",
    "subject_glow",
    "eq_bars",
    "scanlines",
    "row_glitch",
    "digital_rain",
    "chroma_shift",
    "falling_leaves",
    "bubbles",
    "lightning",
    "aurora",
    "smoke",
    "sun_rays",
    "starfield",
    "twinkle",
    "orbit",
    "light_sweep",
    "ambient",
];

impl Layer {
    pub fn intensity(&self) -> f32 {
        match self {
            Layer::FallingParticles { intensity, .. }
            | Layer::HeatCycle { intensity, .. }
            | Layer::Embers { intensity, .. }
            | Layer::Flicker { intensity, .. }
            | Layer::Fog { intensity, .. }
            | Layer::RowSway { intensity, .. }
            | Layer::Fireflies { intensity, .. }
            | Layer::Ripple { intensity, .. }
            | Layer::NeonFlicker { intensity, .. }
            | Layer::Rain { intensity, .. }
            | Layer::HeatShimmer { intensity, .. }
            | Layer::Dust { intensity, .. }
            | Layer::CloudDrift { intensity, .. }
            | Layer::PulseRings { intensity, .. }
            | Layer::SubjectGlow { intensity, .. }
            | Layer::EqBars { intensity, .. }
            | Layer::Scanlines { intensity, .. }
            | Layer::RowGlitch { intensity, .. }
            | Layer::ChromaShift { intensity, .. }
            | Layer::FallingLeaves { intensity, .. }
            | Layer::Bubbles { intensity, .. }
            | Layer::Smoke { intensity, .. }
            | Layer::SunRays { intensity, .. }
            | Layer::Twinkle { intensity, .. }
            | Layer::Orbit { intensity, .. }
            | Layer::LightSweep { intensity, .. }
            | Layer::Ambient { intensity, .. }
            | Layer::Aurora { intensity, .. }
            | Layer::Starfield { intensity, .. }
            | Layer::DigitalRain { intensity, .. }
            | Layer::Lightning { intensity, .. } => *intensity,
        }
    }

    /// The `kind` tag as written in JSON.
    pub fn kind(&self) -> &'static str {
        match self {
            Layer::FallingParticles { .. } => "falling_particles",
            Layer::HeatCycle { .. } => "heat_cycle",
            Layer::Embers { .. } => "embers",
            Layer::Flicker { .. } => "flicker",
            Layer::Fog { .. } => "fog",
            Layer::RowSway { .. } => "row_sway",
            Layer::Fireflies { .. } => "fireflies",
            Layer::Ripple { .. } => "ripple",
            Layer::NeonFlicker { .. } => "neon_flicker",
            Layer::Rain { .. } => "rain",
            Layer::HeatShimmer { .. } => "heat_shimmer",
            Layer::Dust { .. } => "dust",
            Layer::CloudDrift { .. } => "cloud_drift",
            Layer::PulseRings { .. } => "pulse_rings",
            Layer::SubjectGlow { .. } => "subject_glow",
            Layer::EqBars { .. } => "eq_bars",
            Layer::Scanlines { .. } => "scanlines",
            Layer::RowGlitch { .. } => "row_glitch",
            Layer::ChromaShift { .. } => "chroma_shift",
            Layer::FallingLeaves { .. } => "falling_leaves",
            Layer::Bubbles { .. } => "bubbles",
            Layer::Smoke { .. } => "smoke",
            Layer::SunRays { .. } => "sun_rays",
            Layer::Twinkle { .. } => "twinkle",
            Layer::Orbit { .. } => "orbit",
            Layer::LightSweep { .. } => "light_sweep",
            Layer::Ambient { .. } => "ambient",
            Layer::Aurora { .. } => "aurora",
            Layer::Starfield { .. } => "starfield",
            Layer::DigitalRain { .. } => "digital_rain",
            Layer::Lightning { .. } => "lightning",
        }
    }
}

/// Which recipe parameter each audio feature scales (amplitude only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioMapping {
    pub bass: String,
    pub mid: String,
    pub treble: String,
    pub onset: String,
}

impl Default for AudioMapping {
    fn default() -> Self {
        AudioMapping {
            bass: "intensity".into(),
            mid: "density".into(),
            treble: "flicker".into(),
            onset: "pulse".into(),
        }
    }
}

/// The subject mask: cells the motion must never touch. Stored as text rows
/// of `0`/`1` so overrides can be edited by hand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub rows: Vec<String>,
}

impl Mask {
    pub fn from_bits(width: u32, height: u32, bits: &[bool]) -> Mask {
        assert_eq!(bits.len(), width as usize * height as usize);
        let rows = bits
            .chunks(width.max(1) as usize)
            .map(|r| r.iter().map(|&b| if b { '1' } else { '0' }).collect())
            .collect();
        Mask {
            width,
            height,
            rows,
        }
    }

    /// Whether mask pixel (x, y) is subject. Out of range (or malformed
    /// rows) read as background.
    pub fn get(&self, x: u32, y: u32) -> bool {
        self.rows
            .get(y as usize)
            .and_then(|r| r.as_bytes().get(x as usize))
            .is_some_and(|&c| c == b'1')
    }

    /// Fraction of the mask that is subject.
    pub fn coverage(&self) -> f32 {
        let total = (self.width as usize * self.height as usize).max(1);
        let on = self
            .rows
            .iter()
            .map(|r| r.bytes().filter(|&c| c == b'1').count())
            .sum::<usize>();
        on as f32 / total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_json_uses_kind_tag() {
        let l = Layer::Ripple {
            intensity: 0.61,
            amplitude: 1.4,
            wavelength: 9.0,
            cycles: 3,
            direction_deg: 180.0,
        };
        let json = serde_json::to_string(&l).unwrap();
        assert!(
            json.starts_with(r#"{"kind":"ripple","intensity":0.61"#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<Layer>(&json).unwrap(), l);
        assert_eq!(l.kind(), "ripple");
    }

    #[test]
    fn mask_round_trips_bits() {
        let bits = [true, false, false, true, true, true];
        let m = Mask::from_bits(3, 2, &bits);
        assert_eq!(m.rows, vec!["100", "111"]);
        assert!(m.get(0, 0) && !m.get(1, 0) && m.get(2, 1));
        assert!(!m.get(9, 9));
        assert!((m.coverage() - 4.0 / 6.0).abs() < 1e-6);
    }

    #[test]
    fn charsets_are_ascii_and_start_blank() {
        for c in [Charset::Sparse, Charset::Standard, Charset::Dense] {
            let r = c.ramp();
            assert_eq!(r[0], ' ');
            assert!(r.iter().all(|ch| ch.is_ascii()));
        }
    }
}
