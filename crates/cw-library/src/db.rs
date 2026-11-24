//! The SQLite index. One row per audio file; artists and albums are derived
//! by grouping (album artist, falling back to artist, then "Unknown").

use crate::LibraryError;
use cw_audio::{Track, TrackInfo, collect_audio_files};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const UNKNOWN_ARTIST: &str = "Unknown artist";
pub const UNKNOWN_ALBUM: &str = "Unknown album";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tracks (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    mtime INTEGER NOT NULL,
    size INTEGER NOT NULL,
    title TEXT NOT NULL,
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    group_artist TEXT NOT NULL,
    group_album TEXT NOT NULL,
    track_no INTEGER,
    disc_no INTEGER,
    duration REAL,
    play_count INTEGER NOT NULL DEFAULT 0,
    last_played INTEGER,
    genre TEXT
);
CREATE INDEX IF NOT EXISTS tracks_group ON tracks (group_artist, group_album);
";

/// Schema version, in `PRAGMA user_version`. 2 added `genre`.
const SCHEMA_VERSION: i64 = 2;

const COLUMNS: &str = "id, path, title, artist, album, album_artist, group_artist, group_album, \
                       track_no, disc_no, duration, play_count, last_played, genre";
const ORDER: &str = "group_artist COLLATE NOCASE, group_album COLLATE NOCASE, \
                     IFNULL(disc_no, 1), IFNULL(track_no, 0), path";

#[derive(Debug, Clone, PartialEq)]
pub struct LibTrack {
    pub id: i64,
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    /// Album artist, else artist, else "Unknown artist".
    pub group_artist: String,
    /// Album, else "Unknown album".
    pub group_album: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration_secs: Option<f64>,
    pub play_count: u32,
    pub last_played: Option<i64>,
    pub genre: Option<String>,
}

