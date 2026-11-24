//! `render()` at 240x70 with the three heaviest layers (fog, heat cycle,
//! ripple) plus particles. PLAN.md budget: under 4ms per frame.
//!
//! cargo bench -p cw-render

use criterion::{Criterion, criterion_group, criterion_main};
use cw_core::recipe::{AnalysisInfo, AudioMapping, Charset, Features, Global, Layer, Mask, Recipe};
use cw_core::{ArtStyle, AudioFrame, Palette, Rgb, RgbImage};
use cw_render::{ArtParams, Input, RAMP, art_rect, base_art, limit_changes, prepare, render};
use std::hint::black_box;

fn scene(layers: Vec<Layer>) -> cw_render::Scene {
    let n = 256u32;
    let pixels = (0..n * n)
        .map(|i| Rgb((i % n) as u8, (i / n) as u8, ((i * 7) % 255) as u8))
        .collect();
    let img = RgbImage::new(n, n, pixels);
    let palette = Palette {
        colors: vec![Rgb(30, 60, 120), Rgb(200, 100, 50), Rgb(240, 220, 200)],
        weights: vec![0.5, 0.3, 0.2],
        accent: Rgb(240, 160, 80),
    };
    let (w, h) = (240, 70);
    let params = ArtParams {
        cell_aspect: 0.5,
        style: ArtStyle::PaletteLed,
        background: Rgb(14, 15, 18),
        ramp: RAMP,
    };
    let base = base_art(&img, &palette, w, h, params);
    let art = art_rect(n, n, w, h, 0.5).unwrap();
    let bits: Vec<bool> = (0..64 * 64)
        .map(|i| (i % 64) > 24 && (i % 64) < 40 && i / 64 > 20)
        .collect();
    let recipe = Recipe {
        version: 1,
        cover_hash: String::new(),
        seed: 99,
        analysis: AnalysisInfo {
            model: "bench".into(),
            prompts_version: 1,
            pca: "none".into(),
            genre: String::new(),
        },
        scene_weights: Default::default(),
        style_dna: [0.0; 8],
        features: Features {
            brightness: 0.5,
            contrast: 0.3,
            saturation: 0.5,
            temperature_k: 6500.0,
            edge_density: 0.2,
            energy: 0.5,
        },
        palette: vec!["#1e3c78".into(), "#c86432".into(), "#f0dcc8".into()],
        accent: "#f0a050".into(),
        global: Global {
            loop_frames: 240,
            speed: 1.0,
            wind_deg: 100.0,
            density: 0.6,
            flicker_rate: 0.3,
            sway: 1.0,
            charset: Charset::Standard,
        },
        layers,
        audio_mapping: AudioMapping::default(),
        subject_mask: Mask::from_bits(64, 64, &bits),
    };
    prepare(&recipe, base, art, RAMP)
}

fn classic_layers() -> Vec<Layer> {
    vec![
        Layer::Fog {
            intensity: 0.9,
            density: 0.8,
            drift_deg: 0.0,
            cycles: 1,
        },
        Layer::HeatCycle {
            intensity: 0.8,
            noise_scale: 0.15,
            cycles: 1,
        },
        Layer::Ripple {
            intensity: 0.7,
            amplitude: 2.0,
            wavelength: 9.0,
            cycles: 2,
            direction_deg: 0.0,
        },
        Layer::FallingParticles {
            intensity: 0.6,
            density: 0.6,
            fall_cycles: 2,
            drift_deg: 100.0,
            glyphs: ".*+".into(),
        },
    ]
}

/// The recipe-version-2 layers, the heaviest of which are the starfield and
/// digital rain.
fn new_layers() -> Vec<Layer> {
    vec![
        Layer::Aurora {
            intensity: 0.9,
            bands: 3,
            cycles: 1,
        },
        Layer::Starfield {
            intensity: 0.8,
            count: 120,
            cycles: 1,
        },
        Layer::DigitalRain {
            intensity: 0.8,
            density: 0.75,
            fall_cycles: 2,
        },
        Layer::Lightning {
            intensity: 0.8,
            chance: 1.0,
        },
    ]
}

/// The heaviest per-cell-geometry kinds from the genre-aware set.
fn geometry_layers() -> Vec<Layer> {
    vec![
        Layer::PulseRings {
            intensity: 0.9,
            rings: 3,
            cycles: 1,
            width: 1.2,
        },
        Layer::SunRays {
            intensity: 0.8,
            rays: 7,
            cycles: 1,
        },
        Layer::Aurora {
            intensity: 0.7,
            bands: 3,
            cycles: 1,
        },
    ]
}

fn bench(c: &mut Criterion) {
    let s = scene(classic_layers());
    let audio = AudioFrame {
        bands: [0.7; 6],
        level: 0.7,
        onset: 0.5,
        onset_count: 3,
    };
    let mut n = 0u32;
    c.bench_function("render 240x70, 4 layers", |b| {
        b.iter(|| {
            n = n.wrapping_add(1);
            black_box(render(
                &s,
                Input {
                    frame: n,
                    audio: &audio,
                    fade: 1.0,
                },
            ))
        })
    });
    let heavy = scene(geometry_layers());
    c.bench_function("render 240x70, rings + rays + aurora", |b| {
        b.iter(|| {
            n = n.wrapping_add(1);
            black_box(render(
                &heavy,
                Input {
                    frame: n,
                    audio: &audio,
                    fade: 1.0,
                },
            ))
        })
    });
    let prev = render(
        &s,
        Input {
            frame: 0,
            audio: &AudioFrame::default(),
            fade: 1.0,
        },
    );
    c.bench_function("render + limit 240x70", |b| {
        b.iter(|| {
            let next = render(
                &s,
                Input {
                    frame: 1,
                    audio: &audio,
                    fade: 1.0,
                },
            );
            black_box(limit_changes(&s, &prev, next))
        })
    });
    // Worst case: every new layer plus a full beat (thump, shockwave,
    // glitch and lightning all firing) with glow and equaliser.
    let s2 = scene(new_layers());
    let hit = AudioFrame {
        bands: [0.9; 6],
        level: 0.9,
        onset: 1.0,
        onset_count: 5,
    };
    c.bench_function("render 240x70, new layers on a beat", |b| {
        b.iter(|| {
            n = n.wrapping_add(1);
            black_box(render(
                &s2,
                Input {
                    frame: n,
                    audio: &hit,
                    fade: 1.0,
                },
            ))
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
