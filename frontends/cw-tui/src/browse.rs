//! The library browser: a stack of levels (root → artists → albums →
//! tracks, playlists, all tracks, search). Key handling returns an
//! [`Action`] for the app to carry out, so this stays testable without a
//! terminal or audio device.

use cw_audio::Track;
use cw_library::{LibTrack, Library, playlist, search};
use std::path::{Path, PathBuf};

/// Search results shown at most.
const SEARCH_LIMIT: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub enum ViewKind {
    Root,
    Artists,
    /// Albums of one artist, or every album.
    Albums(Option<String>),
    Album {
        artist: String,
        album: String,
    },
    ArtistTracks(String),
    AllTracks,
    Playlists,
    Playlist(PathBuf),
    Search,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    Open(ViewKind),
    /// Index into the level's `tracks`.
    Track(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub label: String,
    pub detail: String,
    pub kind: EntryKind,
}

#[derive(Debug, Clone)]
pub struct Level {
    pub kind: ViewKind,
    pub title: String,
    pub entries: Vec<Entry>,
    pub tracks: Vec<LibTrack>,
    pub cursor: usize,
    pub offset: usize,
}

/// What the app should do after a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Replace the queue with `tracks`, starting at `start`.
    Play {
        tracks: Vec<Track>,
        start: usize,
    },
    /// Add to the end of the queue.
    Append(Vec<Track>),
}

pub struct Browser {
    pub levels: Vec<Level>,
    pub query: String,
    playlists_dir: Option<PathBuf>,
    /// Every track, loaded for search on first use and after rescans.
    all: Option<Vec<LibTrack>>,
}

impl Browser {
    pub fn new(lib: Option<&Library>, playlists_dir: Option<PathBuf>) -> Browser {
        let mut b = Browser {
            levels: Vec::new(),
            query: String::new(),
            playlists_dir,
            all: None,
        };
        let root = b.build(ViewKind::Root, lib);
        b.levels.push(root);
        b
    }

    pub fn current(&self) -> &Level {
        self.levels.last().expect("the root level is never popped")
    }

    fn current_mut(&mut self) -> &mut Level {
        self.levels
            .last_mut()
            .expect("the root level is never popped")
    }

    pub fn in_search(&self) -> bool {
        self.current().kind == ViewKind::Search
    }

    pub fn move_by(&mut self, delta: isize) {
        let l = self.current_mut();
        if l.entries.is_empty() {
            l.cursor = 0;
            return;
        }
        let max = l.entries.len() as isize - 1;
        l.cursor = (l.cursor as isize + delta).clamp(0, max) as usize;
    }

    /// Goes up a level. Returns false at the root.
    pub fn back(&mut self) -> bool {
        if self.levels.len() > 1 {
            self.levels.pop();
            true
        } else {
            false
        }
    }

    /// Enter: open the selected entry, or play its list from the selected
    /// track.
    pub fn enter(&mut self, lib: Option<&Library>) -> Action {
        let l = self.current();
        let Some(entry) = l.entries.get(l.cursor).cloned() else {
            return Action::None;
        };
        match entry.kind {
            EntryKind::Open(kind) => {
                self.open(kind, lib);
                Action::None
            }
            EntryKind::Track(i) => Action::Play {
                tracks: l.tracks.iter().map(LibTrack::to_track).collect(),
                start: i,
            },
        }
    }

    /// `a`: add the selected track, or everything under the selected entry.
    pub fn add(&mut self, lib: Option<&Library>) -> Action {
        let l = self.current();
        let Some(entry) = l.entries.get(l.cursor).cloned() else {
            return Action::None;
        };
        let tracks: Vec<Track> = match entry.kind {
            EntryKind::Track(i) => vec![l.tracks[i].to_track()],
            EntryKind::Open(kind) => {
                let level = self.build(kind, lib);
                level.tracks.iter().map(LibTrack::to_track).collect()
            }
        };
        if tracks.is_empty() {
            Action::None
        } else {
            Action::Append(tracks)
        }
    }

    pub fn open(&mut self, kind: ViewKind, lib: Option<&Library>) {
        if kind == ViewKind::Search {
            self.query.clear();
            if self.all.is_none() {
                self.all = lib.and_then(|l| l.all_tracks().ok());
            }
        }
        let level = self.build(kind, lib);
        self.levels.push(level);
    }

    /// Edits the search query (when in search) and refreshes results.
    pub fn type_char(&mut self, c: char, lib: Option<&Library>) {
        self.query.push(c);
        self.rebuild_search(lib);
    }

    /// Backspace in search. Returns false when the query was already empty.
    pub fn delete_char(&mut self, lib: Option<&Library>) -> bool {
        if self.query.pop().is_none() {
            return false;
        }
        self.rebuild_search(lib);
        true
    }

    fn rebuild_search(&mut self, lib: Option<&Library>) {
        let level = self.build(ViewKind::Search, lib);
        let l = self.current_mut();
        l.entries = level.entries;
        l.tracks = level.tracks;
        l.title = level.title;
        l.cursor = 0;
        l.offset = 0;
    }

