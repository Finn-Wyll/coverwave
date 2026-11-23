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

/// Changed-cell budget per frame, as a fraction of all cells.
pub const BUDGET: f32 = 0.15;
/// Budget for high-energy covers (energy 1.0); it rises linearly from
/// [`BUDGET`] at energy 0.7.
pub const BUDGET_HIGH: f32 = 0.30;
/// Cells per particle at density 1.0.
const CELLS_PER_PARTICLE: f32 = 30.0;
const MAX_PARTICLES: usize = 1500;

/// Owner value for cells that show the base art.
pub const OWNER_BASE: u8 = 0;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub grid: CellGrid,
    pub owner: Vec<u8>,
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
    background: Rgb,
    budget: f32,
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

    /// Maximum cells that may change per frame.
    pub fn max_changes(&self) -> usize {
        (self.budget * self.base.cells().len() as f32).floor() as usize
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
}

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
    let density = base
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

    Scene {
        base,
        art,
        protected,
        density,
        loop_frames: recipe.global.loop_frames.max(1),
        layers,
        accent,
        background,
        budget,
    }
}

struct PrepCtx<'a> {
    w: u16,
    h: u16,
    art: Rect,
    palette: &'a [Rgb],
    accent: Rgb,
    background: Rgb,
    speed_sway: f32,
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
        Layer::Ambient { cycles, .. } => Effect::Field(Field::Ambient {
            cycles: cycles.max(1) as i32,
            wavelength: 24.0 + unit(rng) * 16.0,
        }),
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

/// Audio gains: silence still moves, loud music moves more.
fn gain_intensity(a: &AudioFrame) -> f32 {
    0.7 + 0.6 * a.bass().clamp(0.0, 1.0)
}
fn gain_density(a: &AudioFrame) -> f32 {
    0.6 + 0.8 * a.mid().clamp(0.0, 1.0)
}
fn gain_flicker(a: &AudioFrame) -> f32 {
    0.4 + 0.6 * a.treble().clamp(0.0, 1.0)
}

/// Renders one frame. Pure: no clocks, no global state.
pub fn render(scene: &Scene, input: Input<'_>) -> Output {
    let mut grid = scene.base.clone();
    let mut owner = vec![OWNER_BASE; grid.cells().len()];
    let l = scene.loop_frames;
    let phase = (input.frame % l) as f32 / l as f32;
    let fade = input.fade.clamp(0.0, 1.0);
    let audio = input.audio;

    // Displacement first (it resamples the base), then fields, then
    // particles on top. Weakest layers draw first so stronger ones win.
    for pass in 0..3 {
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
                (Effect::Particles(p), 2) => {
                    let dens = (p.density * gain_density(audio)).clamp(0.0, 1.0) * fade;
                    particles(scene, p, amp, dens, phase, audio, &mut grid, &mut owner, id)
                }
                _ => {}
            }
        }
    }
    Output { grid, owner }
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
            let threshold = 1.0 - 0.9 * density * amp.min(1.2);
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
            let mix = (0.35 * amp).min(0.5);
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
                        let v = col[xi] * fall * amp.min(1.2);
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
            let mix = 0.12 * amp.min(1.2);
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

/// Keeps at most `scene.max_changes()` cells different from `prev`,
/// reverting the changes of the weakest layers first (highest owner id;
/// a cell going back to the base art counts as its previous owner). Ties
/// break by cell index, so the result is deterministic.
pub fn limit_changes(scene: &Scene, prev: &Output, next: Output) -> Output {
    let max = scene.max_changes();
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
