//! Static ASCII cover art: the "base art" layer (PLAN.md section 7, steps
//! 1-2). Computed once per terminal size or style change, not per frame.
//!
//! Glyph density follows perceptual lightness (CIE L*), contrast-stretched
//! per cover so dim covers still use the whole ramp. Glyph colour depends on
//! [`ArtStyle`]. Pure and deterministic like the rest of the crate.

use cw_core::{ArtStyle, Cell, CellGrid, Lab, Palette, Rgb, RgbImage};

/// Density ramp, sparse to dense. Plain ASCII so every font has it.
pub const RAMP: &[char] = &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

/// Typical terminal cell width / height.
pub const DEFAULT_CELL_ASPECT: f32 = 0.5;

/// How far palette-led colours are pulled back toward the true colour.
const PALETTE_TRUE_MIX: f32 = 0.35;
/// The narrowest L* range the contrast stretch may expand to, so flat covers
/// are not blown up into noise.
const MIN_L_RANGE: f32 = 45.0;
/// Lower/upper percentiles used as the black and white points.
const STRETCH_PERCENTILES: (f32, f32) = (0.02, 0.98);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArtParams {
    /// Cell width divided by cell height (about 0.5 for most fonts).
    pub cell_aspect: f32,
    pub style: ArtStyle,
    /// Background for every cell, inside and outside the cover.
    pub background: Rgb,
    /// Density ramp, sparse to dense, starting with a space ([`RAMP`] or a
    /// recipe's charset).
    pub ramp: &'static [char],
}

/// Size in cells of the largest cover that fits in `cols x rows` while
/// keeping the image's proportions on screen. Never zero for a non-empty
/// area.
pub fn fit_cells(img_w: u32, img_h: u32, cols: u16, rows: u16, cell_aspect: f32) -> (u16, u16) {
    if cols == 0 || rows == 0 || img_w == 0 || img_h == 0 {
        return (0, 0);
    }
    let aspect = cell_aspect.clamp(0.2, 2.0) as f64;
    // On screen, c cells wide and r tall is (c * aspect) : r in cell heights.
    let ratio = img_w as f64 / img_h as f64 / aspect; // columns per row
    let (c, r) = if rows as f64 * ratio <= cols as f64 {
        ((rows as f64 * ratio).round(), rows as f64)
    } else {
        (cols as f64, (cols as f64 / ratio).round())
    };
    ((c as u16).clamp(1, cols), (r as u16).clamp(1, rows))
}

/// A rectangle of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// Where [`base_art`] puts the cover inside a `cols x rows` grid, or `None`
/// for an empty area.
pub fn art_rect(img_w: u32, img_h: u32, cols: u16, rows: u16, cell_aspect: f32) -> Option<Rect> {
    let (w, h) = fit_cells(img_w, img_h, cols, rows, cell_aspect);
    (w > 0 && h > 0).then(|| Rect {
        x: (cols - w) / 2,
        y: (rows - h) / 2,
        w,
        h,
    })
}

/// Renders `image` as ASCII art centred in a `cols x rows` grid.
pub fn base_art(
    image: &RgbImage,
    palette: &Palette,
    cols: u16,
    rows: u16,
    params: ArtParams,
) -> CellGrid {
    let mut grid = CellGrid::new(cols, rows);
    let bg = Some(params.background);
    for y in 0..rows {
        for x in 0..cols {
            grid.set(
                x,
                y,
                Cell {
                    ch: ' ',
                    fg: params.background,
                    bg,
                },
            );
        }
    }
    let Some(Rect { x: ox, y: oy, w, h }) =
        art_rect(image.width, image.height, cols, rows, params.cell_aspect)
    else {
        return grid;
    };
    let ramp = if params.ramp.len() >= 2 {
        params.ramp
    } else {
        RAMP
    };

    let cells = image.resize_area(w as u32, h as u32);
    let labs: Vec<Lab> = cells.pixels.iter().map(|&c| Lab::from(c)).collect();
    let (lo, hi) = stretch_range(&labs);
    let palette_labs: Vec<Lab> = palette.colors.iter().map(|&c| Lab::from(c)).collect();
    let accent = Lab::from(palette.accent);
    let duo_dark = duotone_dark(&palette_labs);

    for (i, lab) in labs.iter().enumerate() {
        let t = ((lab.l - lo) / (hi - lo)).clamp(0.0, 1.0);
        let idx = (t * (ramp.len() - 1) as f32).round() as usize;
        let ch = ramp[idx];
        let fg = match params.style {
            ArtStyle::TrueColor => lift(*lab),
            ArtStyle::PaletteLed => {
                let near = palette_labs
                    .iter()
                    .copied()
                    .min_by(|a, b| a.dist2(*lab).total_cmp(&b.dist2(*lab)))
                    .unwrap_or(*lab);
                lift(near.lerp(*lab, PALETTE_TRUE_MIX))
            }
            ArtStyle::Duotone => duo_dark.lerp(accent, t),
        };
        let (x, y) = (i as u16 % w, i as u16 / w);
        grid.set(
            ox + x,
            oy + y,
            Cell {
                ch,
                fg: Rgb::from(fg),
                bg,
            },
        );
    }
    grid
}

