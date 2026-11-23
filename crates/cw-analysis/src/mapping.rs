//! Analysis -> motion recipe (PLAN.md section 5). Every tunable lives in this
//! file. All choices are a pure function of the inputs plus ChaCha8 seeded
//! from the cover seed, so a cover always gets the same recipe.

use crate::features::round3;
use cw_core::recipe::{
    AnalysisInfo, AudioMapping, Charset, DNA_DIMS, Features, Global, Layer, Mask, RECIPE_VERSION,
    Recipe,
};
use cw_core::{Palette, Rgb};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use std::collections::BTreeMap;

/// Scene keys, in the order used to break ties.
pub const SCENES: [&str; 8] = [
    "snow",
    "lava",
    "very dark scene",
    "forest",
    "ocean",
    "city at night",
    "desert",
    "sky",
];

/// Catch-all scenes for covers with no landscape (portraits, text, abstract
/// designs). Their combined weight becomes one gentle `ambient` layer.
pub const NEUTRAL_SCENES: [&str; 3] = ["portrait", "typography", "abstract"];

pub const LOOP_FRAMES: u32 = 240;
/// Layers weaker than this are dropped.
pub const MIN_INTENSITY: f32 = 0.10;
pub const MAX_LAYERS: usize = 3;
const AMBIENT_INTENSITY: f32 = 0.5;

/// A linear map from a DNA value in -1..1 onto `lo..hi`.
#[derive(Debug, Clone, Copy)]
struct Range {
    lo: f32,
    hi: f32,
}

impl Range {
    fn at(self, dna: f32) -> f32 {
        let t = ((dna.clamp(-1.0, 1.0) + 1.0) / 2.0).clamp(0.0, 1.0);
        self.lo + (self.hi - self.lo) * t
    }
}

// Style DNA -> global parameters (PLAN.md section 5). dna[6] and dna[7] are
// reserved.
const SPEED: Range = Range { lo: 0.5, hi: 1.6 }; // dna[0]
const WIND_TILT: Range = Range {
    lo: -40.0,
    hi: 40.0,
}; // dna[1], degrees off straight down
const DENSITY: Range = Range { lo: 0.15, hi: 0.6 }; // dna[2]
const FLICKER: Range = Range { lo: 0.05, hi: 0.5 }; // dna[3]
const SWAY: Range = Range { lo: 0.3, hi: 1.2 }; // dna[4]
const CHARSET_SPLIT: f32 = 0.33; // dna[5]: below -split sparse, above dense

/// Inputs to [`build`]: everything measured about one cover.
pub struct Measured<'a> {
    pub hash: &'a [u8; 32],
    pub seed: u64,
    pub palette: &'a Palette,
    pub features: Features,
    pub mask: Mask,
    pub scene_weights: BTreeMap<String, f32>,
    pub style_dna: [f32; DNA_DIMS],
    pub analysis: AnalysisInfo,
}

