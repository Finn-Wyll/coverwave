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
use crate::{
    ArtRef, Auth, Backend, Command, ItemAction, LibraryItem, LibraryPath, NowPlaying, PlayRequest,
    SourceError,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// Playback plus read-only library access. Logins made before the library
/// scopes were added get a "log in again" message on library calls.
pub const SCOPES: &str = "user-read-playback-state user-modify-playback-state \
user-read-currently-playing playlist-read-private playlist-read-collaborative user-library-read";
/// Page size the library endpoints allow.
const PAGE: usize = 50;
/// At most this many rows per listing (keeps a huge library responsive).
const MAX_ROWS: usize = 1000;
/// Results per kind in a catalogue search (Development Mode allows 10).
const SEARCH_LIMIT: usize = 10;
/// At most this many tracks in one "play these" request.
const MAX_URIS: usize = 100;
/// How long to wait for a Spotify app we launched to appear as a device.
const DEVICE_WAIT: Duration = Duration::from_secs(10);
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
            bpm: None,
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
    /// The logged-in user's id, once looked up.
    user: Option<String>,
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
            user: None,
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
        self.api_body(method, path, None)
    }

    /// Like [`Spotify::api`], with an optional JSON body for PUT/POST.
    fn api_body(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<(u16, String), SourceError> {
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
            let json = "application/json";
            let result = match (method, body) {
                ("GET", _) => a.get(&url).header("Authorization", &auth).call(),
                ("PUT", None) => a.put(&url).header("Authorization", &auth).send_empty(),
                ("PUT", Some(b)) => a
                    .put(&url)
                    .header("Authorization", &auth)
                    .header("Content-Type", json)
                    .send(b),
                (_, None) => a.post(&url).header("Authorization", &auth).send_empty(),
                (_, Some(b)) => a
                    .post(&url)
                    .header("Authorization", &auth)
                    .header("Content-Type", json)
                    .send(b),
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
            403 => Err(premium_needed()),
            404 => {
                // Nothing is active: wake a device and try once more.
                let id = self.pick_device()?;
                self.transfer(&id, false)?;
                std::thread::sleep(Duration::from_millis(300));
                match self.api(method, path)?.0 {
                    200..=299 => Ok(()),
                    403 => Err(premium_needed()),
                    s => Err(SourceError::Http(format!("Spotify: HTTP {s}"))),
                }
            }
            s => Err(SourceError::Http(format!("Spotify: HTTP {s}"))),
        }
    }

    /// Play/pause. With nothing active, resumes on a picked device.
    fn play_pause(&mut self) -> Result<(), SourceError> {
        let path = if self.last_playing {
            "/me/player/pause"
        } else {
            "/me/player/play"
        };
        match self.api("PUT", path)?.0 {
            200..=299 => Ok(()),
            403 => Err(premium_needed()),
            404 => {
                let id = self.pick_device()?;
                self.transfer(&id, true)
            }
            s => Err(SourceError::Http(format!("Spotify: HTTP {s}"))),
        }
    }

    /// `PUT /me/player`: move playback to `device_id`.
    fn transfer(&mut self, device_id: &str, play: bool) -> Result<(), SourceError> {
        let body = json!({ "device_ids": [device_id], "play": play }).to_string();
        match self.api_body("PUT", "/me/player", Some(&body))?.0 {
            200..=299 => Ok(()),
            403 => Err(premium_needed()),
            s => Err(SourceError::Http(format!(
                "Spotify: HTTP {s} moving playback"
            ))),
        }
    }

    /// The device to use when none is active: the active one if any, else
    /// this computer, else any device. With no devices at all, opens the
    /// Spotify app (macOS) and waits for it to appear.
    fn pick_device(&mut self) -> Result<String, SourceError> {
        if let Some(id) = self.choose_device()? {
            return Ok(id);
        }
        if launch_spotify_app() {
            let deadline = Instant::now() + DEVICE_WAIT;
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(800));
                if let Some(id) = self.choose_device()? {
                    return Ok(id);
                }
            }
        }
        Err(SourceError::Failed(
            "no Spotify device: open Spotify on this computer or your phone".into(),
        ))
    }

    fn choose_device(&mut self) -> Result<Option<String>, SourceError> {
        let v = self.get_json("/me/player/devices")?;
        Ok(choose_device(&v))
    }

    /// `PUT /me/player/play` with `req`, picking a device if none is active.
    fn play(&mut self, req: &PlayRequest) -> Result<(), SourceError> {
        let body = play_body(req).to_string();
        match self.api_body("PUT", "/me/player/play", Some(&body))?.0 {
            200..=299 => Ok(()),
            403 => Err(premium_needed()),
            404 => {
                let id = self.pick_device()?;
                let path = format!("/me/player/play?device_id={}", encode(&id));
                match self.api_body("PUT", &path, Some(&body))?.0 {
                    200..=299 => Ok(()),
                    403 => Err(premium_needed()),
                    s => Err(SourceError::Http(format!("Spotify: HTTP {s}"))),
                }
            }
            s => Err(SourceError::Http(format!("Spotify: HTTP {s}"))),
        }
    }

    /// A GET that must return JSON. Explains the common refusals.
    fn get_json(&mut self, path: &str) -> Result<Value, SourceError> {
        let (status, body) = self.api("GET", path)?;
        match status {
            200 => serde_json::from_str(&body)
                .map_err(|e| SourceError::Http(format!("Spotify sent bad JSON: {e}"))),
            401 | 403 if body.to_ascii_lowercase().contains("scope") => Err(SourceError::Failed(
                "press L to log in again: coverwave now asks to read your library".into(),
            )),
            403 => Err(SourceError::Failed(
                "Spotify only lets apps list playlists you own or collaborate on; \
                 press enter on it in the playlist list to play it"
                    .into(),
            )),
            s => Err(SourceError::Http(format!("Spotify: HTTP {s}"))),
        }
    }

    /// Every `items` entry of a paginated listing, up to [`MAX_ROWS`].
    fn get_all(&mut self, path: &str) -> Result<Vec<Value>, SourceError> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut out = Vec::new();
        loop {
            let page = self.get_json(&format!("{path}{sep}limit={PAGE}&offset={}", out.len()))?;
            let items = page["items"].as_array().cloned().unwrap_or_default();
            let got = items.len();
            out.extend(items);
            let total = page["total"].as_u64().unwrap_or(0) as usize;
            if got == 0 || out.len() >= total || out.len() >= MAX_ROWS {
                return Ok(out);
            }
        }
    }

    /// The whole catalogue: songs first (each plays on its own), then
    /// albums (open to their tracks) and playlists (play whole).
    fn search(&mut self, query: &str) -> Result<Vec<LibraryItem>, SourceError> {
        let v = self.get_json(&format!(
            "/search?q={}&type=track,album,playlist&limit={SEARCH_LIMIT}",
            encode(query)
        ))?;
        let list = |key: &str| v[key]["items"].as_array().cloned().unwrap_or_default();
        let mut rows: Vec<LibraryItem> = list("tracks")
            .iter()
            .filter_map(track_row)
            .map(|(uri, mut item)| {
                item.action = ItemAction::Play(PlayRequest {
                    uris: vec![uri],
                    ..Default::default()
                });
                item
            })
            .collect();
        rows.extend(list("albums").iter().filter_map(|a| {
            let id = a["id"].as_str()?.to_string();
            let uri = a["uri"].as_str()?.to_string();
            let name = a["name"].as_str().unwrap_or("Untitled").to_string();
            Some(LibraryItem {
                title: name.clone(),
                subtitle: artists(a).map(|s| format!("album, {s}")),
                count: a["total_tracks"].as_u64().map(|t| t as u32),
                duration_secs: None,
                action: ItemAction::Open(LibraryPath::Album { id, name, uri }),
            })
        }));
        // Playlist results can be null placeholders.
        rows.extend(list("playlists").iter().filter_map(|p| {
            let uri = p["uri"].as_str()?.to_string();
            Some(LibraryItem {
                title: p["name"].as_str().unwrap_or("Untitled").to_string(),
                subtitle: Some("playlist".into()),
                count: None,
                duration_secs: None,
                action: ItemAction::Play(PlayRequest {
                    context_uri: Some(uri),
                    ..Default::default()
                }),
            })
        }));
        Ok(rows)
    }

    /// The logged-in user's id (cached), to tell owned playlists apart.
    fn user_id(&mut self) -> Result<String, SourceError> {
        if let Some(id) = &self.user {
            return Ok(id.clone());
        }
        let me = self.get_json("/me")?;
        let id = me["id"].as_str().unwrap_or_default().to_string();
        self.user = Some(id.clone());
        Ok(id)
    }
}

