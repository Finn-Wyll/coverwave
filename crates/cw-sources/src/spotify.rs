//! Spotify through the Web API, logged in with OAuth PKCE: only the public
//! client ID is needed, never a client secret. The refresh token lives in
//! the OS keychain (macOS Keychain, Windows Credential Manager; in memory
//! on Linux).
//!
//! Login: a browser opens Spotify's consent page, which redirects to
//! `http://127.0.0.1:<port>/callback`. A one-shot listener there catches the
//! code. Register exactly that redirect URI in your Spotify app.
//!
//! Playback control (play/pause, skip, seek, volume) needs Spotify Premium.
//! Without it the player still shows now-playing and the cover.

use crate::util::{agent, base64url, encode, form, open_browser, query_param};
use crate::{ArtRef, Auth, Backend, Command, NowPlaying, SourceError};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

pub const SCOPES: &str =
    "user-read-playback-state user-modify-playback-state user-read-currently-playing";
pub const DEFAULT_PORT: u16 = 8898;
/// How long the login listener waits for the browser.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct SpotifyConfig {
    pub client_id: String,
    pub redirect_port: u16,
    pub api_base: String,
    pub accounts_base: String,
}

impl SpotifyConfig {
    pub fn new(client_id: &str, redirect_port: u16) -> SpotifyConfig {
        SpotifyConfig {
            client_id: client_id.trim().to_string(),
            redirect_port,
            api_base: "https://api.spotify.com/v1".into(),
            accounts_base: "https://accounts.spotify.com".into(),
        }
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port)
    }
}

/// Where the refresh token is kept between runs.
pub trait TokenStore: Send {
    fn load(&self) -> Option<String>;
    fn save(&self, refresh_token: &str);
    fn clear(&self);
}

/// The OS keychain via `keyring` (an in-memory mock on platforms without a
/// native store).
pub struct KeyringStore {
    user: String,
}

impl KeyringStore {
    pub fn new(client_id: &str) -> KeyringStore {
        KeyringStore {
            user: format!("spotify:{client_id}"),
        }
    }

    fn entry(&self) -> Option<keyring::Entry> {
        keyring::Entry::new("coverwave", &self.user).ok()
    }
}

impl TokenStore for KeyringStore {
    fn load(&self) -> Option<String> {
        self.entry()?.get_password().ok()
    }
    fn save(&self, refresh_token: &str) {
        if let Some(e) = self.entry() {
            let _ = e.set_password(refresh_token);
        }
    }
    fn clear(&self) {
        if let Some(e) = self.entry() {
            let _ = e.delete_credential();
        }
    }
}

/// For tests.
#[derive(Default)]
pub struct MemoryStore(pub Mutex<Option<String>>);

impl TokenStore for MemoryStore {
    fn load(&self) -> Option<String> {
        self.0.lock().ok()?.clone()
    }
    fn save(&self, t: &str) {
        if let Ok(mut s) = self.0.lock() {
            *s = Some(t.to_string());
        }
    }
    fn clear(&self) {
        if let Ok(mut s) = self.0.lock() {
            *s = None;
        }
    }
}

/// A PKCE verifier and its S256 challenge (RFC 7636).
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Pkce {
        let mut bytes = [0u8; 48];
        // Without OS randomness there is no safe login; fail loudly.
        getrandom::fill(&mut bytes).expect("OS random source");
        Pkce::from_verifier(base64url(&bytes))
    }

    pub fn from_verifier(verifier: String) -> Pkce {
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Pkce::new()
    }
}

pub fn authorize_url(cfg: &SpotifyConfig, challenge: &str, state: &str) -> String {
    format!(
        "{}/authorize?{}",
        cfg.accounts_base,
        form(&[
            ("client_id", &cfg.client_id),
            ("response_type", "code"),
            ("redirect_uri", &cfg.redirect_uri()),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge),
            ("state", state),
            ("scope", SCOPES),
        ])
    )
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct PlayerResponse {
    is_playing: bool,
    progress_ms: Option<u64>,
    item: Option<Item>,
    device: Option<Device>,
}

#[derive(Deserialize)]
struct Device {
    volume_percent: Option<u8>,
}

#[derive(Deserialize)]
struct Item {
    id: Option<String>,
    uri: Option<String>,
    name: String,
    duration_ms: Option<u64>,
    #[serde(default)]
    artists: Vec<Named>,
    album: Option<Album>,
    /// Podcast episodes have a show instead of artists and an album.
    show: Option<Named>,
    #[serde(default)]
    images: Vec<Image>,
    external_ids: Option<ExternalIds>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
    id: Option<String>,
}

