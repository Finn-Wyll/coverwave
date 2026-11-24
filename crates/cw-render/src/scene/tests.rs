use super::*;
use crate::art::{ArtParams, RAMP, art_rect, base_art};
use cw_core::recipe::{AnalysisInfo, AudioMapping, Charset, Features, Global, Mask};
use cw_core::{ArtStyle, Palette, RgbImage};

const W: u16 = 96;
const H: u16 = 32;
const BG: Rgb = Rgb(14, 15, 18);

fn quiet() -> AudioFrame {
    AudioFrame::default()
}

fn loud() -> AudioFrame {
    AudioFrame {
        bands: [1.0; 6],
        level: 1.0,
        onset: 1.0,
        onset_count: 9,
    }
}

/// Loud with no beat in progress: the steady budget applies.
fn loud_no_beat() -> AudioFrame {
    AudioFrame {
        onset: 0.0,
        ..loud()
    }
}

fn with_onset(onset: f32) -> AudioFrame {
    AudioFrame {
        bands: [0.5; 6],
        level: 0.5,
        onset,
        onset_count: 1,
    }
}

/// A warm disc (the subject) on a cool gradient sky.
fn cover() -> (RgbImage, Palette) {
    let n = 96u32;
    let pixels = (0..n * n)
        .map(|i| {
            let (x, y) = ((i % n) as f32 / n as f32, (i / n) as f32 / n as f32);
            let d = ((x - 0.5).powi(2) + (y - 0.55).powi(2)).sqrt();
            if d < 0.22 {
                Rgb(230, 150, 60)
            } else {
                // Horizontal texture, like real skies, so row shifts show.
                let t = 0.5 + 0.5 * (x * 37.0).sin() * (y * 11.0).cos();
                Rgb(
                    (20.0 + 60.0 * y * t) as u8,
                    (40.0 + 80.0 * y) as u8,
                    (90.0 + 120.0 * y * t) as u8,
                )
            }
        })
        .collect();
    let palette = Palette {
        colors: vec![Rgb(30, 60, 120), Rgb(80, 120, 200), Rgb(230, 150, 60)],
        weights: vec![0.5, 0.3, 0.2],
        accent: Rgb(240, 160, 80),
    };
    (RgbImage::new(n, n, pixels), palette)
}

fn disc_mask() -> Mask {
    let bits: Vec<bool> = (0..64 * 64)
        .map(|i| {
            let (x, y) = ((i % 64) as f32 / 64.0, (i / 64) as f32 / 64.0);
            ((x - 0.5).powi(2) + (y - 0.55).powi(2)).sqrt() < 0.22
        })
        .collect();
    Mask::from_bits(64, 64, &bits)
}

fn recipe(layers: Vec<Layer>, energy: f32, seed: u64) -> Recipe {
    Recipe {
        version: cw_core::recipe::RECIPE_VERSION,
        cover_hash: "00".repeat(32),
        seed,
        analysis: AnalysisInfo {
            model: "test".into(),
            prompts_version: 1,
            pca: "none".into(),
        },
        scene_weights: Default::default(),
        style_dna: [0.0; 8],
        features: Features {
            brightness: 0.4,
            contrast: 0.3,
            saturation: 0.5,
            temperature_k: 6500.0,
            edge_density: 0.1,
            energy,
        },
        palette: vec!["#1e3c78".into(), "#5078c8".into(), "#e6963c".into()],
        accent: "#f0a050".into(),
        global: Global {
            loop_frames: 240,
            speed: 1.0,
            wind_deg: 100.0,
            density: 0.4,
            flicker_rate: 0.3,
            sway: 0.8,
            charset: Charset::Standard,
        },
        layers,
        audio_mapping: AudioMapping::default(),
        subject_mask: disc_mask(),
    }
}

fn scene_for(r: &Recipe, w: u16, h: u16) -> Scene {
    let (img, pal) = cover();
    let params = ArtParams {
        cell_aspect: 0.5,
        style: ArtStyle::PaletteLed,
        background: BG,
        ramp: RAMP,
    };
    let base = base_art(&img, &pal, w, h, params);
    let art = art_rect(img.width, img.height, w, h, 0.5).unwrap();
    prepare(r, base, art, RAMP)
}

