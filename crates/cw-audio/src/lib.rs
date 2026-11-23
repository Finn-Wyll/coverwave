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

mod info;
mod player;
mod reader;

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