    /// Recomputes every level (after a rescan), keeping cursors in range.
    pub fn refresh(&mut self, lib: Option<&Library>) {
        self.all = None;
        if self.levels.iter().any(|l| l.kind == ViewKind::Search) {
            self.all = lib.and_then(|l| l.all_tracks().ok());
        }
        for i in 0..self.levels.len() {
            let kind = self.levels[i].kind.clone();
            let fresh = self.build(kind, lib);
            let l = &mut self.levels[i];
            l.title = fresh.title;
            l.entries = fresh.entries;
            l.tracks = fresh.tracks;
            l.cursor = l.cursor.min(l.entries.len().saturating_sub(1));
        }
    }

    fn build(&self, kind: ViewKind, lib: Option<&Library>) -> Level {
        let open = |label: String, detail: String, k: ViewKind| Entry {
            label,
            detail,
            kind: EntryKind::Open(k),
        };
        let mut tracks = Vec::new();
        let (title, entries) = match (&kind, lib) {
            (ViewKind::Root, _) => {
                let count = lib.and_then(|l| l.count().ok()).unwrap_or(0);
                let artists = lib.and_then(|l| l.artists().ok()).map_or(0, |a| a.len());
                let albums = lib.and_then(|l| l.albums(None).ok()).map_or(0, |a| a.len());
                let lists = self.playlists().len();
                (
                    "Library".to_string(),
                    vec![
                        open("Artists".into(), artists.to_string(), ViewKind::Artists),
                        open("Albums".into(), albums.to_string(), ViewKind::Albums(None)),
                        open("Playlists".into(), lists.to_string(), ViewKind::Playlists),
                        open("All tracks".into(), count.to_string(), ViewKind::AllTracks),
                        open("Search".into(), "/".into(), ViewKind::Search),
                    ],
                )
            }
            (ViewKind::Artists, Some(l)) => (
                "Artists".into(),
                l.artists()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| {
                        let detail = plural(a.albums, "album");
                        open(a.name.clone(), detail, ViewKind::Albums(Some(a.name)))
                    })
                    .collect(),
            ),
            (ViewKind::Albums(artist), Some(l)) => {
                let mut e = Vec::new();
                if let Some(a) = artist {
                    e.push(open(
                        format!("All tracks by {a}"),
                        String::new(),
                        ViewKind::ArtistTracks(a.clone()),
                    ));
                }
                for al in l.albums(artist.as_deref()).unwrap_or_default() {
                    let detail = if artist.is_some() {
                        plural(al.tracks, "track")
                    } else {
                        al.artist.clone()
                    };
                    e.push(open(
                        al.name.clone(),
                        detail,
                        ViewKind::Album {
                            artist: al.artist,
                            album: al.name,
                        },
                    ));
                }
                (artist.clone().unwrap_or_else(|| "Albums".into()), e)
            }
            (ViewKind::Album { artist, album }, Some(l)) => {
                tracks = l.album_tracks(artist, album).unwrap_or_default();
                (album.clone(), track_entries(&tracks, false))
            }
            (ViewKind::ArtistTracks(a), Some(l)) => {
                tracks = l.artist_tracks(a).unwrap_or_default();
                (a.clone(), track_entries(&tracks, false))
            }
            (ViewKind::AllTracks, Some(l)) => {
                tracks = l.all_tracks().unwrap_or_default();
                ("All tracks".into(), track_entries(&tracks, true))
            }
            (ViewKind::Playlists, _) => (
                "Playlists".into(),
                self.playlists()
                    .into_iter()
                    .map(|(name, path)| open(name, String::new(), ViewKind::Playlist(path)))
                    .collect(),
            ),
            (ViewKind::Playlist(path), _) => {
                tracks = playlist::load(path)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p.exists())
                    .map(|p| lib_track(lib, &p))
                    .collect();
                let name = path
                    .file_stem()
                    .map_or("Playlist".into(), |s| s.to_string_lossy().into_owned());
                (name, track_entries(&tracks, true))
            }
            (ViewKind::Search, _) => {
                let all = self.all.as_deref().unwrap_or(&[]);
                let hits = search::search(all, &self.query, cw_library::now_secs(), SEARCH_LIMIT);
                tracks = hits.into_iter().map(|i| all[i].clone()).collect();
                (
                    format!("Search: {}", self.query),
                    track_entries(&tracks, true),
                )
            }
            // A library view without a library: nothing to show.
            (_, None) => ("Library".into(), Vec::new()),
        };
        Level {
            kind,
            title,
            entries,
            tracks,
            cursor: 0,
            offset: 0,
        }
    }

    fn playlists(&self) -> Vec<(String, PathBuf)> {
        self.playlists_dir
            .as_deref()
            .map(playlist::list)
            .unwrap_or_default()
    }
}

