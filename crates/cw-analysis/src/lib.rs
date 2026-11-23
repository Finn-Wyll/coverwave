//! Cover analysis: image bytes in, a [`Cover`] and a motion [`Recipe`] out
//! (PLAN.md sections 4 and 5).
//!
//! Platform-agnostic: works on bytes, never paths. The CLIP model and PCA
//! are passed in by the caller, who decides where they come from.
//!
//! Determinism: the same bytes (and the same model, prompts and PCA) always
//! give the same result. All randomness comes from the cover seed via
//! ChaCha8.

pub mod clip;
pub mod features;
pub mod mapping;
pub mod mask;
pub mod pca;
pub mod scenes;

pub use clip::{ClipModel, Embedding};
pub use pca::Pca;
pub use scenes::ScenePrompts;

use cw_core::recipe::{AnalysisInfo, DNA_DIMS, Recipe};
use cw_core::{Lab, Palette, Rgb, RgbImage};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use sha2::{Digest, Sha256};

/// Longest side kept after decoding. Big enough for a 240x70-cell terminal
/// (about 240x140 pixels of detail) with plenty of headroom for area
/// averaging, small enough to keep per-resize work cheap.
pub const MAX_SIDE: u32 = 512;

/// Number of palette clusters.
pub const PALETTE_SIZE: usize = 5;

/// Side of the square downsample the palette is computed from.
const PALETTE_SAMPLE: u32 = 64;
const KMEANS_ITERATIONS: usize = 25;

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("cannot decode cover image: {0}")]
    Decode(String),
    #[error("cover image is empty")]
    Empty,
}

/// Everything derived from one cover image.
#[derive(Debug, Clone, PartialEq)]
pub struct Cover {
    /// The cover, downscaled so its longest side is at most [`MAX_SIDE`].
    pub image: RgbImage,
    pub palette: Palette,
    /// SHA-256 of the cover bytes: the cache key.
    pub hash: [u8; 32],
    /// First 8 bytes of `hash`, big-endian.
    pub seed: u64,
}

/// The models a recipe is built with. Any of them may be missing: the
/// recipe then gets neutral scenes and DNA (and should not be cached).
#[derive(Clone, Copy)]
pub struct Models<'a> {
    pub clip: Option<&'a ClipModel>,
    pub scenes: &'a ScenePrompts,
    pub pca: Option<&'a Pca>,
}

impl Models<'_> {
    /// What a recipe built with these models records in `analysis`.
    pub fn info(&self, have_embedding: bool) -> AnalysisInfo {
        AnalysisInfo {
            model: if have_embedding {
                clip::MODEL_NAME.to_string()
            } else {
                "none".to_string()
            },
            prompts_version: self.scenes.version,
            pca: self.pca.map_or("none".to_string(), Pca::version),
        }
    }
}

/// Embeds the cover (if a model is given) and builds its recipe. Returns the
/// embedding too, so callers can cache it and rebuild recipes later without
/// running CLIP again.
pub fn recipe(
    cover: &Cover,
    models: Models<'_>,
) -> Result<(Recipe, Option<Embedding>), clip::ClipError> {
    let embedding = match models.clip {
        Some(m) => Some(m.embed(&cover.image)?),
        None => None,
    };
    let recipe = recipe_with_embedding(cover, embedding.as_deref(), models);
    Ok((recipe, embedding))
}

/// Builds a recipe from an already computed embedding (or none).
pub fn recipe_with_embedding(
    cover: &Cover,
    embedding: Option<&[f32]>,
    models: Models<'_>,
) -> Recipe {
    let scene_weights = embedding
        .map(|e| models.scenes.weights(e))
        .unwrap_or_default();
    let style_dna = match (embedding, models.pca) {
        (Some(e), Some(p)) => p.project(e),
        _ => [0.0; DNA_DIMS],
    };
    mapping::build(mapping::Measured {
        hash: &cover.hash,
        seed: cover.seed,
        palette: &cover.palette,
        features: features::features(&cover.image),
        mask: mask::subject_mask(&cover.image),
        scene_weights,
        style_dna,
        analysis: models.info(embedding.is_some()),
    })
}

/// Decodes and analyses a cover.
pub fn analyse(bytes: &[u8]) -> Result<Cover, AnalysisError> {
    let hash: [u8; 32] = Sha256::digest(bytes).into();
    let seed = seed_from_hash(&hash);
    let full = decode(bytes)?;
    let image = shrink(&full, MAX_SIDE);
    let palette = palette(&image, seed);
    Ok(Cover {
        image,
        palette,
        hash,
        seed,
    })
}

/// The cover seed: first 8 bytes of SHA-256, read as a big-endian `u64`.
pub fn cover_seed(bytes: &[u8]) -> u64 {
    seed_from_hash(&Sha256::digest(bytes).into())
}

fn seed_from_hash(hash: &[u8; 32]) -> u64 {
    let mut first = [0u8; 8];
    first.copy_from_slice(&hash[..8]);
    u64::from_be_bytes(first)
}

