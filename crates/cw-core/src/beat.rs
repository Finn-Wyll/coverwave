//! A beat clock: a synthetic [`AudioFrame`] from tempo, rhythm pattern and
//! playback position, for when the music itself cannot be heard (iOS, where
//! other apps' audio is off-limits, or when system capture is unavailable).
//!
//! Kicks drive the bass bands and onsets, snares the mids, hats the treble.
//! It is a pure function of its inputs, so motion stays deterministic and in
//! step with the playback position (seeks included).

use crate::audio::{AudioFrame, BAND_COUNT};

/// Rhythm shapes, chosen from the genre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BeatPattern {
    /// Kick on every beat, hats on the off-beats (electronic, dance).
    FourOnFloor,
    /// Kick on 1 and 3, snare on 2 and 4, eighth-note hats (rock, pop).
    #[default]
    BackBeat,
    /// Kick on 1 and the "and" of 2, snare on 3 (hip-hop).
    HalfTime,
    /// Kick and snare together on 3 (reggae).
    OneDrop,
    /// No hard hits: slow swells once a bar (ambient, classical).
    Swell,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeatClock {
    pub bpm: f32,
    pub pattern: BeatPattern,
    /// 0..1: how hard the hits are.
    pub energy: f32,
}

/// Envelope of a hit `t` beats after it (fast attack, exponential decay).
fn hit(t: f32, decay: f32) -> f32 {
    if t < 0.0 { 0.0 } else { (-t * decay).exp() }
}

impl BeatClock {
    pub fn new(bpm: f32, pattern: BeatPattern, energy: f32) -> BeatClock {
        BeatClock {
            bpm: if bpm.is_finite() {
                bpm.clamp(40.0, 220.0)
            } else {
                110.0
            },
            pattern,
            energy: energy.clamp(0.0, 1.0),
        }
    }

    /// The frame at `position_secs`. Silence when not playing.
    pub fn frame(&self, position_secs: f64, playing: bool) -> AudioFrame {
        if !playing || position_secs < 0.0 {
            return AudioFrame::default();
        }
        let beats = position_secs * self.bpm as f64 / 60.0;
        let bar_beat = (beats % 4.0) as f32; // 0..4 within a 4/4 bar
        // Beats since the most recent hit at bar position `at`.
        let since = |at: f32| (bar_beat - at).rem_euclid(4.0);
        let k = 0.55 + 0.45 * self.energy;
        let (kick, snare, hats) = match self.pattern {
            BeatPattern::FourOnFloor => {
                let kick = hit(bar_beat.fract(), 6.0);
                let hats = hit((bar_beat - 0.5).rem_euclid(1.0), 10.0);
                (kick, 0.4 * hit(since(1.0).min(since(3.0)), 7.0), hats)
            }
            BeatPattern::BackBeat => {
                let kick = hit(since(0.0).min(since(2.0)), 6.0);
                let snare = hit(since(1.0).min(since(3.0)), 7.0);
                (kick, snare, 0.7 * hit((bar_beat * 2.0).fract() / 2.0, 12.0))
            }
            BeatPattern::HalfTime => {
                let kick = hit(since(0.0).min(since(1.5)), 5.0);
                let snare = hit(since(2.0), 6.0);
                (kick, snare, 0.6 * hit((bar_beat * 2.0).fract() / 2.0, 12.0))
            }
            BeatPattern::OneDrop => {
                let drop = hit(since(2.0), 5.0);
                (
                    drop,
                    drop,
                    0.5 * hit((bar_beat - 0.5).rem_euclid(1.0), 10.0),
                )
            }
            BeatPattern::Swell => {
                let swell = 0.5 - 0.5 * (std::f32::consts::TAU * bar_beat / 4.0).cos();
                (0.6 * swell, 0.5 * swell, 0.3 * swell)
            }
        };
        let bed = 0.15 + 0.15 * self.energy;
        let lvl = |v: f32| (bed + k * v).clamp(0.0, 1.0);
        let mut bands = [0.0; BAND_COUNT];
        bands[0] = lvl(kick * 0.9);
        bands[1] = lvl(kick);
        bands[2] = lvl(0.6 * snare + 0.2 * kick);
        bands[3] = lvl(snare);
        bands[4] = lvl(0.6 * hats + 0.3 * snare);
        bands[5] = lvl(hats);
        let level = lvl(0.5 * kick + 0.3 * snare + 0.2 * hats);
        // Onsets: one per main hit, counted from the start of the track.
        let (onset, count) = match self.pattern {
            BeatPattern::Swell => (0.0, (beats / 4.0) as u64),
            _ => (
                kick.max(snare) * (bar_beat.fract() < 0.25) as u8 as f32,
                beats as u64,
            ),
        };
        AudioFrame {
            bands,
            level,
            onset,
            onset_count: count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_when_paused_and_bounded() {
        let c = BeatClock::new(120.0, BeatPattern::BackBeat, 0.7);
        assert_eq!(c.frame(3.0, false), AudioFrame::default());
        for i in 0..400 {
            let f = c.frame(i as f64 * 0.037, true);
            assert!(
                f.bands
                    .iter()
                    .chain([&f.level, &f.onset])
                    .all(|v| (0.0..=1.0).contains(v))
            );
        }
        assert_eq!(BeatClock::new(f32::NAN, BeatPattern::Swell, 2.0).bpm, 110.0);
    }

    #[test]
    fn kicks_land_on_the_beat() {
        // 120 BPM: a beat every 0.5s. Four-on-the-floor peaks bass on each.
        let c = BeatClock::new(120.0, BeatPattern::FourOnFloor, 1.0);
        let on = c.frame(1.0, true).bass();
        let off = c.frame(1.25, true).bass();
        assert!(on > off + 0.3, "on {on} off {off}");
        assert_eq!(c.frame(10.01, true).onset_count, 20);
        // Backbeat: snare (mid) on beat 2, not on beat 1.
        let b = BeatClock::new(120.0, BeatPattern::BackBeat, 1.0);
        assert!(b.frame(0.5, true).bands[3] > b.frame(0.0, true).bands[3] + 0.3);
    }

    #[test]
    fn deterministic_and_seek_consistent() {
        let c = BeatClock::new(96.0, BeatPattern::HalfTime, 0.5);
        assert_eq!(c.frame(42.3, true), c.frame(42.3, true));
        // Swell has no hard onsets.
        let s = BeatClock::new(70.0, BeatPattern::Swell, 0.2);
        assert!((0..100).all(|i| s.frame(i as f64 * 0.1, true).onset == 0.0));
    }
}
