//! What coverwave keeps for listening without the internet (PLAN.md section
//! 9): the library listings last seen (or saved ahead), and per track what
//! the motion needs (cover, genre, tempo). Lyrics are kept by the
//! frontend's own lyrics cache.
//!
//! Spotify's downloaded songs themselves stay inside the Spotify app
//! (encrypted, readable only by it); coverwave follows that app while
//! offline and fills in the rest from here.
//!
//! Layout under the store's folder: `tracks.json` (one record per track),
//! `library/<hash>.json` (one listing per place), `covers/<hash>.img`.

use crate::{LibraryItem, LibraryPath};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// What to save ahead, while online, for listening offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveAhead {
    Off,
    /// Liked songs only.
    #[default]
    Liked,
    /// Liked songs and every playlist that can be listed.
    All,
}

impl SaveAhead {
    pub fn from_name(s: &str) -> Option<SaveAhead> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(SaveAhead::Off),
            "liked" => Some(SaveAhead::Liked),
            "all" | "playlists" => Some(SaveAhead::All),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SaveAhead::Off => "off",
            SaveAhead::Liked => "liked",
            SaveAhead::All => "all",
        }
    }
}

/// One track, as far as it is known.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TrackRecord {
    /// Spotify's bare track ID (as `NowPlaying::track_id`).
    pub id: String,
    pub uri: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
    pub isrc: Option<String>,
    /// The largest cover image's URL.
    pub art_url: Option<String>,
    pub genre: Option<String>,
    pub bpm: Option<f32>,
    /// Lookups already tried (so a miss is not repeated).
    #[serde(default)]
    pub genre_checked: bool,
    #[serde(default)]
    pub bpm_checked: bool,
    #[serde(default)]
    pub lyrics_checked: bool,
}

