//! Colour maths shared by analysis, rendering and frontends: sRGB <-> CIE Lab
//! (D65) and the xterm 256-colour fallback. Pure functions, no tables that
//! depend on the platform.

use crate::Rgb;

/// A colour in CIE L*a*b* (D65). `l` is 0..100; `a`/`b` are roughly -128..127.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

/// sRGB transfer function, 0..1 -> linear 0..1.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Inverse sRGB transfer function, linear 0..1 -> 0..1.
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

const WHITE: (f32, f32, f32) = (0.950_47, 1.0, 1.088_83);

fn lab_f(t: f32) -> f32 {
    const D: f32 = 6.0 / 29.0;
    if t > D * D * D {
        t.cbrt()
    } else {
        t / (3.0 * D * D) + 4.0 / 29.0
    }
}

fn lab_f_inv(t: f32) -> f32 {
    const D: f32 = 6.0 / 29.0;
    if t > D {
        t * t * t
    } else {
        3.0 * D * D * (t - 4.0 / 29.0)
    }
}

impl Lab {
    /// Euclidean distance squared (CIE76). Good enough for clustering and
    /// nearest-colour lookups.
    pub fn dist2(self, o: Lab) -> f32 {
        let (dl, da, db) = (self.l - o.l, self.a - o.a, self.b - o.b);
        dl * dl + da * da + db * db
    }

    pub fn chroma(self) -> f32 {
        (self.a * self.a + self.b * self.b).sqrt()
    }

    pub fn lerp(self, o: Lab, t: f32) -> Lab {
        Lab {
            l: self.l + (o.l - self.l) * t,
            a: self.a + (o.a - self.a) * t,
            b: self.b + (o.b - self.b) * t,
        }
    }

    pub fn from_linear(r: f32, g: f32, b: f32) -> Lab {
        let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
        let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
        let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
        let (fx, fy, fz) = (lab_f(x / WHITE.0), lab_f(y / WHITE.1), lab_f(z / WHITE.2));
        Lab {
            l: 116.0 * fy - 16.0,
            a: 500.0 * (fx - fy),
            b: 200.0 * (fy - fz),
        }
    }

    /// Linear RGB, unclamped (may be out of gamut).
    pub fn to_linear(self) -> (f32, f32, f32) {
        let fy = (self.l + 16.0) / 116.0;
        let fx = fy + self.a / 500.0;
        let fz = fy - self.b / 200.0;
        let (x, y, z) = (
            WHITE.0 * lab_f_inv(fx),
            WHITE.1 * lab_f_inv(fy),
            WHITE.2 * lab_f_inv(fz),
        );
        (
            3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
            -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z,
            0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
        )
    }
}

impl From<Rgb> for Lab {
    fn from(c: Rgb) -> Lab {
        let f = |v: u8| srgb_to_linear(v as f32 / 255.0);
        Lab::from_linear(f(c.0), f(c.1), f(c.2))
    }
}

impl From<Lab> for Rgb {
    /// Converts back to sRGB, clamping each channel into gamut.
    fn from(c: Lab) -> Rgb {
        let (r, g, b) = c.to_linear();
        let f = |v: f32| (linear_to_srgb(v) * 255.0).round() as u8;
        Rgb(f(r), f(g), f(b))
    }
}

/// Nearest xterm 256-colour index (16..=255: the 6x6x6 cube or the grey
/// ramp; the 16 system colours are skipped because terminals theme them).
pub fn ansi256(c: Rgb) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| -> usize {
        // Index of the nearest cube level.
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (**l as i32 - v as i32).abs())
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (level(c.0), level(c.1), level(c.2));
    let cube = Rgb(LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_idx = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;

    // Grey ramp: 232..=255 are 8, 18, ..., 238.
    let avg = (c.0 as u32 + c.1 as u32 + c.2 as u32) / 3;
    let gi = (avg.saturating_sub(3) / 10).min(23) as u8;
    let gv = 8 + 10 * gi;
    let grey = Rgb(gv, gv, gv);

    let d = |a: Rgb, b: Rgb| {
        let (x, y, z) = (
            a.0 as i32 - b.0 as i32,
            a.1 as i32 - b.1 as i32,
            a.2 as i32 - b.2 as i32,
        );
        x * x + y * y + z * z
    };
    if d(c, grey) < d(c, cube) {
        232 + gi
    } else {
        cube_idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_round_trips() {
        for c in [
            Rgb(0, 0, 0),
            Rgb(255, 255, 255),
            Rgb(200, 30, 60),
            Rgb(12, 140, 220),
        ] {
            let back: Rgb = Lab::from(c).into();
            for (x, y) in [(c.0, back.0), (c.1, back.1), (c.2, back.2)] {
                assert!((x as i32 - y as i32).abs() <= 1, "{c:?} -> {back:?}");
            }
        }
        let white = Lab::from(Rgb(255, 255, 255));
        assert!((white.l - 100.0).abs() < 0.1 && white.chroma() < 0.1);
    }

    #[test]
    fn ansi256_picks_cube_and_grey() {
        assert_eq!(ansi256(Rgb(255, 0, 0)), 196);
        assert_eq!(ansi256(Rgb(0, 0, 0)), 16);
        assert_eq!(ansi256(Rgb(128, 128, 128)), 244);
        assert_eq!(ansi256(Rgb(95, 135, 175)), 16 + 36 + 2 * 6 + 3);
    }
}
