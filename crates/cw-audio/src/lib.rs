//! Local audio playback: decoding, resampling, gapless queue playback,
//! seeking and volume.
//!
//! Threads:
//! - the **audio callback** (owned by cpal) only reads a lock-free ring
//!   buffer and touches atomics: no allocation, no locks;
//! - the **decoder thread** owns the queue and the open file, decodes ahead
//!   into the ring buffer, and handles commands;
//! - the **UI thread** holds a [`Player`] handle, sends commands and reads a
//!   [`Snapshot`] each frame.

mod analysis;
mod info;
mod player;
mod reader;

pub use analysis::{Analyser, AnalyserHandle, TempoEstimate, estimate_tempo};
pub use info::{AUDIO_EXTENSIONS, Track, TrackInfo, collect_audio_files, cover_art, is_audio_file};
pub use player::{Player, Snapshot, Status};

/// Decodes a whole file at `out_rate` without playing it and returns the
/// number of stereo frames produced. For diagnostics and tests.
pub fn decode_to_end(path: &std::path::Path, out_rate: u32) -> Result<u64, AudioError> {
    let mut reader = reader::TrackReader::open(path, out_rate)?;
    let mut buf = Vec::new();
    let mut frames = 0u64;
    while reader.read_into(&mut buf)? {
        frames += (buf.len() / reader::OUT_CHANNELS) as u64;
        buf.clear();
    }
    frames += (buf.len() / reader::OUT_CHANNELS) as u64;
    Ok(frames)
}

/// Decodes a whole file at `out_rate` and runs the analyser over it exactly
/// as playback would (mono mix, pre-volume), returning one [`AudioFrame`]
/// every `every_frames` output frames. For diagnostics and tuning.
pub fn analyse_file(
    path: &std::path::Path,
    out_rate: u32,
    every_frames: usize,
) -> Result<Vec<cw_core::AudioFrame>, AudioError> {
    let mut reader = reader::TrackReader::open(path, out_rate)?;
    let mut analyser = Analyser::new(out_rate);
    let every = every_frames.max(1);
    let (mut buf, mut mono, mut out) = (Vec::new(), Vec::new(), Vec::new());
    let mut since = 0;
    loop {
        let more = reader.read_into(&mut buf)?;
        mono.clear();
        mono.extend(
            buf.chunks(reader::OUT_CHANNELS)
                .map(|f| f.iter().sum::<f32>() * 0.5),
        );
        buf.clear();
        for chunk in mono.chunks(every) {
            // Report on the same cadence regardless of decode chunk sizes.
            let take = (every - since).min(chunk.len());
            analyser.push(&chunk[..take]);
            since += take;
            if since == every {
                out.push(analyser.frame());
                since = 0;
            }
            analyser.push(&chunk[take..]);
            since += chunk.len() - take;
        }
        if !more {
            return Ok(out);
        }
    }
}

/// Decodes a whole file to mono at `out_rate` (for tempo estimation).
pub fn decode_mono(path: &std::path::Path, out_rate: u32) -> Result<Vec<f32>, AudioError> {
    let mut reader = reader::TrackReader::open(path, out_rate)?;
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    loop {
        let more = reader.read_into(&mut buf)?;
        out.extend(
            buf.chunks(reader::OUT_CHANNELS)
                .map(|f| f.iter().sum::<f32>() * 0.5),
        );
        buf.clear();
        if !more {
            return Ok(out);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("cannot open {0}: {1}")]
    Open(String, #[source] std::io::Error),
    #[error("unsupported audio: {0}")]
    Unsupported(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("audio device: {0}")]
    Device(String),
}