/// A library row for `path`, or one read from the file's tags if it is not
/// indexed (playlists can point anywhere).
fn lib_track(lib: Option<&Library>, path: &Path) -> LibTrack {
    if let Some(t) = lib.and_then(|l| l.track_by_path(path).ok().flatten()) {
        return t;
    }
    let t = Track::load(path.to_path_buf());
    let i = t.info;
    LibTrack {
        id: -1,
        path: t.path,
        group_artist: i
            .album_artist
            .clone()
            .or(i.artist.clone())
            .unwrap_or_default(),
        group_album: i.album.clone().unwrap_or_default(),
        title: i.title,
        artist: i.artist,
        album: i.album,
        album_artist: i.album_artist,
        track_no: i.track_no,
        disc_no: i.disc_no,
        duration_secs: i.duration_secs,
        play_count: 0,
        last_played: None,
        genre: i.genre,
    }
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn track_entries(tracks: &[LibTrack], with_artist: bool) -> Vec<Entry> {
    tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let label = match (with_artist, &t.artist, t.track_no) {
                (true, Some(a), _) => format!("{} - {a}", t.title),
                (false, _, Some(n)) => format!("{n:>2}  {}", t.title),
                _ => t.title.clone(),
            };
            let detail = t.duration_secs.map(fmt_time).unwrap_or_default();
            Entry {
                label,
                detail,
                kind: EntryKind::Track(i),
            }
        })
        .collect()
}

pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path) {
        let samples = 800u32;
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + samples * 2).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(8000u32.to_le_bytes());
        b.extend(16000u32.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend((samples * 2).to_le_bytes());
        b.extend(vec![0u8; samples as usize * 2]);
        std::fs::write(path, b).unwrap();
    }

    fn fixture(name: &str) -> (Library, PathBuf) {
        let root = std::env::temp_dir().join(format!("cw-browse-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("music")).unwrap();
        for f in ["alpha song.wav", "beta tune.wav", "gamma.wav"] {
            write_wav(&root.join("music").join(f));
        }
        let mut lib = Library::in_memory().unwrap();
        lib.scan(&[root.join("music")]).unwrap();
        (lib, root)
    }

    #[test]
    fn drill_down_play_and_back() {
        let (lib, root) = fixture("drill");
        let mut b = Browser::new(Some(&lib), Some(root.join("playlists")));
        assert_eq!(b.current().entries[3].detail, "3"); // all tracks
        b.levels[0].cursor = 0; // Artists
        assert_eq!(b.enter(Some(&lib)), Action::None);
        assert_eq!(b.current().kind, ViewKind::Artists);
        b.enter(Some(&lib)); // the only artist
        assert_eq!(b.current().entries[0].label, "All tracks by Unknown artist");
        b.move_by(1);
        b.enter(Some(&lib)); // its album
        assert_eq!(b.current().tracks.len(), 3);
        b.move_by(1);
        match b.enter(Some(&lib)) {
            Action::Play { tracks, start } => {
                assert_eq!((tracks.len(), start), (3, 1));
                assert_eq!(tracks[1].info.title, "beta tune");
            }
            other => panic!("{other:?}"),
        }
        assert!(b.back() && b.back() && b.back());
        assert!(!b.back(), "root stays");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn add_whole_album_and_search() {
        let (lib, root) = fixture("search");
        let mut b = Browser::new(Some(&lib), None);
        b.open(ViewKind::Albums(None), Some(&lib));
        match b.add(Some(&lib)) {
            Action::Append(t) => assert_eq!(t.len(), 3),
            other => panic!("{other:?}"),
        }
        b.open(ViewKind::Search, Some(&lib));
        assert!(b.in_search() && b.current().entries.is_empty());
        for c in "beta".chars() {
            b.type_char(c, Some(&lib));
        }
        assert_eq!(b.current().tracks.len(), 1);
        assert_eq!(b.current().title, "Search: beta");
        assert!(b.delete_char(Some(&lib)));
        match b.add(Some(&lib)) {
            Action::Append(t) => assert_eq!(t[0].info.title, "beta tune"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn playlists_round_trip_through_the_browser() {
        let (lib, root) = fixture("pl");
        let dir = root.join("playlists");
        let tracks: Vec<Track> = lib
            .all_tracks()
            .unwrap()
            .iter()
            .map(LibTrack::to_track)
            .collect();
        playlist::save(&dir, "mix", &tracks[..2]).unwrap();
        let mut b = Browser::new(Some(&lib), Some(dir));
        assert_eq!(b.current().entries[2].detail, "1");
        b.open(ViewKind::Playlists, Some(&lib));
        b.enter(Some(&lib));
        assert_eq!(b.current().title, "mix");
        assert_eq!(b.current().tracks.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_library_shows_empty_views() {
        let mut b = Browser::new(None, None);
        assert_eq!(b.current().entries.len(), 5);
        b.enter(None);
        assert!(b.current().entries.is_empty());
        assert_eq!(b.add(None), Action::None);
    }
}
