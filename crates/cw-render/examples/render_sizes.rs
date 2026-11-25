//! Time per frame at the iPhone's Detail grid sizes (columns x rows), with
//! a busy recipe (six layer kinds): `cargo run -p cw-render --release
//! --example render_sizes`. The renderer's budget is 4 ms at 240x70.

use cw_analysis::mapping::make_layer;
use cw_core::recipe::{AnalysisInfo, AudioMapping, Charset, Features, Global, Mask, Recipe};
use cw_core::{ArtStyle, AudioFrame, Palette, Rgb, RgbImage};
use cw_render::{ArtParams, Input, RAMP, art_rect, base_art, prepare, render};

fn main() {
    let global = Global {
        loop_frames: 240,
        speed: 1.0,
        wind_deg: 100.0,
        density: 0.6,
        flicker_rate: 0.3,
        sway: 1.0,
        charset: Charset::Standard,
    };
    let layers = [
        "light_sweep",
        "ripple",
        "rain",
        "starfield",
        "fog",
        "sun_rays",
    ]
    .iter()
    .map(|k| make_layer(k, 1.0, &global, 7))
    .collect();
    let recipe = Recipe {
        version: 3,
        cover_hash: String::new(),
        seed: 7,
        analysis: AnalysisInfo {
            model: "t".into(),
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
            energy: 0.7,
        },
        palette: vec!["#1e3c78".into(), "#c86432".into(), "#f0dcc8".into()],
        accent: "#f0a050".into(),
        global,
        layers,
        audio_mapping: AudioMapping::default(),
        subject_mask: Mask::from_bits(0, 0, &[]),
    };
    let n = 256u32;
    let img = RgbImage::new(
        n,
        n,
        (0..n * n)
            .map(|i| Rgb((i % n) as u8, (i / n) as u8, ((i * 7) % 255) as u8))
            .collect(),
    );
    let palette = Palette {
        colors: vec![Rgb(30, 60, 120), Rgb(200, 100, 50), Rgb(240, 220, 200)],
        weights: vec![0.5, 0.3, 0.2],
        accent: Rgb(240, 160, 80),
    };
    let audio = AudioFrame {
        bands: [0.7; 6],
        level: 0.7,
        onset: 0.5,
        onset_count: 3,
    };
    for (cols, rows) in [(128u16, 138u16), (160, 173), (192, 207), (224, 242)] {
        let aspect = 0.5;
        let params = ArtParams {
            cell_aspect: aspect,
            style: ArtStyle::PaletteLed,
            background: Rgb(14, 15, 18),
            ramp: RAMP,
        };
        let base = base_art(&img, &palette, cols, rows, params);
        let art = art_rect(n, n, cols, rows, aspect).expect("art");
        let scene = prepare(&recipe, base, art, RAMP);
        let runs = 60;
        let t = std::time::Instant::now();
        for f in 0..runs {
            std::hint::black_box(render(
                &scene,
                Input {
                    frame: f,
                    audio: &audio,
                    fade: 1.0,
                },
            ));
        }
        let per = t.elapsed() / runs;
        println!(
            "{cols:>3} x {rows:>3} ({:>5} cells): {per:?} per frame",
            cols as u32 * rows as u32
        );
    }
}
