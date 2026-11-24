//! The second generation of layers: beat & pulse, electronic & glitch,
//! nature & weather, space & light. Same rules as the originals: phase-based
//! with integer cycles (seamless loops), audio scales amplitude only, subject
//! cells are never touched, and colour-only changes are quantised so just the
//! edges of an effect change from frame to frame.

use super::*;

/// Cell width / height assumed for round shapes (rings, orbits).
const ASPECT: f32 = 0.5;

#[derive(Debug, Clone)]
pub(super) struct Glitch {
    start: f32,
    len: f32,
    y0: u16,
    rows: u16,
    shift: i32,
}

/// A lightning bolt: (x, y, glyph) cells from top to bottom.
pub(super) type Bolt = Vec<(u16, u16, char)>;

#[derive(Debug, Clone)]
pub(super) enum Extra {
    PulseRings {
        rings: u32,
        cycles: i32,
        width: f32,
        cx: f32,
        cy: f32,
        max_r: f32,
    },
    SubjectGlow {
        cells: Vec<(usize, f32)>,
        cycles: i32,
    },
    EqBars {
        bars: Vec<(u16, usize, f32)>,
        height: f32,
        cycles: i32,
        color: Rgb,
    },
    Scanlines {
        cycles: i32,
        thickness: f32,
        color: Rgb,
    },
    RowGlitch {
        events: Vec<Glitch>,
    },
    CodeRain {
        cols: Vec<(u16, f32, i32, u16)>,
        glyph_seed: u64,
        color: Rgb,
    },
    ChromaShift {
        cycles: i32,
        offset: i32,
        band: f32,
    },
    Lightning {
        strikes: Vec<(f32, Bolt)>,
        duration: f32,
        color: Rgb,
    },
    Aurora {
        bands: Vec<(f32, f32, f32, i32, f32)>,
        color: Rgb,
    },
    SunRays {
        rays: u32,
        cycles: i32,
        ox: f32,
        oy: f32,
        color: Rgb,
    },
    Starfield {
        stars: Vec<(f32, f32, i32)>,
        cx: f32,
        cy: f32,
        max_r: f32,
        color: Rgb,
    },
    Twinkle {
        stars: Vec<(usize, u32, f32)>,
        color: Rgb,
    },
    Orbit {
        items: Vec<(f32, i32, f32, usize)>,
        cx: f32,
        cy: f32,
        ry: f32,
        color: Rgb,
    },
    LightSweep {
        cycles: i32,
        width: f32,
        color: Rgb,
    },
}

/// Which draw pass: 0 resamples the base, 1 tints, 2 draws on top.
pub(super) fn pass(e: &Extra) -> usize {
    match e {
        Extra::RowGlitch { .. } => 0,
        Extra::CodeRain { .. }
        | Extra::Starfield { .. }
        | Extra::Twinkle { .. }
        | Extra::Orbit { .. }
        | Extra::EqBars { .. }
        | Extra::Lightning { .. } => 2,
        _ => 1,
    }
}

/// Subject centre (or the cover's centre) and a radius reaching the cover's
/// corners, in row units.
fn centre(c: &PrepCtx<'_>) -> (f32, f32, f32) {
    let w = c.w as usize;
    let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0usize);
    for (i, p) in c.protected.iter().enumerate() {
        if *p {
            sx += (i % w) as f32;
            sy += (i / w) as f32;
            n += 1;
        }
    }
    let art = c.art;
    let (cx, cy) = if n > 0 {
        (sx / n as f32, sy / n as f32)
    } else {
        (
            art.x as f32 + art.w as f32 / 2.0,
            art.y as f32 + art.h as f32 / 2.0,
        )
    };
    let max_r = ((art.w as f32 * ASPECT).powi(2) + (art.h as f32).powi(2)).sqrt() * 0.55;
    (cx, cy, max_r.max(2.0))
}

/// Distance in row units, correcting for narrow cells.
fn dist(x: f32, y: f32, cx: f32, cy: f32) -> f32 {
    (((x - cx) * ASPECT).powi(2) + (y - cy).powi(2)).sqrt()
}

