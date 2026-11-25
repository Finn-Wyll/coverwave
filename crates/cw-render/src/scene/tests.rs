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

/// A beat with no bass (so no thump) and moderate level (so no glitch).
fn with_onset(onset: f32) -> AudioFrame {
    AudioFrame {
        bands: [0.0; 6],
        level: 0.3,
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
            genre: String::new(),
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
        Layer::PulseRings {
            intensity: 0.8,
            rings: 2,
            cycles: 1,
            width: 1.0,
        },
        Layer::SubjectGlow {
            intensity: 0.8,
            width: 2,
            cycles: 1,
        },
        Layer::EqBars {
            intensity: 0.8,
            bars: 16,
            height: 0.3,
            cycles: 2,
        },
        Layer::Scanlines {
            intensity: 0.8,
            cycles: 2,
            thickness: 0.05,
        },
        Layer::RowGlitch {
            intensity: 0.8,
            events: 4,
            max_shift: 3,
        },
        Layer::ChromaShift {
            intensity: 0.8,
            cycles: 1,
            offset: 1,
        },
        Layer::FallingLeaves {
            intensity: 0.8,
            density: 0.3,
            fall_cycles: 1,
            drift_deg: 100.0,
        },
        Layer::Bubbles {
            intensity: 0.8,
            density: 0.3,
            rise_cycles: 1,
        },
        Layer::Aurora {
            intensity: 0.8,
            bands: 2,
            cycles: 1,
        },
        Layer::Smoke {
            intensity: 0.8,
            density: 0.3,
            rise_cycles: 1,
        },
        Layer::SunRays {
            intensity: 0.8,
            rays: 5,
            cycles: 1,
        },
        Layer::Twinkle {
            intensity: 0.8,
            density: 0.3,
            cycles: 2,
        },
        Layer::Orbit {
            intensity: 0.8,
            count: 8,
            cycles: 1,
            radius: 0.6,
        },
        Layer::LightSweep {
            intensity: 0.8,
            cycles: 1,
            width: 0.12,
        },
        Layer::Starfield {
            intensity: 0.8,
            count: 60,
            cycles: 1,
        },
        Layer::DigitalRain {
            intensity: 0.8,
            density: 0.5,
            fall_cycles: 2,
        },
        Layer::Lightning {
            intensity: 0.8,
            chance: 1.0,
        },
    ]
}