impl LibTrack {
    /// The player's view of this track (no disk access).
    pub fn to_track(&self) -> Track {
        Track {
            path: self.path.clone(),
            info: TrackInfo {
                title: self.title.clone(),
                artist: self.artist.clone(),
                album: self.album.clone(),
                album_artist: self.album_artist.clone(),
                track_no: self.track_no,
                disc_no: self.disc_no,
                genre: self.genre.clone(),
                duration_secs: self.duration_secs,
            },
        }
    }

    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<LibTrack> {
        Ok(LibTrack {
            id: r.get(0)?,
            path: PathBuf::from(r.get::<_, String>(1)?),
            title: r.get(2)?,
            artist: r.get(3)?,
            album: r.get(4)?,
            album_artist: r.get(5)?,
            group_artist: r.get(6)?,
            group_album: r.get(7)?,
            track_no: r.get(8)?,
            disc_no: r.get(9)?,
            duration_secs: r.get(10)?,
            play_count: r.get(11)?,
            last_played: r.get(12)?,
            genre: r.get(13)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub name: String,
    pub albums: usize,
    pub tracks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub artist: String,
    pub name: String,
    pub tracks: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub total: usize,
}

pub struct Library {
    conn: Connection,
}

impl Library {
    pub fn open(path: &Path) -> Result<Library, LibraryError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    /// An in-memory library, for tests.
    pub fn in_memory() -> Result<Library, LibraryError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Library, LibraryError> {
        // The UI reads while a background scan writes: wait for locks
        // instead of failing, and let readers see the last commit.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch(SCHEMA)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 2 {
            // v1 databases lack `genre`: add it, and mark every row stale so
            // the next scan re-reads tags (play counts are kept).
            if conn.prepare("SELECT genre FROM tracks LIMIT 0").is_err() {
                conn.execute_batch("ALTER TABLE tracks ADD COLUMN genre TEXT")?;
            }
            conn.execute("UPDATE tracks SET mtime = 0", [])?;
        }
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        Ok(Library { conn })
    }

    /// Indexes `roots` incrementally: new or changed files (by mtime and
    /// size) are re-read, rows for files gone from under a root are removed,
    /// and play counts survive re-tagging.
    pub fn scan(&mut self, roots: &[PathBuf]) -> Result<ScanStats, LibraryError> {
        let files = collect_audio_files(roots);
        let mut known: HashMap<String, (i64, i64)> = HashMap::new();
        {
            let mut st = self.conn.prepare("SELECT path, mtime, size FROM tracks")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?;
            for row in rows {
                let (p, m, s) = row?;
                known.insert(p, (m, s));
            }
        }
        let mut stats = ScanStats::default();
        let tx = self.conn.transaction()?;
        {
            let mut upsert = tx.prepare(
                "INSERT INTO tracks (path, mtime, size, title, artist, album, album_artist, \
                 group_artist, group_album, track_no, disc_no, duration, genre) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
                 ON CONFLICT(path) DO UPDATE SET mtime = ?2, size = ?3, title = ?4, artist = ?5, \
                 album = ?6, album_artist = ?7, group_artist = ?8, group_album = ?9, \
                 track_no = ?10, disc_no = ?11, duration = ?12, genre = ?13",
            )?;
            for file in &files {
                let key = file.to_string_lossy().into_owned();
                let (mtime, size) = file_stamp(file);
                let previous = known.remove(&key);
                if previous == Some((mtime, size)) {
                    continue;
                }
                let t = Track::load(file.clone());
                let i = &t.info;
                upsert.execute(params![
                    key,
                    mtime,
                    size,
                    i.title,
                    i.artist,
                    i.album,
                    i.album_artist,
                    group_artist(i),
                    i.album.clone().unwrap_or_else(|| UNKNOWN_ALBUM.into()),
                    i.track_no,
                    i.disc_no,
                    i.duration_secs,
                    i.genre,
                ])?;
                if previous.is_some() {
                    stats.updated += 1;
                } else {
                    stats.added += 1;
                }
            }
            // Whatever is left under a scanned root no longer exists.
            let mut delete = tx.prepare("DELETE FROM tracks WHERE path = ?1")?;
            for path in known.keys() {
                let under_root = roots.iter().any(|r| Path::new(path).starts_with(r));
                if under_root && !Path::new(path).exists() {
                    delete.execute([path])?;
                    stats.removed += 1;
                }
            }
        }
        tx.commit()?;
        stats.total = self.count()?;
        Ok(stats)
    }

    pub fn count(&self) -> Result<usize, LibraryError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0))?
            as usize)
    }

    pub fn artists(&self) -> Result<Vec<Artist>, LibraryError> {
        let mut st = self.conn.prepare(
            "SELECT group_artist, COUNT(DISTINCT group_album), COUNT(*) FROM tracks \
             GROUP BY group_artist ORDER BY group_artist COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Artist {
                name: r.get(0)?,
                albums: r.get::<_, i64>(1)? as usize,
                tracks: r.get::<_, i64>(2)? as usize,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Albums of one artist, or all albums.
    pub fn albums(&self, artist: Option<&str>) -> Result<Vec<Album>, LibraryError> {
        let sql = "SELECT group_artist, group_album, COUNT(*) FROM tracks \
                   WHERE ?1 IS NULL OR group_artist = ?1 \
                   GROUP BY group_artist, group_album \
                   ORDER BY group_album COLLATE NOCASE, group_artist COLLATE NOCASE";
        let mut st = self.conn.prepare(sql)?;
        let rows = st.query_map([artist], |r| {
            Ok(Album {
                artist: r.get(0)?,
                name: r.get(1)?,
                tracks: r.get::<_, i64>(2)? as usize,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn album_tracks(&self, artist: &str, album: &str) -> Result<Vec<LibTrack>, LibraryError> {
        self.query(
            &format!("WHERE group_artist = ?1 AND group_album = ?2 ORDER BY {ORDER}"),
            params![artist, album],
        )
    }

    pub fn artist_tracks(&self, artist: &str) -> Result<Vec<LibTrack>, LibraryError> {
        self.query(
            &format!("WHERE group_artist = ?1 ORDER BY {ORDER}"),
            params![artist],
        )
    }

    pub fn all_tracks(&self) -> Result<Vec<LibTrack>, LibraryError> {
        self.query(&format!("ORDER BY {ORDER}"), params![])
    }

    pub fn track_by_path(&self, path: &Path) -> Result<Option<LibTrack>, LibraryError> {
        let sql = format!("SELECT {COLUMNS} FROM tracks WHERE path = ?1");
        Ok(self
            .conn
            .query_row(&sql, [path.to_string_lossy()], LibTrack::from_row)
            .optional()?)
    }

    /// Counts a play (call when a track has been mostly heard).
    pub fn record_play(&self, path: &Path, at: i64) -> Result<(), LibraryError> {
        self.conn.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE path = ?1",
            params![path.to_string_lossy(), at],
        )?;
        Ok(())
    }

    fn query(&self, tail: &str, p: impl rusqlite::Params) -> Result<Vec<LibTrack>, LibraryError> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM tracks {tail}"))?;
        let rows = st.query_map(p, LibTrack::from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

fn group_artist(i: &TrackInfo) -> String {
    i.album_artist
        .clone()
        .or_else(|| i.artist.clone())
        .unwrap_or_else(|| UNKNOWN_ARTIST.into())
}

/// (mtime seconds, size) of a file, or zeros if unreadable.
fn file_stamp(p: &Path) -> (i64, i64) {
    std::fs::metadata(p).map_or((0, 0), |m| {
        let mtime = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        (mtime, m.len() as i64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny valid WAV (silence), so lofty can read its properties.
    pub(crate) fn write_wav(path: &Path) {
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

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cw-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn v1_database_migrates_keeping_plays() {
        let dir = temp_dir("migrate");
        let path = dir.join("library.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE tracks (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE,
                 mtime INTEGER NOT NULL, size INTEGER NOT NULL, title TEXT NOT NULL, artist TEXT,
                 album TEXT, album_artist TEXT, group_artist TEXT NOT NULL, group_album TEXT NOT NULL,
                 track_no INTEGER, disc_no INTEGER, duration REAL,
                 play_count INTEGER NOT NULL DEFAULT 0, last_played INTEGER);
                 INSERT INTO tracks (path, mtime, size, title, group_artist, group_album, play_count)
                 VALUES ('/x.flac', 123, 4, 'X', 'A', 'B', 7);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let lib = Library::open(&path).unwrap();
        let t = lib.track_by_path(Path::new("/x.flac")).unwrap().unwrap();
        assert_eq!((t.play_count, t.genre), (7, None));
        let mtime: i64 = lib
            .conn
            .query_row("SELECT mtime FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            mtime, 0,
            "rows are marked stale so the next scan reads genres"
        );
        drop(lib);
        // Reopening does not reset again.
        let lib = Library::open(&path).unwrap();
        lib.conn.execute("UPDATE tracks SET mtime = 5", []).unwrap();
        drop(lib);
        let lib = Library::open(&path).unwrap();
        let mtime: i64 = lib
            .conn
            .query_row("SELECT mtime FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mtime, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_is_incremental_and_groups_by_folder_tags() {
        let root = temp_dir("scan");
        std::fs::create_dir_all(root.join("A/One")).unwrap();
        for f in ["A/One/01 a.wav", "A/One/02 b.wav", "c.wav"] {
            write_wav(&root.join(f));
        }
        let mut lib = Library::in_memory().unwrap();
        let s = lib.scan(std::slice::from_ref(&root)).unwrap();
        assert_eq!((s.added, s.updated, s.removed, s.total), (3, 0, 0, 3));
        // Untagged files group under Unknown artist / Unknown album.
        let artists = lib.artists().unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].name, UNKNOWN_ARTIST);
        assert_eq!(lib.albums(Some(UNKNOWN_ARTIST)).unwrap()[0].tracks, 3);
        let titles: Vec<String> = lib
            .all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["01 a", "02 b", "c"]);

        // Nothing changed: nothing re-read.
        let s = lib.scan(std::slice::from_ref(&root)).unwrap();
        assert_eq!((s.added, s.updated, s.removed), (0, 0, 0));

        // Plays survive; deleted files disappear.
        let b = root.join("A/One/02 b.wav");
        lib.record_play(&b, 1234).unwrap();
        let t = lib.track_by_path(&b).unwrap().unwrap();
        assert_eq!((t.play_count, t.last_played), (1, Some(1234)));
        std::fs::remove_file(root.join("c.wav")).unwrap();
        let s = lib.scan(std::slice::from_ref(&root)).unwrap();
        assert_eq!((s.removed, s.total), (1, 2));
        assert_eq!(lib.track_by_path(&b).unwrap().unwrap().play_count, 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
