//! Tempo for tracks we cannot hear: Deezer's public API (no key) finds the
//! track by ISRC, or by artist and title. Its `bpm` field is used when set
//! (it often is not); otherwise the 30-second preview MP3 is downloaded and
//! analysed locally with `cw_audio::estimate_tempo`.
//!
//! Sends the track's ISRC or artist and title to Deezer; nothing about the
//! user. Turned off with `[tempo] online_lookup = false`.

use crate::NowPlaying;
use crate::util::{agent, http_get_bytes};
use serde::Deserialize;

const API: &str = "https://api.deezer.com";
/// Below this confidence an estimate is ignored (the genre's typical tempo
/// is used instead).
pub const MIN_CONFIDENCE: f32 = 0.3;

#[derive(Deserialize)]
struct Track {
    id: Option<u64>,
    #[serde(default)]
    bpm: f32,
    preview: Option<String>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    data: Vec<Track>,
}

/// Deezer's `bpm` when it is a plausible tempo.
pub fn parse_track(json: &str) -> Option<(Option<f32>, Option<String>)> {
    let t: Track = serde_json::from_str(json).ok()?;
    if t.error.is_some() || t.id.is_none() {
        return None;
    }
    let bpm = (40.0..=220.0).contains(&t.bpm).then_some(t.bpm);
    Some((bpm, t.preview.filter(|p| !p.is_empty())))
}

fn get(url: &str) -> Option<String> {
    let mut resp = agent().get(url).call().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.body_mut().read_to_string().ok()
}

fn find(now: &NowPlaying) -> Option<String> {
    if let Some(isrc) = &now.isrc
        && let Some(body) = get(&format!("{API}/track/isrc:{}", crate::util::encode(isrc)))
        && parse_track(&body).is_some()
    {
        return Some(body);
    }
    let artist = now
        .artist
        .as_deref()?
        .split(',')
        .next()?
        .trim()
        .replace('"', "");
    let title = now.title.replace('"', "");
    let q = format!("artist:\"{artist}\" track:\"{title}\"");
    let body = get(&format!(
        "{API}/search?limit=1&q={}",
        crate::util::encode(&q)
    ))?;
    let id = serde_json::from_str::<Search>(&body)
        .ok()?
        .data
        .first()?
        .id?;
    get(&format!("{API}/track/{id}"))
}

/// The track's tempo in BPM, or `None` (blocking: call off the UI thread).
pub fn lookup(now: &NowPlaying) -> Option<f32> {
    let body = find(now)?;
    let (bpm, preview) = parse_track(&body)?;
    if bpm.is_some() {
        return bpm;
    }
    let mp3 = http_get_bytes(&preview?).ok()?;
    let path = std::env::temp_dir().join(format!("coverwave-preview-{}.mp3", std::process::id()));
    std::fs::write(&path, mp3).ok()?;
    let rate = 44_100;
    let samples = cw_audio::decode_mono(&path, rate);
    let _ = std::fs::remove_file(&path);
    let est = cw_audio::estimate_tempo(&samples.ok()?, rate)?;
    (est.confidence >= MIN_CONFIDENCE).then_some(est.bpm)
}

/// The genres of the track's album on Deezer ("Rock, Alternative"), for
/// tracks whose source gave none (blocking).
pub fn album_genre(now: &NowPlaying) -> Option<String> {
    let track: serde_json::Value = serde_json::from_str(&find(now)?).ok()?;
    let id = track["album"]["id"].as_u64()?;
    let album: serde_json::Value =
        serde_json::from_str(&get(&format!("{API}/album/{id}"))?).ok()?;
    let names: Vec<&str> = album["genres"]["data"]
        .as_array()?
        .iter()
        .filter_map(|g| g["name"].as_str())
        .filter(|n| !n.is_empty())
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// Whether the internet answers (Deezer's API, a few seconds at most).
pub fn online() -> bool {
    agent()
        .get(&format!("{API}/infos"))
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(4)))
        .build()
        .call()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bpm_and_preview() {
        let set = r#"{"id":1,"bpm":122.7,"preview":"https://x/p.mp3"}"#;
        assert_eq!(
            parse_track(set),
            Some((Some(122.7), Some("https://x/p.mp3".into())))
        );
        let unset = r#"{"id":1,"bpm":0,"preview":"https://x/p.mp3"}"#;
        assert_eq!(
            parse_track(unset),
            Some((None, Some("https://x/p.mp3".into())))
        );
        let error = r#"{"error":{"type":"DataException","message":"no data","code":800}}"#;
        assert_eq!(parse_track(error), None);
        assert_eq!(parse_track("nope"), None);
    }
}
