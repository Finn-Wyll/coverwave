//! The fixed style-DNA projection (PLAN.md section 4, step 2): CLIP
//! embedding -> 8 principal components -> divide by the corpus std -> tanh.
//!
//! `pca.bin` layout, little-endian:
//! `b"CWPCA1\0\0"`, u32 dim, u32 components, u32 samples,
//! f32 mean[dim], f32 components[components][dim], f32 std[components].

use cw_core::recipe::DNA_DIMS;
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"CWPCA1\0\0";

#[derive(Debug, thiserror::Error)]
#[error("pca.bin: {0}")]
pub struct PcaError(String);

#[derive(Debug, Clone, PartialEq)]
pub struct Pca {
    pub dim: usize,
    /// Number of covers it was fitted on.
    pub samples: u32,
    pub mean: Vec<f32>,
    /// `DNA_DIMS` unit vectors of length `dim`, strongest first.
    pub components: Vec<Vec<f32>>,
    /// Standard deviation of the corpus along each component.
    pub std: Vec<f32>,
}

impl Pca {
    /// Projects an embedding to style DNA in -1..1.
    pub fn project(&self, embedding: &[f32]) -> [f32; DNA_DIMS] {
        let mut dna = [0.0; DNA_DIMS];
        if embedding.len() != self.dim {
            return dna;
        }
        for (k, c) in self.components.iter().enumerate().take(DNA_DIMS) {
            let p: f32 = embedding
                .iter()
                .zip(&self.mean)
                .zip(c)
                .map(|((e, m), w)| (e - m) * w)
                .sum();
            let s = self.std[k];
            dna[k] = if s > 1e-9 { (p / s).tanh() } else { 0.0 };
        }
        dna
    }

    /// Fits on L2-normalised embeddings. Deterministic: each component's
    /// sign is fixed so its largest-magnitude weight is positive.
    pub fn fit(embeddings: &[Vec<f32>]) -> Result<Pca, PcaError> {
        let n = embeddings.len();
        let dim = embeddings.first().map_or(0, Vec::len);
        if n < 2 || dim < DNA_DIMS {
            return Err(PcaError(format!("need at least 2 embeddings, got {n}")));
        }
        if embeddings.iter().any(|e| e.len() != dim) {
            return Err(PcaError("embeddings differ in length".into()));
        }
        let mut mean = vec![0.0f64; dim];
        for e in embeddings {
            for (m, x) in mean.iter_mut().zip(e) {
                *m += *x as f64 / n as f64;
            }
        }
        let mut cov = nalgebra::DMatrix::<f64>::zeros(dim, dim);
        for e in embeddings {
            let d = nalgebra::DVector::from_iterator(
                dim,
                e.iter().zip(&mean).map(|(x, m)| *x as f64 - m),
            );
            cov.ger(1.0 / (n - 1) as f64, &d, &d, 1.0);
        }
        let eig = nalgebra::SymmetricEigen::new(cov);
        let mut order: Vec<usize> = (0..dim).collect();
        order.sort_by(|&a, &b| eig.eigenvalues[b].total_cmp(&eig.eigenvalues[a]));

        let mut components = Vec::with_capacity(DNA_DIMS);
        let mut std = Vec::with_capacity(DNA_DIMS);
        for &i in order.iter().take(DNA_DIMS) {
            let mut v: Vec<f64> = eig.eigenvectors.column(i).iter().copied().collect();
            let (big, _) = v
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap_or((0, &0.0));
            if v[big] < 0.0 {
                v.iter_mut().for_each(|x| *x = -*x);
            }
            components.push(v.iter().map(|&x| x as f32).collect());
            std.push(eig.eigenvalues[i].max(0.0).sqrt() as f32);
        }
        Ok(Pca {
            dim,
            samples: n as u32,
            mean: mean.iter().map(|&m| m as f32).collect(),
            components,
            std,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        for v in [self.dim as u32, self.components.len() as u32, self.samples] {
            out.extend(v.to_le_bytes());
        }
        let floats = self
            .mean
            .iter()
            .chain(self.components.iter().flatten())
            .chain(&self.std);
        for f in floats {
            out.extend(f.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Result<Pca, PcaError> {
        if b.len() < 20 || &b[..8] != MAGIC {
            return Err(PcaError("not a coverwave PCA file".into()));
        }
        let u = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let (dim, k, samples) = (u(8), u(12), u(16) as u32);
        if k != DNA_DIMS || dim == 0 {
            return Err(PcaError(format!("expected {DNA_DIMS} components, got {k}")));
        }
        let floats = dim + k * dim + k;
        if b.len() != 20 + 4 * floats {
            return Err(PcaError("truncated".into()));
        }
        let f: Vec<f32> = b[20..]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        Ok(Pca {
            dim,
            samples,
            mean: f[..dim].to_vec(),
            components: f[dim..dim + k * dim]
                .chunks(dim)
                .map(<[f32]>::to_vec)
                .collect(),
            std: f[dim + k * dim..].to_vec(),
        })
    }

    /// Short content hash, recorded in recipes so a refit invalidates them.
    pub fn version(&self) -> String {
        let h = Sha256::digest(self.to_bytes());
        h[..4].iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points spread mostly along axis 0, then axis 1, in 16 dims.
    fn corpus() -> Vec<Vec<f32>> {
        (0..40)
            .map(|i| {
                let t = i as f32 / 40.0;
                let mut v = vec![0.0f32; 16];
                v[0] = 3.0 * (t * 12.0).sin();
                v[1] = 1.5 * (t * 7.0).cos();
                for (k, x) in v.iter_mut().enumerate().skip(2) {
                    *x = 0.1 * ((t * 31.0 + k as f32) * 1.7).sin();
                }
                v
            })
            .collect()
    }

    #[test]
    fn finds_dominant_axes_and_projects_into_range() {
        let pca = Pca::fit(&corpus()).unwrap();
        assert!(
            pca.components[0][0].abs() > 0.99,
            "{:?}",
            &pca.components[0][..3]
        );
        assert!(pca.components[1][1].abs() > 0.99);
        assert!(pca.std[0] > pca.std[1] && pca.std[1] > pca.std[2]);
        for e in corpus() {
            assert!(pca.project(&e).iter().all(|d| (-1.0..=1.0).contains(d)));
        }
        // The mean projects to zero DNA.
        let dna = pca.project(&pca.mean.clone());
        assert!(dna.iter().all(|d| d.abs() < 1e-5));
    }

    #[test]
    fn bytes_round_trip_and_fit_is_deterministic() {
        let a = Pca::fit(&corpus()).unwrap();
        let b = Pca::from_bytes(&a.to_bytes()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.version(), Pca::fit(&corpus()).unwrap().version());
        assert!(Pca::from_bytes(b"nope").is_err());
        let mut short = a.to_bytes();
        short.pop();
        assert!(Pca::from_bytes(&short).is_err());
    }

    #[test]
    fn wrong_length_embedding_gives_neutral_dna() {
        let pca = Pca::fit(&corpus()).unwrap();
        assert_eq!(pca.project(&[1.0; 3]), [0.0; DNA_DIMS]);
    }
}
