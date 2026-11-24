//! Loads and analyses cover art off the UI thread.
//!
//! Each request reads the track's art (embedded or folder image), runs
//! `cw_analysis::analyse`, and loads the cover's recipe (override, cache or
//! fresh analysis; see `recipes`) on a short-lived worker thread. Results
//! come back over a channel and are polled once per frame. Covers with the
//! same seed (an album's tracks usually share one image) share one entry.

use crate::models::ModelHandle;
use crate::recipes::{Loaded, Source, Store};
use cw_analysis::{Cover, GenreFamily, Pca, ScenePrompts};
use std::collections::HashMap;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// A cover and its recipe.
#[derive(Debug)]
pub struct CoverEntry {
    /// Unique per load, so caches can tell a reloaded recipe apart.
    pub id: u64,
    pub cover: Cover,
    pub recipe: Loaded,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Deref for CoverEntry {
    type Target = Cover;
    fn deref(&self) -> &Cover {
        &self.cover
    }
}

#[derive(Debug, Clone)]
pub enum Slot {
    Loading,
    Ready(Arc<CoverEntry>),
    /// No art found, or it could not be decoded. The reason shows in the
    /// debug overlay.
    Missing(String),
}

/// What workers need to build recipes. Shared, read-only.
pub struct RecipeContext {
    pub store: Store,
    pub scenes: ScenePrompts,
    pub pca: Option<Pca>,
    pub model: ModelHandle,
}

type Message = (String, Result<CoverEntry, String>);

pub struct CoverLoader {
    /// Keyed by audio file path, so a replaced queue never shows a stale
    /// cover.
    slots: HashMap<String, Slot>,
    /// Shared entries by (cover seed, genre family).
    by_seed: HashMap<(u64, &'static str), Arc<CoverEntry>>,
    ctx: Arc<RecipeContext>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}

impl CoverLoader {
    pub fn new(ctx: RecipeContext) -> Self {
        let (tx, rx) = channel();
        CoverLoader {
            slots: HashMap::new(),
            by_seed: HashMap::new(),
            ctx: Arc::new(ctx),
            tx,
            rx,
        }
    }

    pub fn model(&self) -> &ModelHandle {
        &self.ctx.model
    }

    /// Starts loading the cover for the audio file at `path` unless it is
    /// already loaded or loading.
    pub fn request(&mut self, path: &Path, genre: GenreFamily) {
        let p = path.to_path_buf();
        self.request_with(&local_key(path), genre, move || cw_audio::cover_art(&p));
    }

    /// Starts loading the cover identified by `key`, whose bytes `fetch`
    /// produces (it runs on the worker thread: disk or network is fine),
    /// unless it is already loaded or loading.
    pub fn request_with(
        &mut self,
        key: &str,
        genre: GenreFamily,
        fetch: impl FnOnce() -> Option<Vec<u8>> + Send + 'static,
    ) {
        let key = genre_key(key, genre);
        if self.slots.contains_key(&key) {
            return;
        }
        let path = key.clone();
        self.slots.insert(key.clone(), Slot::Loading);
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("cw-cover".into())
            .spawn(move || {
                let result = fetch()
                    .ok_or_else(|| "no cover art".to_string())
                    .and_then(|bytes| cw_analysis::analyse(&bytes).map_err(|e| e.to_string()))
                    .map(|cover| {
                        let clip = ctx.model.ready();
                        let recipe = ctx.store.load(
                            &cover,
                            clip.as_deref(),
                            &ctx.scenes,
                            ctx.pca.as_ref(),
                            genre,
                        );
                        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        CoverEntry { id, cover, recipe }
                    });
                // The receiver only disappears when the app is quitting.
                let _ = tx.send((path, result));
            });
        if let Err(e) = spawned {
            self.slots
                .insert(key, Slot::Missing(format!("cannot start loader: {e}")));
        }
    }

    /// Collects finished loads. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok((path, result)) = self.rx.try_recv() {
            let slot = match result {
                Ok(entry) => {
                    let shared = self
                        .by_seed
                        .entry((entry.cover.seed, genre_of(&entry)))
                        .or_insert_with(|| Arc::new(entry))
                        .clone();
                    Slot::Ready(shared)
                }
                Err(e) => Slot::Missing(e),
            };
            self.slots.insert(path, slot);
        }
    }

    pub fn get(&self, path: &Path, genre: GenreFamily) -> Option<&Slot> {
        self.slots.get(&genre_key(&local_key(path), genre))
    }

    pub fn get_key(&self, key: &str, genre: GenreFamily) -> Option<&Slot> {
        self.slots.get(&genre_key(key, genre))
    }

    /// Forgets finished entries matching `pred`, so the next request loads
    /// them again. Entries still loading are left alone.
    pub fn invalidate(&mut self, pred: impl Fn(&CoverEntry) -> bool) {
        self.slots
            .retain(|_, s| !matches!(s, Slot::Ready(e) if pred(e)));
        self.by_seed.retain(|_, e| !pred(e));
    }

    /// Forgets covers whose recipe was built before the model was ready.
    pub fn invalidate_modelless(&mut self) {
        self.invalidate(|e| e.recipe.source == Source::NoModel);
    }
}

/// The genre family recorded in an entry's recipe.
fn genre_of(e: &CoverEntry) -> &'static str {
    let g = e.recipe.recipe.analysis.genre.as_str();
    GenreFamily::of(g).name()
}

/// Slot key: the cover's key plus the genre family that shapes its recipe.
fn genre_key(key: &str, genre: GenreFamily) -> String {
    format!("{key}#{}", genre.name())
}

/// Cache key for a local file's cover.
pub fn local_key(path: &Path) -> String {
    format!("file:{}", path.display())
}
