//! Zero-shot scene weights: cosine similarity between the cover embedding
//! and precomputed text embeddings of the scene prompts, softmaxed at CLIP's
//! temperature. Text embeddings are made once by `tools/pca-fit prompts` and
//! shipped in `assets/scene_prompts.json`, so the text encoder never runs in
//! the player.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenePrompts {
    /// Bump whenever scenes, subjects or templates change.
    pub version: u32,
    pub model: String,
    pub temperature: f32,
    /// Each subject is dropped into every template (`{}`); the scene's
    /// embedding is the normalised mean of all of them.
    pub templates: Vec<String>,
    pub scenes: Vec<Scene>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Key used in recipes and the layer table.
    pub name: String,
    pub subjects: Vec<String>,
    /// Added to this scene's logit. Negative for the neutral catch-alls
    /// (portrait, typography, abstract) so they only win when no real scene
    /// fits: almost every cover has some text or a plain area.
    #[serde(default)]
    pub bias: f32,
    /// Empty until `pca-fit prompts` fills it in.
    #[serde(default)]
    pub embedding: Vec<f32>,
}

impl ScenePrompts {
    pub fn from_json(s: &str) -> Result<ScenePrompts, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// The prompts shipped with this build.
    pub fn shipped() -> ScenePrompts {
        Self::from_json(include_str!("../../../assets/scene_prompts.json"))
            .expect("assets/scene_prompts.json is valid")
    }

    /// Whether every scene has a text embedding of length `dim`.
    pub fn is_ready(&self, dim: usize) -> bool {
        !self.scenes.is_empty() && self.scenes.iter().all(|s| s.embedding.len() == dim)
    }

    /// Softmax over scenes of `temperature * cosine(image, scene)`. Both
    /// sides are L2-normalised, so the dot product is the cosine. Weights
    /// are rounded to 3 decimals. Empty if the prompts lack embeddings.
    pub fn weights(&self, image: &[f32]) -> BTreeMap<String, f32> {
        if !self.is_ready(image.len()) {
            return BTreeMap::new();
        }
        let logits: Vec<f32> = self
            .scenes
            .iter()
            .map(|s| {
                let cos: f32 = s.embedding.iter().zip(image).map(|(a, b)| a * b).sum();
                self.temperature * cos + s.bias
            })
            .collect();
        let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = logits.iter().map(|l| (l - max).exp()).collect();
        let sum: f32 = exps.iter().sum();
        self.scenes
            .iter()
            .zip(exps)
            .map(|(s, e)| (s.name.clone(), crate::features::round3(e / sum)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompts() -> ScenePrompts {
        ScenePrompts {
            version: 1,
            model: "test".into(),
            temperature: 100.0,
            templates: vec!["{}".into()],
            scenes: vec![
                Scene {
                    name: "a".into(),
                    subjects: vec![],
                    bias: 0.0,
                    embedding: vec![1.0, 0.0],
                },
                Scene {
                    name: "b".into(),
                    subjects: vec![],
                    bias: 0.0,
                    embedding: vec![0.0, 1.0],
                },
            ],
        }
    }

    #[test]
    fn softmax_favours_the_closer_scene() {
        let w = prompts().weights(&[0.8, 0.6]);
        assert!(w["a"] > 0.99 && w["b"] < 0.01, "{w:?}");
        let even = prompts().weights(&[std::f32::consts::FRAC_1_SQRT_2; 2]);
        assert_eq!(even["a"], 0.5);
    }

    #[test]
    fn missing_embeddings_give_no_weights() {
        let mut p = prompts();
        p.scenes[1].embedding.clear();
        assert!(p.weights(&[1.0, 0.0]).is_empty());
        // Wrong embedding width also gives nothing.
        assert!(prompts().weights(&[1.0, 0.0, 0.0]).is_empty());
    }

    #[test]
    fn shipped_prompts_parse_and_cover_the_layer_table() {
        let p = ScenePrompts::shipped();
        let names: Vec<&str> = p.scenes.iter().map(|s| s.name.as_str()).collect();
        for scene in crate::mapping::SCENES
            .iter()
            .chain(&crate::mapping::NEUTRAL_SCENES)
        {
            assert!(
                names.contains(scene),
                "{scene} missing from assets/scene_prompts.json"
            );
        }
    }
}
