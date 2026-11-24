//! System-audio capture (PLAN.md section 8): what another app is playing,
//! as a mono stream for the analyser, so Spotify and Apple Music get the
//! same audio-reactive visuals as local files.
//!
//! | Platform | Backend |
//! |---|---|
//! | macOS 14.2+ | Core Audio process tap on the source app only (no screen-recording permission) |
//! | Windows | WASAPI loopback of the default output, through cpal |
//! | Linux | the default input device, through cpal; set it to your output's monitor (`pactl set-default-source <sink>.monitor`) |
//!
//! Capture only ever feeds the analyser: nothing is recorded or stored. The
//! callback pushes into a lock-free ring buffer and never allocates or locks.

#[cfg(not(target_os = "macos"))]
mod cpal_backend;
#[cfg(target_os = "macos")]
mod macos;

use rtrb::Producer;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// None of the target apps currently has an audio stream.
    #[error("{0} is not playing audio")]
    NotPlaying(String),
    #[error("capture is not supported here: {0}")]
    Unsupported(String),
    /// The OS refused (macOS: the audio-capture permission was denied).
    #[error("capture permission denied: {0}")]
    Denied(String),
    #[error("capture failed: {0}")]
    Failed(String),
}

/// What to capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Only these apps (macOS bundle IDs, matched as prefixes so helper
    /// processes count, e.g. `com.spotify.client`). Other platforms capture
    /// the whole output instead.
    Apps(Vec<String>),
    /// Everything the default output plays.
    System,
}

/// A running capture; dropping it stops capturing and releases the OS
/// resources.
pub struct Capture {
    sample_rate: u32,
    #[cfg(target_os = "macos")]
    _inner: macos::Tap,
    #[cfg(not(target_os = "macos"))]
    _inner: cpal_backend::Stream,
}

impl Capture {
    /// Starts capturing `target`, pushing a mono mix of it into `sink`.
    /// Samples that do not fit (the consumer fell behind) are dropped.
    pub fn start(target: &Target, sink: Producer<f32>) -> Result<Capture, CaptureError> {
        #[cfg(target_os = "macos")]
        {
            let tap = macos::Tap::start(target, sink)?;
            Ok(Capture {
                sample_rate: tap.sample_rate(),
                _inner: tap,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let stream = cpal_backend::Stream::start(target, sink)?;
            Ok(Capture {
                sample_rate: stream.sample_rate(),
                _inner: stream,
            })
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// Mixes interleaved frames of `channels` channels to mono and pushes them,
/// dropping what does not fit. Real-time safe.
pub(crate) fn push_mono(sink: &mut Producer<f32>, interleaved: &[f32], channels: usize) {
    let channels = channels.max(1);
    let scale = 1.0 / channels as f32;
    for frame in interleaved.chunks_exact(channels) {
        let m: f32 = frame.iter().sum::<f32>() * scale;
        if sink.push(m).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_mix_and_overflow_drop() {
        let (mut tx, mut rx) = rtrb::RingBuffer::new(2);
        push_mono(&mut tx, &[1.0, 0.0, 0.5, 0.5, 9.0, 9.0], 2);
        assert_eq!(rx.pop(), Ok(0.5));
        assert_eq!(rx.pop(), Ok(0.5));
        assert!(rx.pop().is_err(), "third frame was dropped, not blocked on");
    }
}
