//! The recipe-driven renderer (PLAN.md sections 6 and 7).
//!
//! - [`prepare`] runs once per cover, size, style or recipe change. It
//!   resamples the subject mask to cells and precomputes every layer's random
//!   choices from the cover seed (ChaCha8).
//! - [`render`] is pure: the same scene, frame and audio give the same grid.
//!   All motion is a function of `phase = (frame mod L) / L` with integer
//!   cycles, so frame `L` equals frame 0. Audio only scales amplitude.
//! - [`limit_changes`] enforces the per-frame change budget against the
//!   previous output, dropping the weakest layer's changes first.
//!
//! Draw order: displacement layers resample the base art, then field layers
//! (fog, heat, clouds, flicker, ambient) tint it, then particles. Subject
//! cells are never touched by any of them.

use crate::art::Rect;
use cw_core::recipe::{Layer, Recipe};
use cw_core::{AudioFrame, Cell, CellGrid, Lab, Rgb};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use std::f32::consts::TAU;

/// Steady changed-cell budget per frame, as a fraction of all cells. A soft
/// guide, not a design rule: it keeps motion from turning into noise.
pub const BUDGET: f32 = 0.25;
/// Steady budget for high-energy covers (energy 1.0); it rises linearly from
/// [`BUDGET`] at energy 0.7.
pub const BUDGET_HIGH: f32 = 0.40;
/// Extra budget at the instant of a beat (`onset` = 1). It decays with the
/// onset envelope, so beats can burst well past the steady budget.
pub const BEAT_BURST: f32 = 0.35;
/// The budget never exceeds this, even on a beat.
pub const BUDGET_MAX: f32 = 0.75;
/// Cells per particle at density 1.0.
const CELLS_PER_PARTICLE: f32 = 30.0;
const MAX_PARTICLES: usize = 1500;

/// Owner value for cells that show the base art.
pub const OWNER_BASE: u8 = 0;
/// Owner of beat effects (ring, thump, glitch): outranks every layer.
const OWNER_BEAT: u8 = 1;
/// Owner of the margin glow and equaliser: the first thing the change
/// limiter drops.
const OWNER_STAGE: u8 = 250;

/// Everything per-frame that is not the scene itself.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    pub frame: u32,
    pub audio: &'a AudioFrame,
    /// 0..1 multiplier on all motion, for fading a new recipe in.
    pub fade: f32,
}

/// A rendered frame plus which layer last wrote each cell (0 = base art,
/// `k + 1` = layer `k`), which the change limiter uses for priority.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub grid: CellGrid,
    pub owner: Vec<u8>,
    /// Beat strength this frame (0..1), which widens the change budget.
    pub burst: f32,
}

/// A cover prepared for rendering at one size.
#[derive(Debug, Clone)]
pub struct Scene {
    base: CellGrid,
    art: Rect,
    /// Subject cells: never changed.
    protected: Vec<bool>,
    /// Base glyph density per cell, 0 (blank) .. 1 (densest).
    density: Vec<f32>,
    loop_frames: u32,
    layers: Vec<Prepared>,
    accent: Rgb,
    /// Brightest palette colour, for beat highlights.
    light: Rgb,
    background: Rgb,
    budget: f32,
    /// Where beat rings start: the subject's centre, else the cover's, in
    /// cells.
    origin: (f32, f32),
    /// Beat rings start at the subject's edge (its equivalent radius) and
    /// reach this far (cells, with rows counted double).
    ring_start: f32,
    ring_reach: f32,
    /// The charset's non-blank glyphs, sparse to dense.
    ramp: Vec<char>,
    seed: u64,
    /// Cover energy 0..1, and the strength of the stage effects it sets.
    energy: f32,
    stage: f32,
    edges: Edges,
    palette: Vec<Rgb>,
    /// Drawn over lyrics (`lyrics::lyric_scene`): the square is mostly
    /// blank around the words, so moving light keeps travelling there.
    pub(crate) lyrics: bool,
}

impl Scene {
    pub fn width(&self) -> u16 {
        self.base.width()
    }

    pub fn height(&self) -> u16 {
        self.base.height()
    }

    pub fn loop_frames(&self) -> u32 {
        self.loop_frames
    }

    /// Maximum cells that may change per frame with no beat.
    pub fn max_changes(&self) -> usize {
        self.max_changes_on_beat(0.0)
    }

    /// Maximum cells that may change per frame at beat strength `burst`.
    pub fn max_changes_on_beat(&self, burst: f32) -> usize {
        let frac = (self.budget + BEAT_BURST * burst.clamp(0.0, 1.0)).min(BUDGET_MAX);
        (frac * self.base.cells().len() as f32).floor() as usize
    }

    pub fn is_protected(&self, x: u16, y: u16) -> bool {
        self.protected[self.idx(x, y)]
    }

    /// The layers in draw-priority order (strongest first), by kind.
    pub fn layer_kinds(&self) -> Vec<&'static str> {
        self.layers.iter().map(|l| l.kind).collect()
    }

    fn idx(&self, x: u16, y: u16) -> usize {
        y as usize * self.base.width() as usize + x as usize
    }
}

#[derive(Debug, Clone)]
struct Prepared {
    kind: &'static str,
    intensity: f32,
    effect: Effect,
}

#[derive(Debug, Clone)]
enum Effect {
    Particles(Particles),
    Displace(Displace),
    Field(Field),
    Extra(extra::Extra),
    Special(Special),
}

/// Layers with their own drawing rules (recipe version 2).
#[derive(Debug, Clone)]
enum Special {
    Aurora {
        bands: Vec<AuroraBand>,
    },
    Starfield {
        stars: Vec<Star>,
        dim: Rgb,
        light: Rgb,
    },
    DigitalRain {
        columns: Vec<RainColumn>,
        head: Rgb,
        tail: Rgb,
    },
    Lightning {
        chance: f32,
        salt: u64,
    },
}

#[derive(Debug, Clone)]
struct AuroraBand {
    /// Curtain top, as a share of the screen height.
    top: f32,
    /// Vertical wave amplitude (rows) and horizontal wavelength (cells).
    amp: f32,
    wavelength: f32,
    /// Whole waves travelled per loop (keeps the loop seamless).
    k: i32,
    phase: f32,
    /// Curtain length in rows.
    length: f32,
    color: Rgb,
}

#[derive(Debug, Clone)]
struct Star {
    angle: f32,
    /// Starting distance along its ray, 0..1.
    r0: f32,
    /// Trips out per loop.
    k: i32,
}

#[derive(Debug, Clone)]
struct RainColumn {
    x: u16,
    y0: f32,
    k: i32,
    len: u16,
    salt: u64,
    /// Visibility rank, like particles.
    rank: f32,
}

/// Colours just inside each edge of the cover, for the margin glow.
#[derive(Debug, Clone, Default)]
struct Edges {
    left: Vec<Rgb>,
    right: Vec<Rgb>,
    top: Vec<Rgb>,
    bottom: Vec<Rgb>,
}

mod extra;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ParticleKind {
    Snow,
    Rain,
    Embers,
    Dust,
    Fireflies,
}

#[derive(Debug, Clone)]
struct Particles {
    kind: ParticleKind,
    items: Vec<Particle>,
    /// Fraction of `items` visible at mid-level audio.
    density: f32,
    glyphs: Vec<char>,
    colors: [Rgb; 3],
}

