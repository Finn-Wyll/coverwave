//! Where a cover's recipe comes from, in order:
//! 1. a user override, `~/.config/coverwave/overrides/<hash>.json`;
//! 2. the cache, `~/.cache/coverwave/recipes/<hash>.json`, if it was built
//!    with the current recipe version, prompts and PCA;
//! 3. rebuilt from the cached CLIP embedding (`<hash>.clip.bin`) when only
//!    the prompts or PCA changed, without running the model;
//! 4. built from scratch with the model, then cached;
//! 5. without the model (not cached, so it is redone once the model is
//!    ready).

use crate::models::{cache_dir, config_dir};
use cw_analysis::clip::EMBED_DIM;
use cw_analysis::{ClipModel, Cover, GenreFamily, Models, Pca, ScenePrompts};
use cw_core::Recipe;
use cw_core::recipe::RECIPE_VERSION;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Override,
    Cache,
    /// Rebuilt from the cached embedding.
    Rebuilt,
    Built,
    /// Built without the model: neutral scenes and DNA.
    NoModel,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Override => "override",
            Source::Cache => "cache",
            Source::Rebuilt => "rebuilt",
            Source::Built => "analysed",
            Source::NoModel => "no model yet",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub recipe: Recipe,
    pub source: Source,
    /// A problem worth showing (bad override, unwritable cache), if any.
    pub warning: Option<String>,
}

/// Directories the store reads and writes; `None` disables that part.
#[derive(Debug, Clone)]
pub struct Store {
    pub cache: Option<PathBuf>,
    pub overrides: Option<PathBuf>,
}

impl Store {
    pub fn default_dirs() -> Store {
        Store {
            cache: cache_dir().map(|d| d.join("recipes")),
            overrides: config_dir().map(|d| d.join("overrides")),
        }
    }

    pub fn load(
        &self,
        cover: &Cover,
        clip: Option<&ClipModel>,
        scenes: &ScenePrompts,
        pca: Option<&Pca>,
        genre: GenreFamily,
    ) -> Loaded {
        let hash = cw_analysis::hash_hex(&cover.hash);
        let mut warning = None;
        if let Some(dir) = &self.overrides {
            let path = dir.join(format!("{hash}.json"));
            if let Ok(text) = std::fs::read_to_string(&path) {
                match serde_json::from_str::<Recipe>(&text) {
                    Ok(recipe) => {
                        return Loaded {
                            recipe,
                            source: Source::Override,
                            warning: None,
                        };
                    }
                    Err(e) => warning = Some(format!("override ignored: {e}")),
                }
            }
        }

        let models = Models {
            clip,
            scenes,
            pca,
            genre,
        };
        let expected = models.info(true);
        let cache = self.cache.as_ref();
        // One cached recipe per cover and genre family; the embedding is
        // shared, so a new genre rebuilds without running the model.
        let json_path = cache.map(|d| d.join(format!("{hash}.{}.json", genre.name())));
        let emb_path = cache.map(|d| d.join(format!("{hash}.clip.bin")));

        if let Some(p) = &json_path
            && let Ok(text) = std::fs::read_to_string(p)
            && let Ok(recipe) = serde_json::from_str::<Recipe>(&text)
            && recipe.version == RECIPE_VERSION
            && recipe.analysis == expected
        {
            return Loaded {
                recipe,
                source: Source::Cache,
                warning,
            };
        }

        let cached_embedding = emb_path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| decode_embedding(&b));
        let (recipe, source, new_embedding) = if let Some(e) = &cached_embedding {
            let r = cw_analysis::recipe_with_embedding(cover, Some(e), models);
            (r, Source::Rebuilt, None)
        } else if clip.is_some() {
            match cw_analysis::recipe(cover, models) {
                Ok((r, e)) => (r, Source::Built, e),
                Err(err) => {
                    warning = Some(err.to_string());
                    let r = cw_analysis::recipe_with_embedding(cover, None, models);
                    (r, Source::NoModel, None)
                }
            }
        } else {
            let r = cw_analysis::recipe_with_embedding(cover, None, models);
            (r, Source::NoModel, None)
        };

