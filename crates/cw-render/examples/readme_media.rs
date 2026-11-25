//! Renders the README's animations: a cover through the real pipeline
//! (analysis, recipe, renderer) at a steady beat, drawn as a terminal would
//! with the bundled Space Mono, saved as looping GIFs.
//!
//! `cargo run -p cw-render --release --example readme_media -- VISION_ONNX COVER OUT_DIR [cover]`
//! (`cover`: lyrics in the cover's own characters and colours)

use cw_analysis::{ClipModel, GenreFamily, Models, Pca, ScenePrompts};
use cw_core::{CellGrid, Rgb};
use cw_render::lyrics::{LyricColors, LyricView, lyric_base, lyric_scene};
use cw_render::{ArtParams, Input, art_rect, base_art, prepare, render};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use std::collections::HashMap;

const BG: Rgb = Rgb(14, 15, 18);
const COLS: u16 = 120;
const ROWS: u16 = 60;
const CELL_W: u32 = 6;
const CELL_H: u32 = 12;
const FRAMES: u32 = 90;
/// Every second app frame (the app runs at 30 fps).
const STEP: u32 = 2;

struct Painter {
    font: fontdue::Font,
    px: f32,
    ascent: f32,
    glyphs: HashMap<char, (fontdue::Metrics, Vec<u8>)>,
}

impl Painter {
    fn new() -> Painter {
        let font = fontdue::Font::from_bytes(
            include_bytes!("../../../assets/fonts/SpaceMono-Bold.ttf") as &[u8],
            fontdue::FontSettings::default(),
        )
        .expect("font");
        let px = 10.0;
        let ascent = font
            .horizontal_line_metrics(px)
            .map_or(px * 0.8, |m| m.ascent);
        Painter {
            font,
            px,
            ascent,
            glyphs: HashMap::new(),
        }
    }

    /// The grid as pixels: each cell's background, then its glyph.
    fn paint(&mut self, grid: &CellGrid) -> RgbaImage {
        let (w, h) = (grid.width() as u32, grid.height() as u32);
        let mut img = RgbaImage::new(w * CELL_W, h * CELL_H);
        let pad = ((CELL_H as f32 - self.px * 1.15) / 2.0).max(0.0);
        for cy in 0..h {
            for cx in 0..w {
                let Some(cell) = grid.get(cx as u16, cy as u16) else {
                    continue;
                };
                let bg = cell.bg.unwrap_or(BG);
                for y in 0..CELL_H {
                    for x in 0..CELL_W {
                        img.put_pixel(
                            cx * CELL_W + x,
                            cy * CELL_H + y,
                            image::Rgba([bg.0, bg.1, bg.2, 255]),
                        );
                    }
                }
                if cell.ch == ' ' || cell.ch == cw_render::lyrics::WIDE_CONTINUATION {
                    continue;
                }
                let px = self.px;
                let font = &self.font;
                let (m, cover) = self
                    .glyphs
                    .entry(cell.ch)
                    .or_insert_with(|| font.rasterize(cell.ch, px));
                let base = cy as f32 * CELL_H as f32 + pad + self.ascent;
                let top = (base - m.height as f32 - m.ymin as f32).round() as i32;
                let left = cx as i32 * CELL_W as i32 + m.xmin;
                for gy in 0..m.height {
                    for gx in 0..m.width {
                        let a = cover[gy * m.width + gx] as u32;
                        if a == 0 {
                            continue;
                        }
                        let (x, y) = (left + gx as i32, top + gy as i32);
                        if x < 0 || y < 0 || x as u32 >= img.width() || y as u32 >= img.height() {
                            continue;
                        }
                        let p = img.get_pixel_mut(x as u32, y as u32);
                        let mix =
                            |b: u8, f: u8| ((b as u32 * (255 - a) + f as u32 * a) / 255) as u8;
                        *p = image::Rgba([
                            mix(p[0], cell.fg.0),
                            mix(p[1], cell.fg.1),
                            mix(p[2], cell.fg.2),
                            255,
                        ]);
                    }
                }
            }
        }
        img
    }
}

