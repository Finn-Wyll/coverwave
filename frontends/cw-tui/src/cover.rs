//! Loads and analyses cover art off the UI thread.
//!
//! Each request reads the track's art (embedded or folder image) and runs
//! `cw_analysis::analyse` on a short-lived worker thread. Results come back
//! over a channel and are polled once per frame. Covers with the same seed
//! (an album's tracks usually share one image) are shared via `Arc`.

use cw_analysis::Cover;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Debug, Clone)]
pub enum Slot {
    Loading,
    Ready(Arc<Cover>),
    /// No art found, or it could not be decoded. The reason shows in the
    /// debug overlay.
    Missing(String),
}

type Message = (usize, Result<Cover, String>);

pub struct CoverLoader {
    slots: HashMap<usize, Slot>,
    by_seed: HashMap<u64, Arc<Cover>>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}

impl CoverLoader {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        CoverLoader {
            slots: HashMap::new(),
            by_seed: HashMap::new(),
            tx,
            rx,
        }
    }

    /// Starts loading the cover for track `index` unless it is already
    /// loaded or loading.
    pub fn request(&mut self, index: usize, path: PathBuf) {
        if self.slots.contains_key(&index) {
            return;
        }
        self.slots.insert(index, Slot::Loading);
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("cw-cover".into())
            .spawn(move || {
                let result = match cw_audio::cover_art(&path) {
                    None => Err("no cover art".to_string()),
                    Some(bytes) => cw_analysis::analyse(&bytes).map_err(|e| e.to_string()),
                };
                // The receiver only disappears when the app is quitting.
                let _ = tx.send((index, result));
            });
        if let Err(e) = spawned {
            self.slots
                .insert(index, Slot::Missing(format!("cannot start loader: {e}")));
        }
    }

    /// Collects finished loads. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok((index, result)) = self.rx.try_recv() {
            let slot = match result {
                Ok(cover) => {
                    let shared = self
                        .by_seed
                        .entry(cover.seed)
                        .or_insert_with(|| Arc::new(cover))
                        .clone();
                    Slot::Ready(shared)
                }
                Err(e) => Slot::Missing(e),
            };
            self.slots.insert(index, slot);
        }
    }

    pub fn get(&self, index: usize) -> Option<&Slot> {
        self.slots.get(&index)
    }
}
