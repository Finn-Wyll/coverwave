//! Analysis -> motion recipe (PLAN.md section 5). Every tunable lives in this
//! file. All choices are a pure function of the inputs plus ChaCha8 seeded
//! from the cover seed, so a cover always gets the same recipe.
//!
//! Selection (v2): every scene offers a *pool* of layer kinds, each genre
//! family favours some kinds, and the cover seed adds a small stable jitter.
//! Each kind is scored
//!
//! `0.75 * sum(scene weight * scene affinity) + GENRE_WEIGHT * genre affinity + jitter`
//!
//! and the best three are kept, with at most one layer that shifts the art
//! (displacement) and at most two that tint large areas (fields), so the
//! motion stays restrained.

use crate::features::round3;
use crate::genre::GenreFamily;
use cw_core::recipe::{
    AnalysisInfo, AudioMapping, Charset, DNA_DIMS, Features, Global, Layer, Mask, RECIPE_VERSION,
    Recipe,
};
use cw_core::{Palette, Rgb};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use std::collections::BTreeMap;

/// Scene keys, in the order used to break ties.
pub const SCENES: [&str; 13] = [
    "snow",
    "lava",
    "very dark scene",
    "forest",
    "ocean",
    "city at night",
    "desert",
    "sky",
    "space",
    "storm",
    "garden",
    "party",
    "underwater",
];

/// Catch-all scenes for covers with no landscape (portraits, text, abstract
/// designs). They share one gentle pool.
pub const NEUTRAL_SCENES: [&str; 3] = ["portrait", "typography", "abstract"];

pub const LOOP_FRAMES: u32 = 240;
pub const MAX_LAYERS: usize = 3;
/// Kinds scoring below this are not used.
pub const MIN_SCORE: f32 = 0.08;
/// How much a known genre counts against the cover's scenes.
pub const GENRE_WEIGHT: f32 = 0.45;
/// Scene evidence is scaled by this before adding genre and jitter.
const SCENE_WEIGHT: f32 = 0.75;
/// Seeded variety added to every kind's score (0..JITTER).
const JITTER: f32 = 0.12;
const AMBIENT_INTENSITY: f32 = 0.5;

/// Each scene's pool: (kind, affinity 0..1).
fn pool(scene: &str) -> &'static [(&'static str, f32)] {
    match scene {
        "snow" => &[
            ("falling_particles", 1.0),
            ("twinkle", 0.4),
            ("aurora", 0.3),
        ],
        "lava" => &[
            ("embers", 1.0),
            ("heat_cycle", 0.9),
            ("smoke", 0.5),
            ("lightning", 0.3),
        ],
        "very dark scene" => &[
            ("fog", 0.7),
            ("flicker", 0.6),
            ("twinkle", 0.4),
            ("starfield", 0.5),
            ("lightning", 0.3),
            ("smoke", 0.4),
            ("light_sweep", 0.3),
        ],
        "forest" => &[
            ("fireflies", 0.8),
            ("falling_leaves", 0.8),
            ("row_sway", 0.7),
            ("sun_rays", 0.5),
        ],
        "ocean" => &[
            ("ripple", 1.0),
            ("bubbles", 0.5),
            ("light_sweep", 0.3),
            ("lightning", 0.3),
        ],
        "city at night" => &[
            ("neon_flicker", 0.8),
            ("rain", 0.7),
            ("digital_rain", 0.6),
            ("scanlines", 0.4),
            ("light_sweep", 0.3),
        ],
        "desert" => &[("heat_shimmer", 0.8), ("dust", 0.8), ("sun_rays", 0.6)],
        "sky" => &[
            ("cloud_drift", 0.9),
            ("sun_rays", 0.5),
            ("aurora", 0.4),
            ("starfield", 0.3),
        ],
        "space" => &[
            ("starfield", 1.0),
            ("twinkle", 0.8),
            ("orbit", 0.6),
            ("aurora", 0.4),
        ],
        "storm" => &[("rain", 1.0), ("lightning", 0.9), ("fog", 0.5)],
        "garden" => &[
            ("falling_leaves", 0.9),
            ("fireflies", 0.5),
            ("sun_rays", 0.5),
            ("bubbles", 0.2),
        ],
        "party" => &[
            ("pulse_rings", 0.9),
            ("neon_flicker", 0.7),
            ("eq_bars", 0.7),
            ("scanlines", 0.5),
            ("chroma_shift", 0.4),
        ],
        "underwater" => &[("bubbles", 1.0), ("ripple", 0.7), ("light_sweep", 0.3)],
        // portrait, typography, abstract
        _ => &[
            ("ambient", 0.6),
            ("subject_glow", 0.6),
            ("light_sweep", 0.6),
            ("pulse_rings", 0.4),
            ("twinkle", 0.15),
        ],
    }
}