pub fn build(m: Measured<'_>) -> Recipe {
    let dna = m.style_dna;
    let energy = m.features.energy.clamp(0.0, 1.0);
    let global = Global {
        loop_frames: LOOP_FRAMES,
        speed: round3(SPEED.at(dna[0]) * (0.8 + 0.4 * energy)),
        wind_deg: round3(90.0 + WIND_TILT.at(dna[1])),
        density: round3((DENSITY.at(dna[2]) * (0.7 + 0.6 * energy)).min(1.0)),
        flicker_rate: round3(FLICKER.at(dna[3])),
        sway: round3(SWAY.at(dna[4])),
        charset: if dna[5] < -CHARSET_SPLIT {
            Charset::Sparse
        } else if dna[5] > CHARSET_SPLIT {
            Charset::Dense
        } else {
            Charset::Standard
        },
    };

    // Strongest scenes first; ties keep the SCENES order.
    let mut scenes: Vec<(usize, f32)> = SCENES
        .iter()
        .enumerate()
        .filter_map(|(i, s)| m.scene_weights.get(*s).map(|w| (i, *w)))
        .filter(|(_, w)| *w >= MIN_INTENSITY)
        .collect();
    scenes.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut layers: Vec<Layer> = scenes
        .iter()
        .flat_map(|&(i, w)| scene_layers(i, round3(w), &global, m.seed))
        .collect();
    let neutral: f32 = NEUTRAL_SCENES
        .iter()
        .filter_map(|s| m.scene_weights.get(*s))
        .sum();
    if neutral >= MIN_INTENSITY {
        layers.push(Layer::Ambient {
            intensity: round3(neutral),
            cycles: cycles(1.0, global.speed),
        });
    }
    // Stable sort keeps a scene's own layers in table order.
    layers.sort_by(|a, b| b.intensity().total_cmp(&a.intensity()));
    layers.truncate(MAX_LAYERS);
    if layers.is_empty() {
        let cycles = cycles(1.0, global.speed);
        layers.push(Layer::Ambient {
            intensity: AMBIENT_INTENSITY,
            cycles,
        });
    }

    Recipe {
        version: RECIPE_VERSION,
        cover_hash: m.hash.iter().map(|b| format!("{b:02x}")).collect(),
        seed: m.seed,
        analysis: m.analysis,
        scene_weights: m.scene_weights,
        style_dna: dna.map(round3),
        features: m.features,
        palette: m.palette.colors.iter().map(|&c| hex(c)).collect(),
        accent: hex(m.palette.accent),
        global,
        layers,
        audio_mapping: AudioMapping::default(),
        subject_mask: m.mask,
    }
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// Integer loop repeats for a motion whose base rate is `base` per loop.
fn cycles(base: f32, speed: f32) -> u32 {
    ((base * speed).round() as u32).max(1)
}

/// Uniform in `lo..hi`.
fn pick(rng: &mut ChaCha8Rng, lo: f32, hi: f32) -> f32 {
    let u = (rng.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
    round3(lo + (hi - lo) * u)
}

/// The layers for scene `index` (see the table in PLAN.md section 5). Each
/// scene draws from its own RNG stream, so its parameters do not depend on
/// which other scenes are present.
fn scene_layers(index: usize, w: f32, g: &Global, seed: u64) -> Vec<Layer> {
    let mut rng =
        ChaCha8Rng::seed_from_u64(seed ^ (0x9E37_79B9_7F4A_7C15u64.wrapping_mul(index as u64 + 1)));
    let s = g.speed;
    match SCENES[index] {
        "snow" => vec![Layer::FallingParticles {
            intensity: w,
            density: g.density,
            fall_cycles: cycles(2.0, s),
            drift_deg: g.wind_deg,
            glyphs: ".*+".into(),
        }],
        "lava" => vec![
            Layer::HeatCycle {
                intensity: w,
                noise_scale: pick(&mut rng, 0.05, 0.2),
                cycles: cycles(1.0, s),
            },
            Layer::Embers {
                intensity: w,
                density: round3(g.density * 0.5),
                rise_cycles: cycles(2.0, s),
            },
        ],
        "very dark scene" => vec![
            Layer::Flicker {
                intensity: w,
                rate: g.flicker_rate,
                cells: pick(&mut rng, 0.02, 0.06),
            },
            Layer::Fog {
                intensity: w,
                density: round3(0.3 + 0.3 * g.density),
                drift_deg: round3(g.wind_deg - 90.0),
                cycles: cycles(1.0, s),
            },
        ],
        "forest" => vec![
            Layer::RowSway {
                intensity: w,
                amplitude: g.sway,
                wavelength: pick(&mut rng, 6.0, 14.0),
                cycles: cycles(1.0, s),
            },
            Layer::Fireflies {
                intensity: w,
                count: 4 + (g.density * 20.0).round() as u32,
                blink_cycles: cycles(3.0, s),
            },
        ],
        "ocean" => vec![Layer::Ripple {
            intensity: w,
            amplitude: round3(g.sway * 1.5),
            wavelength: pick(&mut rng, 6.0, 14.0),
            cycles: cycles(2.0, s),
            direction_deg: round3(g.wind_deg - 90.0),
        }],
        "city at night" => vec![
            Layer::NeonFlicker {
                intensity: w,
                cells: pick(&mut rng, 0.01, 0.04),
                rate: g.flicker_rate,
            },
            Layer::Rain {
                intensity: w,
                density: g.density,
                angle_deg: g.wind_deg,
                fall_cycles: cycles(4.0, s),
            },
        ],
        "desert" => vec![
            Layer::HeatShimmer {
                intensity: w,
                amplitude: round3(g.sway * 0.6),
                cycles: cycles(2.0, s),
            },
            Layer::Dust {
                intensity: w,
                density: round3(g.density * 0.4),
                drift_deg: round3(g.wind_deg - 90.0),
                cycles: cycles(1.0, s),
            },
        ],
        "sky" => vec![Layer::CloudDrift {
            intensity: w,
            layers: 1 + (g.density * 4.0).round().min(2.0) as u32,
            cycles: cycles(1.0, s),
            softness: pick(&mut rng, 0.4, 0.8),
        }],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(weights: &[(&str, f32)], dna: [f32; DNA_DIMS]) -> Measured<'static> {
        static HASH: [u8; 32] = [7; 32];
        static PALETTE: std::sync::OnceLock<Palette> = std::sync::OnceLock::new();
        let palette = PALETTE.get_or_init(|| Palette {
            colors: vec![Rgb(10, 20, 30), Rgb(200, 100, 50)],
            weights: vec![0.7, 0.3],
            accent: Rgb(220, 120, 60),
        });
        Measured {
            hash: &HASH,
            seed: 42,
            palette,
            features: Features {
                brightness: 0.4,
                contrast: 0.3,
                saturation: 0.5,
                temperature_k: 6000.0,
                edge_density: 0.1,
                energy: 0.3,
            },
            mask: Mask::from_bits(2, 1, &[true, false]),
            scene_weights: weights.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            style_dna: dna,
            analysis: AnalysisInfo {
                model: "test".into(),
                prompts_version: 1,
                pca: "none".into(),
            },
        }
    }

    #[test]
    fn strongest_scenes_become_at_most_three_layers() {
        let r = build(measured(
            &[
                ("ocean", 0.5),
                ("city at night", 0.3),
                ("sky", 0.15),
                ("snow", 0.05),
            ],
            [0.0; DNA_DIMS],
        ));
        let kinds: Vec<&str> = r.layers.iter().map(Layer::kind).collect();
        assert_eq!(kinds, ["ripple", "neon_flicker", "rain"]);
        assert_eq!(r.layers[0].intensity(), 0.5);
    }

    #[test]
    fn neutral_scenes_pool_into_one_ambient_layer() {
        let r = build(measured(
            &[
                ("typography", 0.35),
                ("portrait", 0.2),
                ("ocean", 0.3),
                ("abstract", 0.05),
            ],
            [0.0; DNA_DIMS],
        ));
        let kinds: Vec<(&str, f32)> = r.layers.iter().map(|l| (l.kind(), l.intensity())).collect();
        assert_eq!(kinds, [("ambient", 0.6), ("ripple", 0.3)]);
    }

    #[test]
    fn no_strong_scene_falls_back_to_ambient() {
        let r = build(measured(&[("ocean", 0.09)], [0.0; DNA_DIMS]));
        assert_eq!(r.layers.len(), 1);
        assert_eq!(r.layers[0].kind(), "ambient");
        let r = build(measured(&[], [0.0; DNA_DIMS]));
        assert_eq!(r.layers[0].kind(), "ambient");
    }

    #[test]
    fn dna_drives_globals_within_ranges() {
        let lo = build(measured(&[], [-1.0; DNA_DIMS])).global;
        let hi = build(measured(&[], [1.0; DNA_DIMS])).global;
        // energy 0.3 scales speed by 0.92.
        assert_eq!((lo.speed, hi.speed), (0.46, 1.472));
        assert_eq!((lo.wind_deg, hi.wind_deg), (50.0, 130.0));
        assert_eq!((lo.charset, hi.charset), (Charset::Sparse, Charset::Dense));
        assert!(lo.density < hi.density && lo.sway < hi.sway);
        let mid = build(measured(&[], [0.0; DNA_DIMS])).global;
        assert_eq!(mid.charset, Charset::Standard);
    }

    #[test]
    fn deterministic_json_and_loopable_cycles() {
        let w = [("lava", 0.6), ("forest", 0.4)];
        let a = serde_json::to_string(&build(measured(&w, [0.3; DNA_DIMS]))).unwrap();
        let b = serde_json::to_string(&build(measured(&w, [0.3; DNA_DIMS]))).unwrap();
        assert_eq!(a, b);
        let r: Recipe = serde_json::from_str(&a).unwrap();
        assert_eq!(r.cover_hash.len(), 64);
        assert_eq!(r.palette, ["#0a141e", "#c86432"]);
        for l in &r.layers {
            let json = serde_json::to_value(l).unwrap();
            for (k, v) in json.as_object().unwrap() {
                if k.ends_with("cycles") {
                    assert!(v.as_u64().unwrap() >= 1, "{k} in {json}");
                }
            }
        }
    }

    #[test]
    fn a_scene_keeps_its_parameters_when_others_change() {
        let alone = build(measured(&[("forest", 0.9)], [0.0; DNA_DIMS]));
        let mixed = build(measured(&[("forest", 0.6), ("sky", 0.4)], [0.0; DNA_DIMS]));
        let wl = |r: &Recipe| match &r.layers[0] {
            Layer::RowSway { wavelength, .. } => *wavelength,
            other => panic!("{other:?}"),
        };
        assert_eq!(wl(&alone), wl(&mixed));
    }
}
