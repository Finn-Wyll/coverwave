//! Remote sources (PLAN.md section 9): Spotify through its Web API and Apple
//! Music through AppleScript. Each runs on its own thread behind a
//! [`RemoteHandle`], so the UI never waits on the network or `osascript`.
//! The UI reads the latest [`RemoteState`] each frame and sends
//! [`Command`]s.
//!
//! Local files are not a "source" here: the local player already has a
//! richer API (queue, gapless playback) in `cw-audio`.

pub mod apple_music;
#[cfg(feature = "http")]
pub mod spotify;
#[cfg(feature = "http")]
pub mod tempo;
mod util;

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a cover can be fetched from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtRef {
    None,
    Url(String),
    File(std::path::PathBuf),
}

/// What a remote player is playing.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    /// Stable per track, to notice track changes.
    pub track_id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
    /// Position when the state was read (see [`RemoteState::position_now`]).
    pub position_secs: f64,
    pub playing: bool,
    /// 0..=100 when the player reports it.
    pub volume: Option<u8>,
    pub art: ArtRef,
    /// Genre(s) as the service reports them ("Rock; Pop").
    pub genre: Option<String>,
    /// International Standard Recording Code, for tempo lookups.
    pub isrc: Option<String>,
    /// Tempo, once looked up (see `tempo`); filled in by [`RemoteHandle`].
    pub bpm: Option<f32>,
}

/// A place in a remote library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryPath {
    /// The top level: liked songs, playlists, albums.
    Root,
    LikedSongs,
    Playlists,
    Albums,
    /// One playlist's tracks (`id`, display name, context URI).
    Playlist {
        id: String,
        name: String,
        uri: String,
    },
    /// One album's tracks.
    Album {
        id: String,
        name: String,
        uri: String,
    },
}

impl LibraryPath {
    /// A short name for headings and breadcrumbs.
    pub fn title(&self) -> &str {
        match self {
            LibraryPath::Root => "Library",
            LibraryPath::LikedSongs => "Liked Songs",
            LibraryPath::Playlists => "Playlists",
            LibraryPath::Albums => "Albums",
            LibraryPath::Playlist { name, .. } | LibraryPath::Album { name, .. } => name,
        }
    }
}

/// One row of a remote library listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryItem {
    pub title: String,
    /// Artist, owner or other context.
    pub subtitle: Option<String>,
    /// Track count for collections, duration in seconds for tracks.
    pub count: Option<u32>,
    pub duration_secs: Option<u32>,
    /// What `enter` does with it.
    pub action: ItemAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemAction {
    /// Open a nested listing.
    Open(LibraryPath),
    /// Start playback.
    Play(PlayRequest),
}

/// What to start playing, Spotify-style: a context (album or playlist URI),
/// optionally starting at one of its tracks, or an explicit list of tracks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayRequest {
    pub context_uri: Option<String>,
    /// Start at this track within the context.
    pub offset_uri: Option<String>,
    /// Play these tracks in order (used when there is no context, such as
    /// liked songs).
    pub uris: Vec<String>,
}

/// The latest library listing the UI asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryView {
    pub path: LibraryPath,
    pub items: Vec<LibraryItem>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
    /// Absolute position in seconds.
    Seek(u64),
    Volume(u8),
    /// Spotify: start the browser login.
    Login,
    Logout,
    /// Load a library listing into [`RemoteState::library`].
    Browse(LibraryPath),
    /// Start playing something from the library.
    Play(PlayRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// No login needed (Apple Music) or logged in.
    Ready,
    /// Spotify: press the login key.
    NeedsLogin,
    /// Spotify: waiting for the browser to come back.
    LoggingIn,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("{0}")]
    Unavailable(String),
    #[error("not logged in")]
    NeedsLogin,
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    Failed(String),
}