fn premium_needed() -> SourceError {
    SourceError::Failed("Spotify Premium is needed for playback control".into())
}

/// Opens the Spotify app in the background so it registers as a device.
/// Returns whether that was attempted.
fn launch_spotify_app() -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::process::Stdio;
        std::process::Command::new("open")
            .args(["-g", "-a", "Spotify"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// From `GET /me/player/devices`: the active device, else a computer, else
/// any device that accepts commands.
pub fn choose_device(v: &Value) -> Option<String> {
    let devices: Vec<&Value> = v["devices"]
        .as_array()?
        .iter()
        .filter(|d| !d["is_restricted"].as_bool().unwrap_or(false))
        .filter(|d| d["id"].is_string())
        .collect();
    let id = |d: &&Value| d["id"].as_str().map(str::to_string);
    devices
        .iter()
        .find(|d| d["is_active"].as_bool().unwrap_or(false))
        .or_else(|| {
            devices.iter().find(|d| {
                d["type"]
                    .as_str()
                    .is_some_and(|t| t.eq_ignore_ascii_case("computer"))
            })
        })
        .or_else(|| devices.first())
        .and_then(id)
}

/// The JSON body of `PUT /me/player/play`.
pub fn play_body(req: &PlayRequest) -> Value {
    let mut b = serde_json::Map::new();
    if let Some(c) = &req.context_uri {
        b.insert("context_uri".into(), json!(c));
        if let Some(o) = &req.offset_uri {
            b.insert("offset".into(), json!({ "uri": o }));
        }
    } else if !req.uris.is_empty() {
        let uris: Vec<&String> = req.uris.iter().take(MAX_URIS).collect();
        b.insert("uris".into(), json!(uris));
    }
    Value::Object(b)
}

fn artists(v: &Value) -> Option<String> {
    let names: Vec<&str> = v["artists"]
        .as_array()?
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// A playable track (or episode) row; `None` for local files and gaps.
fn track_row(t: &Value) -> Option<(String, LibraryItem)> {
    if t["is_local"].as_bool().unwrap_or(false) {
        return None;
    }
    let uri = t["uri"].as_str()?.to_string();
    let item = LibraryItem {
        title: t["name"].as_str().unwrap_or("Untitled").to_string(),
        subtitle: artists(t).or_else(|| t["show"]["name"].as_str().map(str::to_string)),
        count: None,
        duration_secs: t["duration_ms"].as_u64().map(|ms| (ms / 1000) as u32),
        action: ItemAction::Play(PlayRequest::default()),
    };
    Some((uri, item))
}

/// Playlist entries carry the track under `item` (since February 2026) or
/// `track` (before); either may also be a placeholder.
fn playlist_entry_track(e: &Value) -> &Value {
    if e["item"].is_object() {
        &e["item"]
    } else {
        &e["track"]
    }
}

/// Tracks of a context (album or playlist): each row starts the context at
/// that track, so the rest of it plays on.
fn context_rows(tracks: Vec<(String, LibraryItem)>, context: &str) -> Vec<LibraryItem> {
    let mut rows = vec![play_all_row(
        tracks.len(),
        PlayRequest {
            context_uri: Some(context.to_string()),
            ..Default::default()
        },
    )];
    rows.extend(tracks.into_iter().map(|(uri, mut item)| {
        item.action = ItemAction::Play(PlayRequest {
            context_uri: Some(context.to_string()),
            offset_uri: Some(uri),
            uris: Vec::new(),
        });
        item
    }));
    rows
}

fn play_all_row(n: usize, req: PlayRequest) -> LibraryItem {
    LibraryItem {
        title: "Play all".into(),
        subtitle: None,
        count: Some(n as u32),
        duration_secs: None,
        action: ItemAction::Play(req),
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

    fn browse(&mut self, path: &LibraryPath) -> Result<Vec<LibraryItem>, SourceError> {
        self.finish_login()?;
        let open = |title: &str, sub: &str, to: LibraryPath| LibraryItem {
            title: title.into(),
            subtitle: Some(sub.into()),
            count: None,
            duration_secs: None,
            action: ItemAction::Open(to),
        };
        match path {
            LibraryPath::Root => Ok(vec![
                open(
                    "Liked Songs",
                    "songs you have saved",
                    LibraryPath::LikedSongs,
                ),
                open(
                    "Playlists",
                    "yours and ones you follow",
                    LibraryPath::Playlists,
                ),
                open("Albums", "albums you have saved", LibraryPath::Albums),
            ]),
            LibraryPath::LikedSongs => {
                let tracks: Vec<(String, LibraryItem)> = self
                    .get_all("/me/tracks")?
                    .iter()
                    .filter_map(|e| track_row(&e["track"]))
                    .collect();
                // No context URI exists for liked songs: each row plays the
                // list from that track on.
                let uris: Vec<String> = tracks.iter().map(|(u, _)| u.clone()).collect();
                let mut rows = vec![play_all_row(
                    uris.len(),
                    PlayRequest {
                        uris: uris.clone(),
                        ..Default::default()
                    },
                )];
                rows.extend(tracks.into_iter().enumerate().map(|(i, (_, mut item))| {
                    item.action = ItemAction::Play(PlayRequest {
                        uris: uris[i..].to_vec(),
                        ..Default::default()
                    });
                    item
                }));
                Ok(rows)
            }
            LibraryPath::Playlists => {
                let me = self.user_id()?;
                Ok(self
                    .get_all("/me/playlists")?
                    .iter()
                    .filter_map(|p| {
                        let id = p["id"].as_str()?.to_string();
                        let uri = p["uri"].as_str()?.to_string();
                        let name = p["name"].as_str().unwrap_or("Untitled").to_string();
                        let owner = p["owner"]["id"].as_str().unwrap_or_default();
                        let owner_name = p["owner"]["display_name"].as_str().unwrap_or(owner);
                        // Spotify only lists the tracks of playlists you own
                        // or collaborate on; others can still be played.
                        let readable = owner == me || p["collaborative"].as_bool().unwrap_or(false);
                        let total = p["items"]["total"]
                            .as_u64()
                            .or_else(|| p["tracks"]["total"].as_u64());
                        Some(LibraryItem {
                            title: name.clone(),
                            subtitle: Some(if owner == me {
                                "yours".to_string()
                            } else {
                                format!("by {owner_name}")
                            }),
                            count: total.map(|t| t as u32),
                            duration_secs: None,
                            action: if readable {
                                ItemAction::Open(LibraryPath::Playlist { id, name, uri })
                            } else {
                                ItemAction::Play(PlayRequest {
                                    context_uri: Some(uri),
                                    ..Default::default()
                                })
                            },
                        })
                    })
                    .collect())
            }
            LibraryPath::Playlist { id, uri, .. } => {
                let tracks = self
                    .get_all(&format!("/playlists/{}/items", encode(id)))?
                    .iter()
                    .filter_map(|e| track_row(playlist_entry_track(e)))
                    .collect();
                Ok(context_rows(tracks, uri))
            }
            LibraryPath::Albums => Ok(self
                .get_all("/me/albums")?
                .iter()
                .filter_map(|e| {
                    let a = &e["album"];
                    let id = a["id"].as_str()?.to_string();
                    let uri = a["uri"].as_str()?.to_string();
                    let name = a["name"].as_str().unwrap_or("Untitled").to_string();
                    Some(LibraryItem {
                        title: name.clone(),
                        subtitle: artists(a),
                        count: a["total_tracks"].as_u64().map(|t| t as u32),
                        duration_secs: None,
                        action: ItemAction::Open(LibraryPath::Album { id, name, uri }),
                    })
                })
                .collect()),
            LibraryPath::Search { query } => self.search(query),
            LibraryPath::Album { id, uri, .. } => {
                let tracks = self
                    .get_all(&format!("/albums/{}/tracks", encode(id)))?
                    .iter()
                    .filter_map(track_row)
                    .collect();
                Ok(context_rows(tracks, uri))
            }
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
                self.user = None;
                Ok(())
            }
            Command::PlayPause => self.play_pause(),
            Command::Play(req) => self.play(req),
            // Handled by the source thread through `browse`.
            Command::Browse(_) => Ok(()),
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

    /// Request lines and bodies seen by `mock_recording`.
    type RequestLog = Arc<Mutex<Vec<(String, String)>>>;

    /// Like `mock`, but also records each request line and body.
    fn mock_recording(responses: Vec<(&'static str, u16, String)>) -> (String, RequestLog) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        std::thread::spawn(move || {
            for (expect, status, body) in responses {
                let Ok((mut s, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                }
                let mut buf = vec![0u8; len];
                std::io::Read::read_exact(&mut reader, &mut buf).unwrap();
                l.lock().unwrap().push((
                    line.trim().to_string(),
                    String::from_utf8_lossy(&buf).into(),
                ));
                assert!(line.contains(expect), "expected {expect}, got {line}");
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (base, log)
    }

    fn token() -> (&'static str, u16, String) {
        (
            "POST /api/token",
            200,
            r#"{"access_token":"A","expires_in":3600}"#.into(),
        )
    }

    #[test]
    fn picks_the_active_device_then_a_computer() {
        let v: Value = serde_json::from_str(
            r#"{"devices":[
                {"id":"phone","is_active":false,"is_restricted":false,"type":"Smartphone"},
                {"id":"mac","is_active":false,"is_restricted":false,"type":"Computer"},
                {"id":"tv","is_active":false,"is_restricted":true,"type":"TV"}]}"#,
        )
        .unwrap();
        assert_eq!(choose_device(&v).as_deref(), Some("mac"));
        let active: Value = serde_json::from_str(
            r#"{"devices":[{"id":"mac","is_active":false,"type":"Computer"},
                {"id":"phone","is_active":true,"type":"Smartphone"}]}"#,
        )
        .unwrap();
        assert_eq!(choose_device(&active).as_deref(), Some("phone"));
        let none: Value = serde_json::from_str(r#"{"devices":[]}"#).unwrap();
        assert_eq!(choose_device(&none), None);
    }

    #[test]
    fn play_bodies_use_context_offset_or_uris() {
        let ctx = play_body(&PlayRequest {
            context_uri: Some("spotify:album:A".into()),
            offset_uri: Some("spotify:track:T".into()),
            uris: Vec::new(),
        });
        assert_eq!(
            ctx,
            json!({"context_uri": "spotify:album:A", "offset": {"uri": "spotify:track:T"}})
        );
        let many: Vec<String> = (0..150).map(|i| format!("spotify:track:{i}")).collect();
        let list = play_body(&PlayRequest {
            uris: many,
            ..Default::default()
        });
        assert_eq!(list["uris"].as_array().unwrap().len(), MAX_URIS);
        assert_eq!(list["uris"][0], "spotify:track:0");
    }

    #[test]
    fn browses_playlists_and_opens_only_readable_ones() {
        let (base, log) = mock_recording(vec![
            token(),
            ("GET /v1/me ", 200, r#"{"id":"me"}"#.into()),
            (
                "GET /v1/me/playlists?limit=50&offset=0",
                200,
                r#"{"total":2,"items":[
                  {"id":"p1","uri":"spotify:playlist:p1","name":"Mine","owner":{"id":"me"},
                   "items":{"total":12}},
                  {"id":"p2","uri":"spotify:playlist:p2","name":"Theirs",
                   "owner":{"id":"dj","display_name":"DJ"},"tracks":{"total":30}}]}"#
                    .into(),
            ),
        ]);
        let mut sp = spotify(&base, Some("R"));
        let rows = sp.browse(&LibraryPath::Playlists).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].count, Some(12));
        assert!(
            matches!(&rows[0].action, ItemAction::Open(LibraryPath::Playlist { id, .. }) if id == "p1")
        );
        assert_eq!(rows[1].subtitle.as_deref(), Some("by DJ"));
        assert_eq!(rows[1].count, Some(30), "old `tracks.total` still read");
        assert_eq!(
            rows[1].action,
            ItemAction::Play(PlayRequest {
                context_uri: Some("spotify:playlist:p2".into()),
                ..Default::default()
            })
        );
        assert_eq!(log.lock().unwrap().len(), 3);
    }

    #[test]
    fn searches_the_catalogue_for_songs_albums_and_playlists() {
        let (base, log) = mock_recording(vec![
            token(),
            (
                "GET /v1/search?q=kid%20a&type=track,album,playlist&limit=10",
                200,
                r#"{"tracks":{"items":[
                    {"uri":"spotify:track:t1","name":"Idioteque","duration_ms":309000,
                     "artists":[{"name":"Radiohead"}]}]},
                  "albums":{"items":[
                    {"id":"a1","uri":"spotify:album:a1","name":"Kid A","total_tracks":10,
                     "artists":[{"name":"Radiohead"}]}]},
                  "playlists":{"items":[null,
                    {"uri":"spotify:playlist:p9","name":"Kid A Mnesia"}]}}"#
                    .into(),
            ),
        ]);
        let mut sp = spotify(&base, Some("R"));
        let rows = sp
            .browse(&LibraryPath::Search {
                query: "kid a".into(),
            })
            .unwrap();
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Idioteque", "Kid A", "Kid A Mnesia"]);
        assert_eq!(
            rows[0].action,
            ItemAction::Play(PlayRequest {
                uris: vec!["spotify:track:t1".into()],
                ..Default::default()
            })
        );
        assert!(
            matches!(&rows[1].action, ItemAction::Open(LibraryPath::Album { id, .. }) if id == "a1")
        );
        assert_eq!(rows[1].subtitle.as_deref(), Some("album, Radiohead"));
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[test]
    fn playlist_tracks_read_item_or_track_and_skip_local_files() {
        let (base, _) = mock_recording(vec![
            token(),
            (
                "GET /v1/playlists/p1/items?limit=50&offset=0",
                200,
                r#"{"total":3,"items":[
                  {"item":{"uri":"spotify:track:a","name":"New shape","duration_ms":61000,
                           "artists":[{"name":"X"},{"name":"Y"}]}},
                  {"track":{"uri":"spotify:track:b","name":"Old shape","artists":[]}},
                  {"item":{"uri":"spotify:local:z","name":"Local","is_local":true}}]}"#
                    .into(),
            ),
        ]);
        let mut sp = spotify(&base, Some("R"));
        let rows = sp
            .browse(&LibraryPath::Playlist {
                id: "p1".into(),
                name: "Mine".into(),
                uri: "spotify:playlist:p1".into(),
            })
            .unwrap();
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Play all", "New shape", "Old shape"]);
        assert_eq!(rows[1].subtitle.as_deref(), Some("X, Y"));
        assert_eq!(rows[1].duration_secs, Some(61));
        assert_eq!(
            rows[2].action,
            ItemAction::Play(PlayRequest {
                context_uri: Some("spotify:playlist:p1".into()),
                offset_uri: Some("spotify:track:b".into()),
                uris: Vec::new(),
            })
        );
    }

    #[test]
    fn liked_songs_play_on_from_the_chosen_track() {
        let (base, _) = mock_recording(vec![
            token(),
            (
                "GET /v1/me/tracks?limit=50&offset=0",
                200,
                r#"{"total":2,"items":[{"track":{"uri":"spotify:track:1","name":"One"}},
                                       {"track":{"uri":"spotify:track:2","name":"Two"}}]}"#
                    .into(),
            ),
        ]);
        let mut sp = spotify(&base, Some("R"));
        let rows = sp.browse(&LibraryPath::LikedSongs).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[2].action,
            ItemAction::Play(PlayRequest {
                uris: vec!["spotify:track:2".into()],
                ..Default::default()
            })
        );
    }

    #[test]
    fn play_with_no_active_device_targets_a_listed_one() {
        let (base, log) = mock_recording(vec![
            token(),
            ("PUT /v1/me/player/play ", 404, "{}".into()),
            (
                "GET /v1/me/player/devices",
                200,
                r#"{"devices":[{"id":"mac 1","is_active":false,"type":"Computer"}]}"#.into(),
            ),
            (
                "PUT /v1/me/player/play?device_id=mac%201",
                204,
                String::new(),
            ),
        ]);
        let mut sp = spotify(&base, Some("R"));
        let req = PlayRequest {
            context_uri: Some("spotify:album:A".into()),
            ..Default::default()
        };
        sp.command(&Command::Play(req)).unwrap();
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 4);
        assert_eq!(log[3].1, r#"{"context_uri":"spotify:album:A"}"#);
    }

    #[test]
    fn missing_library_scope_asks_to_log_in_again() {
        let (base, _) = mock_recording(vec![
            token(),
            (
                "GET /v1/me/tracks",
                403,
                r#"{"error":{"status":403,"message":"Insufficient client scope"}}"#.into(),
            ),
        ]);
        let mut sp = spotify(&base, Some("R"));
        let err = sp.browse(&LibraryPath::LikedSongs).unwrap_err().to_string();
        assert!(err.contains("log in again"), "{err}");
    }
}