#[derive(Debug, Clone)]
struct Particle {
    x0: f32,
    y0: f32,
    /// Whole-screen wraps per loop along each axis (keeps the loop seamless).
    kx: i32,
    ky: i32,
    sway_amp: f32,
    sway_cycles: u32,
    sway_phase: f32,
    blink_cycles: u32,
    blink_phase: f32,
    depth: usize,
    /// Visibility rank: shown when `rank < density`.
    rank: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DisplaceKind {
    Ripple,
    RowSway,
    Shimmer,
}

#[derive(Debug, Clone)]
struct Displace {
    kind: DisplaceKind,
    amplitude: f32,
    wavelength: f32,
    cycles: i32,
    phase: f32,
}

#[derive(Debug, Clone)]
enum Field {
    Fog {
        density: f32,
        kx: i32,
        ky: i32,
        phases: [f32; 2],
        color: Rgb,
    },
    Heat {
        scale: f32,
        cycles: i32,
        phases: [f32; 2],
        colors: Vec<Rgb>,
    },
    Clouds {
        bands: Vec<(f32, f32, i32, f32)>, // (centre row 0..1, wavelength, cycles, phase)
        softness: f32,
        color: Rgb,
    },
    Flicker {
        cells: Vec<usize>,
        cycles: Vec<u32>,
        phases: Vec<f32>,
        neon: bool,
    },
    Ambient {
        cycles: i32,
        wavelength: f32,
    },
}

/// Prepares `recipe` over `base` (the static art from
/// [`crate::base_art`]) whose cover occupies `art`. `ramp` is the charset the
/// base art was drawn with.
pub fn prepare(recipe: &Recipe, base: CellGrid, art: Rect, ramp: &[char]) -> Scene {
    let (w, h) = (base.width(), base.height());
    let n = w as usize * h as usize;
    let background = base.cells().first().and_then(|c| c.bg).unwrap_or_default();

    // Subject mask: nearest mask pixel for each cell of the cover.
    let mut protected = vec![false; n];
    let mask = &recipe.subject_mask;
    if mask.width > 0 && mask.height > 0 && art.w > 0 && art.h > 0 {
        for y in art.y..art.y + art.h {
            for x in art.x..art.x + art.w {
                let mx = ((x - art.x) as u32 * mask.width + mask.width / 2) / art.w as u32;
                let my = ((y - art.y) as u32 * mask.height + mask.height / 2) / art.h as u32;
                if mask.get(mx.min(mask.width - 1), my.min(mask.height - 1)) {
                    protected[y as usize * w as usize + x as usize] = true;
                }
            }
        }
    }
    let steps = (ramp.len().max(2) - 1) as f32;
    let density: Vec<f32> = base
        .cells()
        .iter()
        .map(|c| {
            ramp.iter()
                .position(|r| *r == c.ch)
                .map_or(0.5, |i| i as f32 / steps)
        })
        .collect();

    let palette: Vec<Rgb> = recipe.palette.iter().filter_map(|h| parse_hex(h)).collect();
    let accent = parse_hex(&recipe.accent).unwrap_or(Rgb(214, 170, 110));
    let palette = if palette.is_empty() {
        vec![accent]
    } else {
        palette
    };
    let energy = recipe.features.energy.clamp(0.0, 1.0);
    let budget = BUDGET + (BUDGET_HIGH - BUDGET) * ((energy - 0.7) / 0.3).clamp(0.0, 1.0);

    let ctx = PrepCtx {
        w,
        h,
        art,
        palette: &palette,
        accent,
        background,
        speed_sway: recipe.global.sway,
        protected: &protected,
        density: &density,
        loop_frames: recipe.global.loop_frames.max(1),
    };
    let layers = recipe
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let salt = 0xA076_1D64_78BD_642Fu64.wrapping_mul(i as u64 + 1);
            let mut rng = ChaCha8Rng::seed_from_u64(recipe.seed ^ salt);
            prepare_layer(l, &ctx, &mut rng, &base)
        })
        .collect();

    // Beat rings start from the subject, so they read as coming from the
    // art itself (Apple's "anchor motion to its source").
    let (mut sx, mut sy, mut sn) = (0.0f32, 0.0f32, 0usize);
    for (i, _) in protected.iter().enumerate().filter(|(_, p)| **p) {
        sx += (i % w as usize) as f32 + 0.5;
        sy += (i / w as usize) as f32 + 0.5;
        sn += 1;
    }
    let origin = if sn > 0 {
        (sx / sn as f32, sy / sn as f32)
    } else {
        (
            art.x as f32 + art.w as f32 / 2.0,
            art.y as f32 + art.h as f32 / 2.0,
        )
    };
    let corners = [
        (art.x as f32, art.y as f32),
        ((art.x + art.w) as f32, art.y as f32),
        (art.x as f32, (art.y + art.h) as f32),
        ((art.x + art.w) as f32, (art.y + art.h) as f32),
    ];
    let ring_reach = corners
        .iter()
        .map(|&(x, y)| ring_dist(origin, x, y))
        .fold(1.0f32, f32::max);
    // Radius of a disc with the subject's area, in ring units (rows count
    // double, so the area does too).
    let ring_start = (2.0 * sn as f32 / std::f32::consts::PI)
        .sqrt()
        .min(ring_reach * 0.8);
    let light = {
        let lab = Lab::from(ctx.lightest());
        Rgb::from(Lab {
            l: lab.l.max(78.0),
            ..lab
        })
    };
    let edges = edge_colors(&base, art, accent);

    Scene {
        base,
        art,
        protected,
        density,
        loop_frames: recipe.global.loop_frames.max(1),
        layers,
        accent,
        light,
        background,
        budget,
        origin,
        ring_start,
        ring_reach,
        ramp: ramp.iter().copied().filter(|c| *c != ' ').collect(),
        seed: recipe.seed,
        energy,
        stage: 0.6 + 0.6 * energy,
        edges,
        palette: palette.clone(),
        lyrics: false,
    }
}

/// Mean colour of the visible glyphs in the three cells just inside each
/// edge of the cover, per row (left/right) or column (top/bottom).
fn edge_colors(base: &CellGrid, art: Rect, fallback: Rgb) -> Edges {
    if art.w == 0 || art.h == 0 {
        return Edges::default();
    }
    let mean = |cells: &mut dyn Iterator<Item = Cell>| {
        let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
        for c in cells.filter(|c| c.ch != ' ') {
            r += c.fg.0 as u32;
            g += c.fg.1 as u32;
            b += c.fg.2 as u32;
            n += 1;
        }
        match (r.checked_div(n), g.checked_div(n), b.checked_div(n)) {
            (Some(r), Some(g), Some(b)) => Rgb(r as u8, g as u8, b as u8),
            _ => fallback,
        }
    };
    let cell = |x: u16, y: u16| base.get(x, y).copied().unwrap_or_default();
    let depth = 3.min(art.w).min(art.h);
    let (x1, y1) = (art.x + art.w - 1, art.y + art.h - 1);
    Edges {
        left: (art.y..=y1)
            .map(|y| mean(&mut (0..depth).map(|d| cell(art.x + d, y))))
            .collect(),
        right: (art.y..=y1)
            .map(|y| mean(&mut (0..depth).map(|d| cell(x1 - d, y))))
            .collect(),
        top: (art.x..=x1)
            .map(|x| mean(&mut (0..depth).map(|d| cell(x, art.y + d))))
            .collect(),
        bottom: (art.x..=x1)
            .map(|x| mean(&mut (0..depth).map(|d| cell(x, y1 - d))))
            .collect(),
    }
}

/// Distance from `o` to cell position (x, y), counting rows double because
/// terminal cells are about twice as tall as they are wide.
fn ring_dist(o: (f32, f32), x: f32, y: f32) -> f32 {
    let (dx, dy) = (x - o.0, 2.0 * (y - o.1));
    (dx * dx + dy * dy).sqrt()
}

struct PrepCtx<'a> {
    w: u16,
    h: u16,
    art: Rect,
    palette: &'a [Rgb],
    accent: Rgb,
    background: Rgb,
    speed_sway: f32,
    protected: &'a [bool],
    density: &'a [f32],
    loop_frames: u32,
}

impl PrepCtx<'_> {
    /// The palette colour with the highest / lowest lightness.
    fn lightest(&self) -> Rgb {
        *self
            .palette
            .iter()
            .max_by(|a, b| Lab::from(**a).l.total_cmp(&Lab::from(**b).l))
            .unwrap_or(&self.accent)
    }

    /// A colour lifted to read as a soft glyph on the dark background.
    fn soft(&self, c: Rgb, l: f32) -> Rgb {
        let lab = Lab::from(c);
        Rgb::from(Lab { l, ..lab })
    }
}

fn unit(rng: &mut ChaCha8Rng) -> f32 {
    (rng.next_u64() >> 40) as f32 / (1u64 << 24) as f32
}

/// Integer screen wraps per loop along x for something moving `ky` wraps
/// along y at `angle_deg` (90 = straight down), on a `w x h` grid.
fn slant_wraps(ky: i32, angle_deg: f32, w: u16, h: u16) -> i32 {
    let rad = angle_deg.to_radians();
    let dir_y = rad.sin();
    if dir_y.abs() < 1e-3 {
        return 0;
    }
    let cells_y = ky as f32 * h as f32;
    let cells_x = cells_y * rad.cos() / dir_y;
    (cells_x / w.max(1) as f32).round() as i32
}