/// Lower-case hex of a cover hash (the cache file name).
pub fn hash_hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decodes JPEG/PNG/WebP/BMP/GIF bytes. Transparency is composited over
/// black, which matches the player's near-black background.
pub fn decode(bytes: &[u8]) -> Result<RgbImage, AnalysisError> {
    let img = image::load_from_memory(bytes).map_err(|e| AnalysisError::Decode(e.to_string()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err(AnalysisError::Empty);
    }
    let pixels = rgba
        .pixels()
        .map(|p| {
            let [r, g, b, a] = p.0;
            let m = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
            Rgb(m(r), m(g), m(b))
        })
        .collect();
    Ok(RgbImage::new(w, h, pixels))
}

/// Area-downscales so the longest side is at most `max_side`. Never upscales.
pub fn shrink(img: &RgbImage, max_side: u32) -> RgbImage {
    let longest = img.width.max(img.height);
    if longest <= max_side {
        return img.clone();
    }
    let scale = max_side as f64 / longest as f64;
    let w = ((img.width as f64 * scale).round() as u32).max(1);
    let h = ((img.height as f64 * scale).round() as u32).max(1);
    img.resize_area(w, h)
}

/// Uniform f32 in [0, 1).
fn unit(rng: &mut ChaCha8Rng) -> f32 {
    (rng.next_u64() >> 40) as f32 / (1u64 << 24) as f32
}

/// k-means (k = [`PALETTE_SIZE`], k-means++ seeded from `seed`) in Lab space
/// on a 64x64 downsample, sorted by cluster size. Flat images yield fewer
/// than k colours.
pub fn palette(img: &RgbImage, seed: u64) -> Palette {
    let sample = img.resize_area(PALETTE_SAMPLE, PALETTE_SAMPLE);
    let points: Vec<Lab> = sample.pixels.iter().map(|&c| Lab::from(c)).collect();
    if points.is_empty() {
        let grey = Rgb(128, 128, 128);
        return Palette {
            colors: vec![grey],
            weights: vec![1.0],
            accent: grey,
        };
    }

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut centres = vec![points[(rng.next_u64() % points.len() as u64) as usize]];
    let mut d2: Vec<f32> = points.iter().map(|p| p.dist2(centres[0])).collect();
    while centres.len() < PALETTE_SIZE {
        let total: f32 = d2.iter().sum();
        if total <= 1e-3 {
            break; // every point already sits on a centre
        }
        let mut target = unit(&mut rng) * total;
        let mut pick = points.len() - 1;
        for (i, d) in d2.iter().enumerate() {
            if target < *d {
                pick = i;
                break;
            }
            target -= d;
        }
        let c = points[pick];
        centres.push(c);
        for (d, p) in d2.iter_mut().zip(&points) {
            *d = d.min(p.dist2(c));
        }
    }

    let k = centres.len();
    let mut assign = vec![usize::MAX; points.len()];
    let mut counts = vec![0usize; k];
    for _ in 0..KMEANS_ITERATIONS {
        let mut changed = false;
        for (a, p) in assign.iter_mut().zip(&points) {
            let best = nearest(&centres, *p);
            if *a != best {
                *a = best;
                changed = true;
            }
        }
        let mut sums = vec![(0.0f64, 0.0f64, 0.0f64); k];
        counts.iter_mut().for_each(|c| *c = 0);
        for (&a, p) in assign.iter().zip(&points) {
            sums[a].0 += p.l as f64;
            sums[a].1 += p.a as f64;
            sums[a].2 += p.b as f64;
            counts[a] += 1;
        }
        for ((c, s), n) in centres.iter_mut().zip(&sums).zip(&counts) {
            if *n > 0 {
                let n = *n as f64;
                *c = Lab {
                    l: (s.0 / n) as f32,
                    a: (s.1 / n) as f32,
                    b: (s.2 / n) as f32,
                };
            }
        }
        if !changed {
            break;
        }
    }

    let total = points.len() as f32;
    let mut clusters: Vec<(Lab, f32)> = centres
        .into_iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(c, n)| (c, n as f32 / total))
        .collect();
    // Largest first; ties broken by lightness so the order is total.
    clusters.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.l.total_cmp(&a.0.l)));

    let accent = pick_accent(&clusters);
    Palette {
        colors: clusters.iter().map(|(c, _)| Rgb::from(*c)).collect(),
        weights: clusters.iter().map(|(_, w)| *w).collect(),
        accent,
    }
}