/// Prepares one of the new layer kinds; `None` for the original ones.
pub(super) fn prepare(layer: &Layer, c: &PrepCtx<'_>, rng: &mut ChaCha8Rng) -> Option<Effect> {
    let art = c.art;
    let (w, h) = (c.w as f32, c.h as f32);
    let area = w * h;
    let count =
        |d: f32| ((d.clamp(0.0, 1.0) * area / CELLS_PER_PARTICLE) as usize).min(MAX_PARTICLES);
    let frame = 1.0 / c.loop_frames.max(1) as f32;
    let lightest = c.lightest();
    let effect = match *layer {
        Layer::PulseRings {
            rings,
            cycles,
            width,
            ..
        } => {
            let (cx, cy, max_r) = centre(c);
            Effect::Extra(Extra::PulseRings {
                rings: rings.clamp(1, 3),
                cycles: cycles.clamp(1, 2) as i32,
                width: width.clamp(0.6, 2.0),
                cx,
                cy,
                max_r,
            })
        }
        Layer::SubjectGlow { width, cycles, .. } => {
            let (cx, cy, _) = centre(c);
            let width = width.clamp(1, 3) as i32;
            let wi = c.w as i32;
            let mut cells = Vec::new();
            for y in art.y as i32..(art.y + art.h) as i32 {
                for x in art.x as i32..(art.x + art.w) as i32 {
                    let i = (y * wi + x) as usize;
                    if c.protected[i] {
                        continue;
                    }
                    // Near the subject: within `width` rows / 2*width columns.
                    let near = (-width..=width).any(|dy| {
                        (-2 * width..=2 * width).any(|dx| {
                            let (xx, yy) = (x + dx, y + dy);
                            xx >= 0
                                && yy >= 0
                                && xx < wi
                                && yy < c.h as i32
                                && c.protected[(yy * wi + xx) as usize]
                        })
                    });
                    if near {
                        let angle = (y as f32 - cy).atan2((x as f32 - cx) * ASPECT);
                        cells.push((i, angle / TAU + 0.5));
                    }
                }
            }
            Effect::Extra(Extra::SubjectGlow {
                cells,
                cycles: cycles.clamp(1, 3) as i32,
            })
        }
        Layer::EqBars {
            bars,
            height,
            cycles,
            ..
        } => {
            let n = bars.clamp(4, 32) as usize;
            let bars = (0..n)
                .map(|i| {
                    let x = art.x + ((i as f32 + 0.5) / n as f32 * art.w as f32) as u16;
                    (
                        x.min(art.x + art.w.saturating_sub(1)),
                        i * cw_core::BAND_COUNT / n,
                        unit(rng) * TAU,
                    )
                })
                .collect();
            Effect::Extra(Extra::EqBars {
                bars,
                height: height.clamp(0.1, 0.5),
                cycles: cycles.clamp(1, 4) as i32,
                color: c.soft(c.accent, 60.0),
            })
        }
        Layer::Scanlines {
            cycles, thickness, ..
        } => Effect::Extra(Extra::Scanlines {
            cycles: cycles.clamp(1, 4) as i32,
            thickness: thickness.clamp(0.02, 0.12),
            color: c.soft(lightest, 92.0),
        }),
        Layer::RowGlitch {
            events, max_shift, ..
        } => {
            let events = (0..events.clamp(1, 8))
                .map(|_| Glitch {
                    start: unit(rng),
                    len: frame * (2.0 + (unit(rng) * 3.0).floor()),
                    y0: art.y + (unit(rng) * art.h.max(1) as f32) as u16,
                    rows: 1 + (unit(rng) * 3.0) as u16,
                    shift: {
                        let s = 1 + (unit(rng) * max_shift.clamp(1, 6) as f32) as i32;
                        if unit(rng) < 0.5 { -s } else { s }
                    },
                })
                .collect();
            Effect::Extra(Extra::RowGlitch { events })
        }
        Layer::CodeRain {
            density,
            fall_cycles,
            ..
        } => {
            let n = ((density.clamp(0.0, 1.0) * art.w as f32 * 0.35) as usize).max(1);
            let cols = (0..n)
                .map(|_| {
                    (
                        art.x + (unit(rng) * art.w.max(1) as f32) as u16,
                        unit(rng) * h,
                        fall_cycles.clamp(1, 4) as i32 * (1 + (unit(rng) < 0.3) as i32),
                        3 + (unit(rng) * 4.0) as u16,
                    )
                })
                .collect();
            Effect::Extra(Extra::CodeRain {
                cols,
                glyph_seed: rng.next_u64(),
                color: c.soft(c.accent, 62.0),
            })
        }
        Layer::ChromaShift { cycles, offset, .. } => Effect::Extra(Extra::ChromaShift {
            cycles: cycles.clamp(1, 3) as i32,
            offset: offset.clamp(1, 3) as i32,
            band: 0.12,
        }),
        Layer::FallingLeaves {
            density,
            fall_cycles,
            drift_deg,
            ..
        } => {
            let n = count(density);
            let items = (0..n)
                .map(|i| {
                    let depth = (unit(rng) * 3.0) as usize;
                    let ky = fall_cycles.max(1) as i32;
                    Particle {
                        x0: unit(rng) * w,
                        y0: unit(rng) * h,
                        kx: slant_wraps(ky, drift_deg, c.w, c.h),
                        ky,
                        sway_amp: 1.5 + unit(rng) * 2.0,
                        sway_cycles: 2 + (unit(rng) * 2.0) as u32,
                        sway_phase: unit(rng) * TAU,
                        blink_cycles: 0,
                        blink_phase: 0.0,
                        depth,
                        rank: (i as f32 + unit(rng)) / n.max(1) as f32,
                    }
                })
                .collect();
            let warm = c
                .palette
                .get(c.palette.len() / 2)
                .copied()
                .unwrap_or(c.accent);
            Effect::Particles(Particles {
                kind: ParticleKind::Snow,
                items,
                density: 0.75,
                glyphs: vec!['\'', ',', '%'],
                colors: [
                    c.soft(warm, 45.0),
                    c.soft(c.accent, 58.0),
                    c.soft(c.accent, 70.0),
                ],
            })
        }
        Layer::Bubbles {
            density,
            rise_cycles,
            ..
        } => {
            let n = count(density * 0.7);
            let items = (0..n)
                .map(|i| Particle {
                    x0: unit(rng) * w,
                    y0: unit(rng) * h,
                    kx: 0,
                    ky: -(rise_cycles.max(1) as i32),
                    sway_amp: 0.6 + unit(rng),
                    sway_cycles: 1 + (unit(rng) * 3.0) as u32,
                    sway_phase: unit(rng) * TAU,
                    blink_cycles: 0,
                    blink_phase: 0.0,
                    depth: (unit(rng) * 3.0) as usize,
                    rank: (i as f32 + unit(rng)) / n.max(1) as f32,
                })
                .collect();
            Effect::Particles(Particles {
                kind: ParticleKind::Snow,
                items,
                density: 0.75,
                glyphs: vec!['.', 'o', 'O'],
                colors: [
                    c.soft(lightest, 45.0),
                    c.soft(lightest, 62.0),
                    c.soft(lightest, 78.0),
                ],
            })
        }
        Layer::Smoke {
            density,
            rise_cycles,
            ..
        } => {
            let (cx, _, _) = centre(c);
            let n = count(density * 0.8);
            let items = (0..n)
                .map(|i| Particle {
                    x0: cx + (unit(rng) - 0.5) * art.w as f32 * 0.8,
                    y0: unit(rng) * h,
                    kx: 0,
                    ky: -(rise_cycles.max(1) as i32),
                    sway_amp: 2.0 + unit(rng) * 2.0,
                    sway_cycles: 1 + (unit(rng) * 2.0) as u32,
                    sway_phase: unit(rng) * TAU,
                    blink_cycles: 0,
                    blink_phase: 0.0,
                    depth: (unit(rng) * 3.0) as usize,
                    rank: (i as f32 + unit(rng)) / n.max(1) as f32,
                })
                .collect();
            let grey = c.soft(c.palette[0], 40.0);
            Effect::Particles(Particles {
                kind: ParticleKind::Snow,
                items,
                density: 0.75,
                glyphs: vec!['.', '~', '~'],
                colors: [grey, c.soft(grey, 48.0), c.soft(grey, 56.0)],
            })
        }
        Layer::Lightning { strikes, .. } => {
            let strikes = (0..strikes.clamp(1, 4))
                .map(|_| {
                    let start = unit(rng);
                    let mut x = art.x as i32 + (unit(rng) * art.w.max(1) as f32) as i32;
                    let len = ((0.4 + unit(rng) * 0.4) * art.h as f32) as u16;
                    let mut path = Vec::new();
                    for y in art.y..(art.y + len).min(art.y + art.h) {
                        let step = (unit(rng) * 3.0) as i32 - 1;
                        let glyph = match step {
                            -1 => '/',
                            1 => '\\',
                            _ => '|',
                        };
                        x = (x + step).clamp(art.x as i32, (art.x + art.w).max(1) as i32 - 1);
                        path.push((x as u16, y, glyph));
                    }
                    (start, path)
                })
                .collect();
            Effect::Extra(Extra::Lightning {
                strikes,
                duration: frame * 3.0,
                color: c.soft(lightest, 95.0),
            })
        }
        Layer::Aurora { bands, cycles, .. } => {
            let bands = (0..bands.clamp(1, 3))
                .map(|b| {
                    (
                        0.12 + 0.12 * b as f32,
                        1.0 + unit(rng) * 2.0,
                        18.0 + unit(rng) * 20.0,
                        cycles.clamp(1, 2) as i32 * if b % 2 == 0 { 1 } else { -1 },
                        unit(rng) * TAU,
                    )
                })
                .collect();
            let chroma = c
                .palette
                .iter()
                .copied()
                .max_by(|a, b| Lab::from(*a).chroma().total_cmp(&Lab::from(*b).chroma()))
                .unwrap_or(c.accent);
            Effect::Extra(Extra::Aurora {
                bands,
                color: c.soft(chroma, 62.0),
            })
        }
        Layer::SunRays { rays, cycles, .. } => {
            let left = unit(rng) < 0.5;
            Effect::Extra(Extra::SunRays {
                rays: rays.clamp(3, 9),
                cycles: cycles.clamp(1, 2) as i32,
                ox: if left {
                    art.x as f32 - 4.0
                } else {
                    (art.x + art.w) as f32 + 4.0
                },
                oy: art.y as f32 - 3.0,
                color: c.soft(lightest, 88.0),
            })
        }
        Layer::Starfield {
            density, cycles, ..
        } => {
            let (cx, cy, max_r) = centre(c);
            let n = count(density);
            let stars = (0..n)
                .map(|_| {
                    (
                        unit(rng) * TAU,
                        unit(rng),
                        cycles.clamp(1, 2) as i32 * (1 + (unit(rng) < 0.3) as i32),
                    )
                })
                .collect();
            Effect::Extra(Extra::Starfield {
                stars,
                cx,
                cy,
                max_r: max_r * 1.6,
                color: c.soft(lightest, 80.0),
            })
        }
        Layer::Twinkle {
            density, cycles, ..
        } => {
            // Dark, unprotected cells only, so stars never sit on detail.
            let mut dark: Vec<usize> = (0..c.protected.len())
                .filter(|&i| !c.protected[i] && c.density[i] < 0.3)
                .collect();
            let want = count(density).min(dark.len());
            for k in 0..want {
                let j = k + (rng.next_u64() % (dark.len() - k) as u64) as usize;
                dark.swap(k, j);
            }
            dark.truncate(want);
            let stars = dark
                .into_iter()
                .map(|i| {
                    (
                        i,
                        cycles.clamp(1, 4) + (unit(rng) * 3.0) as u32,
                        unit(rng) * TAU,
                    )
                })
                .collect();
            Effect::Extra(Extra::Twinkle {
                stars,
                color: c.soft(lightest, 85.0),
            })
        }
        Layer::Orbit {
            count: n,
            cycles,
            radius,
            ..
        } => {
            let (cx, cy, max_r) = centre(c);
            let ry = (radius.clamp(0.2, 1.0) * max_r).max(2.0);
            let items = (0..n.clamp(1, 24))
                .map(|i| {
                    (
                        unit(rng) * TAU,
                        cycles.clamp(1, 3) as i32,
                        0.85 + unit(rng) * 0.3,
                        i as usize % 3,
                    )
                })
                .collect();
            Effect::Extra(Extra::Orbit {
                items,
                cx,
                cy,
                ry,
                color: c.soft(c.accent, 72.0),
            })
        }
        Layer::LightSweep { cycles, width, .. } => Effect::Extra(Extra::LightSweep {
            cycles: cycles.clamp(1, 2) as i32,
            width: width.clamp(0.05, 0.25),
            color: c.soft(lightest, 92.0),
        }),
        _ => return None,
    };
    Some(effect)
}

