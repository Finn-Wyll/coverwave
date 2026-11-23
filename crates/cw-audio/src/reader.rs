//! Opens one audio file and turns it into interleaved stereo `f32` at the
//! output device's sample rate. Runs on the decoder thread, never on the
//! audio callback, so allocation here is fine.

use crate::AudioError;
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use std::fs::File;
use std::path::Path;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

/// Output is always two interleaved channels; the callback maps it onto
/// whatever the device has.
pub const OUT_CHANNELS: usize = 2;

pub struct TrackReader {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    duration_secs: Option<f64>,
    out_rate: u32,
    src_rate: Option<u32>,
    resampler: Option<Resampling>,
    /// Scratch for the decoder's interleaved output (source channel count).
    decoded: Vec<f32>,
    /// Scratch for the stereo conversion.
    stereo: Vec<f32>,
    /// After a seek: drop decoded frames before this timestamp.
    skip_until: Option<Timestamp>,
    finished: bool,
}

impl TrackReader {
    pub fn open(path: &Path, out_rate: u32) -> Result<Self, AudioError> {
        let file = File::open(path).map_err(|e| AudioError::Open(path.display().to_string(), e))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let unsupported = |e: SymError| AudioError::Unsupported(format!("{}: {e}", path.display()));
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(unsupported)?;
        let track = format.default_track(TrackType::Audio).ok_or_else(|| {
            AudioError::Unsupported(format!("{}: no audio track", path.display()))
        })?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| {
                AudioError::Unsupported(format!("{}: no audio codec", path.display()))
            })?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(unsupported)?;

        let time_base = track.time_base;
        let duration_secs = match (time_base, track.num_frames) {
            (Some(tb), Some(n)) => tb
                .calc_time(Timestamp::new(n as i64))
                .map(|t| t.as_secs_f64()),
            _ => None,
        };

        Ok(TrackReader {
            track_id: track.id,
            time_base,
            duration_secs,
            src_rate: params.sample_rate,
            format,
            decoder,
            out_rate,
            resampler: None,
            decoded: Vec::new(),
            stereo: Vec::new(),
            skip_until: None,
            finished: false,
        })
    }

    pub fn duration_secs(&self) -> Option<f64> {
        self.duration_secs
    }

    /// Appends the next chunk of output (stereo, `out_rate`) to `out`.
    /// Returns `Ok(false)` once the track is exhausted and fully flushed.
    pub fn read_into(&mut self, out: &mut Vec<f32>) -> Result<bool, AudioError> {
        loop {
            if self.finished {
                return Ok(false);
            }
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) | Err(SymError::ResetRequired) => return self.finish(out),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return self.finish(out);
                }
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buf = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                // Corrupt or truncated packet: skip it, keep playing.
                Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => continue,
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            };
            let channels = buf.spec().channels().count().max(1);
            let rate = buf.spec().rate();
            self.decoded.resize(buf.samples_interleaved(), 0.0);
            buf.copy_to_slice_interleaved(&mut self.decoded);
            if buf.frames() == 0 {
                continue;
            }

            // Sample-accurate seek: trim the head of the first packets.
            let mut skip_frames = 0usize;
            if let Some(target) = self.skip_until {
                let end = packet.pts.saturating_add(packet.dur);
                if end.get() <= target.get() {
                    continue;
                }
                skip_frames = (target.get() - packet.pts.get()).max(0) as usize;
                self.skip_until = None;
            }

            to_stereo(&self.decoded, channels, skip_frames, &mut self.stereo);
            if self.src_rate != Some(rate) || (self.resampler.is_none() && rate != self.out_rate) {
                self.src_rate = Some(rate);
                self.resampler = (rate != self.out_rate)
                    .then(|| Resampling::new(rate, self.out_rate))
                    .transpose()?;
            }
            match &mut self.resampler {
                Some(r) => r.push(&self.stereo, out)?,
                None => out.extend_from_slice(&self.stereo),
            }
            return Ok(true);
        }
    }

    fn finish(&mut self, out: &mut Vec<f32>) -> Result<bool, AudioError> {
        self.finished = true;
        if let Some(r) = &mut self.resampler {
            r.finish(out)?;
        }
        Ok(true)
    }

    /// Seeks to `secs`. Returns the position actually reached.
    pub fn seek(&mut self, secs: f64) -> Result<f64, AudioError> {
        let secs = secs.max(0.0);
        let time = Time::try_from_secs_f64(secs).unwrap_or_default();
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| AudioError::Decode(format!("seek failed: {e}")))?;
        self.decoder.reset();
        if let Some(r) = &mut self.resampler {
            r.reset();
        }
        self.finished = false;
        self.skip_until = Some(seeked.required_ts);
        Ok(self
            .time_base
            .and_then(|tb| tb.calc_time(seeked.required_ts))
            .map(|t| t.as_secs_f64())
            .unwrap_or(secs))
    }
}

