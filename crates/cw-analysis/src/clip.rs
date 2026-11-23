//! MobileCLIP-S0 image embeddings, run with tract (pure Rust, no native
//! runtime). The model arrives as ONNX bytes; where they come from (download,
//! cache, bundle) is the frontend's business.
//!
//! Preprocessing matches the model's `preprocessor_config.json`: resize the
//! shortest edge to 256 (bilinear with antialiasing, in sRGB), centre-crop
//! 256x256, scale to 0..1, no mean/std normalisation, NCHW.

use cw_core::RgbImage;
use image::imageops::FilterType;
use tract_onnx::prelude::*;

/// Embedding width of MobileCLIP-S0.
pub const EMBED_DIM: usize = 512;
/// Square input side.
pub const INPUT_SIZE: u32 = 256;
/// Name recorded in recipes.
pub const MODEL_NAME: &str = "mobileclip_s0";

/// An L2-normalised image (or text) embedding.
pub type Embedding = Vec<f32>;

#[derive(Debug, thiserror::Error)]
#[error("CLIP model: {0}")]
pub struct ClipError(String);

impl From<TractError> for ClipError {
    fn from(e: TractError) -> Self {
        ClipError(format!("{e:#}"))
    }
}

/// A loaded, optimised vision encoder. Thread-safe: share it in an `Arc`.
pub struct ClipModel {
    plan: std::sync::Arc<TypedRunnableModel>,
}

impl ClipModel {
    /// Parses and optimises the vision encoder (a few hundred ms).
    pub fn from_onnx_bytes(bytes: &[u8]) -> Result<ClipModel, ClipError> {
        let side = INPUT_SIZE as usize;
        let plan = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(bytes))?
            .with_input_fact(0, f32::fact([1, 3, side, side]).into())?
            .into_optimized()?
            .into_runnable()?;
        let model = ClipModel { plan };
        // Fail now, not on the first cover, if the graph is not what we expect.
        let probe = model.embed_input(vec![0.5; 3 * side * side])?;
        if probe.len() != EMBED_DIM {
            return Err(ClipError(format!(
                "expected a {EMBED_DIM}-dim embedding, got {}",
                probe.len()
            )));
        }
        Ok(model)
    }

    /// Embeds a cover. Deterministic for a given build and CPU.
    pub fn embed(&self, img: &RgbImage) -> Result<Embedding, ClipError> {
        self.embed_input(preprocess(img))
    }

    fn embed_input(&self, chw: Vec<f32>) -> Result<Embedding, ClipError> {
        let side = INPUT_SIZE as usize;
        let input: Tensor = tract_ndarray::Array4::from_shape_vec((1, 3, side, side), chw)
            .map_err(|e| ClipError(e.to_string()))?
            .into();
        let out = self.plan.run(tvec!(input.into()))?;
        let view = out[0].to_plain_array_view::<f32>()?;
        Ok(normalize(view.iter().copied().collect()))
    }
}

/// L2-normalises (a zero vector stays zero).
pub fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

/// Shortest edge to 256, centre crop, 0..1 floats in CHW order.
pub fn preprocess(img: &RgbImage) -> Vec<f32> {
    let side = INPUT_SIZE;
    let (w, h) = (img.width.max(1), img.height.max(1));
    let scale = side as f64 / w.min(h) as f64;
    let (rw, rh) = (
        ((w as f64 * scale).round() as u32).max(side),
        ((h as f64 * scale).round() as u32).max(side),
    );
    let mut buf = image::RgbImage::new(w, h);
    for (x, y, p) in buf.enumerate_pixels_mut() {
        let c = if img.pixels.is_empty() {
            cw_core::Rgb::default()
        } else {
            img.get(x.min(img.width - 1), y.min(img.height - 1))
        };
        *p = image::Rgb([c.0, c.1, c.2]);
    }
    let resized = image::imageops::resize(&buf, rw, rh, FilterType::Triangle);
    let (ox, oy) = ((rw - side) / 2, (rh - side) / 2);
    let n = (side * side) as usize;
    let mut chw = vec![0.0f32; 3 * n];
    for y in 0..side {
        for x in 0..side {
            let p = resized.get_pixel(ox + x, oy + y).0;
            let i = (y * side + x) as usize;
            for c in 0..3 {
                chw[c * n + i] = p[c] as f32 / 255.0;
            }
        }
    }
    chw
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::Rgb;

    #[test]
    fn preprocess_crops_the_centre_of_wide_images() {
        // 512x256: left quarter red, middle half green, right quarter blue.
        let pixels = (0..256)
            .flat_map(|_| {
                (0..512).map(|x| match x {
                    0..128 => Rgb(255, 0, 0),
                    128..384 => Rgb(0, 255, 0),
                    _ => Rgb(0, 0, 255),
                })
            })
            .collect();
        let chw = preprocess(&RgbImage::new(512, 256, pixels));
        let n = 256 * 256;
        assert_eq!(chw.len(), 3 * n);
        // Only the green middle survives the crop.
        let mean = |c: usize| chw[c * n..(c + 1) * n].iter().sum::<f32>() / n as f32;
        assert!(mean(0) < 0.01 && mean(1) > 0.99 && mean(2) < 0.01);
    }

    #[test]
    fn normalize_unit_length() {
        let v = normalize(vec![3.0, 4.0]);
        assert_eq!(v, vec![0.6, 0.8]);
        assert_eq!(normalize(vec![0.0; 3]), vec![0.0; 3]);
    }
}