fn prepare_layer(
    layer: &Layer,
    c: &PrepCtx<'_>,
    rng: &mut ChaCha8Rng,
    base: &CellGrid,
) -> Prepared {
    let area = c.w as f32 * c.h as f32;
    let count = |density: f32| {
        ((density.clamp(0.0, 1.0) * area / CELLS_PER_PARTICLE) as usize).min(MAX_PARTICLES)
    };
    let particles =
        |rng: &mut ChaCha8Rng, n: usize, f: &dyn Fn(&mut ChaCha8Rng, usize) -> Particle| {
            (0..n).map(|i| f(rng, i)).collect::<Vec<_>>()
        };
    let (w, h) = (c.w as f32, c.h as f32);
    let kind = layer.kind();
    let intensity = layer.intensity().clamp(0.0, 1.0);
    if let Some(effect) = extra::prepare(layer, c, rng) {
        return Prepared {
            kind,
            intensity,
            effect,
        };
    }
    let effect = match *layer {
        Layer::FallingParticles {
            density,
            fall_cycles,
            drift_deg,
            ref glyphs,
            ..
        } => {
            let n = count(density * 1.6);
            let items = particles(rng, n, &|rng, i| {
                let depth = (unit(rng) * 3.0) as usize;
                let ky = fall_cycles.max(1) as i32 * (1 + (depth == 2) as i32);
                Particle {
                    x0: unit(rng) * w,
                    y0: unit(rng) * h,
                    kx: slant_wraps(ky, drift_deg, c.w, c.h),
                    ky,
                    sway_amp: 0.5 + depth as f32 * 0.4 * c.speed_sway.max(0.3),
                    sway_cycles: 1 + (unit(rng) * 2.0) as u32,
                    sway_phase: unit(rng) * TAU,
                    blink_cycles: 0,
                    blink_phase: 0.0,
                    depth,
                    rank: (i as f32 + unit(rng)) / n.max(1) as f32,
                }
            });
            let light = c.lightest();
            let g: Vec<char> = glyphs.chars().filter(|g| !g.is_whitespace()).collect();
            Effect::Particles(Particles {
                kind: ParticleKind::Snow,
                items,
                density: 0.75,
                glyphs: if g.is_empty() { vec!['.', '*', '+'] } else { g },
                colors: [
                    c.soft(light, 45.0),
                    c.soft(light, 65.0),
                    c.soft(light, 85.0),
                ],
            })
        }
        Layer::Rain {
            density,
            angle_deg,
            fall_cycles,
            ..
        } => {
            let n = count(density * 1.4);
            let items = particles(rng, n, &|rng, i| {
                let depth = (unit(rng) * 3.0) as usize;
                let ky = fall_cycles.max(1) as i32 * (1 + depth as i32 / 2);
                Particle {
                    x0: unit(rng) * w,
                    y0: unit(rng) * h,
                    kx: slant_wraps(ky, angle_deg, c.w, c.h),
                    ky,
                    sway_amp: 0.0,
                    sway_cycles: 0,
                    sway_phase: 0.0,
                    blink_cycles: 0,
                    blink_phase: 0.0,
                    depth,
                    rank: (i as f32 + unit(rng)) / n.max(1) as f32,
                }
            });
            let tilt = angle_deg - 90.0;
            let glyph = if tilt.abs() < 12.0 {
                '|'
            } else if tilt > 0.0 {
                '/'
            } else {
                '\\'
            };
            let cool = c.soft(*c.palette.last().unwrap_or(&c.accent), 55.0);
            Effect::Particles(Particles {
                kind: ParticleKind::Rain,
                items,
                density: 0.75,
                glyphs: vec![glyph],
                colors: [c.soft(cool, 35.0), c.soft(cool, 48.0), c.soft(cool, 62.0)],
            })
        }
        Layer::Embers {
            density,
            rise_cycles,
            ..
        } => {
            let n = count(density);
            let items = particles(rng, n, &|rng, i| {
                let depth = (unit(rng) * 3.0) as usize;
                Particle {
                    x0: c.art.x as f32 + unit(rng) * c.art.w.max(1) as f32,
                    y0: unit(rng) * h,
                    kx: 0,
                    ky: -(rise_cycles.max(1) as i32) * (1 + (depth == 2) as i32),
                    sway_amp: 0.8 + unit(rng) * 0.8,
                    sway_cycles: 2 + (unit(rng) * 3.0) as u32,
                    sway_phase: unit(rng) * TAU,
                    blink_cycles: 4 + (unit(rng) * 6.0) as u32,
                    blink_phase: unit(rng) * TAU,
                    depth,
                    rank: (i as f32 + unit(rng)) / n.max(1) as f32,
                }
            });
            Effect::Particles(Particles {
                kind: ParticleKind::Embers,
                items,
                density: 0.75,
                glyphs: vec!['.', '\'', '*'],
                colors: [
                    c.soft(c.accent, 50.0),
                    c.soft(c.accent, 65.0),
                    c.soft(c.accent, 80.0),
                ],
            })
        }
        Layer::Dust {
            density,
            drift_deg,
            cycles,
            ..
        } => {
            let n = count(density);
            let dir = if drift_deg.to_radians().cos() < 0.0 {
                -1
            } else {
                1
            };
            let items = particles(rng, n, &|rng, i| {
                let depth = (unit(rng) * 3.0) as usize;
                Particle {
                    x0: unit(rng) * w,
                    y0: c.art.y as f32 + (0.3 + 0.7 * unit(rng)) * c.art.h.max(1) as f32,
                    kx: dir * cycles.max(1) as i32 * (1 + (depth == 2) as i32),
                    ky: 0,
                    sway_amp: 0.6,
                    sway_cycles: 1 + (unit(rng) * 2.0) as u32,
                    sway_phase: unit(rng) * TAU,
                    blink_cycles: 0,
                    blink_phase: 0.0,
                    depth,
                    rank: (i as f32 + unit(rng)) / n.max(1) as f32,
                }
            });
            let mid = c
                .palette
                .get(c.palette.len() / 2)
                .copied()
                .unwrap_or(c.accent);
            Effect::Particles(Particles {
                kind: ParticleKind::Dust,
                items,
                density: 0.75,
                glyphs: vec!['.', ',', '`'],
                colors: [c.soft(mid, 35.0), c.soft(mid, 45.0), c.soft(mid, 55.0)],
            })
        }
        Layer::Fireflies {
            count: n,
            blink_cycles,
            ..
        } => {
            let n = (n as usize).min(MAX_PARTICLES);
            let items = particles(rng, n, &|rng, i| Particle {
                x0: c.art.x as f32 + unit(rng) * c.art.w.max(1) as f32,
                y0: c.art.y as f32 + (0.25 + 0.75 * unit(rng)) * c.art.h.max(1) as f32,
                kx: 0,
                ky: 0,
                sway_amp: 1.5 + unit(rng) * 2.0,
                sway_cycles: 1,
                sway_phase: unit(rng) * TAU,
                blink_cycles: blink_cycles.max(1) + (unit(rng) * 2.0) as u32,
                blink_phase: unit(rng) * TAU,
                depth: 2,
                rank: (i as f32 + unit(rng)) / n.max(1) as f32,
            });
            Effect::Particles(Particles {
                kind: ParticleKind::Fireflies,
                items,
                density: 0.85,
                glyphs: vec!['.', '*', '*'],
                colors: [
                    c.soft(c.accent, 55.0),
                    c.soft(c.accent, 70.0),
                    c.soft(c.accent, 85.0),
                ],
            })
        }
        Layer::Ripple {
            amplitude,
            wavelength,
            cycles,
            direction_deg,
            ..
        } => {
            let dir = if direction_deg.to_radians().cos() < 0.0 {
                -1
            } else {
                1
            };
            Effect::Displace(Displace {
                kind: DisplaceKind::Ripple,
                amplitude: amplitude.clamp(0.0, 4.0),
                wavelength: wavelength.max(2.0),
                cycles: dir * cycles.max(1) as i32,
                phase: unit(rng) * TAU,
            })
        }
        Layer::RowSway {
            amplitude,
            wavelength,
            cycles,
            ..
        } => Effect::Displace(Displace {
            kind: DisplaceKind::RowSway,
            amplitude: amplitude.clamp(0.0, 3.0),
            wavelength: wavelength.max(2.0) * 3.0,
            cycles: cycles.max(1) as i32,
            phase: unit(rng) * TAU,
        }),
        Layer::HeatShimmer {
            amplitude, cycles, ..
        } => Effect::Displace(Displace {
            kind: DisplaceKind::Shimmer,
            amplitude: amplitude.clamp(0.0, 1.2),
            wavelength: 2.5,
            cycles: cycles.max(1) as i32 * 2,
            phase: unit(rng) * TAU,
        }),
        Layer::Fog {
            density,
            drift_deg,
            cycles,
            ..
        } => {
            let k = cycles.max(1) as i32;
            let kx = if drift_deg.to_radians().cos() < 0.0 {
                -k
            } else {
                k
            };
            let dim = c.palette.get(1).copied().unwrap_or(c.accent);
            Effect::Field(Field::Fog {
                density: density.clamp(0.0, 1.0),
                kx,
                ky: (unit(rng) * 2.0) as i32,
                phases: [unit(rng) * TAU, unit(rng) * TAU],
                color: c.soft(dim, 32.0),
            })
        }
        Layer::HeatCycle {
            noise_scale,
            cycles,
            ..
        } => {
            let mut warm: Vec<Rgb> = c.palette.to_vec();
            warm.push(c.accent);
            warm.sort_by(|a, b| Lab::from(*a).l.total_cmp(&Lab::from(*b).l));
            Effect::Field(Field::Heat {
                scale: noise_scale.clamp(0.02, 0.4),
                cycles: cycles.max(1) as i32,
                phases: [unit(rng) * TAU, unit(rng) * TAU],
                colors: warm,
            })
        }
        Layer::CloudDrift {
            layers,
            cycles,
            softness,
            ..
        } => {
            let bands = (0..layers.clamp(1, 3))
                .map(|b| {
                    (
                        0.12 + 0.18 * b as f32 + 0.05 * unit(rng),
                        14.0 + unit(rng) * 16.0,
                        cycles.max(1) as i32 * if b % 2 == 0 { 1 } else { 2 },
                        unit(rng) * TAU,
                    )
                })
                .collect();
            Effect::Field(Field::Clouds {
                bands,
                softness: softness.clamp(0.1, 1.0),
                color: c.soft(c.lightest(), 60.0),
            })
        }
        Layer::Flicker { rate, cells, .. } | Layer::NeonFlicker { rate, cells, .. } => {
            let neon = matches!(layer, Layer::NeonFlicker { .. });
            // Candidates: non-blank cells (bright ones for neon), in the cover.
            let mut candidates = Vec::new();
            for y in c.art.y..c.art.y + c.art.h {
                for x in c.art.x..c.art.x + c.art.w {
                    let i = y as usize * c.w as usize + x as usize;
                    let cell = base.cells()[i];
                    if cell.ch == ' ' {
                        continue;
                    }
                    if neon && Lab::from(cell.fg).l < 55.0 {
                        continue;
                    }
                    candidates.push(i);
                }
            }
            let want = ((cells.clamp(0.0, 0.08) * c.art.w as f32 * c.art.h as f32) as usize)
                .min(candidates.len());
            // Partial Fisher-Yates: a seeded sample of `want` cells.
            for k in 0..want {
                let j = k + (rng.next_u64() % (candidates.len() - k) as u64) as usize;
                candidates.swap(k, j);
            }
            candidates.truncate(want);
            let base_cycles = 1 + (rate.clamp(0.0, 1.0) * 6.0).round() as u32;
            let cycles = candidates
                .iter()
                .map(|_| base_cycles + (unit(rng) * 3.0) as u32)
                .collect();
            let phases = candidates.iter().map(|_| unit(rng) * TAU).collect();
            Effect::Field(Field::Flicker {
                cells: candidates,
                cycles,
                phases,
                neon,
            })
        }
        Layer::Aurora { bands, cycles, .. } => {
            // Curtain colours: the most saturated palette colours, lifted
            // so they glow on a dark background.
            let mut vivid: Vec<Rgb> = c.palette.to_vec();
            vivid.push(c.accent);
            vivid.sort_by(|a, b| {
                let chroma = |c: &Rgb| {
                    let l = Lab::from(*c);
                    l.a * l.a + l.b * l.b
                };
                chroma(b).total_cmp(&chroma(a))
            });
            let bands = (0..bands.clamp(1, 3))
                .map(|b| AuroraBand {
                    top: 0.04 + 0.1 * b as f32 + 0.06 * unit(rng),
                    amp: 1.0 + 2.5 * unit(rng),
                    wavelength: 18.0 + 22.0 * unit(rng),
                    k: (cycles.max(1) as i32) * if b % 2 == 0 { 1 } else { -1 },
                    phase: unit(rng) * TAU,
                    length: 3.0 + 4.0 * unit(rng),
                    color: c.soft(vivid[b as usize % vivid.len()], 68.0 + 8.0 * b as f32),
                })
                .collect();
            Effect::Special(Special::Aurora { bands })
        }
        Layer::Starfield {
            count: n, cycles, ..
        } => {
            let n = (n as usize).min(MAX_PARTICLES);
            let stars = (0..n)
                .map(|_| Star {
                    angle: unit(rng) * TAU,
                    r0: unit(rng),
                    k: cycles.max(1) as i32 * (1 + (unit(rng) * 2.0) as i32),
                })
                .collect();
            let light = c.lightest();
            Effect::Special(Special::Starfield {
                stars,
                dim: c.soft(light, 35.0),
                light: c.soft(light, 92.0),
            })
        }
        Layer::DigitalRain {
            density,
            fall_cycles,
            ..
        } => {
            let n = ((density.clamp(0.0, 1.0) * c.w as f32 * 0.45) as usize).min(MAX_PARTICLES);
            let columns = (0..n)
                .map(|i| RainColumn {
                    x: (unit(rng) * w) as u16 % c.w.max(1),
                    y0: unit(rng) * h,
                    k: fall_cycles.max(1) as i32 * (1 + (unit(rng) * 2.0) as i32),
                    len: 3 + (unit(rng) * (h * 0.35).max(1.0)) as u16,
                    salt: rng.next_u64(),
                    rank: (i as f32 + unit(rng)) / n.max(1) as f32,
                })
                .collect();
            Effect::Special(Special::DigitalRain {
                columns,
                head: c.soft(c.accent, 88.0),
                tail: c.soft(c.accent, 50.0),
            })
        }
        Layer::Lightning { chance, .. } => Effect::Special(Special::Lightning {
            chance: chance.clamp(0.0, 1.0),
            salt: rng.next_u64(),
        }),
        Layer::Ambient { cycles, .. } => Effect::Field(Field::Ambient {
            cycles: cycles.max(1) as i32,
            wavelength: 24.0 + unit(rng) * 16.0,
        }),
        // The second-generation kinds were handled by `extra::prepare`.
        _ => unreachable!("{kind} is prepared in scene::extra"),
    };
    let _ = c.background;
    Prepared {
        kind,
        intensity,
        effect,
    }
}

fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Audio gains: silence still moves, loud music moves more, and every
/// beat kicks the motion briefly on top (the onset envelope decays in about
/// 150ms). Gains only scale amplitude, never phase, so loops stay seamless.
fn gain_intensity(a: &AudioFrame) -> f32 {
    0.8 + 0.9 * a.bass().clamp(0.0, 1.0) + 0.6 * beat(a)
}
fn gain_density(a: &AudioFrame) -> f32 {
    0.5 + 1.0 * a.mid().clamp(0.0, 1.0) + 0.3 * beat(a)
}
fn gain_flicker(a: &AudioFrame) -> f32 {
    0.3 + 0.9 * a.treble().clamp(0.0, 1.0)
}
fn beat(a: &AudioFrame) -> f32 {
    a.onset.clamp(0.0, 1.0)
}

/// Ring width in cells (rows counted double), as a share of its reach.
const RING_WIDTH: f32 = 0.14;
/// Strongest tint a beat ring applies.
const RING_TINT: f32 = 0.55;
/// Furthest the bass thump pushes the background, in cells.
const THUMP_CELLS: f32 = 2.0;
/// Bands in the margin equaliser.
const BAND_COUNT_EQ: usize = 6;
/// How often digital-rain glyphs re-roll per loop.
const RAIN_GLYPH_TICKS: u32 = 80;