fn save(frames: Vec<RgbaImage>, path: &std::path::Path) {
    let file = std::fs::File::create(path).expect("create");
    let mut enc = GifEncoder::new_with_speed(file, 10);
    enc.set_repeat(Repeat::Infinite).expect("repeat");
    let delay = Delay::from_numer_denom_ms(1000 * STEP, 30);
    enc.encode_frames(
        frames
            .into_iter()
            .map(|f| Frame::from_parts(f, 0, 0, delay)),
    )
    .expect("encode");
    println!("{}", path.display());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: readme_media VISION_ONNX COVER OUT_DIR");
        std::process::exit(2);
    }
    let read = |p: &str| std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let model = ClipModel::from_onnx_bytes(&read(&args[0])).expect("model");
    let pca = Pca::from_bytes(&read("assets/pca.bin")).ok();
    let cover = cw_analysis::analyse(&read(&args[1])).expect("cover");
    let scenes = ScenePrompts::shipped();
    let genre = GenreFamily::of_any("electronic");
    let models = Models {
        clip: Some(&model),
        scenes: &scenes,
        pca: pca.as_ref(),
        genre,
    };
    let (recipe, _) = cw_analysis::recipe(&cover, models).expect("recipe");
    let kinds: Vec<_> = recipe.layers.iter().map(|l| l.kind()).collect();
    eprintln!("layers: {kinds:?}");
    let out = std::path::Path::new(&args[2]);
    std::fs::create_dir_all(out).expect("out dir");

    let ramp = recipe.global.charset.ramp();
    let params = ArtParams {
        cell_aspect: CELL_W as f32 / CELL_H as f32,
        style: cw_core::ArtStyle::TrueColor,
        background: BG,
        ramp,
    };
    let base = base_art(&cover.image, &cover.palette, COLS, ROWS, params);
    let art = art_rect(
        cover.image.width,
        cover.image.height,
        COLS,
        ROWS,
        params.cell_aspect,
    )
    .expect("art");
    let clock = cw_core::BeatClock::new(124.0, genre.beat_pattern(), 0.8);
    let mut painter = Painter::new();

    // The cover, moving to the beat.
    let scene = prepare(&recipe, base.clone(), art, ramp);
    let frames = (0..FRAMES)
        .map(|i| {
            let f = i * STEP;
            let audio = clock.frame(f as f64 / 30.0, true);
            let grid = render(
                &scene,
                Input {
                    frame: f,
                    audio: &audio,
                    fade: 1.0,
                },
            )
            .grid;
            painter.paint(&grid)
        })
        .collect();
    save(frames, &out.join("cover.gif"));

    // Lyrics in the cover's square, the same motion running over them.
    let words = cw_core::lyrics::parse_lrc(
        "[00:00.00]moonlight on the water\n[00:01.60]counting every wave\n\
         [00:03.20]the tide keeps the time\n[00:04.80]and the night keeps us\n[00:06.40]awake",
        "readme",
    )
    .expect("lyrics");
    let colors = LyricColors {
        accent: cover.palette.accent,
        text: Rgb(222, 226, 232),
        background: BG,
    };
    let mut view = LyricView::default();
    if args.get(3).is_some_and(|a| a == "cover") {
        view.paint = cw_render::lyrics::LyricPaint::Cover;
    }
    let frames = (0..FRAMES)
        .map(|i| {
            let f = i * STEP;
            let pos = f as f64 / 30.0;
            let audio = clock.frame(pos, true);
            let lb = lyric_base(
                &base,
                art,
                params.cell_aspect,
                &words,
                pos,
                Some(8.0),
                view,
                colors,
                &audio,
            );
            let grid = render(
                &lyric_scene(&recipe, lb, art, ramp),
                Input {
                    frame: f,
                    audio: &audio,
                    fade: 1.0,
                },
            )
            .grid;
            painter.paint(&grid)
        })
        .collect();
    save(frames, &out.join("lyrics.gif"));
}
