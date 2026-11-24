//! Measures a song's [`RhythmMap`] from a short clip (a 30-second preview).
//!
//! 1. Log-mel spectrogram exactly as Beat This! expects it (22050 Hz,
//!    1024-point STFT every 441 samples = 50 frames/s, 128 Slaney mels from
//!    30 Hz to 11 kHz, `log1p(1000 * magnitude)`).
//! 2. The Beat This! "small" model (Foscarin et al., ISMIR 2024; MIT
//!    licence, weights included) gives beat and downbeat logits per frame.
//!    Run with tract, like the cover model.
//! 3. Its "minimal" postprocessing: peaks over +/-3 frames with logit > 0.
//! 4. A straight beat grid fitted through the beats (tempo and phase), the
//!    metre from the downbeats, then the groove: what each of the six bands
//!    does at every sixteenth of the bar, averaged over the clip's bars.
//!
//! Pure: samples in, map out. Fetching and decoding the clip is the app's
//! job.

use cw_core::audio::{BAND_COUNT, BAND_EDGES};
use cw_core::rhythm::{RHYTHM_VERSION, RhythmMap, STEPS_PER_BEAT};
use realfft::RealFftPlanner;
use tract_onnx::prelude::*;

pub const SAMPLE_RATE: u32 = 22050;
pub const N_FFT: usize = 1024;
pub const HOP: usize = 441;
pub const N_MELS: usize = 128;
pub const F_MIN: f32 = 30.0;
pub const F_MAX: f32 = 11000.0;
pub const FPS: f32 = SAMPLE_RATE as f32 / HOP as f32;
/// Grids faster than this are taken to be counting eighth notes.
const MAX_FOLDED_BPM: f32 = 175.0;
/// Frames per model run: 30 seconds, the length of a preview.
pub const CHUNK: usize = 1500;

#[derive(Debug, thiserror::Error)]
pub enum RhythmError {
    #[error("beat model: {0}")]
    Model(String),
    #[error("clip too short or too quiet")]
    TooShort,
    #[error("no steady beat found")]
    NoBeat,
}

impl From<TractError> for RhythmError {
    fn from(e: TractError) -> Self {
        RhythmError::Model(format!("{e:#}"))
    }
}

/// The loaded beat model. Thread-safe: share it in an `Arc`.
pub struct BeatModel {
    plan: std::sync::Arc<TypedRunnableModel>,
}

/// Beat and downbeat times in seconds, from the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Beats {
    pub beats: Vec<f32>,
    pub downbeats: Vec<f32>,
}

