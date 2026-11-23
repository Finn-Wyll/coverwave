//! Heuristic subject mask (PLAN.md section 4): saliency from colour distance
//! to the border colour, a centre prior and local edge density, then Otsu's
//! threshold and a morphological close. Cells in the mask are never moved.

use crate::features::sobel;
use cw_core::recipe::Mask;
use cw_core::{Lab, RgbImage};

pub const MASK_SIDE: u32 = 64;

const W_COLOUR: f32 = 0.55;
const W_CENTRE: f32 = 0.2;
const W_EDGES: f32 = 0.25;
/// Centre prior: Gaussian sigma as a fraction of the image side.
const CENTRE_SIGMA: f32 = 0.3;
/// Masks larger than this are cut back to it: a mask that covers most of the
/// cover would leave nothing to animate, and is usually a wrong guess.
const MAX_COVERAGE: f32 = 0.45;
/// Masks smaller than this are dropped as noise.
const MIN_COVERAGE: f32 = 0.01;
/// A cover needs some part at least this far (Delta E) from its border
/// colour, or an edge at least this strong (Sobel on L*/100), to have a
/// subject at all. Otherwise the centre prior alone would invent one.
const MIN_COLOUR_DELTA: f32 = 10.0;
const MIN_EDGE: f32 = 0.3;

pub fn subject_mask(img: &RgbImage) -> Mask {
    let side = MASK_SIDE as usize;
    let small = img.resize_area(MASK_SIDE, MASK_SIDE);
    let labs: Vec<Lab> = small.pixels.iter().map(|&c| Lab::from(c)).collect();

    // Border colour: mean of the outer two-pixel ring.
    let mut border = (0.0f32, 0.0f32, 0.0f32, 0usize);
    for y in 0..side {
        for x in 0..side {
            if x < 2 || y < 2 || x >= side - 2 || y >= side - 2 {
                let l = labs[y * side + x];
                border = (border.0 + l.l, border.1 + l.a, border.2 + l.b, border.3 + 1);
            }
        }
    }
    let k = border.3.max(1) as f32;
    let border = Lab {
        l: border.0 / k,
        a: border.1 / k,
        b: border.2 / k,
    };
    let colour: Vec<f32> = labs.iter().map(|l| l.dist2(border).sqrt()).collect();

    let ls: Vec<f32> = labs.iter().map(|l| l.l / 100.0).collect();
    let edges = box_blur(&sobel(&ls, side, side), side, 2);

    let norm = |v: &[f32]| -> Vec<f32> {
        let max = v.iter().cloned().fold(0.0f32, f32::max);
        if max > 1e-6 {
            v.iter().map(|x| x / max).collect()
        } else {
            vec![0.0; v.len()]
        }
    };
    let max_of = |v: &[f32]| v.iter().cloned().fold(0.0f32, f32::max);
    let empty = Mask::from_bits(MASK_SIDE, MASK_SIDE, &vec![false; side * side]);
    if max_of(&colour) < MIN_COLOUR_DELTA && max_of(&edges) < MIN_EDGE {
        return empty;
    }
    let (colour, edges) = (norm(&colour), norm(&edges));
    let saliency: Vec<f32> = (0..side * side)
        .map(|i| {
            let (x, y) = ((i % side) as f32 + 0.5, (i / side) as f32 + 0.5);
            let (dx, dy) = (x / side as f32 - 0.5, y / side as f32 - 0.5);
            let centre = (-(dx * dx + dy * dy) / (2.0 * CENTRE_SIGMA * CENTRE_SIGMA)).exp();
            W_COLOUR * colour[i] + W_CENTRE * centre + W_EDGES * edges[i]
        })
        .collect();

    let lo = saliency.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = max_of(&saliency);
    let mut threshold = otsu(&saliency, lo, hi);
    // Cap the coverage by raising the threshold to the right percentile.
    let above = saliency.iter().filter(|&&s| s > threshold).count();
    let max_cells = (MAX_COVERAGE * (side * side) as f32) as usize;
    if above > max_cells {
        let mut sorted = saliency.clone();
        sorted.sort_by(|a, b| b.total_cmp(a));
        threshold = sorted[max_cells];
    }
    let bits: Vec<bool> = saliency.iter().map(|&s| s > threshold).collect();
    let bits = erode(&dilate(&bits, side), side);
    let mask = Mask::from_bits(MASK_SIDE, MASK_SIDE, &bits);
    if mask.coverage() < MIN_COVERAGE {
        empty
    } else {
        mask
    }
}