fn nearest(centres: &[Lab], p: Lab) -> usize {
    let mut best = 0;
    let mut best_d = f32::INFINITY;
    for (i, c) in centres.iter().enumerate() {
        let d = p.dist2(*c);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// Lightness range the accent is pushed into so it reads on near-black
/// without glaring.
const ACCENT_L: (f32, f32) = (62.0, 84.0);
/// Below this chroma a cover counts as greyscale.
const MIN_ACCENT_CHROMA: f32 = 12.0;

/// The most colourful cluster (weighted gently toward larger clusters), with
/// its lightness clamped into [`ACCENT_L`] and its chroma reduced until it
/// fits in sRGB. Greyscale covers get a light neutral.
fn pick_accent(clusters: &[(Lab, f32)]) -> Rgb {
    let best = clusters
        .iter()
        .map(|(c, w)| (*c, c.chroma() * w.powf(0.35)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c)
        .unwrap_or_default();
    let mut c = if best.chroma() < MIN_ACCENT_CHROMA {
        Lab {
            l: 78.0,
            a: 0.0,
            b: 0.0,
        }
    } else {
        Lab {
            l: best.l.clamp(ACCENT_L.0, ACCENT_L.1),
            ..best
        }
    };
    for _ in 0..60 {
        let (r, g, b) = c.to_linear();
        let ok = |v: f32| (-0.001..=1.001).contains(&v);
        if ok(r) && ok(g) && ok(b) {
            break;
        }
        c.a *= 0.95;
        c.b *= 0.95;
    }
    Rgb::from(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(img: &RgbImage) -> Vec<u8> {
        let mut buf = image::RgbImage::new(img.width, img.height);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            let c = img.get(x, y);
            *p = image::Rgb([c.0, c.1, c.2]);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        buf.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn split(w: u32, h: u32, left: Rgb, right: Rgb) -> RgbImage {
        let pixels = (0..h)
            .flat_map(|_| (0..w).map(move |x| if x < w / 2 { left } else { right }))
            .collect();
        RgbImage::new(w, h, pixels)
    }

    #[test]
    fn seed_is_first_eight_bytes_of_sha256() {
        // SHA-256("") = e3b0c44298fc1c14...
        assert_eq!(cover_seed(b""), 0xe3b0_c442_98fc_1c14);
    }

    #[test]
    fn analyse_is_deterministic_and_shrinks() {
        let img = split(1000, 600, Rgb(200, 40, 40), Rgb(30, 60, 160));
        let bytes = png(&img);
        let a = analyse(&bytes).unwrap();
        let b = analyse(&bytes).unwrap();
        assert_eq!(a, b);
        assert_eq!((a.image.width, a.image.height), (512, 307));
        assert_eq!(a.seed, cover_seed(&bytes));
    }

    #[test]
    fn recipe_without_models_is_neutral_and_deterministic() {
        let img = split(200, 200, Rgb(200, 40, 40), Rgb(30, 60, 160));
        let cover = analyse(&png(&img)).unwrap();
        let scenes = ScenePrompts::shipped();
        let models = Models {
            clip: None,
            scenes: &scenes,
            pca: None,
        };
        let (r, emb) = recipe(&cover, models).unwrap();
        assert!(emb.is_none());
        assert_eq!(r.analysis.model, "none");
        assert!(r.scene_weights.is_empty());
        assert_eq!(r.style_dna, [0.0; DNA_DIMS]);
        assert_eq!(r.layers[0].kind(), "ambient");
        assert_eq!(r.cover_hash, hash_hex(&cover.hash));
        assert_eq!(r, recipe(&cover, models).unwrap().0);
    }

    #[test]
    fn palette_finds_both_halves() {
        let red = Rgb(200, 40, 40);
        let blue = Rgb(30, 60, 160);
        let p = palette(&split(64, 64, red, blue), 7);
        assert_eq!(p.colors.len(), 2, "{p:?}");
        assert!(p.colors.contains(&red) && p.colors.contains(&blue), "{p:?}");
        assert!((p.weights[0] - 0.5).abs() < 1e-6);
        // The accent comes from one of them and is lifted to a readable L.
        let l = Lab::from(p.accent).l;
        assert!((ACCENT_L.0 - 1.0..=ACCENT_L.1 + 1.0).contains(&l), "L {l}");
    }

    #[test]
    fn palette_sorted_by_weight_and_capped() {
        // A smooth gradient has many colours: k is capped, weights sorted.
        let pixels = (0..64 * 64)
            .map(|i| Rgb((i % 64 * 4) as u8, (i / 64 * 4) as u8, 90))
            .collect();
        let p = palette(&RgbImage::new(64, 64, pixels), 1);
        assert_eq!(p.colors.len(), PALETTE_SIZE);
        assert!(p.weights.windows(2).all(|w| w[0] >= w[1]));
        assert!((p.weights.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn greyscale_cover_gets_light_neutral_accent() {
        let p = palette(&split(32, 32, Rgb(10, 10, 10), Rgb(90, 90, 90)), 3);
        let lab = Lab::from(p.accent);
        assert!(lab.chroma() < 2.0 && lab.l > 70.0, "{:?}", p.accent);
    }

    #[test]
    fn transparent_pixels_become_black_and_garbage_errors() {
        let mut buf = image::RgbaImage::new(2, 1);
        buf.put_pixel(0, 0, image::Rgba([255, 255, 255, 0]));
        buf.put_pixel(1, 0, image::Rgba([255, 0, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        buf.write_to(&mut out, image::ImageFormat::Png).unwrap();
        let img = decode(&out.into_inner()).unwrap();
        assert_eq!(img.pixels, vec![Rgb(0, 0, 0), Rgb(255, 0, 0)]);
        assert!(decode(b"not an image").is_err());
    }
}
