//! Lyrics from LRCLIB (<https://lrclib.net>), a free, keyless database of
//! time-synced lyrics. Sends the track's artist, title, album and length;
//! nothing about the user. Turned off with `[lyrics] online_lookup = false`.
//!
//! Returns LRC text (synced when LRCLIB has it, else plain lines); parse it
//! with `cw_core::lyrics::parse_lrc`.

use crate::util::{agent, encode};
use serde::Deserialize;

const API: &str = "https://lrclib.net/api";
/// LRCLIB asks clients to say who they are.
const USER_AGENT: &str = concat!("coverwave/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    instrumental: bool,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
    duration: Option<f64>,
}

/// What a song's lyrics are on LRCLIB.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// LRC text, synced or plain.
    Lyrics(String),
    /// The song has no words.
    Instrumental,
}

/// Picks the best text from one LRCLIB record: synced, else plain.
fn pick(r: &Record) -> Option<Found> {
    if r.instrumental {
        return Some(Found::Instrumental);
    }
    let nonempty = |s: &Option<String>| s.as_ref().filter(|t| !t.trim().is_empty()).cloned();
    nonempty(&r.synced_lyrics)
        .or_else(|| nonempty(&r.plain_lyrics))
        .map(Found::Lyrics)
}

/// Parses `GET /api/get` (one record).
pub fn parse_get(json: &str) -> Option<Found> {
    pick(&serde_json::from_str::<Record>(json).ok()?)
}

/// Parses `GET /api/search` (a list): the synced record closest in length
/// to `duration`, else any with lyrics.
pub fn parse_search(json: &str, duration: Option<f64>) -> Option<Found> {
    let records: Vec<Record> = serde_json::from_str(json).ok()?;
    let off = |r: &Record| match (r.duration, duration) {
        (Some(a), Some(b)) => (a - b).abs(),
        _ => 0.0,
    };
    // A different edit (more than 10s apart) would be badly timed.
    let close = |r: &&Record| off(r) <= 10.0;
    records
        .iter()
        .filter(close)
        .filter(|r| {
            r.synced_lyrics
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
        })
        .min_by(|a, b| off(a).total_cmp(&off(b)))
        .or_else(|| records.iter().filter(close).find(|r| pick(r).is_some()))
        .and_then(pick)
}

fn get(url: &str) -> Option<(u16, String)> {
    let mut resp = agent()
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .ok()?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().ok()?;
    Some((status, body))
}

/// Looks a song up: the exact match first (artist, title, album and length
/// within 2s), then a search. `None` when LRCLIB has nothing (or could not
/// be reached; see [`fetch_checked`]).
pub fn fetch(
    artist: &str,
    title: &str,
    album: Option<&str>,
    duration: Option<f64>,
) -> Option<Found> {
    fetch_checked(artist, title, album, duration).flatten()
}

/// Like [`fetch`], telling a miss (`Some(None)`) from no answer at all
/// (`None`: offline), so misses are only remembered when they are real.
pub fn fetch_checked(
    artist: &str,
    title: &str,
    album: Option<&str>,
    duration: Option<f64>,
) -> Option<Option<Found>> {
    // The first artist only: "A, B" rarely matches.
    let artist = artist.split(',').next().unwrap_or(artist).trim();
    if artist.is_empty() || title.trim().is_empty() {
        return Some(None);
    }
    let base = std::env::var("COVERWAVE_LRCLIB_API").unwrap_or_else(|_| API.to_string());
    let mut url = format!(
        "{base}/get?artist_name={}&track_name={}",
        encode(artist),
        encode(title)
    );
    if let Some(album) = album.filter(|a| !a.is_empty()) {
        url += &format!("&album_name={}", encode(album));
    }
    if let Some(d) = duration {
        url += &format!("&duration={}", d.round() as u64);
    }
    match get(&url) {
        None => return None,
        Some((200, body)) => {
            if let Some(found) = parse_get(&body) {
                return Some(Some(found));
            }
        }
        Some(_) => {}
    }
    let url = format!(
        "{base}/search?artist_name={}&track_name={}",
        encode(artist),
        encode(title)
    );
    match get(&url) {
        None => None,
        Some((200, body)) => Some(parse_search(&body, duration)),
        Some(_) => Some(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_synced_then_plain_and_knows_instrumentals() {
        let synced = r#"{"syncedLyrics":"[00:01.00]hi","plainLyrics":"hi","instrumental":false}"#;
        assert_eq!(
            parse_get(synced),
            Some(Found::Lyrics("[00:01.00]hi".into()))
        );
        let plain = r#"{"syncedLyrics":null,"plainLyrics":"hi there","instrumental":false}"#;
        assert_eq!(parse_get(plain), Some(Found::Lyrics("hi there".into())));
        assert_eq!(
            parse_get(r#"{"instrumental":true}"#),
            Some(Found::Instrumental)
        );
        assert_eq!(parse_get(r#"{"syncedLyrics":" ","plainLyrics":""}"#), None);
        assert_eq!(parse_get("not json"), None);
    }

    #[test]
    fn search_picks_the_closest_synced_version() {
        let list = r#"[
          {"duration":200,"syncedLyrics":null,"plainLyrics":"plain one"},
          {"duration":250,"syncedLyrics":"[00:01.00]too long"},
          {"duration":203,"syncedLyrics":"[00:01.00]close"},
          {"duration":206,"syncedLyrics":"[00:01.00]further"}]"#;
        assert_eq!(
            parse_search(list, Some(202.0)),
            Some(Found::Lyrics("[00:01.00]close".into()))
        );
        // Nothing synced within 10s: plain within 10s.
        let plain_only = r#"[{"duration":201,"plainLyrics":"words"},
          {"duration":300,"syncedLyrics":"[00:01.00]other edit"}]"#;
        assert_eq!(
            parse_search(plain_only, Some(200.0)),
            Some(Found::Lyrics("words".into()))
        );
        assert_eq!(parse_search("[]", None), None);
    }
}
