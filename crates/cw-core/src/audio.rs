//! The analyser's per-moment summary of the music, as the renderer sees it.

/// Number of frequency bands in [`AudioFrame::bands`].
pub const BAND_COUNT: usize = 6;

/// Band names, low to high, for display.
pub const BAND_NAMES: [&str; BAND_COUNT] = ["sub", "bass", "lowmid", "mid", "highmid", "treble"];

/// Band edges in Hz: band `i` spans `BAND_EDGES[i]..BAND_EDGES[i + 1]`.
pub const BAND_EDGES: [f32; BAND_COUNT + 1] = [20.0, 60.0, 250.0, 500.0, 2000.0, 4000.0, 16000.0];

/// A snapshot of the music. Every level is normalised to 0..1 against a slow
/// loudness tracker, so quiet tracks move as much as loud ones, and smoothed
/// with a fast attack and slower release. Silence (or pause) reads as zeros.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AudioFrame {
    /// sub-bass, bass, low-mid, mid, high-mid, treble.
    pub bands: [f32; BAND_COUNT],
    /// Overall loudness.
    pub level: f32,
    /// Jumps to 1 on each detected onset (a drum hit, a new note), then
    /// decays over about 150ms. Use it for brief pulses.
    pub onset: f32,
    /// Onsets detected since the analyser started. Lets a consumer that
    /// samples slower than onsets arrive still count every one.
    pub onset_count: u64,
}

impl AudioFrame {
    pub fn bass(&self) -> f32 {
        self.bands[0].max(self.bands[1])
    }

    pub fn mid(&self) -> f32 {
        self.bands[2].max(self.bands[3])
    }

    pub fn treble(&self) -> f32 {
        self.bands[4].max(self.bands[5])
    }
}