/// Layers that only draw on a beat, so they are still in silence.
fn event_only(kind: &str) -> bool {
    kind == "lightning"
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
fn every_kind_alone_stays_within_budget() {
    for layer in every_kind() {
        let kind = layer.kind();
        let s = scene_for(&recipe(vec![layer], 0.3, 21), W, H);
        for audio in [quiet(), loud()] {
            let worst = (0..s.loop_frames())
                .map(|n| {
                    frame(&s, n, &audio)
                        .grid
                        .diff_count(&frame(&s, n + 1, &audio).grid)
                })
                .max()
                .unwrap();
            assert!(
                worst <= s.max_changes(),
                "{kind}: {worst} > {}",
                s.max_changes()
            );
        }
    }
}

#[test]
fn every_kind_is_covered() {
    let kinds: Vec<&str> = every_kind().iter().map(Layer::kind).collect();
    for k in cw_core::recipe::LAYER_KINDS {
        assert!(kinds.contains(k), "{k} missing from every_kind()");
    }
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
        if event_only(kind) {
            assert!(steps.iter().all(|&c| c == 0), "{kind} moved in silence");
            continue;
        }
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
        let l = kinds.swap_remove(i);
        // Randomise through the JSON form, so every kind (now and future)
        // is covered without listing them here.
        let mut v = serde_json::to_value(&l).unwrap();
        let o = v.as_object_mut().unwrap();
        o.insert("intensity".into(), (0.1 + 0.9 * u()).into());
        for key in ["density", "amplitude"] {
            if o.contains_key(key) {
                let val = if key == "density" {
                    0.05 + 0.6 * u()
                } else {
                    0.3 + 1.8 * u()
                };
                o.insert(key.into(), val.into());
            }
        }
        let l: Layer = serde_json::from_value(v).unwrap();
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
    // No layers: inside the cover only the beat ring and thump can change
    // anything, and this audio has no bass, so no thump. Compare against
    // the same audio without the beat, inside the cover only (the margin
    // glow also swells on beats).
    let s = scene_for(&recipe(vec![], 0.3, 4), W, H);
    let base = frame(&s, 0, &with_onset(0.0)).grid;
    // Mean distance of brightened cells from the subject centre grows as the
    // onset decays from the hit.
    let spread = |onset: f32| {
        let g = frame(&s, 0, &with_onset(onset)).grid;
        let (mut sum, mut n) = (0.0, 0);
        for y in 0..H {
            for x in 0..W {
                if inside_art(&s, x, y) && g.get(x, y) != base.get(x, y) {
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

/// Cells (x, y) where two grids differ.
fn diff_cells(a: &CellGrid, b: &CellGrid) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for y in 0..a.height() {
        for x in 0..a.width() {
            if a.get(x, y) != b.get(x, y) {
                out.push((x, y));
            }
        }
    }
    out
}

fn inside_art(s: &Scene, x: u16, y: u16) -> bool {
    let a = s.art;
    x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
}

#[test]
fn fixture_has_margins_for_the_stage_effects() {
    let s = scene_for(&recipe(vec![], 0.5, 1), W, H);
    assert!(s.art.x >= 12, "art at {:?}", s.art);
}

#[test]
fn glow_lives_in_the_margins_and_grows_with_level() {
    let s = scene_for(&recipe(vec![], 0.5, 1), W, H);
    let base = frame(&s, 0, &quiet()).grid;
    let lit = |level: f32| {
        let a = AudioFrame {
            level,
            ..AudioFrame::default()
        };
        let g = frame(&s, 0, &a).grid;
        let cells = diff_cells(&base, &g);
        for &(x, y) in &cells {
            assert!(
                !inside_art(&s, x, y),
                "glow drew inside the art at ({x},{y})"
            );
        }
        cells.len()
    };
    let (soft, loud) = (lit(0.2), lit(1.0));
    assert!(soft > 0, "some glow even when quiet");
    assert!(loud > soft * 2, "louder glows further: {soft} -> {loud}");
}

#[test]
fn equaliser_follows_the_bands_and_is_silent_in_silence() {
    let s = scene_for(&recipe(vec![], 0.5, 1), W, H);
    let base = frame(&s, 0, &quiet()).grid;
    let bars = |bands: [f32; 6]| {
        let a = AudioFrame {
            bands,
            ..AudioFrame::default()
        };
        let g = frame(&s, 0, &a).grid;
        // Equaliser glyphs sit on the bottom row of the margins.
        (0..W)
            .filter(|&x| !inside_art(&s, x, H - 1))
            .filter(|&x| matches!(g.get(x, H - 1).unwrap().ch, '#' | '=' | '-' | '.'))
            .filter(|&x| g.get(x, H - 1) != base.get(x, H - 1))
            .count()
    };
    assert_eq!(bars([0.0; 6]), 0);
    assert_eq!(
        bars([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        2,
        "one band, both sides"
    );
    assert_eq!(bars([1.0; 6]), 12);
}

#[test]
fn thump_pushes_the_background_on_kicks_only() {
    let s = scene_for(&recipe(vec![], 0.8, 1), W, H);
    let base = frame(&s, 0, &quiet()).grid;
    let bass_no_beat = AudioFrame {
        bands: [1.0; 6],
        ..AudioFrame::default()
    };
    let in_art = |g: &CellGrid| {
        diff_cells(&base, g)
            .into_iter()
            .filter(|&(x, y)| inside_art(&s, x, y))
            .count()
    };
    assert_eq!(in_art(&frame(&s, 0, &bass_no_beat).grid), 0);
    let kick = AudioFrame {
        bands: [1.0; 6],
        onset: 1.0,
        onset_count: 1,
        ..AudioFrame::default()
    };
    assert!(in_art(&frame(&s, 0, &kick).grid) > 50);
}

#[test]
fn glitch_hits_some_strong_beats_and_is_deterministic() {
    let s = scene_for(&recipe(vec![], 1.0, 9), W, H);
    let hit = |count: u64| AudioFrame {
        bands: [0.0; 6],
        level: 0.9,
        onset: 1.0,
        onset_count: count,
    };
    // Below the level gate nothing glitches; compare the cover only, since
    // the margin glow depends on level too.
    let in_art = |g: &CellGrid| -> Vec<Cell> {
        (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .filter(|&(x, y)| inside_art(&s, x, y))
            .map(|(x, y)| *g.get(x, y).unwrap())
            .collect()
    };
    let calm_audio = AudioFrame {
        level: 0.44,
        ..hit(0)
    };
    let calm = in_art(&frame(&s, 0, &calm_audio).grid);
    let glitched: Vec<u64> = (0..40)
        .filter(|&c| in_art(&frame(&s, 0, &hit(c)).grid) != calm)
        .collect();
    assert!(!glitched.is_empty() && glitched.len() < 40, "{glitched:?}");
    assert_eq!(frame(&s, 0, &hit(7)), frame(&s, 0, &hit(7)));
}

#[test]
fn lightning_strikes_some_beats_from_the_top() {
    let r = recipe(
        vec![Layer::Lightning {
            intensity: 0.9,
            chance: 0.5,
        }],
        0.0, // no glitch
        21,
    );
    let s = scene_for(&r, W, H);
    let without = scene_for(&recipe(vec![], 0.0, 21), W, H);
    let beat = |count: u64| AudioFrame {
        onset: 0.9,
        onset_count: count,
        ..AudioFrame::default()
    };
    let mut struck = 0;
    for c in 0..30 {
        let with = frame(&s, 0, &beat(c)).grid;
        let bolt = diff_cells(&frame(&without, 0, &beat(c)).grid, &with);
        if bolt.is_empty() {
            continue;
        }
        struck += 1;
        let top = bolt.iter().map(|&(_, y)| y).min().unwrap();
        assert_eq!(top, s.art.y, "bolts start at the top of the cover");
        assert!(bolt.iter().all(|&(x, y)| !s.is_protected(x, y)));
    }
    assert!((5..25).contains(&struck), "{struck} of 30 beats struck");
}

/// A lyrics-view scene: the cover's square cleared but for one line of
/// letters, prepared as `lyrics::lyric_scene` does.
fn lyric_scene_for(layers: Vec<Layer>) -> (Scene, Scene) {
    let r = recipe(layers, 0.5, 7);
    let (img, pal) = cover();
    let params = ArtParams {
        cell_aspect: 0.5,
        style: ArtStyle::PaletteLed,
        background: BG,
        ramp: RAMP,
    };
    let cover_base = base_art(&img, &pal, 120, 50, params);
    let art = art_rect(img.width, img.height, 120, 50, 0.5).unwrap();
    let mut words = crate::lyrics::blank_square(&cover_base, art, BG);
    let y = art.y + art.h / 2;
    for x in art.x + 2..art.x + art.w - 2 {
        words.set(
            x,
            y,
            Cell {
                ch: '#',
                fg: Rgb(230, 230, 230),
                bg: None,
            },
        );
    }
    let cover = prepare(&r, cover_base, art, RAMP);
    (crate::lyrics::lyric_scene(&r, words, art, RAMP), cover)
}

#[test]
fn light_sweep_over_lyrics_never_pauses_and_loops() {
    let sweep = Layer::LightSweep {
        intensity: 0.9,
        cycles: 1,
        width: 0.12,
    };
    let (lyr, _) = lyric_scene_for(vec![sweep]);
    let (plain, _) = lyric_scene_for(Vec::new());
    let audio = loud_no_beat();
    let lit = |f: u32| {
        let a = render(
            &lyr,
            Input {
                frame: f,
                audio: &audio,
                fade: 1.0,
            },
        )
        .grid;
        let b = render(
            &plain,
            Input {
                frame: f,
                audio: &audio,
                fade: 1.0,
            },
        )
        .grid;
        a.diff_count(&b)
    };
    // Every moment of the loop has light on the words: no gap between passes.
    for f in (0..240).step_by(6) {
        assert!(lit(f) > 0, "no light on the lyrics at frame {f}");
    }
    let at = |f| {
        render(
            &lyr,
            Input {
                frame: f,
                audio: &audio,
                fade: 1.0,
            },
        )
        .grid
    };
    assert_eq!(at(0), at(240), "seamless loop");
}

#[test]
fn sun_rays_light_the_blank_lyric_square_but_not_the_cover() {
    let rays = Layer::SunRays {
        intensity: 0.9,
        rays: 5,
        cycles: 1,
    };
    let (lyr, cover) = lyric_scene_for(vec![rays.clone()]);
    let audio = loud_no_beat();
    let art = lyr.art;
    let dust = |s: &Scene, base_blank: &dyn Fn(u16, u16) -> bool| {
        let g = render(
            s,
            Input {
                frame: 40,
                audio: &audio,
                fade: 1.0,
            },
        )
        .grid;
        (art.y..art.y + art.h)
            .flat_map(|y| (art.x..art.x + art.w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                base_blank(x, y) && matches!(g.get(x, y).map(|c| c.ch), Some('.' | ':'))
            })
            .count()
    };
    let lyr_blank = |x, y| lyr.base.get(x, y).is_some_and(|c| c.ch == ' ');
    assert!(
        dust(&lyr, &lyr_blank) > 20,
        "rays show across the empty square"
    );
    // Over the cover itself, blank cells stay blank as before.
    let cover_blank = |x, y| cover.base.get(x, y).is_some_and(|c| c.ch == ' ');
    assert_eq!(dust(&cover, &cover_blank), 0);
    let at = |f| {
        render(
            &lyr,
            Input {
                frame: f,
                audio: &audio,
                fade: 1.0,
            },
        )
        .grid
    };
    assert_eq!(at(0), at(240), "seamless loop");
}