/// Renders one frame. Pure: no clocks, no global state.
///
/// Order: the bass thump (it resamples the base), displacement layers; then
/// the margin glow and equaliser, field layers; then the beat shockwave,
/// particles, starfield and digital rain; then lightning; and last the
/// glitch slices, which shift whatever is on screen.
pub fn render(scene: &Scene, input: Input<'_>) -> Output {
    let mut grid = scene.base.clone();
    let mut owner = vec![OWNER_BASE; grid.cells().len()];
    let l = scene.loop_frames;
    let phase = (input.frame % l) as f32 / l as f32;
    let fade = input.fade.clamp(0.0, 1.0);
    let audio = input.audio;
    let burst = beat(audio) * fade;

    for pass in 0..4 {
        match pass {
            0 => thump(scene, audio, fade, &mut grid, &mut owner),
            1 => {
                glow(scene, audio, fade, &mut grid, &mut owner);
                equaliser(scene, audio, fade, &mut grid, &mut owner);
            }
            2 => beat_ring(scene, burst, &mut grid, &mut owner),
            _ => {}
        }
        // Weakest layers draw first so stronger ones win.
        for (k, layer) in scene.layers.iter().enumerate().rev() {
            let id = k as u8 + 1;
            let amp = layer.intensity * fade * gain_intensity(audio);
            if amp <= 0.0 {
                continue;
            }
            match (&layer.effect, pass) {
                (Effect::Displace(d), 0) => {
                    displace(scene, d, amp, phase, &mut grid, &mut owner, id)
                }
                (Effect::Field(f), 1) => {
                    field(scene, f, amp, phase, audio, &mut grid, &mut owner, id)
                }
                (Effect::Special(Special::Aurora { bands }), 1) => {
                    aurora(scene, bands, amp, phase, audio, &mut grid, &mut owner, id)
                }
                (Effect::Particles(p), 2) => {
                    let dens = (p.density * gain_density(audio)).clamp(0.0, 1.0) * fade;
                    particles(scene, p, amp, dens, phase, audio, &mut grid, &mut owner, id)
                }
                (Effect::Extra(x), p) if p == extra::pass(x) => {
                    extra::draw(scene, x, amp, phase, audio, &mut grid, &mut owner, id)
                }
                (Effect::Special(Special::Starfield { stars, dim, light }), 2) => starfield(
                    scene,
                    stars,
                    (*dim, *light),
                    amp,
                    phase,
                    audio,
                    &mut grid,
                    &mut owner,
                    id,
                ),
                (
                    Effect::Special(Special::DigitalRain {
                        columns,
                        head,
                        tail,
                    }),
                    2,
                ) => {
                    let dens = (0.75 * gain_density(audio)).clamp(0.0, 1.0) * fade;
                    digital_rain(
                        scene,
                        columns,
                        (*head, *tail),
                        dens,
                        phase,
                        &mut grid,
                        &mut owner,
                        id,
                    )
                }
                (Effect::Special(Special::Lightning { chance, salt }), 3) => lightning(
                    scene, *chance, *salt, amp, audio, fade, &mut grid, &mut owner, id,
                ),
                _ => {}
            }
        }
    }
    glitch(scene, audio, fade, &mut grid, &mut owner);
    Output { grid, owner, burst }
}

/// Small deterministic hash to 0..1 (for per-beat and per-cell choices).
fn hash_unit(x: u64) -> f32 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
}

fn hash_u64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The glyph `steps` denser than `ch` in the scene's charset.
fn denser(scene: &Scene, ch: char, steps: usize) -> char {
    match scene.ramp.iter().position(|r| *r == ch) {
        Some(i) => scene.ramp[(i + steps).min(scene.ramp.len() - 1)],
        None => ch,
    }
}

/// On a beat, a shockwave of light travels outward from the subject: the
/// onset envelope jumps to 1 and decays, so the ring starts at the
/// subject's edge and expands as the hit fades. Its crest brightens glyphs
/// and packs them a step or two denser, like a pressure wave through the
/// art. Never touches the subject; silence draws nothing.
fn beat_ring(scene: &Scene, burst: f32, grid: &mut CellGrid, owner: &mut [u8]) {
    if burst < 0.05 || scene.ring_reach <= 1.0 {
        return;
    }
    let radius = scene.ring_start + (1.0 - burst) * (scene.ring_reach - scene.ring_start);
    let half = (RING_WIDTH * scene.ring_reach).max(1.5);
    let art = scene.art;
    let w = scene.width() as usize;
    // Rows the ring can touch, to skip most of the grid.
    let reach_rows = (radius + half) / 2.0;
    let y0 = (scene.origin.1 - reach_rows).floor().max(art.y as f32) as u16;
    let y1 = ((scene.origin.1 + reach_rows).ceil() as u16).min(art.y + art.h);
    for y in y0..y1 {
        for x in art.x..art.x + art.w {
            let d = ring_dist(scene.origin, x as f32 + 0.5, y as f32 + 0.5);
            let edge = 1.0 - (d - radius).abs() / half;
            if edge <= 0.0 {
                continue;
            }
            let i = y as usize * w + x as usize;
            let cell = grid.cells()[i];
            if cell.ch == ' ' {
                continue;
            }
            let fg = cell.fg.lerp(scene.light, RING_TINT * burst.sqrt() * edge);
            let crest = (edge * burst * 2.2) as usize; // 0, 1 or 2 steps
            let ch = denser(scene, cell.ch, crest);
            set(scene, grid, owner, i, Cell { ch, fg, ..cell }, OWNER_BEAT);
        }
    }
}

/// Bass thump: on kicks the cover's background is pushed outward from the
/// subject by up to two cells, like a speaker cone, then settles as the
/// onset decays.
fn thump(scene: &Scene, a: &AudioFrame, fade: f32, grid: &mut CellGrid, owner: &mut [u8]) {
    let kick = beat(a) * (0.35 + 0.65 * a.bass().clamp(0.0, 1.0)) * scene.stage * fade;
    let push = (THUMP_CELLS * kick).round();
    if push < 1.0 {
        return;
    }
    let art = scene.art;
    let w = scene.width() as usize;
    let (ox, oy) = scene.origin;
    for y in art.y..art.y + art.h {
        for x in art.x..art.x + art.w {
            let i = y as usize * w + x as usize;
            if scene.protected[i] {
                continue;
            }
            let (dx, dy) = (x as f32 + 0.5 - ox, 2.0 * (y as f32 + 0.5 - oy));
            let d = (dx * dx + dy * dy).sqrt();
            if d <= push {
                continue;
            }
            // Sample from nearer the subject: the art appears pushed out.
            let f = (d - push) / d;
            let sx = (ox + dx * f)
                .floor()
                .clamp(art.x as f32, (art.x + art.w - 1) as f32) as usize;
            let sy = (oy + dy * f / 2.0)
                .floor()
                .clamp(art.y as f32, (art.y + art.h - 1) as f32) as usize;
            let si = sy * w + sx;
            let src = if scene.protected[si] {
                scene.base.cells()[i]
            } else {
                scene.base.cells()[si]
            };
            if src != grid.cells()[i] {
                set(scene, grid, owner, i, src, OWNER_BEAT);
            }
        }
    }
}

/// Ambient glow: the empty margins beside the cover light up with the
/// cover's own edge colours, as a stippled bloom that reaches further when
/// the music is louder and swells on beats. Steps are quantised so small
/// level changes do not churn cells.
fn glow(scene: &Scene, a: &AudioFrame, fade: f32, grid: &mut CellGrid, owner: &mut [u8]) {
    let art = scene.art;
    let (w, h) = (scene.width(), scene.height());
    if art.w == 0
        || art.h == 0
        || (art.x == 0 && art.x + art.w >= w && art.y == 0 && art.y + art.h >= h)
    {
        return;
    }
    let q = |v: f32, steps: f32| (v.clamp(0.0, 1.0) * steps).round() / steps;
    let level = q(a.level, 6.0);
    let strength = (0.3 + 0.7 * level) * scene.stage.min(1.0) * fade;
    if strength < 0.05 {
        return;
    }
    let reach = 2.0 + 10.0 * level + 6.0 * q(beat(a), 3.0);
    let bg = scene.background;
    let (x1, y1) = (art.x + art.w - 1, art.y + art.h - 1);
    for y in 0..h {
        for x in 0..w {
            let inside_x = x >= art.x && x <= x1;
            let inside_y = y >= art.y && y <= y1;
            if inside_x && inside_y {
                continue;
            }
            let dx = if x < art.x {
                art.x - x
            } else {
                x.saturating_sub(x1)
            } as f32;
            let dy = if y < art.y {
                art.y - y
            } else {
                y.saturating_sub(y1)
            } as f32
                * 2.0;
            let d = (dx * dx + dy * dy).sqrt();
            let f = (-d / reach).exp() * strength;
            let i = y as usize * w as usize + x as usize;
            // Stipple: fewer glyphs the further from the cover.
            if f < 0.12 || f * 1.3 < hash_unit(i as u64 ^ 0x6C0F) {
                continue;
            }
            let ry = (y.clamp(art.y, y1) - art.y) as usize;
            let rx = (x.clamp(art.x, x1) - art.x) as usize;
            let edge = if dx >= dy {
                if x < art.x {
                    scene.edges.left[ry]
                } else {
                    scene.edges.right[ry]
                }
            } else if y < art.y {
                scene.edges.top[rx]
            } else {
                scene.edges.bottom[rx]
            };
            let ch = if f > 0.55 {
                ':'
            } else if f > 0.3 {
                '.'
            } else {
                '`'
            };
            let fg = bg.lerp(edge, (0.35 + 0.65 * f).min(1.0));
            let cell = grid.cells()[i];
            set(scene, grid, owner, i, Cell { ch, fg, ..cell }, OWNER_STAGE);
        }
    }
}