impl BeatModel {
    /// Parses and optimises the model (input `(1, CHUNK, N_MELS)`, outputs
    /// beat and downbeat logits `(1, CHUNK)`).
    pub fn from_onnx_bytes(bytes: &[u8]) -> Result<BeatModel, RhythmError> {
        let plan = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(bytes))?
            .with_input_fact(0, f32::fact([1, CHUNK, N_MELS]).into())?
            .into_optimized()?
            .into_runnable()?;
        let model = BeatModel { plan };
        let (b, d) = model.logits(&vec![0.0; CHUNK * N_MELS])?;
        if b.len() != CHUNK || d.len() != CHUNK {
            return Err(RhythmError::Model("unexpected output shape".into()));
        }
        Ok(model)
    }

    /// Beat and downbeat logits for one chunk of log-mel frames (row-major
    /// `CHUNK x N_MELS`).
    pub fn logits(&self, mel: &[f32]) -> Result<(Vec<f32>, Vec<f32>), RhythmError> {
        let input: Tensor = tract_ndarray::Array3::from_shape_vec((1, CHUNK, N_MELS), mel.to_vec())
            .map_err(|e| RhythmError::Model(e.to_string()))?
            .into();
        let out = self.plan.run(tvec!(input.into()))?;
        if out.len() < 2 {
            return Err(RhythmError::Model(
                "expected beat and downbeat outputs".into(),
            ));
        }
        let take = |t: &TValue| -> Result<Vec<f32>, RhythmError> {
            Ok(t.to_plain_array_view::<f32>()?.iter().copied().collect())
        };
        Ok((take(&out[0])?, take(&out[1])?))
    }

    /// Beats and downbeats in a mono clip (any sample rate). Uses the first
    /// 30 seconds.
    pub fn track(
        &self,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<(Beats, Spectrum), RhythmError> {
        let audio = resample(samples, sample_rate, SAMPLE_RATE);
        let spec = Spectrum::of(&audio);
        if spec.frames < (FPS * 6.0) as usize {
            return Err(RhythmError::TooShort);
        }
        let mut input = vec![0.0; CHUNK * N_MELS];
        let used = spec.frames.min(CHUNK);
        input[..used * N_MELS].copy_from_slice(&spec.log_mel[..used * N_MELS]);
        let (beat, downbeat) = self.logits(&input)?;
        let beats = peaks(&beat[..used]);
        let mut downbeats = peaks(&downbeat[..used]);
        // Snap downbeats to the nearest beat, as Beat This! does.
        if !beats.is_empty() {
            for d in downbeats.iter_mut() {
                *d = *beats
                    .iter()
                    .min_by(|a, b| (*a - *d).abs().total_cmp(&(*b - *d).abs()))
                    .unwrap_or(d);
            }
            downbeats.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        }
        Ok((Beats { beats, downbeats }, spec))
    }

    /// The rhythm map of a mono clip.
    pub fn map(&self, samples: &[f32], sample_rate: u32) -> Result<RhythmMap, RhythmError> {
        let (beats, spec) = self.track(samples, sample_rate)?;
        map_from(&beats, &spec)
    }
}

/// "Minimal" postprocessing: frames that are the maximum over +/-3 frames
/// with a positive logit; runs of adjacent peaks become their mean.
pub fn peaks(logits: &[f32]) -> Vec<f32> {
    let n = logits.len();
    let mut idx: Vec<usize> = Vec::new();
    for i in 0..n {
        let lo = i.saturating_sub(3);
        let hi = (i + 4).min(n);
        let max = logits[lo..hi]
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        if logits[i] > 0.0 && logits[i] >= max {
            idx.push(i);
        }
    }
    let mut out = Vec::new();
    let mut group: Vec<usize> = Vec::new();
    for i in idx {
        if group.last().is_some_and(|&last| i - last > 1) {
            out.push(group.iter().sum::<usize>() as f32 / group.len() as f32 / FPS);
            group.clear();
        }
        group.push(i);
    }
    if !group.is_empty() {
        out.push(group.iter().sum::<usize>() as f32 / group.len() as f32 / FPS);
    }
    out
}

/// Mel spectrogram of a 22050 Hz clip.
pub struct Spectrum {
    pub frames: usize,
    /// `frames x N_MELS`, `log1p(1000 * magnitude)`: the model's input.
    pub log_mel: Vec<f32>,
    /// `frames x BAND_COUNT`: log energy per analyser band.
    pub bands: Vec<[f32; BAND_COUNT]>,
}