/// Otsu's threshold over a 64-bin histogram of `v` in `lo..hi`.
fn otsu(v: &[f32], lo: f32, hi: f32) -> f32 {
    const BINS: usize = 64;
    let mut hist = [0usize; BINS];
    let span = (hi - lo).max(1e-6);
    for &x in v {
        hist[(((x - lo) / span) * (BINS - 1) as f32).round() as usize] += 1;
    }
    let total = v.len() as f64;
    let sum_all: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &c)| i as f64 * c as f64)
        .sum();
    let (mut w0, mut sum0, mut best, mut best_i) = (0.0f64, 0.0f64, -1.0f64, 0usize);
    for (i, &c) in hist.iter().enumerate() {
        w0 += c as f64;
        sum0 += i as f64 * c as f64;
        let w1 = total - w0;
        if w0 == 0.0 || w1 == 0.0 {
            continue;
        }
        let (m0, m1) = (sum0 / w0, (sum_all - sum0) / w1);
        let between = w0 * w1 * (m0 - m1) * (m0 - m1);
        if between > best {
            best = between;
            best_i = i;
        }
    }
    lo + (best_i as f32 + 0.5) / (BINS - 1) as f32 * span
}

fn box_blur(v: &[f32], side: usize, r: isize) -> Vec<f32> {
    let mut out = vec![0.0; v.len()];
    for y in 0..side as isize {
        for x in 0..side as isize {
            let (mut s, mut n) = (0.0, 0.0);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx >= 0 && yy >= 0 && xx < side as isize && yy < side as isize {
                        s += v[yy as usize * side + xx as usize];
                        n += 1.0;
                    }
                }
            }
            out[y as usize * side + x as usize] = s / n;
        }
    }
    out
}

/// 3x3 morphology. `dilate = true` sets a pixel if any neighbour is set
/// (outside the image counts as unset); otherwise erodes, keeping a pixel
/// only if all neighbours are set (outside counts as set, so a closing does
/// not eat subjects touching the edge).
fn morph(b: &[bool], side: usize, dilate: bool) -> Vec<bool> {
    (0..side * side)
        .map(|i| {
            let (x, y) = ((i % side) as isize, (i / side) as isize);
            let mut hits = 0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (xx, yy) = (x + dx, y + dy);
                    let inside = xx >= 0 && yy >= 0 && xx < side as isize && yy < side as isize;
                    let on = if inside {
                        b[yy as usize * side + xx as usize]
                    } else {
                        !dilate
                    };
                    hits += on as usize;
                }
            }
            if dilate { hits > 0 } else { hits == 9 }
        })
        .collect()
}

fn dilate(b: &[bool], side: usize) -> Vec<bool> {
    morph(b, side, true)
}

fn erode(b: &[bool], side: usize) -> Vec<bool> {
    morph(b, side, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::Rgb;

    fn disc(cx: f32, cy: f32, r: f32) -> RgbImage {
        let n = 128u32;
        let pixels = (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f32 / n as f32, (i / n) as f32 / n as f32);
                if ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < r {
                    Rgb(230, 200, 60)
                } else {
                    Rgb(20, 30, 70)
                }
            })
            .collect();
        RgbImage::new(n, n, pixels)
    }

    #[test]
    fn finds_a_disc() {
        let m = subject_mask(&disc(0.5, 0.5, 0.25));
        let expected = std::f32::consts::PI * 0.25 * 0.25;
        let c = m.coverage();
        assert!((c - expected).abs() < 0.05, "coverage {c} vs {expected}");
        assert!(m.get(32, 32), "centre is subject");
        assert!(!m.get(2, 2) && !m.get(61, 61), "corners are background");
    }

    #[test]
    fn off_centre_subject_still_found() {
        let m = subject_mask(&disc(0.3, 0.35, 0.15));
        assert!(m.get(19, 22), "{:?}", m.rows);
        assert!(!m.get(48, 48));
    }

    #[test]
    fn flat_cover_has_no_subject() {
        let img = RgbImage::new(32, 32, vec![Rgb(90, 90, 90); 1024]);
        assert_eq!(subject_mask(&img).coverage(), 0.0);
    }

    #[test]
    fn huge_subject_is_capped() {
        let c = subject_mask(&disc(0.5, 0.5, 0.48)).coverage();
        assert!(c <= MAX_COVERAGE + 0.05, "{c}");
    }

    #[test]
    fn deterministic() {
        let img = disc(0.4, 0.6, 0.2);
        assert_eq!(subject_mask(&img), subject_mask(&img));
    }
}