/// Margin equaliser: six bands rise from the bottom of each side margin,
/// mirrored so the bass sits next to the cover. Quiet music draws nothing.
fn equaliser(scene: &Scene, a: &AudioFrame, fade: f32, grid: &mut CellGrid, owner: &mut [u8]) {
    let art = scene.art;
    let (w, h) = (scene.width(), scene.height());
    let max_rows = (h as f32 * 0.28).floor();
    if max_rows < 2.0 || fade <= 0.0 {
        return;
    }
    let margins = [
        (0u16, art.x, true),
        (art.x + art.w, w.saturating_sub(art.x + art.w), false),
    ];
    let n = scene.palette.len().max(1);
    // Six one-cell columns with gaps, centred in the margin.
    let span = (BAND_COUNT_EQ * 2 - 1) as u16;
    for (start, width, left) in margins {
        if width < span + 2 {
            continue;
        }
        let x0 = start + (width - span) / 2;
        for b in 0..BAND_COUNT_EQ {
            let v = (a.bands[b].clamp(0.0, 1.0) * fade).powf(1.3);
            let rows = (v * max_rows).round() as u16;
            // Bass next to the cover on both sides.
            let slot = if left { BAND_COUNT_EQ - 1 - b } else { b } as u16;
            let x = x0 + slot * 2;
            let color = scene.palette[b % n];
            for r in 0..rows {
                let y = h - 1 - r;
                let t = r as f32 / max_rows; // 0 at the floor
                let ch = if t < 0.35 {
                    '#'
                } else if t < 0.6 {
                    '='
                } else if t < 0.8 {
                    '-'
                } else {
                    '.'
                };
                let lab = Lab::from(color);
                let fg = Rgb::from(Lab {
                    l: (lab.l.max(45.0) + 25.0 * (1.0 - t)).min(90.0),
                    ..lab
                });
                let i = y as usize * w as usize + x as usize;
                let cell = grid.cells()[i];
                set(scene, grid, owner, i, Cell { ch, fg, ..cell }, OWNER_STAGE);
            }
        }
    }
}

/// Glitch: on the hardest hits of energetic covers, two or three thin
/// slices of the background jump sideways in the accent colour for the
/// couple of frames the onset stays high. Which hits glitch, and where, is
/// decided by the seed and the beat count, so it is pure.
fn glitch(scene: &Scene, a: &AudioFrame, fade: f32, grid: &mut CellGrid, owner: &mut [u8]) {
    let art = scene.art;
    if beat(a) < 0.8 || a.level < 0.45 || fade < 0.5 || art.w < 8 || art.h < 4 {
        return;
    }
    let h = hash_u64(scene.seed ^ a.onset_count.wrapping_mul(0xA24B_AED4_963E_E407));
    let chance = 0.15 + 0.5 * scene.energy;
    if hash_unit(h) > chance {
        return;
    }
    let w = scene.width() as usize;
    for s in 0..2 + (h % 2) {
        let hs = hash_u64(h ^ (s + 1));
        let y0 = art.y + (hs % art.h as u64) as u16;
        let rows = 1 + ((hs >> 8) % 2) as u16;
        let shift = (2 + ((hs >> 16) % 5) as i32) * if hs & (1 << 40) == 0 { 1 } else { -1 };
        for y in y0..(y0 + rows).min(art.y + art.h) {
            let row: Vec<Cell> = (art.x..art.x + art.w)
                .map(|x| grid.cells()[y as usize * w + x as usize])
                .collect();
            for x in art.x..art.x + art.w {
                let i = y as usize * w + x as usize;
                let src = (x as i32 - art.x as i32 - shift).rem_euclid(art.w as i32) as usize;
                let mut cell = row[src];
                cell.fg = cell.fg.lerp(scene.accent, 0.35);
                set(scene, grid, owner, i, cell, OWNER_BEAT);
            }
        }
    }
}

/// Aurora: curtains hang from wavy tops across the upper screen, folding
/// and shimmering as they travel. They paint over sky and darkness, never
/// over detailed parts of the art, and brighten with the mids.
#[allow(clippy::too_many_arguments)]
fn aurora(
    scene: &Scene,
    bands: &[AuroraBand],
    amp: f32,
    phase: f32,
    a: &AudioFrame,
    grid: &mut CellGrid,
    owner: &mut [u8],
    id: u8,
) {
    let (w, h) = (scene.width(), scene.height());
    let wi = w as usize;
    let lift = 0.6 + 0.6 * a.mid().clamp(0.0, 1.0);
    for band in bands {
        for x in 0..w {
            let xf = x as f32;
            let top = band.top * h as f32
                + band.amp
                    * (TAU * (xf / band.wavelength - band.k as f32 * phase) + band.phase).sin();
            // Vertical folds: bright where the curtain turns toward us.
            let fold = (TAU * (xf / (band.wavelength * 0.37) + 2.0 * band.k as f32 * phase)
                + band.phase * 1.7)
                .cos()
                .abs();
            let b = amp.min(1.6) * lift * (0.35 + 0.65 * fold);
            if b < 0.3 {
                continue;
            }
            let len = (band.length * (0.6 + 0.4 * fold)).max(1.0);
            for r in 0..len.ceil() as u16 {
                let y = top.round() as i32 + r as i32;
                if y < 0 || y >= h as i32 {
                    continue;
                }
                let strength = b * (1.0 - r as f32 / len);
                if strength < 0.2 {
                    break;
                }
                let i = y as usize * wi + x as usize;
                if scene.density[i] > 0.55 {
                    continue; // detail stays in front of the sky
                }
                let cell = grid.cells()[i];
                let ch = if cell.ch != ' ' && scene.density[i] > 0.3 {
                    cell.ch
                } else if r == 0 {
                    '\''
                } else if strength > 0.7 {
                    '|'
                } else if strength > 0.45 {
                    ':'
                } else {
                    '.'
                };
                let fg = cell.fg.lerp(band.color, (0.3 + 0.55 * strength).min(0.9));
                set(scene, grid, owner, i, Cell { ch, fg, ..cell }, id);
            }
        }
    }
}

/// Starfield warp: stars fly outward along rays from the subject,
/// accelerating as they go. Streaks lengthen with loudness. Stars only
/// show over dark cells and the margins.
#[allow(clippy::too_many_arguments)]
fn starfield(
    scene: &Scene,
    stars: &[Star],
    (dim, light): (Rgb, Rgb),
    amp: f32,
    phase: f32,
    a: &AudioFrame,
    grid: &mut CellGrid,
    owner: &mut [u8],
    id: u8,
) {
    let (w, h) = (scene.width() as f32, scene.height() as f32);
    let wi = scene.width() as usize;
    let (ox, oy) = scene.origin;
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
    let far = corners
        .iter()
        .map(|&(x, y)| ring_dist(scene.origin, x, y))
        .fold(1.0f32, f32::max);
    let near = scene.ring_start.min(far * 0.5);
    let level = a.level.clamp(0.0, 1.0);
    for star in stars {
        let r = (star.r0 + star.k as f32 * phase).fract();
        let d = near + r * r * (far - near);
        let (c, s) = (star.angle.cos(), star.angle.sin());
        // Rows count double in ring units, so halve the vertical step.
        let step = |t: f32| (ox + c * t, oy + s * t / 2.0);
        let streak = (1.0 + r * (1.5 + 4.0 * level) * amp.min(1.5)).round() as i32;
        let trail = if (c * s).abs() < 0.2 {
            if c.abs() > s.abs() { '-' } else { '|' }
        } else if c * s > 0.0 {
            '\\'
        } else {
            '/'
        };
        for t in 0..streak {
            let (x, y) = step(d - t as f32);
            if x < 0.0 || y < 0.0 || x >= w || y >= h {
                continue;
            }
            let i = y as usize * wi + x as usize;
            if scene.protected[i] || scene.density[i] > 0.35 {
                continue;
            }
            let ch = if t > 0 {
                trail
            } else if r < 0.35 {
                '.'
            } else if r < 0.75 {
                '+'
            } else {
                '*'
            };
            let fade_t = 1.0 - t as f32 / streak.max(1) as f32;
            let fg = dim.lerp(light, (0.25 + 0.75 * r) * fade_t);
            let cell = grid.cells()[i];
            set(scene, grid, owner, i, Cell { ch, fg, ..cell }, id);
        }
    }
}

