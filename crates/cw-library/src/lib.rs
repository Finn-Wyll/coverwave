//! The local music library (PLAN.md section 10): a SQLite index of music
//! folders, fuzzy search, `.m3u8` playlists, synced lyrics and the saved
//! queue.
//!
//! This crate reads the filesystem and is not one of the platform-agnostic
//! ones; the pure parts (LRC parsing, search ranking) take strings and
//! slices so they are easy to test and to reuse.

pub mod db;
pub mod lyrics;
pub mod playlist;
pub mod search;
pub mod state;

pub use db::{Album, Artist, LibTrack, Library, ScanStats};
pub use lyrics::Lyrics;

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("library database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Seconds since the Unix epoch, for play statistics.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
