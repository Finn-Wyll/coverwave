//! Pure, deterministic renderer: inputs in, `CellGrid` out.
//!
//! No clocks, no global RNG, no I/O. Every function here must return the same
//! grid for the same arguments, and every animation must loop seamlessly over
//! `loop_frames` (see PLAN.md section 6).
//!
//! M0 only ships the idle scene shown before anything is playing. The recipe
//! driven renderer lands in M5.

use cw_core::{Cell, CellGrid, Rgb};
use std::f32::consts::TAU;

/// Default loop length: 8 seconds at 30fps.
pub const DEFAULT_LOOP_FRAMES: u32 = 240;

/// SplitMix64: a tiny, high-quality deterministic hash for seeding per-item
/// values without pulling an RNG into the pure renderer.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Uniform float in [0, 1) derived from `seed` and a stream index.
fn unit(seed: u64, stream: u64) -> f32 {
    (splitmix64(seed ^ splitmix64(stream)) >> 40) as f32 / (1u64 << 24) as f32
}

/// Colours for the idle scene.
#[derive(Debug, Clone, Copy)]
pub struct IdleStyle {
    pub background: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
}

impl Default for IdleStyle {
    fn default() -> Self {
        IdleStyle {
            background: Rgb(14, 15, 18),
            dim: Rgb(58, 62, 70),
            accent: Rgb(196, 204, 214),
        }
    }
}

const WORDMARK: &str = "coverwave";

/// Idle scene: slow drifting particles (three depth layers) behind a static
/// wordmark. Particles wrap on a torus with an integer number of cycles per
/// loop, so frame `loop_frames` is identical to frame 0.
pub fn idle(
    width: u16,
    height: u16,
    frame: u32,
    loop_frames: u32,
    seed: u64,
    style: IdleStyle,
) -> CellGrid {
    let mut grid = CellGrid::new(width, height);
    if width == 0 || height == 0 {
        return grid;
    }
    let bg = Some(style.background);
    for y in 0..height {
        for x in 0..width {
            grid.set(
                x,
                y,
                Cell {
                    ch: ' ',
                    fg: style.dim,
                    bg,
                },
            );
        }
    }

    let loop_frames = loop_frames.max(1);
    let phase = (frame % loop_frames) as f32 / loop_frames as f32;
    let (w, h) = (width as f32, height as f32);

    // Wordmark cells are the "subject": particles never draw over them.
    let mark_len = WORDMARK.chars().count() as u16;
    let mark_y = height / 2;
    let mark_x = width.saturating_sub(mark_len) / 2;
    let in_mark = |x: u16, y: u16| y == mark_y && x >= mark_x && x < mark_x + mark_len;

    // Roughly one particle per 45 cells keeps the change budget far below 15%.
    let count = (width as usize * height as usize / 45).max(1) as u64;
    for i in 0..count {
        let depth = (unit(seed, i * 8) * 3.0) as u32; // 0 = far, 2 = near
        let x0 = unit(seed, i * 8 + 1) * w;
        let y0 = unit(seed, i * 8 + 2) * h;
        let sway_phase = unit(seed, i * 8 + 3) * TAU;
        // Integer cycles per loop: one full wrap of the screen height for far
        // particles, two for nearer ones. Seamless by construction.
        let fall_cycles = if depth == 0 { 1.0 } else { 2.0 };
        let sway_cycles = 1.0 + (unit(seed, i * 8 + 4) * 2.0).floor();
        let sway_amp = 0.6 + depth as f32 * 0.5;

        let y = (y0 + fall_cycles * h * phase).rem_euclid(h);
        let x = (x0 + sway_amp * (TAU * sway_cycles * phase + sway_phase).sin()).rem_euclid(w);
        let (cx, cy) = (x as u16 % width, y as u16 % height);
        if in_mark(cx, cy) {
            continue;
        }
        let (ch, t) = match depth {
            0 => ('.', 0.0),
            1 => (':', 0.35),
            _ => ('*', 0.6),
        };
        grid.set(
            cx,
            cy,
            Cell {
                ch,
                fg: style.dim.lerp(style.accent, t),
                bg,
            },
        );
    }

    for (k, ch) in WORDMARK.chars().enumerate() {
        grid.set(
            mark_x + k as u16,
            mark_y,
            Cell {
                ch,
                fg: style.accent,
                bg,
            },
        );
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u16 = 120;
    const H: u16 = 40;
    const SEED: u64 = 0xC0FFEE;

    fn frame(n: u32) -> CellGrid {
        idle(W, H, n, DEFAULT_LOOP_FRAMES, SEED, IdleStyle::default())
    }

    #[test]
    fn deterministic() {
        assert_eq!(frame(17), frame(17));
    }

    #[test]
    fn loop_seam_is_seamless() {
        assert_eq!(frame(DEFAULT_LOOP_FRAMES), frame(0));
        // The step across the seam changes no more than an ordinary step does.
        let seam = frame(DEFAULT_LOOP_FRAMES - 1).diff_count(&frame(0));
        let worst_normal = (0..DEFAULT_LOOP_FRAMES - 1)
            .map(|n| frame(n).diff_count(&frame(n + 1)))
            .max()
            .unwrap();
        assert!(seam <= worst_normal, "seam {seam} > worst {worst_normal}");
    }

    #[test]
    fn change_budget_under_15_percent() {
        let total = (W as usize * H as usize) as f32;
        for n in 0..DEFAULT_LOOP_FRAMES {
            let changed = frame(n).diff_count(&frame(n + 1)) as f32;
            assert!(
                changed / total <= 0.15,
                "frame {n}: {:.1}% changed",
                100.0 * changed / total
            );
        }
    }

    #[test]
    fn wordmark_never_changes() {
        let mark_x = (W - WORDMARK.len() as u16) / 2;
        let reference = frame(0);
        for n in 0..DEFAULT_LOOP_FRAMES {
            let g = frame(n);
            for k in 0..WORDMARK.len() as u16 {
                assert_eq!(g.get(mark_x + k, H / 2), reference.get(mark_x + k, H / 2));
            }
        }
    }

    #[test]
    fn tiny_and_empty_sizes_do_not_panic() {
        for (w, h) in [(0, 0), (1, 1), (3, 1), (1, 5)] {
            let g = idle(w, h, 5, DEFAULT_LOOP_FRAMES, SEED, IdleStyle::default());
            assert_eq!((g.width(), g.height()), (w, h));
        }
    }
}