/// Black and white points (in L*) for the contrast stretch.
fn stretch_range(labs: &[Lab]) -> (f32, f32) {
    let mut ls: Vec<f32> = labs.iter().map(|l| l.l).collect();
    ls.sort_by(f32::total_cmp);
    let at = |q: f32| ls[((ls.len() - 1) as f32 * q).round() as usize];
    let (mut lo, mut hi) = (at(STRETCH_PERCENTILES.0), at(STRETCH_PERCENTILES.1));
    if hi - lo < MIN_L_RANGE {
        let mid = (lo + hi) / 2.0;
        lo = (mid - MIN_L_RANGE / 2.0).max(0.0);
        hi = lo + MIN_L_RANGE;
        if hi > 100.0 {
            hi = 100.0;
            lo = 100.0 - MIN_L_RANGE;
        }
    }
    (lo, hi)
}

/// Raises dark colours so sparse glyphs stay readable on near-black:
/// L* 0 maps to 30, L* 100 stays 100.
fn lift(c: Lab) -> Lab {
    Lab {
        l: 30.0 + 0.7 * c.l,
        ..c
    }
}

/// The dark end of the duotone ramp: the darkest palette colour, set to a
/// fixed low lightness with its chroma capped so it reads as a shadow tint.
fn duotone_dark(palette: &[Lab]) -> Lab {
    let darkest = palette
        .iter()
        .copied()
        .min_by(|a, b| a.l.total_cmp(&b.l))
        .unwrap_or_default();
    let chroma = darkest.chroma();
    let scale = if chroma > 20.0 { 20.0 / chroma } else { 1.0 };
    Lab {
        l: 24.0,
        a: darkest.a * scale,
        b: darkest.b * scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: Rgb = Rgb(14, 15, 18);

    fn params(style: ArtStyle) -> ArtParams {
        ArtParams {
            cell_aspect: 0.5,
            style,
            background: BG,
            ramp: RAMP,
        }
    }

    /// A warm disc on a cool, darker background with a bright highlight: a
    /// stand-in for a cover with a clear subject.
    fn fixture() -> (RgbImage, Palette) {
        let n = 96u32;
        let pixels = (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f32 / n as f32, (i / n) as f32 / n as f32);
                let d = ((x - 0.5).powi(2) + (y - 0.55).powi(2)).sqrt();
                let hl = ((x - 0.4).powi(2) + (y - 0.42).powi(2)).sqrt();
                if hl < 0.08 {
                    Rgb(250, 236, 200)
                } else if d < 0.3 {
                    let k = 1.0 - d / 0.3;
                    Rgb((150.0 + 90.0 * k) as u8, (60.0 + 70.0 * k) as u8, 40)
                } else {
                    Rgb(20, (30.0 + 40.0 * y) as u8, (60.0 + 60.0 * y) as u8)
                }
            })
            .collect();
        let palette = Palette {
            colors: vec![
                Rgb(22, 45, 90),
                Rgb(200, 100, 40),
                Rgb(240, 130, 40),
                Rgb(250, 236, 200),
            ],
            weights: vec![0.6, 0.2, 0.15, 0.05],
            accent: Rgb(240, 150, 80),
        };
        (RgbImage::new(n, n, pixels), palette)
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
    fn fit_keeps_on_screen_proportions() {
        // Square cover, 1:2 cells: twice as many columns as rows.
        assert_eq!(fit_cells(500, 500, 240, 70, 0.5), (140, 70));
        // Narrow terminal: width-limited.
        assert_eq!(fit_cells(500, 500, 60, 70, 0.5), (60, 30));
        // Wide image.
        assert_eq!(fit_cells(1000, 500, 100, 100, 0.5), (100, 25));
        assert_eq!(fit_cells(500, 500, 0, 10, 0.5), (0, 0));
        assert_eq!(fit_cells(1, 1000, 10, 3, 0.5), (1, 3));
    }

    #[test]
    fn deterministic_and_centred() {
        let (img, pal) = fixture();
        let a = base_art(&img, &pal, 120, 40, params(ArtStyle::PaletteLed));
        assert_eq!(
            a,
            base_art(&img, &pal, 120, 40, params(ArtStyle::PaletteLed))
        );
        // 80x40 cover centred in 120 columns: 20 blank columns each side.
        for y in 0..40 {
            for x in (0..20).chain(100..120) {
                let c = a.get(x, y).unwrap();
                assert_eq!((c.ch, c.bg), (' ', Some(BG)));
            }
        }
        assert!((20..100).any(|x| a.get(x, 20).unwrap().ch != ' '));
    }

    #[test]
    fn styles_share_glyphs_but_not_colours() {
        let (img, pal) = fixture();
        let grids: Vec<CellGrid> = ArtStyle::ALL
            .iter()
            .map(|&s| base_art(&img, &pal, 60, 30, params(s)))
            .collect();
        assert_eq!(chars(&grids[0]), chars(&grids[1]));
        assert_eq!(chars(&grids[0]), chars(&grids[2]));
        assert_ne!(grids[0], grids[1]);
        assert_ne!(grids[1], grids[2]);
        // Duotone's brightest glyphs are exactly the accent.
        let top = grids[2]
            .cells()
            .iter()
            .find(|c| c.ch == '@')
            .expect("a full-density cell");
        assert_eq!(top.fg, pal.accent);
    }

    #[test]
    fn flat_image_does_not_blow_up_into_noise() {
        let img = RgbImage::new(8, 8, vec![Rgb(120, 120, 120); 64]);
        let pal = Palette {
            colors: vec![Rgb(120, 120, 120)],
            weights: vec![1.0],
            accent: Rgb(200, 200, 200),
        };
        let g = base_art(&img, &pal, 30, 10, params(ArtStyle::TrueColor));
        let mut glyphs: Vec<char> = g.cells().iter().map(|c| c.ch).collect();
        glyphs.sort();
        glyphs.dedup();
        // Background blanks plus one mid-ramp glyph, not '@'.
        assert_eq!(glyphs.len(), 2, "{glyphs:?}");
        assert!(!glyphs.contains(&'@'));
    }

    #[test]
    fn tiny_sizes_do_not_panic() {
        let (img, pal) = fixture();
        for (w, h) in [(0, 0), (1, 1), (2, 1), (1, 7)] {
            let g = base_art(&img, &pal, w, h, params(ArtStyle::Duotone));
            assert_eq!((g.width(), g.height()), (w, h));
        }
    }

    #[test]
    fn golden_glyphs() {
        let (img, pal) = fixture();
        let g = base_art(&img, &pal, 48, 24, params(ArtStyle::PaletteLed));
        insta::assert_snapshot!(chars(&g));
    }

    #[test]
    fn golden_colours() {
        // One sample row per style, as hex, to pin the colour maths.
        let (img, pal) = fixture();
        let mut out = String::new();
        for s in ArtStyle::ALL {
            let g = base_art(&img, &pal, 48, 24, params(s));
            out.push_str(s.name());
            out.push(':');
            for x in (12..36).step_by(3) {
                let c = g.get(x, 13).unwrap().fg;
                out.push_str(&format!(" {:02x}{:02x}{:02x}", c.0, c.1, c.2));
            }
            out.push('\n');
        }
        insta::assert_snapshot!(out);
    }
}
