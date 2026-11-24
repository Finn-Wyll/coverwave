//! A song's rhythm map: tempo, metre, where its bars start, and its groove
//! (what each band does at each sixteenth of a bar). It is measured from a
//! short clip of the song (a 30-second preview) by `cw_analysis::rhythm`,
//! then replayed against the playback position for music the app cannot
//! hear (Spotify and Apple Music on iOS).
//!
//! Unlike [`crate::BeatClock`], which plays a generic pattern for the genre,
//! the groove is the song's own: kicks, snares and hats land where they do
//! in the recording. The bar phase comes from where the clip sits in the
//! song (`clip_start`): Deezer cuts its previews from 48 seconds in, so the
//! clip's beat grid extends to the whole track.
//!
//! Everything here is a pure function of its inputs.

use crate::audio::{AudioFrame, BAND_COUNT};
use serde::{Deserialize, Serialize};

/// Groove resolution: sixteenth notes.
pub const STEPS_PER_BEAT: usize = 4;

/// Bump when the map's meaning changes; cached maps with another version are
/// measured again.
/// 2: `clip_start` (2026-11-25).
pub const RHYTHM_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RhythmMap {
    pub version: u32,
    pub bpm: f32,
    /// 3 or 4.
    pub beats_per_bar: u8,
    /// Seconds into the clip of its first downbeat.
    pub first_downbeat: f32,
    /// Seconds into the song where the clip starts, when known. Without it
    /// the bar phase is a guess.
    #[serde(default)]
    pub clip_start: Option<f32>,
    /// Per step of a bar (`beats_per_bar * STEPS_PER_BEAT`): how much each
    /// band rises at that step (0..1, the loudest step of each band is 1).
    pub hits: Vec<[f32; BAND_COUNT]>,
    /// Per step: each band's average level (0..1).
    pub levels: Vec<[f32; BAND_COUNT]>,
    /// Per step: overall attack strength (0..1), what the onsets follow.
    pub accents: Vec<f32>,
    /// How regular the clip's beats were (0..1).
    pub confidence: f32,
}

/// Envelope of a hit `t` seconds after it.
fn decay(t: f32, per_sec: f32) -> f32 {
    if t < 0.0 { 0.0 } else { (-t * per_sec).exp() }
}

/// An accent at least this strong counts as an onset.
const ONSET_ACCENT: f32 = 0.45;

impl RhythmMap {
    pub fn steps(&self) -> usize {
        self.beats_per_bar as usize * STEPS_PER_BEAT
    }

    pub fn beat_secs(&self) -> f64 {
        60.0 / self.bpm.clamp(30.0, 300.0) as f64
    }

    pub fn bar_secs(&self) -> f64 {
        self.beat_secs() * self.beats_per_bar as f64
    }

    pub fn step_secs(&self) -> f64 {
        self.beat_secs() / STEPS_PER_BEAT as f64
    }

    /// A downbeat time in the song: the clip's first downbeat, placed where
    /// the clip starts (or as if it started the song, when unknown).
    pub fn downbeat(&self) -> f64 {
        self.clip_start.unwrap_or(0.0) as f64 + self.first_downbeat as f64
    }

    /// Whether the bar phase is known rather than guessed.
    pub fn is_aligned(&self) -> bool {
        self.clip_start.is_some()
    }

    /// Whether the map is usable: right length and a sane tempo.
    pub fn is_valid(&self) -> bool {
        let n = self.steps();
        self.version == RHYTHM_VERSION
            && (3..=4).contains(&self.beats_per_bar)
            && (40.0..=240.0).contains(&self.bpm)
            && self.hits.len() == n
            && self.levels.len() == n
            && self.accents.len() == n
    }