/// Converts interleaved `channels`-channel audio to interleaved stereo,
/// dropping the first `skip_frames` frames. Mono is duplicated; for more
/// than two channels the first two (front left/right) are kept.
pub(crate) fn to_stereo(input: &[f32], channels: usize, skip_frames: usize, out: &mut Vec<f32>) {
    out.clear();
    for frame in input.chunks_exact(channels).skip(skip_frames) {
        let (l, r) = if channels == 1 {
            (frame[0], frame[0])
        } else {
            (frame[0], frame[1])
        };
        out.push(l);
        out.push(r);
    }
}

/// Streaming sample-rate conversion that produces exactly
/// `round(input_frames * ratio)` frames per track, with the filter delay
/// removed, so consecutive tracks join without gaps or clicks.
struct Resampling {
    rs: Async<f32>,
    ratio: f64,
    pending: Vec<f32>,
    scratch: Vec<f32>,
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

const CHUNK: usize = 1024;

impl Resampling {
    fn new(from: u32, to: u32) -> Result<Self, AudioError> {
        let ratio = to as f64 / from as f64;
        let params = SincInterpolationParameters::new(128, WindowFunction::Blackman2)
            .oversampling_factor(256)
            .interpolation(SincInterpolationType::Cubic);
        let rs =
            Async::<f32>::new_sinc(ratio, 1.1, &params, CHUNK, OUT_CHANNELS, FixedAsync::Input)
                .map_err(|e| AudioError::Decode(format!("resampler: {e}")))?;
        let scratch = vec![0.0; rs.output_frames_max() * OUT_CHANNELS];
        let delay_left = rs.output_delay();
        Ok(Resampling {
            rs,
            ratio,
            pending: Vec::new(),
            scratch,
            delay_left,
            in_total: 0,
            out_total: 0,
        })
    }

    fn reset(&mut self) {
        self.rs.reset();
        self.pending.clear();
        self.delay_left = self.rs.output_delay();
        self.in_total = 0;
        self.out_total = 0;
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), AudioError> {
        self.pending.extend_from_slice(input);
        self.in_total += (input.len() / OUT_CHANNELS) as u64;
        while self.pending.len() / OUT_CHANNELS >= self.rs.input_frames_next() {
            let consumed = self.process(None, out, u64::MAX)?;
            self.pending.drain(..consumed * OUT_CHANNELS);
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), AudioError> {
        let expected = (self.in_total as f64 * self.ratio).round() as u64;
        // A few zero-padded partial chunks flush the filter's tail.
        for _ in 0..64 {
            if self.out_total >= expected {
                break;
            }
            let partial = self.pending.len() / OUT_CHANNELS;
            self.process(Some(partial), out, expected)?;
            self.pending.clear();
        }
        Ok(())
    }

    /// Runs one chunk. Returns input frames consumed. Output beyond `limit`
    /// total frames is discarded.
    fn process(
        &mut self,
        partial: Option<usize>,
        out: &mut Vec<f32>,
        limit: u64,
    ) -> Result<usize, AudioError> {
        let need = self.rs.input_frames_next();
        // The adapter must expose a full chunk even for a partial call.
        if self.pending.len() < need * OUT_CHANNELS {
            self.pending.resize(need * OUT_CHANNELS, 0.0);
        }
        let input = InterleavedSlice::new(&self.pending[..need * OUT_CHANNELS], OUT_CHANNELS, need)
            .map_err(|e| AudioError::Decode(format!("resampler input: {e}")))?;
        let out_frames = self.scratch.len() / OUT_CHANNELS;
        let mut output = InterleavedSlice::new_mut(&mut self.scratch, OUT_CHANNELS, out_frames)
            .map_err(|e| AudioError::Decode(format!("resampler output: {e}")))?;
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: partial,
            active_channels_mask: None,
        };
        let (consumed, produced) = self
            .rs
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|e| AudioError::Decode(format!("resampler: {e}")))?;

