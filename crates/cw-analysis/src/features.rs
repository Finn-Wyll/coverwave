//! Cheap global image statistics (PLAN.md section 4, step 3).

use cw_core::color::srgb_to_linear;
use cw_core::recipe::Features;
use cw_core::{Lab, RgbImage};

/// Side of the square the features are computed on.
const SIDE: u32 = 128;
/// Sobel magnitude on L*/100 that counts as an edge. A clean step of about
/// 12 L* units between neighbours reaches it.
const EDGE_THRESHOLD: f32 = 0.5;

pub fn features(img: &RgbImage) -> Features {
    let small = img.resize_area(SIDE, SIDE);
    let n = small.pixels.len().max(1) as f32;
    let ls: Vec<f32> = small
        .pixels
        .iter()
        .map(|&c| Lab::from(c).l / 100.0)
        .collect();

    let brightness = ls.iter().sum::<f32>() / n;
    let contrast = (ls.iter().map(|l| (l - brightness).powi(2)).sum::<f32>() / n).sqrt();

    let saturation = small
        .pixels
        .iter()
        .map(|c| {
            let max = c.0.max(c.1).max(c.2) as f32;
            let min = c.0.min(c.1).min(c.2) as f32;
            if max > 0.0 { (max - min) / max } else { 0.0 }
        })
        .sum::<f32>()
        / n;

    let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
    for c in &small.pixels {
        r += srgb_to_linear(c.0 as f32 / 255.0);
        g += srgb_to_linear(c.1 as f32 / 255.0);
        b += srgb_to_linear(c.2 as f32 / 255.0);
    }
    let temperature_k = cct(r / n, g / n, b / n);

    let edge_density = {
        let mag = sobel(&ls, SIDE as usize, SIDE as usize);
        mag.iter().filter(|&&m| m > EDGE_THRESHOLD).count() as f32 / n
    };
    let energy = 0.5 * saturation + 0.5 * edge_density;

    Features {
        brightness: round3(brightness),
        contrast: round3(contrast),
        saturation: round3(saturation),
        temperature_k: temperature_k.round(),
        edge_density: round3(edge_density),
        energy: round3(energy),
    }
}

/// Rounds to 3 decimals, so recipes are stable and readable.
pub fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

/// Correlated colour temperature (McCamy's approximation) of a linear-RGB
/// colour, clamped to 1000..20000 K. Black reads as 6500 K.
fn cct(r: f32, g: f32, b: f32) -> f32 {
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
    let sum = x + y + z;
    if sum <= 1e-6 {
        return 6500.0;
    }
    let (cx, cy) = (x / sum, y / sum);
    let denom = 0.1858 - cy;
    if denom.abs() < 1e-6 {
        return 20_000.0;
    }
    let n = (cx - 0.3320) / denom;
    (449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33).clamp(1000.0, 20_000.0)
}

/// Sobel gradient magnitude with clamped borders.
pub(crate) fn sobel(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let at = |x: isize, y: isize| {
        let x = x.clamp(0, w as isize - 1) as usize;
        let y = y.clamp(0, h as isize - 1) as usize;
        v[y * w + x]
    };
    let mut out = vec![0.0; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x - 1, y)
                - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x, y - 1)
                - at(x + 1, y - 1);
            out[y as usize * w + x as usize] = (gx * gx + gy * gy).sqrt();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::Rgb;

    fn flat(c: Rgb) -> RgbImage {
        RgbImage::new(16, 16, vec![c; 256])
    }

    #[test]
    fn flat_images() {
        let f = features(&flat(Rgb(0, 0, 0)));
        assert_eq!(
            (f.brightness, f.contrast, f.saturation, f.edge_density),
            (0.0, 0.0, 0.0, 0.0)
        );
        let white = features(&flat(Rgb(255, 255, 255)));
        assert_eq!(white.brightness, 1.0);
        // D65 white is about 6500 K.
        assert!(
            (6300.0..6700.0).contains(&white.temperature_k),
            "{}",
            white.temperature_k
        );
        let red = features(&flat(Rgb(255, 0, 0)));
        assert_eq!(red.saturation, 1.0);
        assert_eq!(red.energy, 0.5);
    }

    #[test]
    fn warm_is_warmer_than_cool() {
        let warm = features(&flat(Rgb(255, 180, 100))).temperature_k;
        let cool = features(&flat(Rgb(120, 170, 255))).temperature_k;
        assert!(warm < 4000.0 && cool > 9000.0, "{warm} {cool}");
    }

    #[test]
    fn stripes_have_edges_and_contrast() {
        let pixels = (0..64 * 64)
            .map(|i| {
                if (i % 64) / 8 % 2 == 0 {
                    Rgb(0, 0, 0)
                } else {
                    Rgb(255, 255, 255)
                }
            })
            .collect();
        let f = features(&RgbImage::new(64, 64, pixels));
        assert!(f.contrast > 0.45, "{f:?}");
        assert!((0.1..0.5).contains(&f.edge_density), "{f:?}");
    }
}
