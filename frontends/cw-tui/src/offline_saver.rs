//! Saves ahead, in the background, what each saved Spotify track needs to
//! play offline with its own motion: the cover, the genre (from Deezer when
//! Spotify gave none), the tempo and the lyrics. The track list itself comes
//! from the Spotify source (`cw_sources::offline`), which saves the liked
//! songs and playlists while online.
//!
//! One track at a time with a pause between, and only while the internet
//! answers; a pass ends at the first sign of being offline and the next
//! starts a few minutes later.

use crate::lyrics_store::{TrackRef, cache_file_in, look_up};
use cw_sources::offline::{OfflineStore, SaveAhead, TrackRecord};
use cw_sources::{ArtRef, NowPlaying};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

const PAUSE: Duration = Duration::from_millis(250);
const BETWEEN_PASSES: Duration = Duration::from_secs(300);

/// Shared settings and progress.
#[derive(Default)]
pub struct SaverState {
    /// `SaveAhead` as a number (0 off, 1 liked, 2 all).
    pub mode: AtomicU8,
    pub tempo: AtomicBool,
    pub lyrics: AtomicBool,
    /// Tracks with everything saved, and tracks known.
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub stop: AtomicBool,
}

impl SaverState {
    pub fn set_mode(&self, m: SaveAhead) {
        let n = match m {
            SaveAhead::Off => 0,
            SaveAhead::Liked => 1,
            SaveAhead::All => 2,
        };
        self.mode.store(n, Ordering::Relaxed);
    }

    fn on(&self) -> bool {
        self.mode.load(Ordering::Relaxed) != 0 && !self.stop.load(Ordering::Relaxed)
    }
}

/// Whether a record has all it can get.
fn complete(r: &TrackRecord, store: &OfflineStore, tempo: bool, lyrics: bool) -> bool {
    let cover = r
        .art_url
        .as_deref()
        .is_none_or(|u| store.has_cover_for_url(u));
    cover && r.genre_checked && (!tempo || r.bpm_checked) && (!lyrics || r.lyrics_checked)
}

fn now_playing(r: &TrackRecord) -> NowPlaying {
    NowPlaying {
        track_id: r.id.clone(),
        title: r.title.clone(),
        artist: r.artist.clone(),
        album: r.album.clone(),
        duration_secs: r.duration_secs,
        position_secs: 0.0,
        playing: false,
        volume: None,
        art: ArtRef::None,
        genre: r.genre.clone(),
        isrc: r.isrc.clone(),
        bpm: None,
        device: None,
    }
}

/// Fills in one track. `false` when the internet stopped answering.
fn save_one(
    r: &TrackRecord,
    store: &OfflineStore,
    lyrics_dir: Option<&PathBuf>,
    st: &SaverState,
) -> bool {
    let mut update = TrackRecord {
        id: r.id.clone(),
        ..Default::default()
    };
    if let Some(url) = r.art_url.as_deref().filter(|u| !store.has_cover_for_url(u)) {
        match cw_sources::fetch_art(&ArtRef::Url(url.to_string())) {
            Some(bytes) => store.save_cover(url, &bytes),
            None => return false,
        }
    }
    let np = now_playing(r);
    if !r.genre_checked {
        update.genre = r
            .genre
            .clone()
            .or_else(|| cw_sources::tempo::album_genre(&np));
        update.genre_checked = true;
    }
    if st.tempo.load(Ordering::Relaxed) && !r.bpm_checked {
        update.bpm = cw_sources::tempo::lookup(&np);
        update.bpm_checked = true;
    }
    if st.lyrics.load(Ordering::Relaxed) && !r.lyrics_checked {
        let track = TrackRef {
            key: format!("remote:{}", r.id),
            path: None,
            artist: r.artist.clone(),
            title: r.title.clone(),
            album: r.album.clone(),
            duration: r.duration_secs,
        };
        let file = lyrics_dir.map(|d| cache_file_in(d, &track.key));
        // Already cached by the lyrics view: nothing to ask.
        if !file.as_ref().is_some_and(|f| f.exists()) && look_up(file.as_deref(), &track).is_none()
        {
            return false;
        }
        update.lyrics_checked = true;
    }
    store.upsert([update]);
    true
}

/// Starts the saver thread.
pub fn spawn(store: OfflineStore, lyrics_dir: Option<PathBuf>, state: Arc<SaverState>) {
    let _ = std::thread::Builder::new()
        .name("cw-offline-saver".into())
        .spawn(move || {
            loop {
                if state.stop.load(Ordering::Relaxed) {
                    return;
                }
                if state.on() && cw_sources::tempo::online() {
                    let tempo = state.tempo.load(Ordering::Relaxed);
                    let lyrics = state.lyrics.load(Ordering::Relaxed);
                    let records = store.records();
                    state.total.store(records.len(), Ordering::Relaxed);
                    let mut done = records
                        .iter()
                        .filter(|r| complete(r, &store, tempo, lyrics))
                        .count();
                    state.done.store(done, Ordering::Relaxed);
                    for r in records
                        .iter()
                        .filter(|r| !complete(r, &store, tempo, lyrics))
                    {
                        if !state.on() || !save_one(r, &store, lyrics_dir.as_ref(), &state) {
                            break;
                        }
                        done += 1;
                        state.done.store(done, Ordering::Relaxed);
                        if done % 20 == 0 {
                            store.flush();
                        }
                        std::thread::sleep(PAUSE);
                    }
                    store.flush();
                }
                // Wake often enough to notice a stop or a new setting.
                for _ in 0..BETWEEN_PASSES.as_secs() {
                    if state.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        });
}
