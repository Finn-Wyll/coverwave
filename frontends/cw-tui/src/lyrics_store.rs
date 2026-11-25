//! Lyrics for whatever is playing, local or remote: a sidecar `.lrc` or the
//! file's tags first, then a disk cache, then LRCLIB on a background thread
//! (`cw_sources::lrclib`, when `[lyrics] online_lookup` is on). Results are
//! cached in `<cache>/lyrics/`, misses for a week.

use cw_library::lyrics::{Lyrics, parse_lrc};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, SystemTime};

/// A track to find lyrics for.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRef {
    /// Stable per track: a file path or a remote track ID.
    pub key: String,
    pub path: Option<PathBuf>,
    pub artist: Option<String>,
    pub title: String,
    pub album: Option<String>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Searching,
    Found(Lyrics),
    Instrumental,
    NotFound,
}

/// How long a miss is remembered before LRCLIB is asked again.
const MISS_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const INSTRUMENTAL: &str = "[instrumental]";

pub struct LyricsStore {
    key: Option<String>,
    status: Status,
    pending: Option<Receiver<(String, Status)>>,
    cache: Option<PathBuf>,
    online: bool,
}

impl LyricsStore {
    pub fn new(cache: Option<PathBuf>, online: bool) -> LyricsStore {
        LyricsStore {
            key: None,
            status: Status::Idle,
            pending: None,
            cache,
            online,
        }
    }

    pub fn online(&self) -> bool {
        self.online
    }

    /// Turns LRCLIB lookups on or off for the next songs.
    pub fn set_online(&mut self, on: bool) {
        self.online = on;
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn current(&self) -> Option<&Lyrics> {
        match &self.status {
            Status::Found(l) => Some(l),
            _ => None,
        }
    }

    /// Call every tick with what is playing: starts a lookup on a new track
    /// and collects finished ones.
    pub fn update(&mut self, track: Option<TrackRef>) {
        if let Some(rx) = &self.pending
            && let Ok((key, status)) = rx.try_recv()
        {
            self.pending = None;
            if self.key.as_deref() == Some(key.as_str()) {
                self.status = status;
            }
        }
        let Some(track) = track else {
            self.key = None;
            self.status = Status::Idle;
            return;
        };
        if self.key.as_deref() == Some(track.key.as_str()) {
            return;
        }
        self.key = Some(track.key.clone());
        self.status = self.load_now(&track).unwrap_or_else(|| self.fetch(track));
    }

    /// Local files and the cache, quickly, on this thread.
    fn load_now(&self, t: &TrackRef) -> Option<Status> {
        if let Some(l) = t.path.as_deref().and_then(cw_library::lyrics::load) {
            return Some(Status::Found(l));
        }
        let file = self.cache_file(&t.key)?;
        let text = std::fs::read_to_string(&file).ok()?;
        if text.trim().is_empty() {
            let age = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
            let fresh = SystemTime::now().duration_since(age).unwrap_or_default() < MISS_TTL;
            return fresh.then_some(Status::NotFound);
        }
        if text.trim() == INSTRUMENTAL {
            return Some(Status::Instrumental);
        }
        parse_lrc(&text, "lrclib").map(Status::Found)
    }

    /// LRCLIB on a background thread.
    fn fetch(&mut self, t: TrackRef) -> Status {
        if !self.online {
            return Status::NotFound;
        }
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let file = self.cache_file(&t.key);
        std::thread::spawn(move || {
            let status = look_up(file.as_deref(), &t).unwrap_or(Status::NotFound);
            let _ = tx.send((t.key, status));
        });
        Status::Searching
    }

    fn cache_file(&self, key: &str) -> Option<PathBuf> {
        Some(cache_file_in(self.cache.as_ref()?, key))
    }
}

/// The cache file for a track key in `dir`.
pub fn cache_file_in(dir: &Path, key: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    dir.join(format!("{:016x}.lrc", h.finish()))
}

/// Asks LRCLIB and caches the answer in `file`. `None` when LRCLIB could
/// not be reached (nothing is cached then, so it is asked again later).
pub fn look_up(file: Option<&Path>, t: &TrackRef) -> Option<Status> {
    let artist = t.artist.clone().unwrap_or_default();
    let found =
        cw_sources::lrclib::fetch_checked(&artist, &t.title, t.album.as_deref(), t.duration)?;
    let (status, saved) = match found {
        Some(cw_sources::lrclib::Found::Lyrics(text)) => match parse_lrc(&text, "lrclib") {
            Some(l) => (Status::Found(l), text),
            None => (Status::NotFound, String::new()),
        },
        Some(cw_sources::lrclib::Found::Instrumental) => {
            (Status::Instrumental, INSTRUMENTAL.to_string())
        }
        None => (Status::NotFound, String::new()),
    };
    if let Some(file) = file {
        let _ = std::fs::create_dir_all(file.parent().unwrap_or(file));
        let _ = std::fs::write(file, saved);
    }
    Some(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(key: &str) -> TrackRef {
        TrackRef {
            key: key.into(),
            path: None,
            artist: Some("A".into()),
            title: "T".into(),
            album: None,
            duration: Some(100.0),
        }
    }

    #[test]
    fn reads_the_cache_and_remembers_misses_offline() {
        let dir = std::env::temp_dir().join(format!("cw-lyrics-{}", std::process::id()));
        let mut s = LyricsStore::new(Some(dir.clone()), false);
        // A cached LRC is used without any lookup.
        std::fs::create_dir_all(&dir).unwrap();
        let file = s.cache_file("spotify:track:x").unwrap();
        std::fs::write(&file, "[00:01.00]hello").unwrap();
        s.update(Some(track("spotify:track:x")));
        assert_eq!(s.current().unwrap().lines[0].text, "hello");
        // Instrumental marker.
        std::fs::write(s.cache_file("inst").unwrap(), INSTRUMENTAL).unwrap();
        s.update(Some(track("inst")));
        assert_eq!(s.status(), &Status::Instrumental);
        // Offline and not cached: not found, no thread.
        s.update(Some(track("unknown")));
        assert_eq!(s.status(), &Status::NotFound);
        assert!(s.pending.is_none());
        s.update(None);
        assert_eq!(s.status(), &Status::Idle);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