        let skip = self.delay_left.min(produced);
        self.delay_left -= skip;
        let available = (produced - skip) as u64;
        let take = available.min(limit.saturating_sub(self.out_total)) as usize;
        out.extend_from_slice(&self.scratch[skip * OUT_CHANNELS..(skip + take) * OUT_CHANNELS]);
        self.out_total += take as u64;
        Ok(consumed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_conversion() {
        let mut out = Vec::new();
        to_stereo(&[0.1, 0.2, 0.3], 1, 0, &mut out);
        assert_eq!(out, vec![0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
        to_stereo(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 3, 1, &mut out);
        assert_eq!(out, vec![4.0, 5.0]);
    }

    #[test]
    fn resampler_output_length_is_exact() {
        let mut r = Resampling::new(44_100, 48_000).unwrap();
        let mut out = Vec::new();
        // 1.5 seconds pushed in uneven blocks, as a decoder would.
        let mut fed = 0;
        for block in [1152usize, 777, 4096, 1, 3000].iter().cycle() {
            if fed >= 66_150 {
                break;
            }
            let n = (*block).min(66_150 - fed);
            r.push(&vec![0.25; n * 2], &mut out).unwrap();
            fed += n;
        }
        r.finish(&mut out).unwrap();
        assert_eq!(out.len() / 2, 72_000);
        // DC input stays DC once the filter has settled (no gap, no click).
        let mid = &out[20_000..120_000];
        assert!(mid.iter().all(|s| (s - 0.25).abs() < 0.01));
    }

    /// Writes a 16-bit mono PCM WAV with a 440 Hz sine.
    fn write_wav(path: &Path, rate: u32, secs: f64) {
        let frames = (rate as f64 * secs) as u32;
        let mut b = Vec::new();
        let data_len = frames * 2;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let s = (i as f64 / rate as f64 * 440.0 * std::f64::consts::TAU).sin() * 0.5;
            b.extend_from_slice(&((s * i16::MAX as f64) as i16).to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    fn read_all(r: &mut TrackReader) -> Vec<f32> {
        let mut out = Vec::new();
        while r.read_into(&mut out).unwrap() {}
        out
    }

    #[test]
    fn decodes_resamples_and_seeks_a_wav() {
        let dir = std::env::temp_dir().join(format!("cw-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        write_wav(&path, 44_100, 2.0);

        let mut r = TrackReader::open(&path, 48_000).unwrap();
        assert!((r.duration_secs().unwrap() - 2.0).abs() < 1e-3);
        let all = read_all(&mut r);
        assert_eq!(all.len() / OUT_CHANNELS, 96_000);
        // Mono duplicated into both channels.
        assert!(all.as_chunks::<2>().0.iter().all(|[l, r]| l == r));

        let mut r = TrackReader::open(&path, 48_000).unwrap();
        let at = r.seek(1.5).unwrap();
        assert!((at - 1.5).abs() < 0.01, "seeked to {at}");
        let rest = read_all(&mut r).len() / OUT_CHANNELS;
        assert!(
            (rest as i64 - 24_000).abs() <= 2,
            "{rest} frames after seek"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