#[derive(Deserialize)]
struct ExternalIds {
    isrc: Option<String>,
}

#[derive(Deserialize)]
struct ArtistResponse {
    #[serde(default)]
    genres: Vec<String>,
}

/// Parses `GET /artists/{id}` into a genre list ("rock; pop").
pub fn parse_artist_genres(json: &str) -> Option<String> {
    let a: ArtistResponse = serde_json::from_str(json).ok()?;
    (!a.genres.is_empty()).then(|| a.genres.join("; "))
}

#[derive(Deserialize)]
struct Album {
    name: String,
    #[serde(default)]
    images: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    url: String,
    width: Option<u32>,
}

/// Parses `GET /me/player`.
pub fn parse_player(json: &str) -> Result<Option<NowPlaying>, SourceError> {
    Ok(parse_player_full(json)?.map(|(n, _)| n))
}

/// Like [`parse_player`], also returning the first artist's ID.
fn parse_player_full(json: &str) -> Result<Option<(NowPlaying, Option<String>)>, SourceError> {
    let p: PlayerResponse = serde_json::from_str(json)
        .map_err(|e| SourceError::Failed(format!("Spotify sent odd data: {e}")))?;
    let Some(item) = p.item else { return Ok(None) };
    let images = item.album.as_ref().map_or(&item.images, |a| &a.images);
    // The largest image (Spotify lists 640, 300, 64).
    let art = images
        .iter()
        .max_by_key(|i| i.width.unwrap_or(0))
        .map_or(ArtRef::None, |i| ArtRef::Url(i.url.clone()));
    let artist_id = item.artists.first().and_then(|a| a.id.clone());
    let isrc = item.external_ids.and_then(|e| e.isrc);
    let artist = if item.artists.is_empty() {
        item.show.map(|s| s.name)
    } else {
        Some(
            item.artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    };
    Ok(Some((
        NowPlaying {
            track_id: item.id.or(item.uri).unwrap_or_else(|| item.name.clone()),
            title: item.name,
            artist,
            album: item.album.map(|a| a.name),
            duration_secs: item.duration_ms.map(|ms| ms as f64 / 1000.0),
            position_secs: p.progress_ms.unwrap_or(0) as f64 / 1000.0,
            playing: p.is_playing,
            volume: p.device.and_then(|d| d.volume_percent),
            art,
            genre: None,
            isrc,
        },
        artist_id,
    )))
}

struct Login {
    verifier: String,
    code: Receiver<Result<String, String>>,
}

pub struct Spotify {
    cfg: SpotifyConfig,
    store: Box<dyn TokenStore>,
    access: Option<(String, Instant)>,
    refresh: Option<String>,
    login: Option<Login>,
    /// After a 429, wait until then.
    backoff_until: Option<Instant>,
    last_playing: bool,
    /// Artist ID -> genres (or none), so each artist is looked up once.
    genres: std::collections::HashMap<String, Option<String>>,
}

impl Spotify {
    pub fn new(cfg: SpotifyConfig, store: Box<dyn TokenStore>) -> Spotify {
        let refresh = store.load();
        Spotify {
            cfg,
            store,
            access: None,
            refresh,
            login: None,
            backoff_until: None,
            last_playing: false,
            genres: std::collections::HashMap::new(),
        }
    }

    /// Starts the browser login: a one-shot listener on the redirect port,
    /// then the consent page.
    fn start_login(&mut self) -> Result<(), SourceError> {
        let listener = TcpListener::bind(("127.0.0.1", self.cfg.redirect_port)).map_err(|e| {
            SourceError::Failed(format!(
                "cannot listen on port {}: {e}",
                self.cfg.redirect_port
            ))
        })?;
        let pkce = Pkce::new();
        let state = Pkce::new().verifier[..16].to_string();
        let url = authorize_url(&self.cfg, &pkce.challenge, &state);
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("cw-spotify-login".into())
            .spawn(move || {
                let _ = tx.send(wait_for_code(&listener, &state));
            })
            .map_err(|e| SourceError::Failed(e.to_string()))?;
        self.login = Some(Login {
            verifier: pkce.verifier,
            code: rx,
        });
        if !open_browser(&url) {
            return Err(SourceError::Failed(format!(
                "open this in a browser to log in: {url}"
            )));
        }
        Ok(())
    }

    fn finish_login(&mut self) -> Result<(), SourceError> {
        let Some(login) = &self.login else {
            return Ok(());
        };
        let code = match login.code.try_recv() {
            Ok(Ok(code)) => code,
            Ok(Err(e)) => {
                self.login = None;
                return Err(SourceError::Failed(format!("login failed: {e}")));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.login = None;
                return Err(SourceError::Failed("login listener stopped".into()));
            }
        };
        let verifier = login.verifier.clone();
        self.login = None;
        let body = form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &self.cfg.redirect_uri()),
            ("client_id", &self.cfg.client_id),
            ("code_verifier", &verifier),
        ]);
        self.token_request(&body)
    }

    fn token_request(&mut self, body: &str) -> Result<(), SourceError> {
        let url = format!("{}/api/token", self.cfg.accounts_base);
        let mut resp = agent()
            .post(&url)
            .content_type("application/x-www-form-urlencoded")
            .send(body)
            .map_err(|e| SourceError::Http(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        if status == 400 || status == 401 {
            // The refresh token was revoked or expired: log in again.
            self.store.clear();
            self.refresh = None;
            self.access = None;
            return Err(SourceError::NeedsLogin);
        }
        if status != 200 {
            return Err(SourceError::Http(format!("Spotify login: HTTP {status}")));
        }
        let t: TokenResponse =
            serde_json::from_str(&text).map_err(|e| SourceError::Failed(e.to_string()))?;
        let expires = Instant::now() + Duration::from_secs(t.expires_in.saturating_sub(60));
        self.access = Some((t.access_token, expires));
        if let Some(r) = t.refresh_token {
            self.store.save(&r);
            self.refresh = Some(r);
        }
        Ok(())
    }

    fn access_token(&mut self) -> Result<String, SourceError> {
        if let Some((t, exp)) = &self.access
            && Instant::now() < *exp
        {
            return Ok(t.clone());
        }
        let Some(refresh) = self.refresh.clone() else {
            return Err(SourceError::NeedsLogin);
        };
        let body = form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &self.cfg.client_id),
        ]);
        self.token_request(&body)?;
        self.access
            .as_ref()
            .map(|(t, _)| t.clone())
            .ok_or(SourceError::NeedsLogin)
    }

    /// One Web API call; retries once after refreshing on 401. Returns the
    /// status and body.
    fn api(&mut self, method: &str, path: &str) -> Result<(u16, String), SourceError> {
        if let Some(until) = self.backoff_until
            && Instant::now() < until
        {
            return Err(SourceError::Http("rate limited by Spotify; waiting".into()));
        }
        for attempt in 0..2 {
            let token = self.access_token()?;
            let url = format!("{}{path}", self.cfg.api_base);
            let auth = format!("Bearer {token}");
            let a = agent();
            let result = match method {
                "GET" => a.get(&url).header("Authorization", &auth).call(),
                "PUT" => a.put(&url).header("Authorization", &auth).send_empty(),
                _ => a.post(&url).header("Authorization", &auth).send_empty(),
            };
            let mut resp = result.map_err(|e| SourceError::Http(e.to_string()))?;
            let status = resp.status().as_u16();
            if status == 401 && attempt == 0 {
                self.access = None;
                continue;
            }
            if status == 429 {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5u64);
                self.backoff_until = Some(Instant::now() + Duration::from_secs(wait));
            }
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            return Ok((status, body));
        }
        Err(SourceError::NeedsLogin)
    }

    fn control(&mut self, method: &str, path: &str) -> Result<(), SourceError> {
        match self.api(method, path)?.0 {
            200..=299 => Ok(()),
            403 => Err(SourceError::Failed(
                "Spotify Premium is needed for playback control".into(),
            )),
            404 => Err(SourceError::Failed(
                "no active Spotify device: start playing in Spotify first".into(),
            )),
            s => Err(SourceError::Http(format!("Spotify: HTTP {s}"))),
        }
    }
}

