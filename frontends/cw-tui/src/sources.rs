//! Which player coverwave follows: local files, Spotify or Apple Music
//! (`S` cycles). Remote sources run on their own threads (`cw-sources`);
//! while one is active, its audio is captured (`cw-capture`) and analysed so
//! the cover still reacts to the music.

use crate::config::Config;
use cw_analysis::GenreFamily;
use cw_capture::{Capture, CaptureError, Target};
use cw_core::{AudioFrame, BeatClock};
use cw_sources::{Command, RemoteHandle, RemoteState};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Local,
    Spotify,
    AppleMusic,
}

impl SourceKind {
    pub fn name(self) -> &'static str {
        match self {
            SourceKind::Local => "Local",
            SourceKind::Spotify => "Spotify",
            SourceKind::AppleMusic => "Apple Music",
        }
    }

    /// macOS bundle IDs whose audio is captured for this source.
    fn bundle_ids(self) -> Vec<String> {
        match self {
            SourceKind::Local => Vec::new(),
            SourceKind::Spotify => vec!["com.spotify.client".into()],
            SourceKind::AppleMusic => vec!["com.apple.Music".into()],
        }
    }
}

/// Capture is retried this often while the app is not playing yet.
const CAPTURE_RETRY: Duration = Duration::from_secs(3);
/// If capture hears silence this long while the source says it is playing,
/// the app probably restarted: start a fresh capture.
const SILENT_RESTART: Duration = Duration::from_secs(8);
/// Ring-buffer room for captured audio (about a second).
const CAPTURE_BUFFER: usize = 96_000;

struct Running {
    _capture: Capture,
    analyser: cw_audio::AnalyserHandle,
    rate: u32,
    last_sound: Instant,
}

pub struct Sources {
    pub active: SourceKind,
    available: Vec<SourceKind>,
    spotify_cfg: Option<cw_sources::spotify::SpotifyConfig>,
    remotes: Vec<(SourceKind, RemoteHandle)>,
    /// The active remote's state, refreshed each tick.
    pub state: Option<RemoteState>,
    capture_enabled: bool,
    /// Look up remote tracks' tempo online (Deezer) for the beat clock.
    tempo_lookup: bool,
    capture: Option<Running>,
    /// Why capture is not running (shown in the debug overlay).
    pub capture_status: String,
    next_try: Instant,
}

impl Sources {
    pub fn new(config: &Config) -> Sources {
        let mut available = vec![SourceKind::Local];
        let spotify_cfg = (!config.spotify.client_id.trim().is_empty()).then(|| {
            let mut c = cw_sources::spotify::SpotifyConfig::new(
                &config.spotify.client_id,
                config.spotify.redirect_port,
            );
            // For testing against a mock server.
            if let Ok(v) = std::env::var("COVERWAVE_SPOTIFY_API") {
                c.api_base = v;
            }
            if let Ok(v) = std::env::var("COVERWAVE_SPOTIFY_ACCOUNTS") {
                c.accounts_base = v;
            }
            c
        });
        if spotify_cfg.is_some() {
            available.push(SourceKind::Spotify);
        }
        if cfg!(target_os = "macos") {
            available.push(SourceKind::AppleMusic);
        }
        Sources {
            active: SourceKind::Local,
            available,
            spotify_cfg,
            remotes: Vec::new(),
            state: None,
            capture_enabled: config.capture.enabled,
            tempo_lookup: config.tempo.online_lookup,
            capture: None,
            capture_status: String::new(),
            next_try: Instant::now(),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.active != SourceKind::Local
    }

    /// Why `S` does nothing, if there is only the local source.
    pub fn hint(&self) -> Option<&'static str> {
        (self.available.len() == 1).then_some(
            "no other sources: set [spotify] client_id in config.toml (Apple Music needs macOS)",
        )
    }

    /// Switches to the next available source.
    pub fn cycle(&mut self) -> SourceKind {
        let i = self
            .available
            .iter()
            .position(|k| *k == self.active)
            .unwrap_or(0);
        let next = self.available[(i + 1) % self.available.len()];
        self.select(next);
        next
    }

