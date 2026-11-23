//! The queue saved on quit and restored on the next start.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedQueue {
    /// Tracks in list order.
    pub paths: Vec<PathBuf>,
    /// Index into `paths` of the track that was playing.
    pub current: usize,
    pub position_secs: f64,
    pub shuffle: bool,
    pub repeat: String,
    pub volume: u8,
}

impl SavedQueue {
    pub fn load(path: &Path) -> Option<SavedQueue> {
        let text = std::fs::read_to_string(path).ok()?;
        let q: SavedQueue = serde_json::from_str(&text).ok()?;
        (!q.paths.is_empty()).then_some(q)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_empty_is_none() {
        let dir = std::env::temp_dir().join(format!("cw-state-{}", std::process::id()));
        let path = dir.join("queue.json");
        let q = SavedQueue {
            paths: vec!["/a.flac".into(), "/b.mp3".into()],
            current: 1,
            position_secs: 42.5,
            shuffle: true,
            repeat: "all".into(),
            volume: 70,
        };
        q.save(&path).unwrap();
        assert_eq!(SavedQueue::load(&path), Some(q.clone()));
        SavedQueue { paths: vec![], ..q }.save(&path).unwrap();
        assert_eq!(SavedQueue::load(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