/// What the UI sees.
#[derive(Debug, Clone)]
pub struct RemoteState {
    pub now: Option<NowPlaying>,
    /// When `now` was read.
    pub read_at: Instant,
    pub auth: Auth,
    /// The last error (cleared by the next successful poll).
    pub error: Option<String>,
    /// The last library listing requested with [`Command::Browse`].
    pub library: Option<LibraryView>,
}

impl RemoteState {
    /// The playback position now, extrapolated from the last read while
    /// playing, clamped to the duration.
    pub fn position_now(&self, at: Instant) -> f64 {
        let Some(n) = &self.now else { return 0.0 };
        let mut pos = n.position_secs;
        if n.playing {
            pos += at.saturating_duration_since(self.read_at).as_secs_f64();
        }
        n.duration_secs.map_or(pos, |d| pos.min(d))
    }
}

/// One remote player. Runs on the source's own thread.
pub trait Backend: Send + 'static {
    fn name(&self) -> &'static str;
    /// The current state; `Ok(None)` when nothing is playing.
    fn poll(&mut self) -> Result<Option<NowPlaying>, SourceError>;
    fn command(&mut self, c: &Command) -> Result<(), SourceError>;
    fn auth(&self) -> Auth {
        Auth::Ready
    }
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(1)
    }
    /// Lists part of the user's library. Sources without one say so.
    fn browse(&mut self, _path: &LibraryPath) -> Result<Vec<LibraryItem>, SourceError> {
        Err(SourceError::Unavailable(format!(
            "{} has no library to browse here",
            self.name()
        )))
    }
}

/// The UI's handle on a source thread. Dropping it stops the thread.
pub struct RemoteHandle {
    pub name: &'static str,
    state: Arc<Mutex<RemoteState>>,
    commands: Sender<Command>,
}

impl RemoteHandle {
    pub fn spawn(backend: Box<dyn Backend>) -> RemoteHandle {
        Self::spawn_with(backend, false)
    }