        if source != Source::NoModel
            && let (Some(dir), Some(jp)) = (cache, &json_path)
        {
            let write = || -> std::io::Result<()> {
                std::fs::create_dir_all(dir)?;
                if let (Some(e), Some(ep)) = (&new_embedding, &emb_path) {
                    std::fs::write(ep, encode_embedding(e))?;
                }
                let json = serde_json::to_string_pretty(&recipe).map_err(std::io::Error::other)?;
                std::fs::write(jp, json)
            };
            if let Err(e) = write() {
                warning = Some(format!("cannot cache recipe: {e}"));
            }
        }
        Loaded {
            recipe,
            source,
            warning,
        }
    }
}

fn encode_embedding(e: &[f32]) -> Vec<u8> {
    e.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn decode_embedding(b: &[u8]) -> Option<Vec<f32>> {
    (b.len() == EMBED_DIM * 4).then(|| {
        b.as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::Rgb;

    fn cover() -> Cover {
        let mut img = image::RgbImage::new(64, 64);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 4) as u8, 80, 160]);
        }
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        cw_analysis::analyse(&png.into_inner()).unwrap()
    }

    fn temp_store(name: &str) -> (Store, PathBuf) {
        let root = std::env::temp_dir().join(format!("cw-recipes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (
            Store {
                cache: Some(root.join("cache")),
                overrides: Some(root.join("overrides")),
            },
            root,
        )
    }

    #[test]
    fn no_model_is_not_cached_but_embedding_cache_rebuilds() {
        let (store, root) = temp_store("emb");
        let scenes = ScenePrompts::shipped();
        let c = cover();
        let first = store.load(&c, None, &scenes, None, GenreFamily::Unknown);
        assert_eq!(first.source, Source::NoModel);
        assert!(!root.join("cache").exists());

        // Pretend an earlier run cached the embedding: the recipe is rebuilt
        // from it without a model, cached, and then served from the cache.
        let hash = cw_analysis::hash_hex(&c.hash);
        std::fs::create_dir_all(root.join("cache")).unwrap();
        let mut e = vec![0.0f32; EMBED_DIM];
        e[0] = 1.0;
        std::fs::write(
            root.join("cache").join(format!("{hash}.clip.bin")),
            encode_embedding(&e),
        )
        .unwrap();
        let rebuilt = store.load(&c, None, &scenes, None, GenreFamily::Unknown);
        assert_eq!(rebuilt.source, Source::Rebuilt);
        assert_eq!(rebuilt.recipe.analysis.model, "mobileclip_s0");
        assert_eq!(rebuilt.recipe.scene_weights.len(), scenes.scenes.len());
        let cached = store.load(&c, None, &scenes, None, GenreFamily::Unknown);
        assert_eq!(cached.source, Source::Cache);
        assert_eq!(cached.recipe, rebuilt.recipe);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn override_wins_and_bad_override_warns() {
        let (store, root) = temp_store("ovr");
        let scenes = ScenePrompts::shipped();
        let c = cover();
        let hash = cw_analysis::hash_hex(&c.hash);
        let mut custom = store
            .load(&c, None, &scenes, None, GenreFamily::Unknown)
            .recipe;
        custom.accent = cw_analysis::mapping::hex(Rgb(1, 2, 3));
        let dir = root.join("overrides");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{hash}.json"));
        std::fs::write(&path, serde_json::to_string(&custom).unwrap()).unwrap();
        let got = store.load(&c, None, &scenes, None, GenreFamily::Unknown);
        assert_eq!(
            (got.source, got.recipe.accent.as_str()),
            (Source::Override, "#010203")
        );

        std::fs::write(&path, "{ not json").unwrap();
        let got = store.load(&c, None, &scenes, None, GenreFamily::Unknown);
        assert_eq!(got.source, Source::NoModel);
        assert!(got.warning.unwrap().starts_with("override ignored"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