/// One of every layer kind, with mid-range parameters.
fn every_kind() -> Vec<Layer> {
    vec![
        Layer::FallingParticles {
            intensity: 0.8,
            density: 0.4,
            fall_cycles: 2,
            drift_deg: 100.0,
            glyphs: ".*+".into(),
        },
        Layer::HeatCycle {
            intensity: 0.8,
            noise_scale: 0.1,
            cycles: 1,
        },
        Layer::Embers {
            intensity: 0.8,
            density: 0.3,
            rise_cycles: 2,
        },
        Layer::Flicker {
            intensity: 0.8,
            rate: 0.3,
            cells: 0.05,
        },
        Layer::Fog {
            intensity: 0.8,
            density: 0.5,
            drift_deg: 10.0,
            cycles: 1,
        },
        Layer::RowSway {
            intensity: 0.8,
            amplitude: 1.0,
            wavelength: 10.0,
            cycles: 1,
        },
        Layer::Fireflies {
            intensity: 0.8,
            count: 12,
            blink_cycles: 3,
        },
        Layer::Ripple {
            intensity: 0.8,
            amplitude: 1.5,
            wavelength: 9.0,
            cycles: 2,
            direction_deg: 0.0,
        },
        Layer::NeonFlicker {
            intensity: 0.8,
            cells: 0.03,
            rate: 0.4,
        },
        Layer::Rain {
            intensity: 0.8,
            density: 0.4,
            angle_deg: 105.0,
            fall_cycles: 4,
        },
        Layer::HeatShimmer {
            intensity: 0.8,
            amplitude: 0.6,
            cycles: 2,
        },
        Layer::Dust {
            intensity: 0.8,
            density: 0.3,
            drift_deg: 10.0,
            cycles: 1,
        },
        Layer::CloudDrift {
            intensity: 0.8,
            layers: 2,
            cycles: 1,
            softness: 0.6,
        },
        Layer::Ambient {
            intensity: 0.8,
            cycles: 1,
        },
    ]
}

fn frame(s: &Scene, n: u32, audio: &AudioFrame) -> Output {
    render(
        s,
        Input {
            frame: n,
            audio,
            fade: 1.0,
        },
    )
}

#[test]
fn deterministic() {
    for layer in every_kind() {
        let r = recipe(vec![layer.clone()], 0.3, 7);
        let (a, b) = (scene_for(&r, W, H), scene_for(&r, W, H));
        assert_eq!(
            frame(&a, 37, &loud()),
            frame(&b, 37, &loud()),
            "{}",
            layer.kind()
        );
    }
}

#[test]
fn every_kind_moves_and_loops_seamlessly() {
    for layer in every_kind() {
        let kind = layer.kind();
        let s = scene_for(&recipe(vec![layer], 0.3, 11), W, H);
        let l = s.loop_frames();
        let a = quiet();
        assert_eq!(
            frame(&s, 0, &a),
            frame(&s, l, &a),
            "{kind}: frame L != frame 0"
        );
        let steps: Vec<usize> = (0..l)
            .map(|n| frame(&s, n, &a).grid.diff_count(&frame(&s, n + 1, &a).grid))
            .collect();
        assert!(steps.iter().any(|&c| c > 0), "{kind} never moves");
        let seam = steps[l as usize - 1];
        let worst = *steps[..l as usize - 1].iter().max().unwrap();
        assert!(seam <= worst, "{kind}: seam step {seam} > worst {worst}");
    }
}