/// Digital rain: columns of the cover's own glyphs cascade down, a bright
/// head in the accent and a fading tail. The glyphs re-roll a few times a
/// second (on the loop's clock, so the loop stays seamless). Shows over dark
/// cells and the margins only.
#[allow(clippy::too_many_arguments)]
fn digital_rain(
    scene: &Scene,
    columns: &[RainColumn],
    (head, tail): (Rgb, Rgb),
    density: f32,
    phase: f32,
    grid: &mut CellGrid,
    owner: &mut [u8],
    id: u8,
) {
    if scene.ramp.is_empty() {
        return;
    }
    let (w, h) = (scene.width() as usize, scene.height() as f32);
    let bg = scene.background;
    let tick = (phase * RAIN_GLYPH_TICKS as f32) as u64;
    for col in columns {
        if col.rank >= density || col.x as usize >= w {
            continue;
        }
        let y_head = (col.y0 + col.k as f32 * h * phase).rem_euclid(h);
        for t in 0..col.len {
            let y = (y_head - t as f32).rem_euclid(h) as usize;
            let i = y * w + col.x as usize;
            if scene.protected[i] || scene.density[i] > 0.4 {
                continue;
            }
            let pick = hash_u64(col.salt ^ ((y as u64) << 20) ^ tick);
            // Denser half of the charset reads as "code".
            let half = scene.ramp.len() / 2;
            let ch = scene.ramp[half + (pick as usize) % (scene.ramp.len() - half)];
            let fg = if t == 0 {
                head
            } else {
                tail.lerp(bg, t as f32 / col.len as f32)
            };
            let cell = grid.cells()[i];
            set(scene, grid, owner, i, Cell { ch, fg, ..cell }, id);
        }
    }
}