impl Spectrum {
    /// torchaudio's `MelSpectrogram` with Beat This!'s settings: centred
    /// frames (reflect padding), periodic Hann window, magnitude divided by
    /// sqrt(n_fft) ("frame_length"), Slaney mels, no mel norm. Checked
    /// against PyTorch with `examples/beat_check.rs`.
    pub fn of(audio: &[f32]) -> Spectrum {
        let pad = N_FFT / 2;
        if audio.len() <= pad {
            return Spectrum {
                frames: 0,
                log_mel: Vec::new(),
                bands: Vec::new(),
            };
        }
        let frames = 1 + audio.len() / HOP;
        let window: Vec<f32> = (0..N_FFT)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N_FFT as f32).cos())
            .collect();
        // "frame_length" normalisation: torch.stft(normalized=True) divides
        // by sqrt(n_fft), not by the window's energy.
        let norm = (N_FFT as f32).sqrt();
        let fb = mel_filters();
        let bins = N_FFT / 2 + 1;
        let band_of: Vec<Option<usize>> = (0..bins)
            .map(|k| {
                let f = k as f32 * SAMPLE_RATE as f32 / N_FFT as f32;
                (0..BAND_COUNT).find(|&b| f >= BAND_EDGES[b] && f < BAND_EDGES[b + 1])
            })
            .collect();
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N_FFT);
        let mut buf = fft.make_input_vec();
        let mut spec = fft.make_output_vec();
        let mut mag = vec![0.0f32; bins];
        let mut log_mel = vec![0.0f32; frames * N_MELS];
        let mut bands = vec![[0.0f32; BAND_COUNT]; frames];
        // Reflect padding: index -1 is sample 1, len is len - 2.
        let n = audio.len() as isize;
        let at = |i: isize| -> f32 {
            let j = if i < 0 {
                -i
            } else if i >= n {
                2 * (n - 1) - i
            } else {
                i
            };
            audio[j.clamp(0, n - 1) as usize]
        };
        for f in 0..frames {
            let start = (f * HOP) as isize - pad as isize;
            for (i, b) in buf.iter_mut().enumerate() {
                *b = at(start + i as isize) * window[i];
            }
            if fft.process(&mut buf, &mut spec).is_err() {
                continue;
            }
            for (m, c) in mag.iter_mut().zip(&spec) {
                *m = c.norm() / norm;
            }
            for (j, filt) in fb.iter().enumerate() {
                let e: f32 = filt.iter().map(|&(k, w)| w * mag[k]).sum();
                log_mel[f * N_MELS + j] = (1000.0 * e).ln_1p();
            }
            let mut e = [0.0f32; BAND_COUNT];
            for (k, m) in mag.iter().enumerate() {
                if let Some(b) = band_of[k] {
                    e[b] += m * m;
                }
            }
            for b in 0..BAND_COUNT {
                bands[f][b] = (1000.0 * e[b].sqrt()).ln_1p();
            }
        }
        Spectrum {
            frames,
            log_mel,
            bands,
        }
    }
}

fn hz_to_mel(f: f32) -> f32 {
    let f_sp = 200.0 / 3.0;
    if f >= 1000.0 {
        15.0 + (f / 1000.0).ln() / (6.4f32.ln() / 27.0)
    } else {
        f / f_sp
    }
}

fn mel_to_hz(m: f32) -> f32 {
    let f_sp = 200.0 / 3.0;
    if m >= 15.0 {
        1000.0 * ((6.4f32.ln() / 27.0) * (m - 15.0)).exp()
    } else {
        m * f_sp
    }
}

/// torchaudio's `melscale_fbanks` (Slaney scale, no norm), as sparse
/// (bin, weight) lists per mel.
fn mel_filters() -> Vec<Vec<(usize, f32)>> {
    let bins = N_FFT / 2 + 1;
    let all: Vec<f32> = (0..bins)
        .map(|k| k as f32 * (SAMPLE_RATE as f32 / 2.0) / (bins - 1) as f32)
        .collect();
    let (m0, m1) = (hz_to_mel(F_MIN), hz_to_mel(F_MAX));
    let pts: Vec<f32> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(m0 + (m1 - m0) * i as f32 / (N_MELS + 1) as f32))
        .collect();
    (0..N_MELS)
        .map(|j| {
            let (lo, mid, hi) = (pts[j], pts[j + 1], pts[j + 2]);
            all.iter()
                .enumerate()
                .filter_map(|(k, &f)| {
                    let down = (f - lo) / (mid - lo);
                    let up = (hi - f) / (hi - mid);
                    let w = down.min(up).max(0.0);
                    (w > 0.0).then_some((k, w))
                })
                .collect()
        })
        .collect()
}