    /// Like [`spawn`](Self::spawn); with `tempo_lookup`, each new track's
    /// tempo is looked up once in the background (see `tempo`) and
    /// reported in `NowPlaying::bpm`.
    pub fn spawn_with(mut backend: Box<dyn Backend>, tempo_lookup: bool) -> RemoteHandle {
        let name = backend.name();
        let state = Arc::new(Mutex::new(RemoteState {
            now: None,
            read_at: Instant::now(),
            auth: backend.auth(),
            error: None,
            library: None,
        }));
        let (tx, rx): (Sender<Command>, Receiver<Command>) = channel();
        let shared = state.clone();
        // Track ID -> tempo (None: looked up, nothing found). Filled by
        // short-lived lookup threads so polling never waits on the network.
        let tempos: Arc<Mutex<std::collections::HashMap<String, Option<f32>>>> = Arc::default();
        let _ = std::thread::Builder::new()
            .name(format!("cw-source-{name}"))
            .spawn(move || {
                let publish = |b: &dyn Backend, r: Result<Option<NowPlaying>, SourceError>| {
                    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    s.auth = b.auth();
                    s.read_at = Instant::now();
                    match r {
                        Ok(now) => {
                            s.now = now;
                            s.error = None;
                        }
                        Err(SourceError::NeedsLogin) => {
                            s.now = None;
                            s.error = None;
                        }
                        Err(e) => s.error = Some(e.to_string()),
                    }
                };
                loop {
                    let mut r = backend.poll();
                    if let Ok(Some(now)) = r.as_mut() {
                        let mut known = tempos.lock().unwrap_or_else(|e| e.into_inner());
                        match known.get(&now.track_id) {
                            Some(bpm) => now.bpm = *bpm,
                            None if tempo_lookup => {
                                known.insert(now.track_id.clone(), None);
                                spawn_tempo_lookup(now.clone(), tempos.clone());
                            }
                            None => {}
                        }
                    }
                    publish(backend.as_ref(), r);
                    match rx.recv_timeout(backend.poll_interval()) {
                        Ok(Command::Browse(path)) => {
                            let view = |items, loading, error| LibraryView {
                                path: path.clone(),
                                items,
                                loading,
                                error,
                            };
                            {
                                let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                                // Keep showing the old rows only if they are
                                // for the same place (a refresh).
                                let keep = s
                                    .library
                                    .as_ref()
                                    .filter(|l| l.path == path)
                                    .map(|l| l.items.clone())
                                    .unwrap_or_default();
                                s.library = Some(view(keep, true, None));
                            }
                            let result = backend.browse(&path);
                            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                            s.library = Some(match result {
                                Ok(items) => view(items, false, None),
                                Err(e) => view(Vec::new(), false, Some(e.to_string())),
                            });
                        }
                        Ok(cmd) => {
                            if let Err(e) = backend.command(&cmd) {
                                let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                                s.error = Some(e.to_string());
                            }
                            // Let the player apply the command before re-reading.
                            std::thread::sleep(Duration::from_millis(150));
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            });
        RemoteHandle {
            name,
            state,
            commands: tx,
        }
    }

    pub fn state(&self) -> RemoteState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn send(&self, c: Command) {
        let _ = self.commands.send(c);
    }
}

#[cfg(feature = "http")]
fn spawn_tempo_lookup(
    now: NowPlaying,
    out: Arc<Mutex<std::collections::HashMap<String, Option<f32>>>>,
) {
    let _ = std::thread::Builder::new()
        .name("cw-tempo".into())
        .spawn(move || {
            let bpm = tempo::lookup(&now);
            out.lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(now.track_id, bpm);
        });
}

#[cfg(not(feature = "http"))]
fn spawn_tempo_lookup(
    _now: NowPlaying,
    _out: Arc<Mutex<std::collections::HashMap<String, Option<f32>>>>,
) {
}

/// Fetches cover bytes (blocking; call off the UI thread).
pub fn fetch_art(art: &ArtRef) -> Option<Vec<u8>> {
    match art {
        ArtRef::None => None,
        ArtRef::File(p) => std::fs::read(p).ok(),
        #[cfg(feature = "http")]
        ArtRef::Url(u) => util::http_get_bytes(u).ok(),
        #[cfg(not(feature = "http"))]
        ArtRef::Url(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        playing: bool,
        commands: Arc<Mutex<Vec<Command>>>,
    }

    impl Backend for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn poll(&mut self) -> Result<Option<NowPlaying>, SourceError> {
            Ok(Some(NowPlaying {
                track_id: "t1".into(),
                title: "Song".into(),
                artist: None,
                album: None,
                duration_secs: Some(100.0),
                position_secs: 10.0,
                playing: self.playing,
                volume: Some(50),
                art: ArtRef::None,
                genre: None,
                isrc: None,
                bpm: None,
            }))
        }
        fn command(&mut self, c: &Command) -> Result<(), SourceError> {
            if *c == Command::PlayPause {
                self.playing = !self.playing;
            }
            self.commands.lock().unwrap().push(c.clone());
            Ok(())
        }
        fn poll_interval(&self) -> Duration {
            Duration::from_millis(20)
        }
    }

    #[test]
    fn handle_polls_and_forwards_commands() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let h = RemoteHandle::spawn(Box::new(Fake {
            playing: true,
            commands: log.clone(),
        }));
        std::thread::sleep(Duration::from_millis(60));
        let s = h.state();
        assert_eq!(s.now.as_ref().unwrap().title, "Song");
        // Extrapolates while playing, clamps to the duration.
        let pos = s.position_now(s.read_at + Duration::from_secs(5));
        assert!((pos - 15.0).abs() < 1e-6);
        assert_eq!(s.position_now(s.read_at + Duration::from_secs(500)), 100.0);
        h.send(Command::PlayPause);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(*log.lock().unwrap(), [Command::PlayPause]);
        let s = h.state();
        assert!(!s.now.as_ref().unwrap().playing);
        assert_eq!(s.position_now(s.read_at + Duration::from_secs(5)), 10.0);
    }
}