    pub fn select(&mut self, kind: SourceKind) {
        if kind == self.active {
            return;
        }
        self.active = kind;
        self.capture = None;
        self.capture_status.clear();
        self.next_try = Instant::now();
        self.state = None;
        if kind != SourceKind::Local && !self.remotes.iter().any(|(k, _)| *k == kind) {
            let backend: Box<dyn cw_sources::Backend> = match kind {
                SourceKind::Spotify => {
                    let cfg = self
                        .spotify_cfg
                        .clone()
                        .expect("Spotify is only offered when configured");
                    let store = cw_sources::spotify::KeyringStore::new(&cfg.client_id);
                    Box::new(cw_sources::spotify::Spotify::new(cfg, Box::new(store)))
                }
                _ => Box::new(cw_sources::apple_music::AppleMusic::new()),
            };
            self.remotes
                .push((kind, RemoteHandle::spawn_with(backend, self.tempo_lookup)));
        }
    }

    fn handle(&self) -> Option<&RemoteHandle> {
        self.remotes
            .iter()
            .find(|(k, _)| *k == self.active)
            .map(|(_, h)| h)
    }

    pub fn send(&self, c: Command) {
        if let Some(h) = self.handle() {
            h.send(c);
        }
    }

    /// Refreshes the remote state and keeps capture running. Call once per
    /// frame.
    pub fn tick(&mut self) {
        self.state = self.handle().map(RemoteHandle::state);
        if !self.is_remote() || !self.capture_enabled {
            return;
        }
        let playing = self
            .state
            .as_ref()
            .and_then(|s| s.now.as_ref())
            .is_some_and(|n| n.playing);
        if let Some(run) = self.capture.as_mut() {
            if run.analyser.frame().level > 0.0 {
                run.last_sound = Instant::now();
            } else if playing && run.last_sound.elapsed() > SILENT_RESTART {
                self.capture = None;
                self.capture_status = "restarting capture".into();
                self.next_try = Instant::now();
            }
            return;
        }
        if Instant::now() < self.next_try {
            return;
        }
        self.next_try = Instant::now() + CAPTURE_RETRY;
        let target = if cfg!(target_os = "macos") {
            Target::Apps(self.active.bundle_ids())
        } else {
            Target::System
        };
        let (tx, rx) = rtrb::RingBuffer::new(CAPTURE_BUFFER);
        match Capture::start(&target, tx) {
            Ok(capture) => {
                let rate = capture.sample_rate();
                match cw_audio::AnalyserHandle::spawn(rx, rate) {
                    Ok(analyser) => {
                        self.capture = Some(Running {
                            _capture: capture,
                            analyser,
                            rate,
                            last_sound: Instant::now(),
                        });
                        self.capture_status.clear();
                    }
                    Err(e) => self.capture_status = format!("analyser: {e}"),
                }
            }
            Err(CaptureError::NotPlaying(_)) => {
                self.capture_status = format!("waiting for {} audio", self.active.name());
            }
            Err(e) => {
                self.capture_status = e.to_string();
                self.next_try = Instant::now() + CAPTURE_RETRY * 4;
            }
        }
    }

    /// Analysis of the captured audio, when capturing.
    pub fn audio_frame(&self) -> Option<AudioFrame> {
        self.capture.as_ref().map(|r| r.analyser.frame())
    }

    /// Synthetic motion for a remote track that cannot be heard: a beat
    /// clock at the track's tempo (looked up online), or the genre's typical
    /// tempo, with the genre's rhythm.
    pub fn beat_frame(&self, genre: GenreFamily) -> Option<AudioFrame> {
        let state = self.state.as_ref()?;
        let now = state.now.as_ref()?;
        let bpm = now.bpm.unwrap_or_else(|| genre.typical_bpm());
        let energy = (0.5 + genre.energy_bias()).clamp(0.0, 1.0);
        let clock = BeatClock::new(bpm, genre.beat_pattern(), energy);
        Some(clock.frame(state.position_now(Instant::now()), now.playing))
    }

    /// Where the motion comes from, for the debug overlay.
    pub fn tempo_description(&self, genre: GenreFamily) -> String {
        if self.capture.is_some() {
            return "heard".into();
        }
        match self.state.as_ref().and_then(|s| s.now.as_ref()) {
            None => "-".into(),
            Some(n) => match n.bpm {
                Some(b) => format!("{b:.0} BPM (looked up)"),
                None => format!("{:.0} BPM ({})", genre.typical_bpm(), genre.name()),
            },
        }
    }

    pub fn capture_description(&self) -> String {
        match &self.capture {
            Some(r) => format!("on, {} Hz", r.rate),
            None if !self.capture_enabled => "off".into(),
            None if self.capture_status.is_empty() => "-".into(),
            None => self.capture_status.clone(),
        }
    }
}