/// Lightning: on some strong beats a jagged bolt drops from the top of the
/// cover toward the subject, with one fork, fading with the onset. Which
/// beats strike is decided by the seed and the beat count.
#[allow(clippy::too_many_arguments)]
fn lightning(
    scene: &Scene,
    chance: f32,
    salt: u64,
    amp: f32,
    a: &AudioFrame,
    fade: f32,
    grid: &mut CellGrid,
    owner: &mut [u8],
    id: u8,
) {
    let art = scene.art;
    let flash = beat(a) * fade;
    if flash < 0.35 || art.w < 4 || art.h < 4 {
        return;
    }
    let hb = hash_u64(salt ^ scene.seed ^ a.onset_count.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    if hash_unit(hb) > chance {
        return;
    }
    let w = scene.width() as usize;
    let fg = scene
        .background
        .lerp(scene.light, (0.4 + 0.6 * flash) * amp.clamp(0.6, 1.0));
    let mut bolt = |start_x: i32, start_y: u16, rows: u16, bias: i32, seed: u64| {
        let mut x = start_x;
        for step in 0..rows {
            let y = start_y + step;
            if y >= art.y + art.h {
                break;
            }
            let r = (hash_u64(seed ^ step as u64) % 3) as i32 - 1;
            let toward = (scene.origin.0 as i32 - x).signum();
            // Forks lean away; the main bolt leans toward the subject on
            // about half of its sideways steps.
            let dir = if r == 0 {
                0
            } else if bias != 0 {
                bias
            } else if (seed >> (step % 60)) & 1 == 0 && toward != 0 {
                toward
            } else {
                r
            };
            x = (x + dir).clamp(art.x as i32, (art.x + art.w - 1) as i32);
            let i = y as usize * w + x as usize;
            if scene.protected[i] {
                break; // the bolt strikes the subject's edge
            }
            let ch = match dir {
                -1 => '/',
                1 => '\\',
                _ => '|',
            };
            let cell = grid.cells()[i];
            set(scene, grid, owner, i, Cell { ch, fg, ..cell }, id);
            // The bolt lights what is next to it, fading with distance:
            // local light, not a screen flash.
            for dx in -3i32..=3 {
                let nx = x + dx;
                if dx == 0 || nx < art.x as i32 || nx >= (art.x + art.w) as i32 {
                    continue;
                }
                let j = y as usize * w + nx as usize;
                let near = grid.cells()[j];
                let k = 0.5 * flash * (1.0 - dx.unsigned_abs() as f32 / 4.0);
                let lit = Cell {
                    ch: near.ch,
                    fg: near.fg.lerp(scene.light, k),
                    bg: near.bg.map(|b| b.lerp(scene.light, 0.5 * k)),
                };
                if lit != near {
                    set(scene, grid, owner, j, lit, id);
                }
            }
        }
    };
    let x0 = art.x as i32 + (hb >> 8) as i32 % art.w as i32;
    bolt(x0, art.y, art.h, 0, hb);
    // One fork from a third of the way down, angling away.
    let fork_y = art.y + art.h / 3;
    let away = if (x0 as f32) < scene.origin.0 { -1 } else { 1 };
    bolt(x0, fork_y, art.h / 5 + 2, away, hb ^ 0xF0F0);
}

fn set(scene: &Scene, grid: &mut CellGrid, owner: &mut [u8], i: usize, cell: Cell, id: u8) {
    if scene.protected[i] {
        return;
    }
    let w = scene.width() as usize;
    grid.set((i % w) as u16, (i / w) as u16, cell);
    owner[i] = id;
}

fn displace(
    scene: &Scene,
    d: &Displace,
    amp: f32,
    phase: f32,
    grid: &mut CellGrid,
    owner: &mut [u8],
    id: u8,
) {
    let art = scene.art;
    let w = scene.width() as usize;
    for y in art.y..art.y + art.h {
        let row_t = (y - art.y) as f32 / art.h.max(1) as f32;
        let shape = match d.kind {
            DisplaceKind::Ripple => 1.0,
            DisplaceKind::RowSway => (1.0 - row_t).powf(1.5),
            DisplaceKind::Shimmer => 0.4 + 0.6 * row_t,
        };
        let wave = (TAU * (d.cycles as f32 * phase + y as f32 / d.wavelength) + d.phase).sin();
        let offset = match d.kind {
            // Shimmer is a one-cell twitch whenever the wave is strong.
            DisplaceKind::Shimmer => (2.0 * d.amplitude * amp * shape * wave)
                .round()
                .clamp(-1.0, 1.0) as i32,
            _ => (d.amplitude * amp * shape * wave).round() as i32,
        };
        if offset == 0 {
            continue;
        }
        for x in art.x..art.x + art.w {
            let i = y as usize * w + x as usize;
            if scene.protected[i] {
                continue;
            }
            let sx = (x as i32 - offset).clamp(art.x as i32, (art.x + art.w - 1) as i32) as u16;
            let si = y as usize * w + sx as usize;
            // Never smear the subject into the background.
            let src = if scene.protected[si] {
                scene.base.cells()[i]
            } else {
                scene.base.cells()[si]
            };
            if src != grid.cells()[i] {
                set(scene, grid, owner, i, src, id);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn field(
    scene: &Scene,
    f: &Field,
    amp: f32,
    phase: f32,
    audio: &AudioFrame,
    grid: &mut CellGrid,
    owner: &mut [u8],
    id: u8,
) {
    let (w, h) = (scene.width() as usize, scene.height() as usize);
    let bg = scene.background;
    match f {
        Field::Fog {
            density,
            kx,
            ky,
            phases,
            color,
        } => {
            // Separable waves: one table per axis per frame.
            let col: Vec<f32> = (0..w)
                .map(|x| (TAU * (x as f32 / 23.0 + *kx as f32 * phase) + phases[0]).sin())
                .collect();
            let row: Vec<f32> = (0..h)
                .map(|y| (TAU * (y as f32 / 9.0 + *ky as f32 * phase) + phases[1]).sin())
                .collect();
            let threshold = 1.0 - 0.9 * density * amp.min(1.8);
            for (y, ry) in row.iter().enumerate() {
                for (x, cx) in col.iter().enumerate() {
                    let v = 0.5 + 0.3 * cx + 0.2 * ry;
                    let level = ((v - threshold) / 0.12).floor();
                    if level < 1.0 {
                        continue;
                    }
                    let i = y * w + x;
                    let mut cell = grid.cells()[i];
                    if cell.ch == ' ' {
                        cell.ch = if level >= 2.0 { ':' } else { '.' };
                        cell.fg = *color;
                    } else {
                        cell.fg = cell.fg.lerp(*color, 0.18 * level.min(2.0));
                    }
                    set(scene, grid, owner, i, cell, id);
                }
            }
        }
        Field::Heat {
            scale,
            cycles,
            phases,
            colors,
        } => {
            let art = scene.art;
            let col: Vec<f32> = (art.x..art.x + art.w)
                .map(|x| (TAU * (x as f32 * scale + *cycles as f32 * phase) + phases[0]).sin())
                .collect();
            let row: Vec<f32> = (art.y..art.y + art.h)
                .map(|y| {
                    (TAU * (y as f32 * scale * 2.0 - *cycles as f32 * phase) + phases[1]).sin()
                })
                .collect();
            let n = colors.len().max(1);
            let mix = (0.35 * amp).min(0.6);
            for (yi, y) in (art.y..art.y + art.h).enumerate() {
                for (xi, x) in (art.x..art.x + art.w).enumerate() {
                    let i = y as usize * w + x as usize;
                    let cell = grid.cells()[i];
                    if cell.ch == ' ' {
                        continue;
                    }
                    let v = scene.density[i] + 0.35 * (col[xi] + row[yi]);
                    let idx = ((v * n as f32).floor() as isize).rem_euclid(n as isize) as usize;
                    let tinted = Cell {
                        fg: cell.fg.lerp(colors[idx], mix),
                        ..cell
                    };
                    set(scene, grid, owner, i, tinted, id);
                }
            }
        }
        Field::Clouds {
            bands,
            softness,
            color,
        } => {
            let art = scene.art;
            for &(centre, wavelength, cycles, ph) in bands {
                let cy = art.y as f32 + centre * art.h as f32;
                let half = (art.h as f32 * 0.06).max(1.0);
                let y0 = (cy - half).floor().max(art.y as f32) as u16;
                let y1 = ((cy + half).ceil() as u16).min(art.y + art.h);
                let col: Vec<f32> = (art.x..art.x + art.w)
                    .map(|x| (TAU * (x as f32 / wavelength - cycles as f32 * phase) + ph).sin())
                    .collect();
                for y in y0..y1 {
                    let fall = 1.0 - ((y as f32 + 0.5 - cy).abs() / (half + 0.5));
                    for (xi, x) in (art.x..art.x + art.w).enumerate() {
                        let v = col[xi] * fall * amp.min(1.8);
                        if v < 1.0 - softness {
                            continue;
                        }
                        let i = y as usize * w + x as usize;
                        let cell = grid.cells()[i];
                        if scene.density[i] > 0.45 {
                            continue; // clouds sit over sky, not over detail
                        }
                        let ch = if v > 1.0 - softness * 0.4 { '~' } else { '-' };
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
        }
        Field::Flicker {
            cells,
            cycles,
            phases,
            neon,
        } => {
            let depth = (gain_flicker(audio) * amp).min(1.0);
            for ((&i, &k), &ph) in cells.iter().zip(cycles).zip(phases) {
                let s = (TAU * k as f32 * phase + ph).sin();
                let cell = grid.cells()[i];
                let fg = if *neon {
                    if s > 0.92 {
                        cell.fg.lerp(bg, 0.7) // brief buzz-off
                    } else {
                        let pulse = audio.onset.clamp(0.0, 1.0) * 0.3;
                        cell.fg.lerp(scene.accent, (0.25 + pulse) * depth)
                    }
                } else if s > 0.55 {
                    cell.fg.lerp(bg, 0.55 * depth)
                } else {
                    continue;
                };
                set(scene, grid, owner, i, Cell { fg, ..cell }, id);
            }
        }
        Field::Ambient { cycles, wavelength } => {
            let art = scene.art;
            let mix = 0.12 * amp.min(2.0);
            for y in art.y..art.y + art.h {
                for x in art.x..art.x + art.w {
                    let i = y as usize * w + x as usize;
                    let cell = grid.cells()[i];
                    if cell.ch == ' ' {
                        continue;
                    }
                    let s = (TAU
                        * ((x as f32 * 0.5 + y as f32) / wavelength - *cycles as f32 * phase))
                        .sin();
                    let fg = if s > 0.6 {
                        cell.fg.lerp(scene.accent, mix)
                    } else if s < -0.6 {
                        cell.fg.lerp(bg, mix)
                    } else {
                        continue;
                    };
                    set(scene, grid, owner, i, Cell { fg, ..cell }, id);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn particles(
    scene: &Scene,
    p: &Particles,
    amp: f32,
    density: f32,
    phase: f32,
    audio: &AudioFrame,
    grid: &mut CellGrid,
    owner: &mut [u8],
    id: u8,
) {
    let (w, h) = (scene.width() as f32, scene.height() as f32);
    let wi = scene.width() as usize;
    let pulse = audio.onset.clamp(0.0, 1.0) * 0.5 * amp.min(1.0);
    for item in &p.items {
        if item.rank >= density {
            continue;
        }
        if item.blink_cycles > 0 {
            let b = (TAU * item.blink_cycles as f32 * phase + item.blink_phase).sin();
            let threshold = if p.kind == ParticleKind::Fireflies {
                0.35
            } else {
                -0.6
            };
            if b < threshold {
                continue;
            }
        }
        let sway = item.sway_amp * (TAU * item.sway_cycles as f32 * phase + item.sway_phase).sin();
        let (mut x, mut y) = (
            item.x0 + item.kx as f32 * w * phase,
            item.y0 + item.ky as f32 * h * phase,
        );
        match p.kind {
            ParticleKind::Dust => y += sway * 0.5,
            ParticleKind::Fireflies => {
                x += sway;
                y += 0.5 * item.sway_amp * (TAU * phase + item.sway_phase * 1.7).cos();
            }
            _ => x += sway,
        }
        let cx = x.rem_euclid(w) as usize % wi;
        let cy = y.rem_euclid(h) as usize % scene.height() as usize;
        let i = cy * wi + cx;
        if scene.protected[i] {
            continue; // behind the subject
        }
        let depth = item.depth.min(2);
        let glyph = p.glyphs[depth.min(p.glyphs.len() - 1)];
        let base_fg = p.colors[depth];
        let fg = base_fg.lerp(scene.accent, pulse);
        let cell = grid.cells()[i];
        set(
            scene,
            grid,
            owner,
            i,
            Cell {
                ch: glyph,
                fg,
                ..cell
            },
            id,
        );
    }
}

/// Keeps at most `scene.max_changes_on_beat(next.burst)` cells different
/// from `prev`, reverting the changes of the weakest layers first (highest
/// owner id; a cell going back to the base art counts as its previous
/// owner). Ties break by cell index, so the result is deterministic.
pub fn limit_changes(scene: &Scene, prev: &Output, next: Output) -> Output {
    let max = scene.max_changes_on_beat(next.burst);
    if prev.grid.width() != next.grid.width() || prev.grid.height() != next.grid.height() {
        return next;
    }
    let mut changed: Vec<(u8, usize)> = next
        .grid
        .cells()
        .iter()
        .zip(prev.grid.cells())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| {
            let o = next.owner[i].max(prev.owner[i]);
            (o, i)
        })
        .collect();
    if changed.len() <= max {
        return next;
    }
    // Weakest (highest id) first, then by index.
    changed.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let drop = changed.len() - max;
    let mut out = next;
    let w = out.grid.width() as usize;
    for &(_, i) in changed.iter().take(drop) {
        let cell = prev.grid.cells()[i];
        out.grid.set((i % w) as u16, (i / w) as u16, cell);
        out.owner[i] = prev.owner[i];
    }
    out
}

#[cfg(test)]
mod tests;