#[test]
fn subject_cells_never_change() {
    for layer in every_kind() {
        let kind = layer.kind();
        let s = scene_for(&recipe(vec![layer], 0.3, 5), W, H);
        let base = frame(&s, 0, &quiet()).grid; // compare against the base art
        let base = {
            let fade0 = render(
                &s,
                Input {
                    frame: 0,
                    audio: &quiet(),
                    fade: 0.0,
                },
            );
            assert_eq!(
                fade0.owner.iter().filter(|&&o| o != OWNER_BASE).count(),
                0,
                "{kind}: fade 0 draws"
            );
            let _ = base;
            fade0.grid
        };
        let protected: Vec<(u16, u16)> = (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .filter(|&(x, y)| s.is_protected(x, y))
            .collect();
        assert!(protected.len() > 50, "fixture has a subject");
        for n in (0..s.loop_frames()).step_by(3) {
            for audio in [quiet(), loud()] {
                let g = frame(&s, n, &audio).grid;
                for &(x, y) in &protected {
                    assert_eq!(
                        g.get(x, y),
                        base.get(x, y),
                        "{kind} changed subject cell ({x},{y}) at frame {n}"
                    );
                }
            }
        }
    }
}

/// Seeded random recipes: up to three layers of any kind, random strengths.
fn random_recipe(seed: u64) -> Recipe {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut u = || unit(&mut rng);
    let mut kinds = every_kind();
    let n = 1 + (u() * 3.0) as usize;
    let mut layers = Vec::new();
    for _ in 0..n {
        let i = (u() * kinds.len() as f32) as usize % kinds.len();
        let mut l = kinds.swap_remove(i);
        let intensity = 0.1 + 0.9 * u();
        match &mut l {
            Layer::FallingParticles {
                intensity: x,
                density,
                ..
            }
            | Layer::Embers {
                intensity: x,
                density,
                ..
            }
            | Layer::Dust {
                intensity: x,
                density,
                ..
            }
            | Layer::Rain {
                intensity: x,
                density,
                ..
            }
            | Layer::Fog {
                intensity: x,
                density,
                ..
            } => {
                *x = intensity;
                *density = 0.6 * u() + 0.05;
            }
            Layer::Ripple {
                intensity: x,
                amplitude,
                ..
            }
            | Layer::RowSway {
                intensity: x,
                amplitude,
                ..
            } => {
                *x = intensity;
                *amplitude = 0.3 + 1.8 * u();
            }
            Layer::HeatCycle { intensity: x, .. }
            | Layer::Flicker { intensity: x, .. }
            | Layer::Fireflies { intensity: x, .. }
            | Layer::NeonFlicker { intensity: x, .. }
            | Layer::HeatShimmer { intensity: x, .. }
            | Layer::CloudDrift { intensity: x, .. }
            | Layer::Ambient { intensity: x, .. } => *x = intensity,
        }
        layers.push(l);
    }
    layers.sort_by(|a, b| b.intensity().total_cmp(&a.intensity()));
    recipe(layers, u(), seed)
}

#[test]
fn steady_audio_stays_within_budget_without_the_limiter() {
    for seed in 0..12u64 {
        let r = random_recipe(seed);
        let s = scene_for(&r, W, H);
        for audio in [quiet(), loud()] {
            for n in 0..s.loop_frames() {
                let changed = frame(&s, n, &audio)
                    .grid
                    .diff_count(&frame(&s, n + 1, &audio).grid);
                assert!(
                    changed <= s.max_changes(),
                    "seed {seed} {:?} frame {n}: {changed} > {}",
                    s.layer_kinds(),
                    s.max_changes()
                );
            }
        }
    }
}

#[test]
fn limiter_caps_audio_jumps_and_drops_weakest_first() {
    let r = recipe(
        vec![
            Layer::Fog {
                intensity: 0.9,
                density: 0.9,
                drift_deg: 0.0,
                cycles: 1,
            },
            Layer::FallingParticles {
                intensity: 0.4,
                density: 0.9,
                fall_cycles: 2,
                drift_deg: 90.0,
                glyphs: ".*+".into(),
            },
        ],
        0.2,
        3,
    );
    let s = scene_for(&r, W, H);
    // Silence then a full-scale swell (no beat): fog thickens and snow
    // appears at once.
    let prev = frame(&s, 10, &quiet());
    let raw = frame(&s, 11, &loud_no_beat());
    let raw_changes = raw.grid.diff_count(&prev.grid);
    let limited = limit_changes(&s, &prev, raw.clone());
    let kept = limited.grid.diff_count(&prev.grid);
    assert!(kept <= s.max_changes());
    assert!(
        raw_changes > s.max_changes(),
        "fixture should exceed the budget"
    );
    assert_eq!(kept, s.max_changes());
    // Every reverted change ranks weaker than (or equal to) every kept one.
    let priority = |i: usize| raw.owner[i].max(prev.owner[i]);
    let changed = |g: &CellGrid, i: usize| g.cells()[i] != prev.grid.cells()[i];
    let (mut weakest_kept, mut strongest_dropped) = (0u8, u8::MAX);
    for i in 0..raw.owner.len() {
        if changed(&raw.grid, i) {
            if changed(&limited.grid, i) {
                weakest_kept = weakest_kept.max(priority(i));
            } else {
                strongest_dropped = strongest_dropped.min(priority(i));
            }
        }
    }
    assert!(
        weakest_kept <= strongest_dropped,
        "kept {weakest_kept} > dropped {strongest_dropped}"
    );
    // Under budget, the limiter changes nothing.
    let a = frame(&s, 20, &loud_no_beat());
    let b = frame(&s, 21, &loud_no_beat());
    assert!(b.grid.diff_count(&a.grid) <= s.max_changes());
    assert_eq!(limit_changes(&s, &a, b.clone()), b);
}

#[test]
fn high_energy_covers_get_a_bigger_budget() {
    let calm = scene_for(&recipe(vec![], 0.2, 1), W, H);
    let wild = scene_for(&recipe(vec![], 1.0, 1), W, H);
    let cells = W as usize * H as usize;
    assert_eq!(calm.max_changes(), (BUDGET * cells as f32) as usize);
    assert_eq!(wild.max_changes(), (BUDGET_HIGH * cells as f32) as usize);
}

#[test]
fn beats_widen_the_budget_and_decay_back() {
    let s = scene_for(&recipe(vec![], 0.2, 1), W, H);
    let cells = (W as usize * H as usize) as f32;
    assert_eq!(s.max_changes_on_beat(0.0), s.max_changes());
    assert_eq!(
        s.max_changes_on_beat(1.0),
        ((BUDGET + BEAT_BURST).min(BUDGET_MAX) * cells) as usize
    );
    let wild = scene_for(&recipe(vec![], 1.0, 1), W, H);
    assert_eq!(wild.max_changes_on_beat(1.0), (BUDGET_MAX * cells) as usize);
    // The limiter uses the beat budget of the frame it is limiting.
    let r = recipe(
        vec![Layer::Fog {
            intensity: 0.9,
            density: 0.9,
            drift_deg: 0.0,
            cycles: 1,
        }],
        0.2,
        3,
    );
    let s = scene_for(&r, W, H);
    let prev = frame(&s, 10, &quiet());
    let hit = frame(&s, 11, &loud());
    assert_eq!(hit.burst, 1.0);
    let kept = limit_changes(&s, &prev, hit.clone())
        .grid
        .diff_count(&prev.grid);
    assert!(kept <= s.max_changes_on_beat(1.0));
    assert!(
        kept > s.max_changes(),
        "a beat should get past the steady budget"
    );
}

#[test]
fn beat_ring_starts_at_the_subject_and_expands() {
    // No layers at all: only the beat ring can change anything.
    let s = scene_for(&recipe(vec![], 0.3, 4), W, H);
    let base = frame(&s, 0, &quiet()).grid;
    assert_eq!(
        frame(&s, 0, &with_onset(0.0)).grid,
        base,
        "silence draws no ring"
    );
    // Mean distance of brightened cells from the subject centre grows as the
    // onset decays from the hit.
    let spread = |onset: f32| {
        let g = frame(&s, 0, &with_onset(onset)).grid;
        let (mut sum, mut n) = (0.0, 0);
        for y in 0..H {
            for x in 0..W {
                if g.get(x, y) != base.get(x, y) {
                    assert!(!s.is_protected(x, y), "ring touched the subject");
                    sum += ring_dist(s.origin, x as f32 + 0.5, y as f32 + 0.5);
                    n += 1;
                }
            }
        }
        assert!(n > 0, "onset {onset} drew nothing");
        sum / n as f32
    };
    let (early, mid, late) = (spread(0.9), spread(0.6), spread(0.3));
    assert!(early < mid && mid < late, "{early} {mid} {late}");
    // Pure: same input, same ring.
    assert_eq!(
        frame(&s, 5, &with_onset(0.6)),
        frame(&s, 5, &with_onset(0.6))
    );
}

#[test]
fn tiny_grids_do_not_panic() {
    for layer in every_kind() {
        for (w, h) in [(1, 1), (3, 2), (10, 4)] {
            let s = scene_for(&recipe(vec![layer.clone()], 0.5, 2), w, h);
            for n in [0, 1, 119] {
                let _ = frame(&s, n, &loud());
            }
        }
    }
}

fn chars(g: &CellGrid) -> String {
    let mut s = String::new();
    for y in 0..g.height() {
        for x in 0..g.width() {
            s.push(g.get(x, y).unwrap().ch);
        }
        s.push('\n');
    }
    s
}

#[test]
fn golden_frames() {
    let mut out = String::new();
    for layer in every_kind() {
        let kind = layer.kind();
        let s = scene_for(&recipe(vec![layer], 0.3, 42), 60, 20);
        out.push_str(&format!("== {kind} (frame 37, mid audio)\n"));
        let mid = AudioFrame {
            bands: [0.5; 6],
            level: 0.5,
            onset: 0.0,
            onset_count: 0,
        };
        out.push_str(&chars(&frame(&s, 37, &mid).grid));
    }
    insta::assert_snapshot!(out);
}