/// Accepts one browser redirect on `listener` and returns its `code`.
fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let mut line = String::new();
                BufReader::new(&stream)
                    .read_line(&mut line)
                    .map_err(|e| e.to_string())?;
                // "GET /callback?code=...&state=... HTTP/1.1"
                let target = line.split_whitespace().nth(1).unwrap_or("");
                let Some(query) = target.strip_prefix("/callback?") else {
                    let _ =
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                    continue; // e.g. a favicon request
                };
                let ok = query_param(query, "state").as_deref() == Some(expected_state);
                let result = match (query_param(query, "code"), query_param(query, "error")) {
                    (Some(code), _) if ok => Ok(code),
                    (_, Some(err)) => Err(err),
                    _ => Err("state mismatch".into()),
                };
                let page = if result.is_ok() {
                    "coverwave is logged in to Spotify. You can close this tab."
                } else {
                    "Spotify login failed. You can close this tab and try again."
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                );
                return result;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err("timed out waiting for the browser".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

impl Backend for Spotify {
    fn name(&self) -> &'static str {
        "Spotify"
    }

    fn auth(&self) -> Auth {
        if self.login.is_some() {
            Auth::LoggingIn
        } else if self.refresh.is_none() && self.access.is_none() {
            Auth::NeedsLogin
        } else {
            Auth::Ready
        }
    }

    fn poll(&mut self) -> Result<Option<NowPlaying>, SourceError> {
        self.finish_login()?;
        if self.login.is_some() {
            return Ok(None);
        }
        let (status, body) = self.api("GET", "/me/player")?;
        match status {
            204 => Ok(None), // nothing playing
            200 => {
                let Some((mut now, artist_id)) = parse_player_full(&body)? else {
                    self.last_playing = false;
                    return Ok(None);
                };
                self.last_playing = now.playing;
                if let Some(id) = artist_id {
                    if !self.genres.contains_key(&id) {
                        // Best effort: no genre is fine.
                        let g = match self.api("GET", &format!("/artists/{}", encode(&id))) {
                            Ok((200, body)) => parse_artist_genres(&body),
                            _ => None,
                        };
                        self.genres.insert(id.clone(), g);
                    }
                    now.genre = self.genres.get(&id).cloned().flatten();
                }
                Ok(Some(now))
            }
            s => Err(SourceError::Http(format!("Spotify: HTTP {s}"))),
        }
    }

    fn command(&mut self, c: &Command) -> Result<(), SourceError> {
        match c {
            Command::Login => self.start_login(),
            Command::Logout => {
                self.store.clear();
                self.refresh = None;
                self.access = None;
                Ok(())
            }
            Command::PlayPause if self.last_playing => self.control("PUT", "/me/player/pause"),
            Command::PlayPause => self.control("PUT", "/me/player/play"),
            Command::Next => self.control("POST", "/me/player/next"),
            Command::Previous => self.control("POST", "/me/player/previous"),
            Command::Seek(secs) => self.control(
                "PUT",
                &format!("/me/player/seek?position_ms={}", secs * 1000),
            ),
            Command::Volume(v) => self.control(
                "PUT",
                &format!(
                    "/me/player/volume?volume_percent={}",
                    encode(&v.min(&100).to_string())
                ),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn artist_genres_and_isrc() {
        assert_eq!(
            parse_artist_genres(r#"{"genres":["christian rock","worship"]}"#).as_deref(),
            Some("christian rock; worship")
        );
        assert_eq!(parse_artist_genres(r#"{"genres":[]}"#), None);
        let json = r#"{"is_playing":true,"item":{"id":"t","name":"N",
            "artists":[{"name":"A","id":"art1"}],"external_ids":{"isrc":"USABC1234567"}}}"#;
        let (n, artist) = parse_player_full(json).unwrap().unwrap();
        assert_eq!(
            (n.isrc.as_deref(), artist.as_deref()),
            (Some("USABC1234567"), Some("art1"))
        );
    }

    #[test]
    fn pkce_matches_rfc7636_vector() {
        // RFC 7636 appendix B.
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let random = Pkce::new();
        assert_eq!(random.verifier.len(), 64);
        assert_ne!(random.verifier, Pkce::new().verifier);
    }

    #[test]
    fn authorize_url_has_pkce_and_loopback_redirect() {
        let cfg = SpotifyConfig::new(" abc123 ", 8898);
        let url = authorize_url(&cfg, "CH", "ST");
        assert!(url.starts_with(
            "https://accounts.spotify.com/authorize?client_id=abc123&response_type=code"
        ));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8898%2Fcallback"));
        assert!(url.contains("code_challenge_method=S256&code_challenge=CH&state=ST"));
        assert!(url.contains("scope=user-read-playback-state%20user-modify-playback-state"));
    }

    #[test]
    fn parses_tracks_episodes_and_nothing() {
        let track = r#"{"is_playing":true,"progress_ms":61500,"device":{"volume_percent":40},
            "item":{"id":"t1","name":"Aerials","duration_ms":235000,
              "artists":[{"name":"System Of A Down"},{"name":"Guest"}],
              "album":{"name":"Toxicity","images":[{"url":"s","width":64},{"url":"big","width":640}]}}}"#;
        let n = parse_player(track).unwrap().unwrap();
        assert_eq!(n.title, "Aerials");
        assert_eq!(n.artist.as_deref(), Some("System Of A Down, Guest"));
        assert_eq!(
            (n.position_secs, n.duration_secs, n.volume),
            (61.5, Some(235.0), Some(40))
        );
        assert_eq!(n.art, ArtRef::Url("big".into()));
        let episode = r#"{"is_playing":false,"progress_ms":0,"item":{"uri":"spotify:episode:9",
            "name":"Ep 1","show":{"name":"A Podcast"},"images":[{"url":"cover","width":300}]}}"#;
        let n = parse_player(episode).unwrap().unwrap();
        assert_eq!(
            (n.artist.as_deref(), n.track_id.as_str()),
            (Some("A Podcast"), "spotify:episode:9")
        );
        assert_eq!(n.art, ArtRef::Url("cover".into()));
        assert_eq!(
            parse_player(r#"{"is_playing":false,"item":null}"#).unwrap(),
            None
        );
        assert!(parse_player("nope").is_err());
    }

    /// A tiny HTTP server answering each request with the next canned
    /// response; counts requests per path.
    fn mock(responses: Vec<(&'static str, u16, &'static str)>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for (expect, status, body) in responses {
                let Ok((mut s, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.contains(expect), "expected {expect}, got {line}");
                // Drain headers and any body.
                let mut len = 0usize;
                loop {
                    let mut l = String::new();
                    reader.read_line(&mut l).unwrap();
                    if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if l == "\r\n" || l.is_empty() {
                        break;
                    }
                }
                let mut buf = vec![0u8; len];
                std::io::Read::read_exact(&mut reader, &mut buf).unwrap();
                h.fetch_add(1, Ordering::SeqCst);
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (base, hits)
    }

    fn spotify(base: &str, refresh: Option<&str>) -> Spotify {
        let mut cfg = SpotifyConfig::new("cid", 0);
        cfg.api_base = format!("{base}/v1");
        cfg.accounts_base = base.to_string();
        let store = MemoryStore::default();
        if let Some(r) = refresh {
            store.save(r);
        }
        Spotify::new(cfg, Box::new(store))
    }

    #[test]
    fn refreshes_then_polls_and_controls() {
        let (base, hits) = mock(vec![
            (
                "POST /api/token",
                200,
                r#"{"access_token":"A1","expires_in":3600,"refresh_token":"R2"}"#,
            ),
            ("GET /v1/me/player", 204, ""),
            (
                "GET /v1/me/player",
                200,
                r#"{"is_playing":true,"progress_ms":1000,"item":{"id":"x","name":"N","duration_ms":2000}}"#,
            ),
            ("PUT /v1/me/player/pause", 204, ""),
            ("POST /v1/me/player/next", 403, "{}"),
        ]);
        let mut s = spotify(&base, Some("R1"));
        assert_eq!(s.auth(), Auth::Ready);
        assert_eq!(s.poll().unwrap(), None);
        assert_eq!(
            s.store.load().as_deref(),
            Some("R2"),
            "rotated refresh token saved"
        );
        assert!(s.poll().unwrap().unwrap().playing);
        s.command(&Command::PlayPause).unwrap();
        let err = s.command(&Command::Next).unwrap_err().to_string();
        assert!(err.contains("Premium"), "{err}");
        assert_eq!(hits.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn revoked_refresh_token_asks_for_login() {
        let (base, _) = mock(vec![(
            "POST /api/token",
            400,
            r#"{"error":"invalid_grant"}"#,
        )]);
        let mut s = spotify(&base, Some("old"));
        assert!(matches!(s.poll(), Err(SourceError::NeedsLogin)));
        assert_eq!(s.auth(), Auth::NeedsLogin);
        assert_eq!(s.store.load(), None);
        let mut fresh = spotify(&base, None);
        assert_eq!(fresh.auth(), Auth::NeedsLogin);
        assert!(matches!(fresh.poll(), Err(SourceError::NeedsLogin)));
    }

    #[test]
    fn login_listener_takes_code_and_checks_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let t = std::thread::spawn(move || wait_for_code(&listener, "good"));
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n").unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"GET /callback?code=C%2F1&state=good HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        assert_eq!(t.join().unwrap(), Ok("C/1".to_string()));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let t = std::thread::spawn(move || wait_for_code(&listener, "good"));
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"GET /callback?code=C&state=evil HTTP/1.1\r\n\r\n")
            .unwrap();
        assert_eq!(t.join().unwrap(), Err("state mismatch".to_string()));
    }
}