    /// The music at `position_secs` into the track, with a downbeat at
    /// `downbeat_secs` (any downbeat; only its phase within a bar matters).
    /// Silence when not playing.
    pub fn frame(&self, position_secs: f64, playing: bool, downbeat_secs: f64) -> AudioFrame {
        if !playing || position_secs < 0.0 || !self.is_valid() {
            return AudioFrame::default();
        }
        let n = self.steps();
        let step_len = self.step_secs();
        let t = position_secs - downbeat_secs;
        let global = (t / step_len).floor();
        let since = ((t - global * step_len) as f32).max(0.0);
        let step = (global as i64).rem_euclid(n as i64) as usize;
        // Hits ring on for a few steps: sum the current and previous ones.
        let mut bands = [0.0f32; BAND_COUNT];
        for back in 0..4usize {
            let s = (step + n - back) % n;
            let age = since + back as f32 * step_len as f32;
            for (b, out) in bands.iter_mut().enumerate() {
                // Low bands ring longer than high ones.
                let rate = 5.0 + 4.0 * b as f32;
                *out += self.hits[s][b] * decay(age, rate);
            }
        }
        let mut level_sum = 0.0;
        for (b, out) in bands.iter_mut().enumerate() {
            *out = (0.35 * self.levels[step][b] + 0.75 * *out).clamp(0.0, 1.0);
            level_sum += *out;
        }
        let accent = self.accents[step];
        let onset = if accent >= ONSET_ACCENT {
            accent * decay(since, 9.0)
        } else {
            0.0
        };
        AudioFrame {
            bands,
            level: (level_sum / BAND_COUNT as f32 * 1.3).clamp(0.0, 1.0),
            onset: onset.clamp(0.0, 1.0),
            // Steps since the track's first bar: monotonic, seek-consistent.
            onset_count: global.max(0.0) as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 BPM 4/4: kick on 1 and 3, snare on 2 and 4, hats on eighths.
    fn backbeat() -> RhythmMap {
        let n = 16;
        let mut hits = vec![[0.0; BAND_COUNT]; n];
        let mut accents = vec![0.0; n];
        for s in 0..n {
            if s % 2 == 0 {
                hits[s][5] = 0.5;
                accents[s] = 0.3;
            }
            if s == 0 || s == 8 {
                hits[s][0] = 1.0;
                hits[s][1] = 1.0;
                accents[s] = 1.0;
            }
            if s == 4 || s == 12 {
                hits[s][3] = 1.0;
                accents[s] = 0.8;
            }
        }
        RhythmMap {
            version: RHYTHM_VERSION,
            bpm: 120.0,
            beats_per_bar: 4,
            first_downbeat: 0.0,
            clip_start: None,
            hits,
            levels: vec![[0.3; BAND_COUNT]; n],
            accents,
            confidence: 0.9,
        }
    }

    #[test]
    fn groove_lands_where_the_song_has_it() {
        let m = backbeat();
        // Bar of 2s; downbeat at 0.25s. Kick at 0.25, snare at 0.75.
        let kick = m.frame(0.26, true, 0.25);
        let snare = m.frame(0.76, true, 0.25);
        assert!(kick.bass() > snare.bass() + 0.2, "{kick:?} {snare:?}");
        assert!(snare.bands[3] > kick.bands[3] + 0.2);
        assert!(kick.onset > 0.5 && snare.onset > 0.4);
        // Between hits: quieter, no onset.
        let gap = m.frame(0.25 + 0.125 * 1.5, true, 0.25);
        assert!(gap.bass() < kick.bass() && gap.onset == 0.0);
        // Paused or invalid: silence. Bounded and deterministic.
        assert_eq!(m.frame(1.0, false, 0.0), AudioFrame::default());
        for i in 0..500 {
            let f = m.frame(i as f64 * 0.031, true, 0.1);
            assert_eq!(f, m.frame(i as f64 * 0.031, true, 0.1));
            assert!(
                f.bands
                    .iter()
                    .chain([&f.level, &f.onset])
                    .all(|v| (0.0..=1.0).contains(v))
            );
        }
        let mut bad = m.clone();
        bad.accents.pop();
        assert_eq!(bad.frame(1.0, true, 0.0), AudioFrame::default());
    }

    #[test]
    fn onset_count_is_monotonic_and_seek_consistent() {
        let m = backbeat();
        let mut last = 0;
        for i in 0..300 {
            let c = m.frame(i as f64 * 0.04, true, 0.0).onset_count;
            assert!(c >= last);
            last = c;
        }
        assert_eq!(m.frame(10.01, true, 0.0).onset_count, 80);
    }

    #[test]
    fn clip_start_places_the_bar() {
        let mut m = backbeat();
        m.first_downbeat = 0.3;
        assert!(!m.is_aligned());
        assert_eq!(m.downbeat(), 0.3f32 as f64);
        m.clip_start = Some(48.0);
        assert!(m.is_aligned());
        assert!((m.downbeat() - 48.3).abs() < 1e-5);
        // A 2s bar: 48.3s puts downbeats at 0.3s, 2.3s, ... into the song.
        let kick = m.frame(2.31, true, m.downbeat());
        let off = m.frame(2.31 + 0.25, true, m.downbeat());
        assert!(kick.bass() > off.bass() + 0.2);
    }

    #[test]
    fn round_trips_as_json() {
        let m = backbeat();
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<RhythmMap>(&json).unwrap(), m);
    }
}