impl TrackRecord {
    /// Takes what `other` knows that this does not.
    fn merge(&mut self, other: TrackRecord) {
        let keep = |a: &mut Option<String>, b: Option<String>| {
            if b.is_some() {
                *a = b;
            }
        };
        if !other.title.is_empty() {
            self.title = other.title;
        }
        if !other.uri.is_empty() {
            self.uri = other.uri;
        }
        keep(&mut self.artist, other.artist);
        keep(&mut self.album, other.album);
        keep(&mut self.isrc, other.isrc);
        keep(&mut self.art_url, other.art_url);
        keep(&mut self.genre, other.genre);
        if other.duration_secs.is_some() {
            self.duration_secs = other.duration_secs;
        }
        if other.bpm.is_some() {
            self.bpm = other.bpm;
        }
        self.genre_checked |= other.genre_checked;
        self.bpm_checked |= other.bpm_checked;
        self.lyrics_checked |= other.lyrics_checked;
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    tracks: HashMap<String, TrackRecord>,
    /// Unix seconds of the last full save-ahead pass.
    #[serde(default)]
    last_pass: u64,
}

#[derive(Serialize, Deserialize)]
struct Listing {
    path: LibraryPath,
    items: Vec<LibraryItem>,
}

struct Inner {
    dir: PathBuf,
    index: Index,
    dirty: bool,
}

/// Shared between the source thread and the saver; cheap to clone.
#[derive(Clone)]
pub struct OfflineStore(Arc<Mutex<Inner>>);

/// A short file name for a key.
fn hashed(key: &str) -> String {
    Sha256::digest(key.as_bytes())[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Writes through a temporary file, so a crash never leaves half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

impl OfflineStore {
    /// Opens (or starts) the store in `dir`.
    pub fn open(dir: PathBuf) -> OfflineStore {
        let index = std::fs::read(dir.join("tracks.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        OfflineStore(Arc::new(Mutex::new(Inner {
            dir,
            index,
            dirty: false,
        })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn dir(&self) -> PathBuf {
        self.lock().dir.clone()
    }

    // Listings

    pub fn save_listing(&self, path: &LibraryPath, items: &[LibraryItem]) {
        if matches!(path, LibraryPath::Search { .. } | LibraryPath::Devices) {
            return;
        }
        let file = self.listing_file(path);
        let listing = Listing {
            path: path.clone(),
            items: items.to_vec(),
        };
        if let Ok(bytes) = serde_json::to_vec(&listing) {
            let _ = write_atomic(&file, &bytes);
        }
    }

    pub fn listing(&self, path: &LibraryPath) -> Option<Vec<LibraryItem>> {
        let bytes = std::fs::read(self.listing_file(path)).ok()?;
        let l: Listing = serde_json::from_slice(&bytes).ok()?;
        (l.path == *path).then_some(l.items)
    }

    fn listing_file(&self, path: &LibraryPath) -> PathBuf {
        // Paths with names: only the stable parts make the key.
        let key = match path {
            LibraryPath::Playlist { id, .. } => format!("playlist:{id}"),
            LibraryPath::Album { id, .. } => format!("album:{id}"),
            other => format!("{other:?}"),
        };
        self.lock()
            .dir
            .join("library")
            .join(format!("{}.json", hashed(&key)))
    }

    // Tracks

    /// Adds or updates tracks; call [`flush`](Self::flush) to write.
    pub fn upsert(&self, records: impl IntoIterator<Item = TrackRecord>) {
        let mut inner = self.lock();
        for r in records {
            if r.id.is_empty() {
                continue;
            }
            match inner.index.tracks.get_mut(&r.id) {
                Some(old) => {
                    let before = old.clone();
                    old.merge(r);
                    if *old != before {
                        inner.dirty = true;
                    }
                }
                None => {
                    inner.index.tracks.insert(r.id.clone(), r);
                    inner.dirty = true;
                }
            }
        }
    }

    /// Writes the track index if it changed.
    pub fn flush(&self) {
        let mut inner = self.lock();
        if !inner.dirty {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec(&inner.index) {
            let file = inner.dir.join("tracks.json");
            if write_atomic(&file, &bytes).is_ok() {
                inner.dirty = false;
            }
        }
    }

    pub fn record(&self, id: &str) -> Option<TrackRecord> {
        self.lock().index.tracks.get(id).cloned()
    }

    pub fn track_count(&self) -> usize {
        self.lock().index.tracks.len()
    }

    /// Every record, for the saver.
    pub fn records(&self) -> Vec<TrackRecord> {
        self.lock().index.tracks.values().cloned().collect()
    }

    /// Saved tracks whose title, artist or album contains every word of
    /// `query` (case-insensitive), at most `limit`, by title.
    pub fn search(&self, query: &str, limit: usize) -> Vec<TrackRecord> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|w| !w.is_empty())
            .collect();
        if words.is_empty() {
            return Vec::new();
        }
        let inner = self.lock();
        let mut hits: Vec<TrackRecord> = inner
            .index
            .tracks
            .values()
            .filter(|t| {
                let hay = format!(
                    "{} {} {}",
                    t.title,
                    t.artist.as_deref().unwrap_or(""),
                    t.album.as_deref().unwrap_or("")
                )
                .to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .cloned()
            .collect();
        hits.sort_by_key(|t| t.title.to_lowercase());
        hits.truncate(limit);
        hits
    }

    // Covers

    /// The saved cover for a track, if any.
    pub fn cover_path(&self, id: &str) -> Option<PathBuf> {
        let url = self.record(id)?.art_url?;
        let p = self.cover_file(&url);
        p.exists().then_some(p)
    }

    pub fn has_cover_for_url(&self, url: &str) -> bool {
        self.cover_file(url).exists()
    }

    pub fn save_cover(&self, url: &str, bytes: &[u8]) {
        let _ = write_atomic(&self.cover_file(url), bytes);
    }

    fn cover_file(&self, url: &str) -> PathBuf {
        self.lock()
            .dir
            .join("covers")
            .join(format!("{}.img", hashed(url)))
    }

    // Save-ahead passes

    /// Seconds since the last full save-ahead pass.
    pub fn since_last_pass(&self) -> u64 {
        now_secs().saturating_sub(self.lock().index.last_pass)
    }

    pub fn finish_pass(&self) {
        let mut inner = self.lock();
        inner.index.last_pass = now_secs();
        inner.dirty = true;
        drop(inner);
        self.flush();
    }

    /// Bytes used on disk (covers, listings and the index).
    pub fn disk_bytes(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            std::fs::read_dir(p).map_or(0, |rd| {
                rd.flatten()
                    .map(|e| match e.metadata() {
                        Ok(m) if m.is_dir() => walk(&e.path()),
                        Ok(m) => m.len(),
                        Err(_) => 0,
                    })
                    .sum()
            })
        }
        walk(&self.dir())
    }

    /// Forgets everything saved.
    pub fn clear(&self) {
        let mut inner = self.lock();
        let _ = std::fs::remove_dir_all(&inner.dir);
        inner.index = Index::default();
        inner.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ItemAction, PlayRequest};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cw-offline-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn records_merge_persist_and_search() {
        let dir = temp("records");
        let s = OfflineStore::open(dir.clone());
        s.upsert([TrackRecord {
            id: "a".into(),
            uri: "spotify:track:a".into(),
            title: "Karma Police".into(),
            artist: Some("Radiohead".into()),
            art_url: Some("https://i/1".into()),
            ..Default::default()
        }]);
        // A later record without the art keeps it, and adds a genre.
        s.upsert([TrackRecord {
            id: "a".into(),
            title: "Karma Police".into(),
            genre: Some("rock".into()),
            genre_checked: true,
            ..Default::default()
        }]);
        s.flush();
        let again = OfflineStore::open(dir.clone());
        let r = again.record("a").unwrap();
        assert_eq!(r.art_url.as_deref(), Some("https://i/1"));
        assert_eq!(r.genre.as_deref(), Some("rock"));
        assert!(r.genre_checked && !r.bpm_checked);
        assert_eq!(again.search("radiohead karma", 10).len(), 1);
        assert!(again.search("blur", 10).is_empty());
        assert!(again.search("  ", 10).is_empty());
        // Covers by URL.
        assert!(again.cover_path("a").is_none());
        again.save_cover("https://i/1", b"jpeg");
        assert_eq!(
            std::fs::read(again.cover_path("a").unwrap()).unwrap(),
            b"jpeg"
        );
        assert!(again.disk_bytes() > 0);
        again.clear();
        assert_eq!(again.track_count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listings_round_trip_by_stable_key() {
        let dir = temp("listings");
        let s = OfflineStore::open(dir.clone());
        let path = LibraryPath::Playlist {
            id: "p1".into(),
            name: "Road".into(),
            uri: "spotify:playlist:p1".into(),
        };
        let items = vec![LibraryItem {
            title: "Song".into(),
            subtitle: None,
            count: None,
            duration_secs: Some(200),
            action: ItemAction::Play(PlayRequest {
                context_uri: Some("spotify:playlist:p1".into()),
                offset_uri: Some("spotify:track:x".into()),
                uris: Vec::new(),
            }),
        }];
        s.save_listing(&path, &items);
        assert_eq!(s.listing(&path), Some(items));
        assert_eq!(s.listing(&LibraryPath::LikedSongs), None);
        // Searches are never saved.
        s.save_listing(&LibraryPath::Search { query: "x".into() }, &[]);
        assert_eq!(s.listing(&LibraryPath::Search { query: "x".into() }), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