/// A per-cell glyph for code rain, fixed so trails do not flicker.
fn rain_glyph(seed: u64, x: u16, y: u16) -> char {
    const G: &[char] = &['0', '1', '|', ':', ';', '!', '<', '>', '+', '=', '*'];
    let h = seed ^ ((x as u64) << 32 | y as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    G[(h >> 59) as usize % G.len()]
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    scene: &Scene,
    e: &Extra,
    amp: f32,
    phase: f32,
    audio: &AudioFrame,
    grid: &mut CellGrid,
    owner: &mut [u8],
    id: u8,
) {
    let (w, h) = (scene.width() as usize, scene.height() as usize);
    let art = scene.art;
    let accent = scene.accent;
    let frac = |v: f32| v.rem_euclid(1.0);
    match e {
        Extra::PulseRings {
            rings,
            cycles,
            width,
            cx,
            cy,
            max_r,
        } => {
            let radii: Vec<f32> = (0..*rings)
                .map(|j| frac(j as f32 / *rings as f32 + *cycles as f32 * phase) * max_r)
                .collect();
            let boost = 1.0 + 0.5 * audio.onset.clamp(0.0, 1.0);
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    if scene.protected[i] {
                        continue;
                    }
                    let d = dist(x as f32, y as f32, *cx, *cy);
                    let Some(r) = radii.iter().find(|r| (d - **r).abs() < width / 2.0) else {
                        continue;
                    };
                    // Fades in three steps as the ring grows.
                    let level = ((1.0 - r / max_r) * 3.0).ceil() / 3.0;
                    let mix = (0.45 * amp * boost * level).min(0.8);
                    if mix < 0.05 {
                        continue;
                    }
                    let mut cell = grid.cells()[i];
                    if cell.ch == ' ' {
                        cell.ch = '.';
                        cell.fg = scene.background.lerp(accent, mix + 0.2);
                    } else {
                        cell.fg = cell.fg.lerp(accent, mix);
                    }
                    set(scene, grid, owner, i, cell, id);
                }
            }
        }
        Extra::SubjectGlow { cells, cycles } => {
            // A highlight travelling around the subject: only its edges move.
            let mix = (0.3 * amp).min(0.5);
            for &(i, angle) in cells {
                let s = (TAU * (angle - *cycles as f32 * phase)).sin();
                if s < 0.3 {
                    continue;
                }
                let cell = grid.cells()[i];
                if cell.ch == ' ' {
                    continue;
                }
                let level = if s > 0.75 { 1.0 } else { 0.5 };
                set(
                    scene,
                    grid,
                    owner,
                    i,
                    Cell {
                        fg: cell.fg.lerp(accent, mix * level),
                        ..cell
                    },
                    id,
                );
            }
        }
        Extra::EqBars {
            bars,
            height,
            cycles,
            color,
        } => {
            let bottom = (art.y + art.h) as usize;
            for (k, &(x, band, ph)) in bars.iter().enumerate() {
                let level = audio.bands[band.min(cw_core::BAND_COUNT - 1)].clamp(0.0, 1.0);
                let wave =
                    0.55 + 0.45 * (TAU * (*cycles as f32 * phase) + ph + k as f32 * 0.4).sin();
                let rows = (height * art.h as f32 * (0.3 + 0.7 * level) * wave * amp.min(1.2))
                    .round() as usize;
                for r in 0..rows.min(art.h as usize) {
                    let y = bottom - 1 - r;
                    let i = y * w + x as usize;
                    let ch = if r + 1 == rows { ':' } else { '|' };
                    let cell = grid.cells()[i];
                    set(
                        scene,
                        grid,
                        owner,
                        i,
                        Cell {
                            ch,
                            fg: *color,
                            ..cell
                        },
                        id,
                    );
                }
            }
        }
        Extra::Scanlines {
            cycles,
            thickness,
            color,
        } => {
            let span = art.h as f32 * (1.0 + 2.0 * thickness);
            let centre_row =
                art.y as f32 - art.h as f32 * thickness + frac(*cycles as f32 * phase) * span;
            let half = (art.h as f32 * thickness).max(0.6);
            let mix = (0.35 * amp).min(0.6);
            for y in art.y..art.y + art.h {
                let d = (y as f32 - centre_row).abs();
                if d > half {
                    continue;
                }
                let m = if d < half / 2.0 { mix } else { mix * 0.5 };
                for x in art.x..art.x + art.w {
                    let i = y as usize * w + x as usize;
                    let cell = grid.cells()[i];
                    if cell.ch == ' ' {
                        continue;
                    }
                    set(
                        scene,
                        grid,
                        owner,
                        i,
                        Cell {
                            fg: cell.fg.lerp(*color, m),
                            ..cell
                        },
                        id,
                    );
                }
            }
        }
        Extra::RowGlitch { events } => {
            let strength = (amp * (0.6 + 0.8 * audio.onset.clamp(0.0, 1.0))).min(1.2);
            for g in events {
                if frac(phase - g.start) >= g.len {
                    continue;
                }
                let shift = (g.shift as f32 * strength).round() as i32;
                if shift == 0 {
                    continue;
                }
                for y in g.y0..(g.y0 + g.rows).min(art.y + art.h) {
                    for x in art.x..art.x + art.w {
                        let i = y as usize * w + x as usize;
                        if scene.protected[i] {
                            continue;
                        }
                        let sx = (x as i32 - shift).clamp(art.x as i32, (art.x + art.w - 1) as i32);
                        let si = y as usize * w + sx as usize;
                        if scene.protected[si] {
                            continue;
                        }
                        let src = scene.base.cells()[si];
                        if src != grid.cells()[i] {
                            set(scene, grid, owner, i, src, id);
                        }
                    }
                }
            }
        }
        Extra::CodeRain {
            cols,
            glyph_seed,
            color,
        } => {
            let head_color = color.lerp(accent, 0.5 + 0.3 * audio.onset.clamp(0.0, 1.0));
            for &(x, y0, ky, trail) in cols {
                let head = (y0 + ky as f32 * h as f32 * phase).rem_euclid(h as f32) as i32;
                let trail = ((trail as f32) * amp.min(1.2)).round().max(1.0) as i32;
                for k in 0..trail {
                    let y = (head - k).rem_euclid(h as i32) as usize;
                    let i = y * w + x as usize;
                    let cell = grid.cells()[i];
                    let fg = if k == 0 {
                        head_color
                    } else {
                        color.lerp(scene.background, k as f32 / (trail + 1) as f32)
                    };
                    let ch = rain_glyph(*glyph_seed, x, y as u16);
                    set(scene, grid, owner, i, Cell { ch, fg, ..cell }, id);
                }
            }
        }
        Extra::ChromaShift {
            cycles,
            offset,
            band,
        } => {
            let centre_row = art.y as f32 + frac(*cycles as f32 * phase) * art.h as f32;
            let half = (art.h as f32 * band).max(1.0);
            let ghost = accent.lerp(scene.background, 0.35);
            for y in art.y..art.y + art.h {
                if (y as f32 - centre_row).abs() > half {
                    continue;
                }
                for x in art.x..art.x + art.w {
                    let i = y as usize * w + x as usize;
                    let sx = x as i32 - offset;
                    if sx < art.x as i32 {
                        continue;
                    }
                    let src = scene.base.cells()[y as usize * w + sx as usize];
                    let cell = grid.cells()[i];
                    // A coloured echo of the glyph to the left, where the
                    // cell itself is sparser.
                    if src.ch != ' ' && scene.density[i] < 0.4 {
                        let fg = cell
                            .bg
                            .unwrap_or(scene.background)
                            .lerp(ghost, (0.6 * amp).min(0.9));
                        set(
                            scene,
                            grid,
                            owner,
                            i,
                            Cell {
                                ch: src.ch,
                                fg,
                                ..cell
                            },
                            id,
                        );
                    }
                }
            }
        }
        Extra::Lightning {
            strikes,
            duration,
            color,
        } => {
            for (start, path) in strikes {
                if frac(phase - start) >= *duration {
                    continue;
                }
                let fg = scene.background.lerp(*color, (0.5 + 0.5 * amp).min(1.0));
                for &(x, y, ch) in path {
                    let i = y as usize * w + x as usize;
                    let cell = grid.cells()[i];
                    set(scene, grid, owner, i, Cell { ch, fg, ..cell }, id);
                }
            }
        }
        Extra::Aurora { bands, color } => {
            let mix = (0.3 * amp).min(0.5);
            for &(row, amplitude, wavelength, cycles, ph) in bands {
                for x in 0..w {
                    let top = row * h as f32
                        + amplitude
                            * (TAU * (x as f32 / wavelength + cycles as f32 * phase) + ph).sin();
                    let top = top.round() as i32;
                    for k in 0..3 {
                        let y = top + k;
                        if y < 0 || y as usize >= h {
                            continue;
                        }
                        let i = y as usize * w + x;
                        let mut cell = grid.cells()[i];
                        if cell.ch == ' ' {
                            cell.ch = if k == 0 { ':' } else { '.' };
                            cell.fg = scene.background.lerp(*color, 0.4 + mix - 0.12 * k as f32);
                        } else {
                            cell.fg = cell.fg.lerp(*color, mix * (1.0 - 0.3 * k as f32));
                        }
                        set(scene, grid, owner, i, cell, id);
                    }
                }
            }
        }
        Extra::SunRays {
            rays,
            cycles,
            ox,
            oy,
            color,
        } => {
            let mix = (0.18 * amp).min(0.3);
            for y in art.y..art.y + art.h {
                for x in art.x..art.x + art.w {
                    let i = y as usize * w + x as usize;
                    let cell = grid.cells()[i];
                    if cell.ch == ' ' {
                        continue;
                    }
                    let a = (y as f32 - oy).atan2((x as f32 - ox) * ASPECT);
                    let v = frac(
                        a / std::f32::consts::PI * *rays as f32 * 2.0 + *cycles as f32 * phase,
                    );
                    if v < 0.28 {
                        set(
                            scene,
                            grid,
                            owner,
                            i,
                            Cell {
                                fg: cell.fg.lerp(*color, mix),
                                ..cell
                            },
                            id,
                        );
                    }
                }
            }
        }
        Extra::Starfield {
            stars,
            cx,
            cy,
            max_r,
            color,
        } => {
            for &(angle, r0, k) in stars {
                let r = frac(r0 + k as f32 * phase);
                let (x, y) = (
                    cx + angle.cos() * r * max_r / ASPECT,
                    cy + angle.sin() * r * max_r,
                );
                if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                    continue;
                }
                let i = y as usize * w + x as usize;
                if scene.density[i] > 0.35 {
                    continue; // only over dark areas
                }
                let ch = if r < 0.35 {
                    '.'
                } else if r < 0.7 {
                    '+'
                } else {
                    '*'
                };
                let fg = scene
                    .background
                    .lerp(*color, (0.4 + 0.6 * r) * amp.min(1.0));
                let cell = grid.cells()[i];
                set(scene, grid, owner, i, Cell { ch, fg, ..cell }, id);
            }
        }
        Extra::Twinkle { stars, color } => {
            for &(i, cycles, ph) in stars {
                let s = (TAU * cycles as f32 * phase + ph).sin();
                let ch = if s > 0.7 {
                    '*'
                } else if s > 0.3 {
                    '.'
                } else {
                    continue;
                };
                let cell = grid.cells()[i];
                let fg = scene.background.lerp(*color, (0.5 + 0.4 * amp).min(1.0));
                set(scene, grid, owner, i, Cell { ch, fg, ..cell }, id);
            }
        }
        Extra::Orbit {
            items,
            cx,
            cy,
            ry,
            color,
        } => {
            const G: [char; 3] = ['o', '*', '.'];
            for &(a0, k, scale, depth) in items {
                let a = a0 + TAU * k as f32 * phase;
                let r = ry * scale;
                let (x, y) = (cx + a.cos() * r / ASPECT, cy + a.sin() * r * 0.45);
                if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                    continue;
                }
                let i = y as usize * w + x as usize;
                let fg = scene.background.lerp(*color, (0.55 + 0.4 * amp).min(1.0));
                let cell = grid.cells()[i];
                set(
                    scene,
                    grid,
                    owner,
                    i,
                    Cell {
                        ch: G[depth],
                        fg,
                        ..cell
                    },
                    id,
                );
            }
        }
        Extra::LightSweep {
            cycles,
            width,
            color,
        } => {
            let span = art.w as f32 * ASPECT + art.h as f32;
            let mix = (0.4 * amp).min(0.6);
            for y in art.y..art.y + art.h {
                for x in art.x..art.x + art.w {
                    let i = y as usize * w + x as usize;
                    let cell = grid.cells()[i];
                    if cell.ch == ' ' {
                        continue;
                    }
                    let t = ((x - art.x) as f32 * ASPECT + (y - art.y) as f32) / span;
                    // The band travels from (just outside) one corner across.
                    let v = frac(t * (1.0 - *width) - *cycles as f32 * phase);
                    if v < *width {
                        let level = if (v / width - 0.5).abs() < 0.25 {
                            1.0
                        } else {
                            0.5
                        };
                        set(
                            scene,
                            grid,
                            owner,
                            i,
                            Cell {
                                fg: cell.fg.lerp(*color, mix * level),
                                ..cell
                            },
                            id,
                        );
                    }
                }
            }
        }
    }
}