/// Layers that shift the art sideways; at most one per recipe.
const DISPLACEMENT: &[&str] = &["ripple", "row_sway", "heat_shimmer", "row_glitch"];
/// Layers that tint large areas; at most two per recipe.
const FIELDS: &[&str] = &[
    "fog",
    "heat_cycle",
    "aurora",
    "sun_rays",
    "light_sweep",
    "scanlines",
    "ambient",
    "cloud_drift",
    "chroma_shift",
    "subject_glow",
    "pulse_rings",
];

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

/// Inputs to [`build`]: everything measured about one cover and track.
pub struct Measured<'a> {
    pub hash: &'a [u8; 32],
    pub seed: u64,
    pub palette: &'a Palette,
    pub features: Features,
    pub mask: Mask,
    pub scene_weights: BTreeMap<String, f32>,
    pub style_dna: [f32; DNA_DIMS],
    pub analysis: AnalysisInfo,
    pub genre: GenreFamily,
}

pub fn build(m: Measured<'_>) -> Recipe {
    let dna = m.style_dna;
    let energy = (m.features.energy + m.genre.energy_bias()).clamp(0.0, 1.0);
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

    let chosen = choose(&m.scene_weights, m.genre, m.seed);
    let mut layers: Vec<Layer> = chosen
        .iter()
        .map(|&(kind, intensity)| make_layer(kind, intensity, &global, m.seed))
        .collect();
    if layers.is_empty() {
        layers.push(make_layer("ambient", AMBIENT_INTENSITY, &global, m.seed));
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

/// Scores every kind and keeps the best (kind, intensity) pairs, strongest
/// first, under the restraint rules.
pub fn choose(
    scene_weights: &BTreeMap<String, f32>,
    genre: GenreFamily,
    seed: u64,
) -> Vec<(&'static str, f32)> {
    let mut scores: BTreeMap<&'static str, f32> = BTreeMap::new();
    for (scene, w) in scene_weights {
        for &(kind, aff) in pool(scene) {
            *scores.entry(kind).or_default() += SCENE_WEIGHT * w * aff;
        }
    }
    for &(kind, aff) in genre.affinities() {
        *scores.entry(kind).or_default() += GENRE_WEIGHT * aff;
    }
    // Stable per-cover variety: a jitter per kind from the seed.
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x51ED_2701_A5A5_0F0F);
    for kind in cw_core::recipe::LAYER_KINDS {
        let j = JITTER * unit(&mut rng);
        if let Some(s) = scores.get_mut(kind) {
            *s += j;
        }
    }
    let mut ranked: Vec<(&'static str, f32)> = scores
        .into_iter()
        .filter(|(_, s)| *s >= MIN_SCORE)
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    let ranked_all = ranked.clone();

    let top = ranked.first().map_or(1.0, |r| r.1).max(1e-6);
    let (mut displacement, mut fields) = (0, 0);
    let mut out = Vec::new();
    for (kind, score) in ranked {
        if out.len() == MAX_LAYERS {
            break;
        }
        let is_disp = DISPLACEMENT.contains(&kind);
        let is_field = FIELDS.contains(&kind);
        if (is_disp && displacement >= 1) || (is_field && fields >= 2) {
            continue;
        }
        displacement += is_disp as usize;
        fields += is_field as usize;
        // Strongest layer at 0.85, others relative to it.
        out.push((kind, round3((0.85 * score / top).clamp(0.25, 0.85))));
    }
    guarantee_genre(&mut out, &ranked_all, genre);
    out
}

/// A known genre always shows: if none of its three favourite kinds made it,
/// the best-scoring favourite that fits replaces a pick. To make room it
/// replaces the weakest pick of the same restraint class (a field for a
/// field, the displacement for a displacement), else the weakest pick.
fn guarantee_genre(
    out: &mut Vec<(&'static str, f32)>,
    ranked: &[(&'static str, f32)],
    genre: GenreFamily,
) {
    let favourites: Vec<&str> = genre.affinities().iter().take(3).map(|(k, _)| *k).collect();
    if favourites.is_empty() || out.iter().any(|(k, _)| favourites.contains(k)) {
        return;
    }
    let top = ranked.first().map_or(1.0, |r| r.1).max(1e-6);
    for &(kind, score) in ranked.iter().filter(|(k, _)| favourites.contains(k)) {
        let class = |k: &str| (DISPLACEMENT.contains(&k), FIELDS.contains(&k));
        let (disp, field) = class(kind);
        let mut trial = out.clone();
        let count = |t: &[(&str, f32)], disp: bool| {
            t.iter()
                .filter(|(k, _)| if disp { class(k).0 } else { class(k).1 })
                .count()
        };
        // Remove the weakest pick that frees the needed slot.
        let victim = if disp && count(&trial, true) >= 1 {
            trial.iter().rposition(|(k, _)| class(k).0)
        } else if field && count(&trial, false) >= 2 {
            trial.iter().rposition(|(k, _)| class(k).1)
        } else if trial.len() == MAX_LAYERS {
            Some(trial.len() - 1)
        } else {
            None
        };
        if let Some(v) = victim {
            trial.remove(v);
        }
        trial.push((kind, round3((0.85 * score / top).clamp(0.35, 0.85))));
        let ok = trial.len() <= MAX_LAYERS && count(&trial, true) <= 1 && count(&trial, false) <= 2;
        if ok {
            *out = trial;
            return;
        }
    }
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// Integer loop repeats for a motion whose base rate is `base` per loop.
fn cycles(base: f32, speed: f32) -> u32 {
    ((base * speed).round() as u32).max(1)
}

fn unit(rng: &mut ChaCha8Rng) -> f32 {
    (rng.next_u64() >> 40) as f32 / (1u64 << 24) as f32
}

/// Uniform in `lo..hi`.
fn pick(rng: &mut ChaCha8Rng, lo: f32, hi: f32) -> f32 {
    round3(lo + (hi - lo) * unit(rng))
}

/// One layer of `kind` with parameters from the globals and the seed. Each
/// kind draws from its own RNG stream, so its look does not depend on which
/// other layers were chosen.
pub fn make_layer(kind: &str, w: f32, g: &Global, seed: u64) -> Layer {
    let salt = kind.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01B3)
    });
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ salt);
    let s = g.speed;
    let rng = &mut rng;
    match kind {
        "falling_particles" => Layer::FallingParticles {
            intensity: w,
            density: g.density,
            fall_cycles: cycles(2.0, s),
            drift_deg: g.wind_deg,
            glyphs: ".*+".into(),
        },
        "heat_cycle" => Layer::HeatCycle {
            intensity: w,
            noise_scale: pick(rng, 0.05, 0.2),
            cycles: cycles(1.0, s),
        },
        "embers" => Layer::Embers {
            intensity: w,
            density: round3(g.density * 0.5),
            rise_cycles: cycles(2.0, s),
        },
        "flicker" => Layer::Flicker {
            intensity: w,
            rate: g.flicker_rate,
            cells: pick(rng, 0.02, 0.06),
        },
        "fog" => Layer::Fog {
            intensity: w,
            density: round3(0.3 + 0.3 * g.density),
            drift_deg: round3(g.wind_deg - 90.0),
            cycles: cycles(1.0, s),
        },
        "row_sway" => Layer::RowSway {
            intensity: w,
            amplitude: g.sway,
            wavelength: pick(rng, 6.0, 14.0),
            cycles: cycles(1.0, s),
        },
        "fireflies" => Layer::Fireflies {
            intensity: w,
            count: 4 + (g.density * 20.0).round() as u32,
            blink_cycles: cycles(3.0, s),
        },
        "ripple" => Layer::Ripple {
            intensity: w,
            amplitude: round3(g.sway * 1.5),
            wavelength: pick(rng, 6.0, 14.0),
            cycles: cycles(2.0, s),
            direction_deg: round3(g.wind_deg - 90.0),
        },
        "neon_flicker" => Layer::NeonFlicker {
            intensity: w,
            cells: pick(rng, 0.01, 0.04),
            rate: g.flicker_rate,
        },
        "rain" => Layer::Rain {
            intensity: w,
            density: g.density,
            angle_deg: g.wind_deg,
            fall_cycles: cycles(4.0, s),
        },
        "heat_shimmer" => Layer::HeatShimmer {
            intensity: w,
            amplitude: round3(g.sway * 0.6),
            cycles: cycles(2.0, s),
        },
        "dust" => Layer::Dust {
            intensity: w,
            density: round3(g.density * 0.4),
            drift_deg: round3(g.wind_deg - 90.0),
            cycles: cycles(1.0, s),
        },
        "cloud_drift" => Layer::CloudDrift {
            intensity: w,
            layers: 1 + (g.density * 4.0).round().min(2.0) as u32,
            cycles: cycles(1.0, s),
            softness: pick(rng, 0.4, 0.8),
        },
        "pulse_rings" => Layer::PulseRings {
            intensity: w,
            rings: 1 + (unit(rng) * 2.0) as u32,
            cycles: 1,
            width: pick(rng, 0.8, 1.4),
        },
        "subject_glow" => Layer::SubjectGlow {
            intensity: w,
            width: 1 + (unit(rng) * 2.0) as u32,
            cycles: cycles(1.0, s),
        },
        "eq_bars" => Layer::EqBars {
            intensity: w,
            bars: 8 + (unit(rng) * 16.0) as u32,
            height: pick(rng, 0.15, 0.3),
            cycles: cycles(2.0, s),
        },
        "scanlines" => Layer::Scanlines {
            intensity: w,
            cycles: cycles(1.5, s),
            thickness: pick(rng, 0.03, 0.07),
        },
        "row_glitch" => Layer::RowGlitch {
            intensity: w,
            events: 2 + (g.flicker_rate * 8.0).round() as u32,
            max_shift: 2 + (unit(rng) * 3.0) as u32,
        },
        "digital_rain" => Layer::DigitalRain {
            intensity: w,
            density: round3(0.25 + 0.5 * g.density),
            fall_cycles: cycles(2.0, s),
        },
        "chroma_shift" => Layer::ChromaShift {
            intensity: w,
            cycles: cycles(1.0, s),
            offset: 1 + (unit(rng) * 2.0) as u32,
        },
        "falling_leaves" => Layer::FallingLeaves {
            intensity: w,
            density: round3(g.density * 0.6),
            fall_cycles: cycles(1.0, s),
            drift_deg: g.wind_deg,
        },
        "bubbles" => Layer::Bubbles {
            intensity: w,
            density: round3(g.density * 0.6),
            rise_cycles: cycles(1.5, s),
        },
        "lightning" => Layer::Lightning {
            intensity: w,
            chance: pick(rng, 0.2, 0.45),
        },
        "aurora" => Layer::Aurora {
            intensity: w,
            bands: 1 + (g.density * 2.0).round() as u32,
            cycles: cycles(1.0, s),
        },
        "smoke" => Layer::Smoke {
            intensity: w,
            density: round3(g.density * 0.6),
            rise_cycles: cycles(1.0, s),
        },
        "sun_rays" => Layer::SunRays {
            intensity: w,
            rays: 3 + (unit(rng) * 4.0) as u32,
            cycles: 1,
        },
        "starfield" => Layer::Starfield {
            intensity: w,
            count: 30 + (g.density * 90.0).round() as u32,
            cycles: cycles(1.0, s),
        },
        "twinkle" => Layer::Twinkle {
            intensity: w,
            density: round3(g.density * 0.5),
            cycles: cycles(2.0, s),
        },
        "orbit" => Layer::Orbit {
            intensity: w,
            count: 4 + (g.density * 12.0).round() as u32,
            cycles: cycles(1.0, s),
            radius: pick(rng, 0.45, 0.8),
        },
        "light_sweep" => Layer::LightSweep {
            intensity: w,
            cycles: 1,
            width: pick(rng, 0.08, 0.16),
        },
        _ => Layer::Ambient {
            intensity: w,
            cycles: cycles(1.0, s),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(
        weights: &[(&str, f32)],
        dna: [f32; DNA_DIMS],
        genre: GenreFamily,
        seed: u64,
    ) -> Measured<'static> {
        static HASH: [u8; 32] = [7; 32];
        static PALETTE: std::sync::OnceLock<Palette> = std::sync::OnceLock::new();
        let palette = PALETTE.get_or_init(|| Palette {
            colors: vec![Rgb(10, 20, 30), Rgb(200, 100, 50)],
            weights: vec![0.7, 0.3],
            accent: Rgb(220, 120, 60),
        });
        Measured {
            hash: &HASH,
            seed,
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
                genre: genre.name().into(),
            },
            genre,
        }
    }

    fn kinds(r: &Recipe) -> Vec<&'static str> {
        r.layers.iter().map(Layer::kind).collect()
    }

    #[test]
    fn make_layer_knows_every_kind() {
        let g = build(measured(&[], [0.0; DNA_DIMS], GenreFamily::Unknown, 1)).global;
        for k in cw_core::recipe::LAYER_KINDS {
            assert_eq!(make_layer(k, 0.5, &g, 3).kind(), *k);
        }
    }

    #[test]
    fn strong_scene_leads_and_restraint_holds() {
        let r = build(measured(
            &[("ocean", 0.9), ("party", 0.1)],
            [0.0; DNA_DIMS],
            GenreFamily::Unknown,
            5,
        ));
        assert_eq!(r.layers[0].kind(), "ripple");
        assert!(r.layers.len() <= MAX_LAYERS);
        let k = kinds(&r);
        assert!(
            k.iter().filter(|k| DISPLACEMENT.contains(k)).count() <= 1,
            "{k:?}"
        );
        assert!(
            k.iter().filter(|k| FIELDS.contains(k)).count() <= 2,
            "{k:?}"
        );
        assert_eq!(r.layers[0].intensity(), 0.85);
    }

    #[test]
    fn genre_changes_the_mix_for_the_same_cover() {
        let w = [("very dark scene", 0.8), ("abstract", 0.2)];
        let metal = kinds(&build(measured(&w, [0.0; DNA_DIMS], GenreFamily::Metal, 9)));
        let electronic = kinds(&build(measured(
            &w,
            [0.0; DNA_DIMS],
            GenreFamily::Electronic,
            9,
        )));
        assert_ne!(metal, electronic);
        assert!(
            metal.iter().any(|k| ["embers", "lightning"].contains(k)),
            "{metal:?}"
        );
        assert!(
            electronic
                .iter()
                .any(|k| ["scanlines", "pulse_rings", "row_glitch"].contains(k)),
            "{electronic:?}"
        );
    }

    #[test]
    fn same_scene_different_covers_vary() {
        // The jitter is per cover: across many seeds the dark-scene recipe
        // must not always be the same three layers.
        let w = [("very dark scene", 1.0)];
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..40 {
            seen.insert(kinds(&build(measured(
                &w,
                [0.0; DNA_DIMS],
                GenreFamily::Unknown,
                seed,
            ))));
        }
        assert!(
            seen.len() >= 4,
            "only {} distinct mixes: {seen:?}",
            seen.len()
        );
    }

    #[test]
    fn nothing_known_falls_back_to_ambient() {
        let r = build(measured(&[], [0.0; DNA_DIMS], GenreFamily::Unknown, 1));
        assert_eq!(kinds(&r), ["ambient"]);
    }

    #[test]
    fn genre_alone_drives_motion_without_a_model() {
        let r = build(measured(&[], [0.0; DNA_DIMS], GenreFamily::Ambient, 1));
        assert!(kinds(&r).contains(&"aurora"), "{:?}", kinds(&r));
    }

    #[test]
    fn dna_drives_globals_within_ranges() {
        let lo = build(measured(&[], [-1.0; DNA_DIMS], GenreFamily::Unknown, 1)).global;
        let hi = build(measured(&[], [1.0; DNA_DIMS], GenreFamily::Unknown, 1)).global;
        assert_eq!((lo.speed, hi.speed), (0.46, 1.472));
        assert_eq!((lo.wind_deg, hi.wind_deg), (50.0, 130.0));
        assert_eq!((lo.charset, hi.charset), (Charset::Sparse, Charset::Dense));
        // Metal adds energy: faster than the same cover untagged.
        let metal = build(measured(&[], [0.0; DNA_DIMS], GenreFamily::Metal, 1)).global;
        let plain = build(measured(&[], [0.0; DNA_DIMS], GenreFamily::Unknown, 1)).global;
        assert!(metal.speed > plain.speed);
    }

    #[test]
    fn deterministic_json_and_loopable_cycles() {
        let w = [("lava", 0.6), ("forest", 0.4)];
        let a = serde_json::to_string(&build(measured(&w, [0.3; DNA_DIMS], GenreFamily::Rock, 4)))
            .unwrap();
        let b = serde_json::to_string(&build(measured(&w, [0.3; DNA_DIMS], GenreFamily::Rock, 4)))
            .unwrap();
        assert_eq!(a, b);
        let r: Recipe = serde_json::from_str(&a).unwrap();
        assert_eq!(r.analysis.genre, "rock");
        for l in &r.layers {
            let json = serde_json::to_value(l).unwrap();
            for (k, v) in json.as_object().unwrap() {
                if k.ends_with("cycles") {
                    assert!(v.as_u64().unwrap() >= 1, "{k} in {json}");
                }
            }
        }
    }
}
