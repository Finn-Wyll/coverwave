//! Fuzzy search over title, artist and album (nucleo's matcher, the one
//! Helix uses), with a small boost for tracks played often and recently.

use crate::LibTrack;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Days over which the "recently played" boost fades.
const RECENCY_DAYS: f64 = 30.0;

/// Indices into `tracks` of the best matches for `query`, best first, at
/// most `limit`. An empty query matches nothing.
pub fn search(tracks: &[LibTrack], query: &str, now: i64, limit: usize) -> Vec<usize> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(f64, usize)> = tracks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let hay = format!(
                "{} {} {}",
                t.title,
                t.artist.as_deref().unwrap_or(""),
                t.album.as_deref().unwrap_or("")
            );
            let score = pattern.score(Utf32Str::new(&hay, &mut buf), &mut matcher)?;
            Some((score as f64 * boost(t, now), i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, i)| i).collect()
}

/// 1.0 for a never-played track, up to about 1.5 for a favourite played
/// today.
fn boost(t: &LibTrack, now: i64) -> f64 {
    let plays = 0.08 * (1.0 + t.play_count as f64).ln();
    let recent = t.last_played.map_or(0.0, |at| {
        let days = (now - at).max(0) as f64 / 86_400.0;
        0.15 * (-days / RECENCY_DAYS).exp()
    });
    1.0 + plays.min(0.35) + recent
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn t(title: &str, artist: &str, album: &str, plays: u32, last: Option<i64>) -> LibTrack {
        LibTrack {
            id: 0,
            path: PathBuf::from(format!("/{title}")),
            title: title.into(),
            artist: Some(artist.into()),
            album: Some(album.into()),
            album_artist: None,
            group_artist: artist.into(),
            group_album: album.into(),
            track_no: None,
            disc_no: None,
            duration_secs: None,
            play_count: plays,
            last_played: last,
        }
    }

    #[test]
    fn matches_across_fields_and_ignores_case() {
        let tracks = vec![
            t("Aerials", "System Of A Down", "Toxicity", 0, None),
            t("Chop Suey!", "System Of A Down", "Toxicity", 0, None),
            t(
                "Banana Pancakes",
                "Jack Johnson",
                "In Between Dreams",
                0,
                None,
            ),
        ];
        assert_eq!(search(&tracks, "aerial", 0, 10), [0]);
        assert_eq!(search(&tracks, "jack pancake", 0, 10), [2]);
        let soad = search(&tracks, "toxicity", 0, 10);
        assert_eq!(soad.len(), 2);
        assert!(search(&tracks, "zzzz", 0, 10).is_empty());
        assert!(search(&tracks, "  ", 0, 10).is_empty());
    }

    #[test]
    fn plays_and_recency_break_ties() {
        let now = 100 * 86_400;
        let tracks = vec![
            t("Intro", "A", "X", 0, None),
            t("Intro", "A", "Y", 12, Some(now - 3600)),
        ];
        assert_eq!(search(&tracks, "intro", now, 10), [1, 0]);
        assert_eq!(search(&tracks, "intro", now, 1), [1]);
    }
}
