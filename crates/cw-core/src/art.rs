//! Cover-art types: a decoded RGB image, the cover palette and the art
//! colour style. Decoding lives in `cw-analysis`; turning these into cells
//! lives in `cw-render`.

use crate::Rgb;
use crate::color::{linear_to_srgb, srgb_to_linear};

/// A row-major 8-bit sRGB image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<Rgb>,
}

impl RgbImage {
    /// Panics if `pixels.len() != width * height`.
    pub fn new(width: u32, height: u32, pixels: Vec<Rgb>) -> RgbImage {
        assert_eq!(pixels.len(), width as usize * height as usize);
        RgbImage {
            width,
            height,
            pixels,
        }
    }

    pub fn get(&self, x: u32, y: u32) -> Rgb {
        self.pixels[y as usize * self.width as usize + x as usize]
    }

    /// Area-average resample to `w x h`, averaging in linear light so dark
    /// detail is not muddied. Each output pixel is the exact coverage-weighted
    /// mean of the source pixels under it, which is what an ASCII cell
    /// should show. Deterministic; `w`/`h` of 0 give an empty image.
    pub fn resize_area(&self, w: u32, h: u32) -> RgbImage {
        if w == 0 || h == 0 || self.width == 0 || self.height == 0 {
            return RgbImage {
                width: w,
                height: h,
                pixels: vec![Rgb::default(); w as usize * h as usize],
            };
        }
        let lut: Vec<f32> = (0..=255u8)
            .map(|v| srgb_to_linear(v as f32 / 255.0))
            .collect();
        // Per-axis spans: for each output index, the source range and the
        // weights of the partially covered edge pixels.
        let spans = |src: u32, dst: u32| -> Vec<(u32, Vec<f32>)> {
            let scale = src as f64 / dst as f64;
            (0..dst)
                .map(|i| {
                    let a = i as f64 * scale;
                    let b = (i + 1) as f64 * scale;
                    let first = a.floor() as u32;
                    let last = (b.ceil() as u32).min(src);
                    let weights = (first..last)
                        .map(|p| {
                            let lo = a.max(p as f64);
                            let hi = b.min(p as f64 + 1.0);
                            (hi - lo).max(0.0) as f32
                        })
                        .collect();
                    (first, weights)
                })
                .collect()
        };
        let xs = spans(self.width, w);
        let ys = spans(self.height, h);
        let mut out = Vec::with_capacity(w as usize * h as usize);
        for (y0, wy) in &ys {
            for (x0, wx) in &xs {
                let (mut r, mut g, mut b, mut total) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                for (dy, fy) in wy.iter().enumerate() {
                    let row = (y0 + dy as u32) as usize * self.width as usize;
                    for (dx, fx) in wx.iter().enumerate() {
                        let p = self.pixels[row + (x0 + dx as u32) as usize];
                        let k = fy * fx;
                        r += lut[p.0 as usize] * k;
                        g += lut[p.1 as usize] * k;
                        b += lut[p.2 as usize] * k;
                        total += k;
                    }
                }
                let f = |v: f32| (linear_to_srgb(v / total) * 255.0).round() as u8;
                out.push(Rgb(f(r), f(g), f(b)));
            }
        }
        RgbImage::new(w, h, out)
    }
}

/// The cover's dominant colours, largest cluster first, plus the single
/// accent used for UI highlights.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// Cluster centres, sorted by `weights` descending.
    pub colors: Vec<Rgb>,
    /// Fraction of pixels in each cluster; sums to about 1.
    pub weights: Vec<f32>,
    /// One colour chosen for UI highlights: saturated, and light enough to
    /// read on a near-black background.
    pub accent: Rgb,
}

/// How the ASCII cover is coloured. Glyph density always follows brightness;
/// this only picks glyph colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ArtStyle {
    /// Nearest palette colour, blended part-way back toward the true colour.
    #[default]
    PaletteLed,
    /// Every glyph takes its true pixel colour.
    TrueColor,
    /// A two-colour ramp from a dark palette colour to the accent.
    Duotone,
}

impl ArtStyle {
    pub const ALL: [ArtStyle; 3] = [ArtStyle::PaletteLed, ArtStyle::TrueColor, ArtStyle::Duotone];

    pub fn next(self) -> ArtStyle {
        match self {
            ArtStyle::PaletteLed => ArtStyle::TrueColor,
            ArtStyle::TrueColor => ArtStyle::Duotone,
            ArtStyle::Duotone => ArtStyle::PaletteLed,
        }
    }

    /// Name used in settings and the UI.
    pub fn name(self) -> &'static str {
        match self {
            ArtStyle::PaletteLed => "palette",
            ArtStyle::TrueColor => "truecolor",
            ArtStyle::Duotone => "duotone",
        }
    }

    pub fn from_name(s: &str) -> Option<ArtStyle> {
        ArtStyle::ALL
            .into_iter()
            .find(|a| a.name().eq_ignore_ascii_case(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_area_averages_exactly() {
        // 2x1 black/white -> 1x1 is mid grey in linear light (~188 in sRGB).
        let img = RgbImage::new(2, 1, vec![Rgb(0, 0, 0), Rgb(255, 255, 255)]);
        let one = img.resize_area(1, 1);
        assert_eq!(one.pixels, vec![Rgb(188, 188, 188)]);
        // Identity size is lossless.
        let same = img.resize_area(2, 1);
        assert_eq!(same, img);
        // Non-integer ratios cover every source pixel once.
        let img = RgbImage::new(3, 3, vec![Rgb(10, 20, 30); 9]);
        assert_eq!(img.resize_area(2, 2).pixels, vec![Rgb(10, 20, 30); 4]);
        assert_eq!(img.resize_area(0, 4).pixels.len(), 0);
    }

    #[test]
    fn art_style_names_round_trip() {
        for s in ArtStyle::ALL {
            assert_eq!(ArtStyle::from_name(s.name()), Some(s));
        }
        assert_eq!(
            ArtStyle::default().next().next().next(),
            ArtStyle::default()
        );
    }
}