/// Windowed-sinc resampling (Blackman, 32 zero crossings), good enough for
/// a beat tracker and dependency-free.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = (input.len() as f64 * ratio).floor() as usize;
    // Lowpass at the lower Nyquist.
    let cutoff = ratio.min(1.0) * 0.95;
    let half = 32.0 / cutoff;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let center = i as f64 / ratio;
        let lo = (center - half).ceil().max(0.0) as usize;
        let hi = ((center + half).floor() as usize).min(input.len() - 1);
        let (mut acc, mut wsum) = (0.0f64, 0.0f64);
        for (j, &x) in input.iter().enumerate().take(hi + 1).skip(lo) {
            let d = j as f64 - center;
            let x_d = d * cutoff;
            let sinc = if x_d.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * x_d).sin() / (std::f64::consts::PI * x_d)
            };
            let p = (d / half + 1.0) / 2.0;
            let w = 0.42 - 0.5 * (std::f64::consts::TAU * p).cos()
                + 0.08 * (2.0 * std::f64::consts::TAU * p).cos();
            let k = sinc * w;
            acc += x as f64 * k;
            wsum += k;
        }
        out.push(if wsum.abs() > 1e-12 {
            (acc / wsum) as f32
        } else {
            0.0
        });
    }
    out
}

/// A straight grid `t = origin + period * k` through the beats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub origin: f32,
    pub period: f32,
    /// Share of beats within [`GRID_TOLERANCE`] of the grid.
    pub fit: f32,
}

/// How far a beat may sit from the grid and still count as on it. Live
/// drummers drift; the microphone lock corrects what this lets through.
const GRID_TOLERANCE: f32 = 0.07;
/// Below this share of beats on the grid there is no usable tempo.
const MIN_FIT: f32 = 0.3;

/// Least-squares beat grid, with each beat's number on it (`None` for beats
/// off the grid, like the in-between hits of a half-time fill). Beats are
/// numbered gap by gap from the last one on the grid, so a missed beat
/// makes a gap of two and frame rounding (20ms) cannot accumulate.
pub fn fit_grid(beats: &[f32]) -> Option<(Grid, Vec<Option<i64>>)> {
    if beats.len() < 8 {
        return None;
    }
    let mut d: Vec<f32> = beats.windows(2).map(|w| w[1] - w[0]).collect();
    d.sort_by(f32::total_cmp);
    let p0 = d[d.len() / 2];
    if !(0.25..=1.5).contains(&p0) {
        return None;
    }
    let mut ks: Vec<Option<i64>> = vec![None; beats.len()];
    ks[0] = Some(0);
    let (mut k, mut anchor) = (0i64, beats[0]);
    for (i, &t) in beats.iter().enumerate().skip(1) {
        let r = (t - anchor) / p0;
        let n = r.round();
        if n >= 1.0 && (r - n).abs() < 0.3 {
            k += n as i64;
            anchor = t;
            ks[i] = Some(k);
        }
    }
    let on: Vec<(i64, f32)> = ks
        .iter()
        .zip(beats)
        .filter_map(|(k, &t)| Some(((*k)?, t)))
        .collect();
    let n = on.len() as f64;
    if n < 6.0 {
        return None;
    }
    let sk = on.iter().map(|(k, _)| *k as f64).sum::<f64>();
    let st = on.iter().map(|(_, t)| *t as f64).sum::<f64>();
    let skk = on.iter().map(|(k, _)| (*k * *k) as f64).sum::<f64>();
    let skt = on.iter().map(|(k, t)| *k as f64 * *t as f64).sum::<f64>();
    let den = n * skk - sk * sk;
    if den.abs() < 1e-9 {
        return None;
    }
    let period = ((n * skt - sk * st) / den) as f32;
    let origin = ((st - period as f64 * sk) / n) as f32;
    let fit = on
        .iter()
        .filter(|(k, t)| (t - (origin + period * *k as f32)).abs() < GRID_TOLERANCE)
        .count() as f32
        / beats.len() as f32;
    Some((
        Grid {
            origin,
            period,
            fit,
        },
        ks,
    ))
}

