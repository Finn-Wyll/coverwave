//! For every layer kind, what it does to lyrics. Lyrics are drawn into the
//! cover's square (`lyric_base`) and the recipe runs over them
//! (`lyric_scene`); each frame is compared with the same frame without the
//! layer, so the numbers are the layer's own work: the share of letter
//! cells it changes, and of the whole square. A kind at 0 does nothing.
//!
//! `cargo run -p cw-render --release --example lyric_effects [-- --time]`

use cw_analysis::mapping::make_layer;
use cw_core::recipe::{
    AnalysisInfo, AudioMapping, Charset, Features, Global, LAYER_KINDS, Layer, Mask, Recipe,
};
use cw_core::{ArtStyle, AudioFrame, Palette, Rgb, RgbImage};
use cw_render::lyrics::{LyricColors, LyricView, lyric_base, lyric_scene};
use cw_render::{ArtParams, Input, RAMP, art_rect, base_art, prepare, render};

const BG: Rgb = Rgb(14, 15, 18);

fn recipe(layers: Vec<Layer>, global: Global) -> Recipe {
    let bits: Vec<bool> = (0..64 * 64)
        .map(|i| (i % 64) > 24 && (i % 64) < 40 && i / 64 > 20)
        .collect();
    Recipe {
        version: 3,
        cover_hash: String::new(),
        seed: 99,
        analysis: AnalysisInfo {
            model: "check".into(),
            prompts_version: 1,
            pca: "none".into(),
            genre: String::new(),
        },
        scene_weights: Default::default(),
        style_dna: [0.0; 8],
        features: Features {
            brightness: 0.4,
            contrast: 0.4,
            saturation: 0.5,
            temperature_k: 6500.0,
            edge_density: 0.2,
            energy: 0.6,
        },
        palette: vec!["#1e3c78".into(), "#c86432".into(), "#f0dcc8".into()],
        accent: "#f0a050".into(),
        global,
        layers,
        audio_mapping: AudioMapping::default(),
        subject_mask: Mask::from_bits(64, 64, &bits),
    }
}

fn main() {
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
    let params = ArtParams {
        cell_aspect: 0.5,
        style: ArtStyle::PaletteLed,
        background: BG,
        ramp: RAMP,
    };
    let global = Global {
        loop_frames: 240,
        speed: 1.0,
        wind_deg: 100.0,
        density: 0.6,
        flicker_rate: 0.3,
        sway: 1.0,
        charset: Charset::Standard,
    };

    if std::env::args().any(|a| a == "--time") {
        // Rebuilding the scene each frame (lyrics move): its cost at 240x70.
        let (w, h) = (240u16, 70u16);
        let base = base_art(&img, &palette, w, h, params);
        let art = art_rect(n, n, w, h, 0.5).expect("art");
        for (label, layers) in [
            ("one kind", vec![make_layer("ripple", 1.0, &global, 99)]),
            (
                "every kind",
                LAYER_KINDS
                    .iter()
                    .map(|k| make_layer(k, 1.0, &global, 99))
                    .collect(),
            ),
        ] {
            let r = recipe(layers, global.clone());
            let t = std::time::Instant::now();
            for _ in 0..20 {
                std::hint::black_box(prepare(&r, base.clone(), art, RAMP));
            }
            println!("prepare, {label}, 240x70: {:?}", t.elapsed() / 20);
        }
        return;
    }

    let (w, h) = (120u16, 50u16);
    let cover_base = base_art(&img, &palette, w, h, params);
    let art = art_rect(n, n, w, h, 0.5).expect("art");
    let words = cw_core::lyrics::parse_lrc(
        "[00:00.00]Sung right now in the square\n[00:20.00]and the next line waits below",
        "check",
    )
    .expect("lyrics");
    let colors = LyricColors {
        accent: Rgb(240, 160, 80),
        text: Rgb(220, 220, 230),
        background: BG,
    };
    let view = LyricView::default();
    let none = recipe(Vec::new(), global.clone());

    println!(
        "{:<16} {:>10} {:>10}   reaches the words",
        "kind", "letters%", "square%"
    );
    for kind in LAYER_KINDS {
        let r = recipe(vec![make_layer(kind, 1.0, &global, 99)], global.clone());
        let (mut letters_hit, mut letters, mut square_hit, mut cells) = (0u64, 0u64, 0u64, 0u64);
        // One full loop, a beat every half second.
        for f in 0..240u32 {
            let hit = f % 15 == 0;
            let audio = AudioFrame {
                bands: [0.7, 0.8, 0.6, 0.6, 0.5, 0.5],
                level: 0.7,
                onset: if hit { 1.0 } else { 0.2 },
                onset_count: (f / 15) as u64,
            };
            let base = lyric_base(
                &cover_base,
                art,
                0.5,
                &words,
                1.0,
                None,
                view,
                colors,
                &audio,
            );
            let input = Input {
                frame: f,
                audio: &audio,
                fade: 1.0,
            };
            let with = render(&lyric_scene(&r, base.clone(), art, RAMP), input).grid;
            let input = Input {
                frame: f,
                audio: &audio,
                fade: 1.0,
            };
            let without = render(&lyric_scene(&none, base.clone(), art, RAMP), input).grid;
            for y in art.y..art.y + art.h {
                for x in art.x..art.x + art.w {
                    let changed = with.get(x, y) != without.get(x, y);
                    cells += 1;
                    square_hit += changed as u64;
                    if base.get(x, y).is_some_and(|c| c.ch != ' ') {
                        letters += 1;
                        letters_hit += changed as u64;
                    }
                }
            }
        }
        let lp = 100.0 * letters_hit as f64 / letters.max(1) as f64;
        let sp = 100.0 * square_hit as f64 / cells as f64;
        let verdict = if lp > 0.05 {
            "yes"
        } else if sp > 0.05 {
            "around them"
        } else {
            "NO"
        };
        println!("{kind:<16} {lp:>10.2} {sp:>10.2}   {verdict}");
    }
}