/// Builds the map from the model's beats and the clip's spectrum.
pub fn map_from(beats: &Beats, spec: &Spectrum) -> Result<RhythmMap, RhythmError> {
    let (mut grid, mut ks) = fit_grid(&beats.beats).ok_or(RhythmError::NoBeat)?;
    // The model sometimes counts eighth notes in busy passages: fold very
    // fast grids to half speed, keeping the half that holds the downbeats.
    let is_downbeat = |i: usize| {
        beats
            .downbeats
            .iter()
            .any(|d| (d - beats.beats[i]).abs() < 1e-3)
    };
    while 60.0 / grid.period > MAX_FOLDED_BPM {
        let count = |parity: i64| {
            ks.iter()
                .enumerate()
                .filter(|(i, k)| k.is_some_and(|k| k.rem_euclid(2) == parity) && is_downbeat(*i))
                .count()
        };
        let parity = i64::from(count(1) > count(0));
        grid.origin += grid.period * parity as f32;
        grid.period *= 2.0;
        // Off-beats of the slower grid drop out.
        ks = ks
            .iter()
            .map(|k| {
                k.filter(|k| (k - parity).rem_euclid(2) == 0)
                    .map(|k| (k - parity) / 2)
            })
            .collect();
    }
    let bpm = 60.0 / grid.period;
    if !(40.0..=240.0).contains(&bpm) || grid.fit < MIN_FIT {
        return Err(RhythmError::NoBeat);
    }
    // Metre and bar phase: which beat numbers the downbeats fall on.
    let down_k: Vec<i64> = (0..beats.beats.len())
        .filter(|&i| is_downbeat(i))
        .filter_map(|i| ks[i])
        .collect();
    let mut per_bar = [0usize; 5];
    for w in down_k.windows(2) {
        if let Ok(gap) = usize::try_from(w[1] - w[0])
            && (3..=4).contains(&gap)
        {
            per_bar[gap] += 1;
        }
    }
    let beats_per_bar: u8 = if per_bar[3] > per_bar[4] { 3 } else { 4 };
    let bpb = beats_per_bar as i64;
    let mut votes = vec![0usize; bpb as usize];
    for k in &down_k {
        votes[k.rem_euclid(bpb) as usize] += 1;
    }
    let first = votes
        .iter()
        .enumerate()
        .max_by_key(|(_, v)| **v)
        .map_or(0, |(i, _)| i);
    let mut first_downbeat = grid.origin + grid.period * first as f32;
    while first_downbeat < 0.0 {
        first_downbeat += grid.period * beats_per_bar as f32;
    }

    // Groove: fold every frame onto the nearest sixteenth of the bar.
    let steps = beats_per_bar as usize * STEPS_PER_BEAT;
    let step_len = grid.period / STEPS_PER_BEAT as f32;
    let mut hit_sum = vec![[0.0f32; BAND_COUNT]; steps];
    let mut lvl_sum = vec![[0.0f32; BAND_COUNT]; steps];
    let mut acc_sum = vec![0.0f32; steps];
    let mut count = vec![0usize; steps];
    let last = spec.frames.min(CHUNK);
    for f in 1..last {
        let t = f as f32 / FPS;
        let u = ((t - first_downbeat) / step_len).round() as i64;
        let s = u.rem_euclid(steps as i64) as usize;
        let mut total = 0.0;
        for b in 0..BAND_COUNT {
            let rise = (spec.bands[f][b] - spec.bands[f - 1][b]).max(0.0);
            hit_sum[s][b] += rise;
            lvl_sum[s][b] += spec.bands[f][b];
            total += rise;
        }
        acc_sum[s] += total;
        count[s] += 1;
    }
    let mean = |v: f32, n: usize| if n == 0 { 0.0 } else { v / n as f32 };
    let mut hits = vec![[0.0f32; BAND_COUNT]; steps];
    let mut levels = vec![[0.0f32; BAND_COUNT]; steps];
    // Normalise hits per band against the loudest band's peak, so a band
    // with no real hits stays low instead of being blown up to 1.
    let band_peak: Vec<f32> = (0..BAND_COUNT)
        .map(|b| {
            (0..steps)
                .map(|s| mean(hit_sum[s][b], count[s]))
                .fold(0.0, f32::max)
        })
        .collect();
    let global_peak = band_peak.iter().copied().fold(0.0, f32::max).max(1e-6);
    for b in 0..BAND_COUNT {
        let scale = band_peak[b].max(0.3 * global_peak);
        // Each band's level: its frames' 10th..95th percentile mapped to 0..1.
        let mut all: Vec<f32> = (1..last).map(|f| spec.bands[f][b]).collect();
        all.sort_by(f32::total_cmp);
        let q = |p: f32| {
            all.get(((all.len() as f32 - 1.0) * p) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let (lo, hi) = (q(0.10), q(0.95));
        for s in 0..steps {
            // Relative to the step's average rise: subtract the floor so
            // steady sound does not read as a hit.
            hits[s][b] = (mean(hit_sum[s][b], count[s]) / scale).clamp(0.0, 1.0);
            levels[s][b] =
                ((mean(lvl_sum[s][b], count[s]) - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
        }
    }
    // Contrast: the quietest step of each band becomes 0.
    for b in 0..BAND_COUNT {
        let floor = (0..steps).map(|s| hits[s][b]).fold(1.0, f32::min);
        let top = (0..steps).map(|s| hits[s][b]).fold(0.0, f32::max);
        for h in hits.iter_mut() {
            h[b] = if top - floor > 1e-6 {
                (h[b] - floor) / (top - floor) * top
            } else {
                0.0
            };
        }
    }
    let acc: Vec<f32> = (0..steps).map(|s| mean(acc_sum[s], count[s])).collect();
    let (amin, amax) = (
        acc.iter().copied().fold(f32::INFINITY, f32::min),
        acc.iter().copied().fold(0.0, f32::max),
    );
    let accents = acc
        .iter()
        .map(|a| {
            if amax - amin > 1e-6 {
                (a - amin) / (amax - amin)
            } else {
                0.0
            }
        })
        .collect();
    Ok(RhythmMap {
        version: RHYTHM_VERSION,
        bpm,
        beats_per_bar,
        first_downbeat,
        clip_start: None,
        hits,
        levels,
        accents,
        confidence: grid.fit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clicks: a low thump on beats 1 and 3, a bright tick on 2 and 4, at
    /// `bpm`, with the first downbeat at `offset` seconds.
    fn drums(bpm: f32, offset: f32, secs: f32, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f32) as usize;
        let mut out = vec![0.0f32; n];
        let beat = 60.0 / bpm;
        let mut k = 0;
        loop {
            let t0 = offset + k as f32 * beat;
            if t0 >= secs {
                break;
            }
            let low = k % 2 == 0;
            let start = (t0 * sr as f32) as usize;
            for i in 0..(0.12 * sr as f32) as usize {
                let Some(o) = out.get_mut(start + i) else {
                    break;
                };
                let t = i as f32 / sr as f32;
                let env = (-t * 30.0).exp();
                let f = if low { 60.0 } else { 3000.0 };
                *o += env * (std::f32::consts::TAU * f * t).sin() * 0.6;
            }
            k += 1;
        }
        out
    }

    #[test]
    fn mel_filters_match_torchaudio_shape() {
        let fb = mel_filters();
        assert_eq!(fb.len(), N_MELS);
        assert!(fb.iter().all(|f| !f.is_empty()), "every mel has bins");
        // Slaney: 1000 Hz is mel 15, and the scale is linear below it.
        assert!((hz_to_mel(1000.0) - 15.0).abs() < 1e-4);
        assert!((mel_to_hz(hz_to_mel(440.0)) - 440.0).abs() < 1e-2);
        assert!((mel_to_hz(hz_to_mel(8000.0)) - 8000.0).abs() < 1e-1);
    }

    #[test]
    fn spectrum_frames_and_resampling() {
        let audio = drums(120.0, 0.25, 10.0, 44100);
        let r = resample(&audio, 44100, SAMPLE_RATE);
        assert_eq!(r.len(), audio.len() / 2);
        let s = Spectrum::of(&r);
        assert_eq!(s.frames, 1 + r.len() / HOP);
        assert_eq!(s.log_mel.len(), s.frames * N_MELS);
        assert!(s.log_mel.iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    #[test]
    fn peaks_pick_maxima_and_merge_neighbours() {
        let mut l = vec![-5.0f32; 200];
        l[50] = 2.0;
        l[51] = 2.0; // a plateau of two: one peak at 50.5
        l[100] = 1.0;
        l[102] = 3.0; // within 3 frames: only the larger
        l[150] = -0.5; // below threshold
        let p = peaks(&l);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!((p[0] - 50.5 / FPS).abs() < 1e-6 && (p[1] - 102.0 / FPS).abs() < 1e-6);
    }

    #[test]
    fn grid_numbering_survives_frame_rounding() {
        // 118.3 BPM quantised to 20ms frames, as the model reports it.
        let beats: Vec<f32> = (0..60)
            .map(|k| ((0.2 + 0.5072 * k as f32) * FPS).round() / FPS)
            .collect();
        let (grid, _) = fit_grid(&beats).unwrap();
        assert!(
            (grid.period - 0.5072).abs() < 0.002 && grid.fit > 0.95,
            "{grid:?}"
        );
    }

    #[test]
    fn grid_and_groove_from_known_beats() {
        // As if the model had found these beats: 100 BPM, downbeats every
        // 4 from 0.3s, one beat missed and one 20ms late.
        let period = 0.6;
        let mut beats: Vec<f32> = (0..45).map(|k| 0.3 + period * k as f32).collect();
        beats.remove(10);
        beats[20] += 0.02;
        let downbeats: Vec<f32> = beats
            .iter()
            .copied()
            .filter(|t| ((t - 0.3) / period).round() as i64 % 4 == 0)
            .collect();
        let (grid, _) = fit_grid(&beats).unwrap();
        assert!((60.0 / grid.period - 100.0).abs() < 0.2, "{grid:?}");
        let audio = resample(&drums(100.0, 0.3, 28.0, 44100), 44100, SAMPLE_RATE);
        let spec = Spectrum::of(&audio);
        let map = map_from(&Beats { beats, downbeats }, &spec).unwrap();
        assert!(map.is_valid());
        assert_eq!(map.beats_per_bar, 4);
        assert!(
            (map.first_downbeat - 0.3).abs() < 0.03,
            "{}",
            map.first_downbeat
        );
        // The groove: thumps (bass) on steps 0 and 8, ticks (treble) on 4, 12.
        let bass = |s: usize| map.hits[s][0].max(map.hits[s][1]);
        // (The tick's sharp attack leaks into the bass bands a little.)
        assert!(
            bass(0) > 0.9 && bass(8) > 0.9 && bass(4) < bass(0) - 0.3,
            "{:?}",
            map.hits
        );
        assert!(map.hits[4][5] > map.hits[0][5] + 0.5);
        assert!(
            map.hits[2].iter().all(|v| *v < 0.05),
            "nothing between beats"
        );
        assert!(map.accents[0] > 0.6 && map.accents[2] < 0.3);
    }
}
