//! Real-time music analysis: mono samples in, [`AudioFrame`] out.
//!
//! - 2048-point FFT with a Hann window, every 512 samples (hop).
//! - Six bands ([`cw_core::audio::BAND_EDGES`]), each normalised against
//!   its own slow loudness ceiling (instant rise, slow fall), floored
//!   relative to the overall ceiling so near-empty bands don't amplify noise.
//! - Spectral-flux onsets on log-compressed magnitudes, with an adaptive
//!   threshold (k * recent median + delta) and one hop of peak picking. The
//!   median ignores earlier hits, so a steady beat whose period matches the
//!   history window (120 BPM and 0.5s) is never masked by its previous hit.
//! - Every output is smoothed with a fast attack and slower release.
//!
//! The analyser does not care where samples come from: local playback today,
//! system capture later. It allocates only in [`Analyser::new`].

use cw_core::AudioFrame;
use cw_core::audio::{BAND_COUNT, BAND_EDGES};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const FFT_SIZE: usize = 2048;
pub const HOP: usize = 512;

/// Loudness range mapped onto 0..1 below each ceiling.
const RANGE_DB: f32 = 30.0;
/// How fast ceilings fall after a loud passage.
const CEILING_RELEASE_DB_PER_SEC: f32 = 4.0;
/// A band's ceiling never sits more than this below the overall ceiling.
const BAND_FLOOR_BELOW_OVERALL_DB: f32 = 24.0;
/// Ceilings never fall below this (dB relative to a full-scale sine).
const ABS_CEILING_FLOOR_DB: f32 = -60.0;
/// Below this overall level everything reads as silence.
const SILENCE_DB: f32 = -70.0;

const ATTACK_SECS: f32 = 0.012;
const RELEASE_SECS: f32 = 0.12;
const ONSET_DECAY_SECS: f32 = 0.05;

/// Seconds of flux history for the adaptive threshold.
const FLUX_HISTORY_SECS: f32 = 0.5;
const FLUX_K: f32 = 2.0;
/// Added to the threshold: the minimum flux (log-magnitude rise per bin,
/// averaged per band) for an onset.
const FLUX_MIN: f32 = 0.05;
const MIN_ONSET_GAP_SECS: f32 = 0.08;
/// Log compression for flux: ln(1 + GAMMA * magnitude).
const FLUX_GAMMA: f32 = 100.0;

pub struct Analyser {
    dt: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// Circular history of the last FFT_SIZE samples.
    history: Vec<f32>,
    write: usize,
    since_hop: usize,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Bin range per band.
    band_bins: [(usize, usize); BAND_COUNT],
    prev_log: Vec<f32>,
    band_ceiling: [f32; BAND_COUNT],
    level_ceiling: f32,
    flux: Vec<f32>,
    /// Scratch for the median, so the hop never allocates.
    flux_sorted: Vec<f32>,
    flux_pos: usize,
    /// Flux of the previous two hops, for peak picking.
    flux_prev: [f32; 2],
    threshold_prev: f32,
    hops_since_onset: u32,
    min_gap_hops: u32,
    frame: AudioFrame,
    /// When set, every hop's onset flux is recorded (tempo estimation).
    flux_trace: Option<Vec<f32>>,
}

impl Analyser {
    pub fn new(sample_rate: u32) -> Analyser {
        let rate = sample_rate.max(1) as f32;
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).cos())
            .collect();
        let bins = FFT_SIZE / 2 + 1;
        let hz_per_bin = rate / FFT_SIZE as f32;
        let bin = |hz: f32| ((hz / hz_per_bin).round() as usize).clamp(1, bins - 1);
        let mut band_bins = [(0, 0); BAND_COUNT];
        for (b, range) in band_bins.iter_mut().enumerate() {
            let lo = bin(BAND_EDGES[b]);
            // At least one bin per band, even at low sample rates.
            let hi = bin(BAND_EDGES[b + 1]).max(lo + 1).min(bins);
            *range = (lo, hi);
        }
        let dt = HOP as f32 / rate;
        let flux_len = ((FLUX_HISTORY_SECS / dt).round() as usize).max(4);
        Analyser {
            dt,
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window,
            history: vec![0.0; FFT_SIZE],
            write: 0,
            since_hop: 0,
            band_bins,
            prev_log: vec![0.0; bins],
            band_ceiling: [ABS_CEILING_FLOOR_DB; BAND_COUNT],
            level_ceiling: ABS_CEILING_FLOOR_DB,
            flux: vec![0.0; flux_len],
            flux_sorted: vec![0.0; flux_len],
            flux_pos: 0,
            flux_prev: [0.0; 2],
            threshold_prev: f32::INFINITY,
            hops_since_onset: u32::MAX,
            min_gap_hops: (MIN_ONSET_GAP_SECS / dt).ceil() as u32,
            frame: AudioFrame::default(),
            flux_trace: None,
        }
    }

    /// The latest analysis.
    pub fn frame(&self) -> AudioFrame {
        self.frame
    }

    /// Feeds mono samples; runs one analysis step per [`HOP`] samples.
    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            // Clamp so a corrupt sample cannot overflow the power sums.
            self.history[self.write] = if s.is_finite() {
                s.clamp(-4.0, 4.0)
            } else {
                0.0
            };
            self.write = (self.write + 1) % FFT_SIZE;
            self.since_hop += 1;
            if self.since_hop == HOP {
                self.since_hop = 0;
                self.hop();
            }
        }
    }

    fn hop(&mut self) {
        // Oldest sample first.
        for i in 0..FFT_SIZE {
            self.input[i] = self.history[(self.write + i) % FFT_SIZE] * self.window[i];
        }
        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
            .is_err()
        {
            return; // only possible with wrongly sized buffers
        }
        // A full-scale sine peaks at |X| = N/4 with a Hann window.
        let norm = 4.0 / FFT_SIZE as f32;

        let mut band_power = [0.0f32; BAND_COUNT];
        for (b, &(lo, hi)) in self.band_bins.iter().enumerate() {
            band_power[b] = self.spectrum[lo..hi]
                .iter()
                .map(|c| c.norm_sqr() * norm * norm)
                .sum();
        }
        let total: f32 = band_power.iter().sum();
        let db = |p: f32| 10.0 * (p + 1e-12).log10();
        let level_db = db(total);
        let silent = level_db < SILENCE_DB;

        let fall = CEILING_RELEASE_DB_PER_SEC * self.dt;
        self.level_ceiling = level_db
            .max(self.level_ceiling - fall)
            .max(ABS_CEILING_FLOOR_DB);
        let to_unit =
            |v: f32, ceiling: f32| ((v - (ceiling - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);

        let mut band_target = [0.0f32; BAND_COUNT];
        for b in 0..BAND_COUNT {
            let d = db(band_power[b]);
            self.band_ceiling[b] = d
                .max(self.band_ceiling[b] - fall)
                .max(self.level_ceiling - BAND_FLOOR_BELOW_OVERALL_DB)
                .max(ABS_CEILING_FLOOR_DB);
            band_target[b] = if silent {
                0.0
            } else {
                to_unit(d, self.band_ceiling[b])
            };
        }
        let level_target = if silent {
            0.0
        } else {
            to_unit(level_db, self.level_ceiling)
        };

        // Spectral flux: the rise in log magnitude, averaged per band and
        // then across bands, so a kick in a few bass bins counts as much as
        // a cymbal across thousands of treble bins.
        let mut flux = 0.0f32;
        for &(lo, hi) in &self.band_bins {
            let mut rise = 0.0f32;
            for k in lo..hi {
                let lm = (1.0 + FLUX_GAMMA * self.spectrum[k].norm() * norm).ln();
                rise += (lm - self.prev_log[k]).max(0.0);
                self.prev_log[k] = lm;
            }
            flux += rise / (hi - lo) as f32;
        }
        flux /= BAND_COUNT as f32;
        if silent {
            flux = 0.0;
        }

        // Peak picking with one hop of delay: the previous hop is an onset
        // if it beat its threshold and both neighbours.
        let [p1, p2] = self.flux_prev;
        self.hops_since_onset = self.hops_since_onset.saturating_add(1);
        let mut onset = false;
        if p1 > self.threshold_prev
            && p1 >= FLUX_MIN
            && p1 >= p2
            && p1 > flux
            && self.hops_since_onset >= self.min_gap_hops
        {
            onset = true;
            self.hops_since_onset = 0;
        }
        // Threshold for the current hop, from history before it.
        self.flux_sorted.copy_from_slice(&self.flux);
        let mid = self.flux_sorted.len() / 2;
        let (_, median, _) = self.flux_sorted.select_nth_unstable_by(mid, f32::total_cmp);
        self.threshold_prev = FLUX_K * *median + FLUX_MIN;
        self.flux[self.flux_pos] = flux;
        self.flux_pos = (self.flux_pos + 1) % self.flux.len();
        self.flux_prev = [flux, p1];
        if let Some(t) = self.flux_trace.as_mut() {
            t.push(flux);
        }

        // Smooth.
        let attack = 1.0 - (-self.dt / ATTACK_SECS).exp();
        let release = 1.0 - (-self.dt / RELEASE_SECS).exp();
        let smooth = |cur: f32, target: f32| {
            let k = if target > cur { attack } else { release };
            cur + (target - cur) * k
        };
        let f = &mut self.frame;
        for (v, t) in f.bands.iter_mut().zip(band_target) {
            *v = smooth(*v, t);
        }
        f.level = smooth(f.level, level_target);
        f.onset *= (-self.dt / ONSET_DECAY_SECS).exp();
        if onset {
            f.onset = 1.0;
            f.onset_count += 1;
        }
    }
}

/// A tempo estimate from a stretch of audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoEstimate {
    pub bpm: f32,
    /// 0..1: how clearly periodic the onsets were.
    pub confidence: f32,
}

/// Estimates the tempo of mono `samples` (a whole clip, e.g. a 30-second
/// preview): spectral-flux onsets, autocorrelated over 60..200 BPM with a
/// mild preference for tempos near 120 (a log-normal prior, as usual in beat
/// tracking). `None` for clips too short or without a clear pulse.
pub fn estimate_tempo(samples: &[f32], sample_rate: u32) -> Option<TempoEstimate> {
    let mut a = Analyser::new(sample_rate);
    a.flux_trace = Some(Vec::new());
    a.push(samples);
    let flux = a.flux_trace.take().unwrap_or_default();
    let hop_secs = HOP as f32 / sample_rate.max(1) as f32;
    if flux.len() as f32 * hop_secs < 6.0 {
        return None;
    }
    let mean = flux.iter().sum::<f32>() / flux.len() as f32;
    let raw: Vec<f32> = flux.iter().map(|f| (f - mean).max(0.0)).collect();
    // Smooth over a few hops so beat periods that fall between whole hops
    // (150 BPM is 37.5 hops at 48 kHz) are not penalised against their
    // whole-hop sub-harmonics.
    const K: [f32; 5] = [0.1, 0.2, 0.4, 0.2, 0.1];
    let x: Vec<f32> = (0..raw.len())
        .map(|i| {
            K.iter()
                .enumerate()
                .map(|(k, w)| w * raw.get((i + k).wrapping_sub(2)).copied().unwrap_or(0.0))
                .sum()
        })
        .collect();
    let energy: f32 = x.iter().map(|v| v * v).sum();
    if energy <= 1e-9 {
        return None;
    }
    let lag_for = |bpm: f32| 60.0 / bpm / hop_secs;
    let (lo, hi) = (
        lag_for(200.0).floor() as usize,
        lag_for(60.0).ceil() as usize,
    );
    let ac = |lag: usize| -> f32 {
        x.iter()
            .zip(&x[lag.min(x.len())..])
            .map(|(a, b)| a * b)
            .sum()
    };
    let mut best = (0.0f32, 0usize);
    for lag in lo.max(1)..=hi.min(x.len() / 2) {
        // Reward lags whose double also correlates (a real beat repeats).
        let score = ac(lag) + 0.5 * ac(2 * lag);
        let bpm = 60.0 / (lag as f32 * hop_secs);
        let prior = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
        let s = score * prior;
        if s > best.0 {
            best = (s, lag);
        }
    }
    let lag = best.1;
    if lag == 0 {
        return None;
    }
    // Parabolic refinement of the peak for sub-hop precision.
    let (y0, y1, y2) = (ac(lag - 1), ac(lag), ac(lag + 1));
    let denom = y0 - 2.0 * y1 + y2;
    let shift = if denom.abs() > 1e-9 {
        (0.5 * (y0 - y2) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let bpm = 60.0 / ((lag as f32 + shift) * hop_secs);
    Some(TempoEstimate {
        bpm: (bpm * 10.0).round() / 10.0,
        confidence: (y1 / energy).clamp(0.0, 1.0),
    })
}

/// How often the analyser thread wakes to drain its input.
const POLL: std::time::Duration = std::time::Duration::from_millis(4);

/// An [`Analyser`] on its own thread, fed mono samples through a lock-free
/// ring buffer (from the playback callback or a system-capture callback).
/// The thread exits when the producer side is dropped.
#[derive(Clone)]
pub struct AnalyserHandle {
    frame: Arc<std::sync::Mutex<AudioFrame>>,
}

impl AnalyserHandle {
    pub fn spawn(
        mut input: rtrb::Consumer<f32>,
        sample_rate: u32,
    ) -> std::io::Result<AnalyserHandle> {
        let frame = Arc::new(std::sync::Mutex::new(AudioFrame::default()));
        let out = frame.clone();
        std::thread::Builder::new()
            .name("cw-analyser".into())
            .spawn(move || {
                let mut analyser = Analyser::new(sample_rate);
                loop {
                    let n = input.slots();
                    if n > 0 {
                        if let Ok(chunk) = input.read_chunk(n) {
                            let (a, b) = chunk.as_slices();
                            analyser.push(a);
                            analyser.push(b);
                            chunk.commit_all();
                        }
                        *out.lock().unwrap_or_else(|e| e.into_inner()) = analyser.frame();
                    } else if input.is_abandoned() {
                        return;
                    }
                    std::thread::sleep(POLL);
                }
            })?;
        Ok(AnalyserHandle { frame })
    }

    /// The latest analysis.
    pub fn frame(&self) -> AudioFrame {
        *self.frame.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: u32 = 48_000;

    fn sine(hz: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(secs * RATE as f32) as usize)
            .map(|i| amp * (TAU * hz * i as f32 / RATE as f32).sin())
            .collect()
    }

    /// Deterministic white-ish noise.
    fn noise(amp: f32, secs: f32, seed: u32) -> Vec<f32> {
        let mut x = seed.max(1);
        (0..(secs * RATE as f32) as usize)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                amp * (x as f32 / u32::MAX as f32 * 2.0 - 1.0)
            })
            .collect()
    }

    fn run(samples: &[f32]) -> Analyser {
        let mut a = Analyser::new(RATE);
        a.push(samples);
        a
    }

    #[test]
    fn silence_is_zero() {
        let f = run(&vec![0.0; RATE as usize]).frame();
        assert_eq!(f, AudioFrame::default());
    }

    #[test]
    fn sine_lights_its_own_band() {
        for (hz, band) in [
            (40.0, 0),
            (120.0, 1),
            (350.0, 2),
            (1000.0, 3),
            (3000.0, 4),
            (8000.0, 5),
        ] {
            let f = run(&sine(hz, 0.5, 1.0)).frame();
            assert!(f.bands[band] > 0.8, "{hz}Hz: {:?}", f.bands);
            for (b, v) in f.bands.iter().enumerate() {
                if b.abs_diff(band) > 1 {
                    assert!(*v < 0.2, "{hz}Hz leaks into band {b}: {:?}", f.bands);
                }
            }
            assert!(f.level > 0.8);
        }
    }

    #[test]
    fn quiet_tracks_move_like_loud_ones() {
        let loud = run(&noise(0.5, 2.0, 7)).frame();
        let quiet = run(&noise(0.05, 2.0, 7)).frame(); // -20 dB
        for (l, q) in loud.bands.iter().zip(&quiet.bands) {
            assert!((l - q).abs() < 0.1, "{:?} vs {:?}", loud.bands, quiet.bands);
        }
    }

    #[test]
    fn levels_fall_after_the_music_stops() {
        let mut a = run(&noise(0.3, 1.0, 3));
        assert!(a.frame().level > 0.8);
        a.push(&vec![0.0; RATE as usize]);
        let f = a.frame();
        assert!(f.level < 0.01 && f.bands.iter().all(|b| *b < 0.01), "{f:?}");
    }

    #[test]
    fn clicks_are_onsets_and_steady_tone_is_not() {
        // Steady tone: no onsets after the first moment.
        let mut a = run(&sine(440.0, 0.3, 0.5));
        let start = a.frame().onset_count;
        a.push(&sine(440.0, 0.3, 2.0));
        assert_eq!(a.frame().onset_count, start, "steady tone triggered onsets");

        // Eight noise bursts, 250ms apart, over a quiet bed.
        let mut a = Analyser::new(RATE);
        let mut signal = noise(0.01, 2.0, 11);
        let burst = noise(0.6, 0.03, 5);
        for k in 0..8 {
            let at = (0.1 + 0.25 * k as f32) * RATE as f32;
            for (i, s) in burst.iter().enumerate() {
                if let Some(x) = signal.get_mut(at as usize + i) {
                    *x += s;
                }
            }
        }
        signal.extend(noise(0.01, 0.2, 13));
        a.push(&signal);
        let count = a.frame().onset_count;
        assert!((7..=9).contains(&count), "{count} onsets for 8 bursts");
    }

    #[test]
    fn steady_120_bpm_kick_never_drops_a_beat() {
        // A decaying 55Hz kick every 0.5s: the period equals the flux history.
        let n = 6 * RATE as usize;
        let signal: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                0.8 * (TAU * 55.0 * t).sin() * (-12.0 * (t % 0.5)).exp()
            })
            .collect();
        assert_eq!(run(&signal).frame().onset_count, 12);
    }

    #[test]
    fn steady_noise_rarely_triggers() {
        let mut a = run(&noise(0.3, 1.0, 21));
        let warm = a.frame().onset_count;
        a.push(&noise(0.3, 4.0, 22));
        let extra = a.frame().onset_count - warm;
        assert!(extra <= 2, "{extra} onsets in 4s of steady noise");
    }

    #[test]
    fn onset_pulse_decays() {
        let mut a = Analyser::new(RATE);
        a.push(&vec![0.0; 4096]);
        a.push(&noise(0.5, 0.05, 9));
        assert!(a.frame().onset_count >= 1);
        // A steady tone after the burst: no new onsets, so the pulse fades.
        a.push(&sine(440.0, 0.3, 0.3));
        assert!(a.frame().onset < 0.05, "{}", a.frame().onset);
    }

    #[test]
    fn handle_analyses_on_its_own_thread_and_exits() {
        let (mut tx, rx) = rtrb::RingBuffer::new(RATE as usize);
        let h = AnalyserHandle::spawn(rx, RATE).unwrap();
        for s in sine(120.0, 0.5, 0.5) {
            while tx.push(s).is_err() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while h.frame().bands[1] < 0.5 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.frame().bands[1] > 0.5, "{:?}", h.frame());
        drop(tx);
    }

    fn clicks(bpm: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let period = 60.0 / bpm * RATE as f32;
        let burst = noise(0.6, 0.02, 3);
        let mut s = noise(0.01, secs, 4);
        let mut t = 0.0;
        while (t as usize) < n {
            for (i, b) in burst.iter().enumerate() {
                if let Some(x) = s.get_mut(t as usize + i) {
                    *x += b;
                }
            }
            t += period;
        }
        s
    }

    #[test]
    fn tempo_of_click_tracks() {
        for bpm in [72.0, 100.0, 128.0, 150.0, 174.0] {
            let est = estimate_tempo(&clicks(bpm, 30.0), RATE).unwrap();
            // Up to 160 BPM the octave must be right. Faster tempos may read
            // as half-time (174 drum and bass is often felt and listed as 87),
            // which also suits the restrained motion.
            let ok =
                (est.bpm - bpm).abs() <= 2.0 || (bpm > 160.0 && (est.bpm - bpm / 2.0).abs() <= 1.5);
            assert!(ok, "{bpm} -> {est:?}");
            assert!(est.confidence > 0.1, "{est:?}");
        }
        assert_eq!(estimate_tempo(&vec![0.0; RATE as usize * 10], RATE), None);
        assert_eq!(estimate_tempo(&clicks(120.0, 3.0), RATE), None, "too short");
    }

    #[test]
    fn odd_rates_and_garbage_do_not_panic() {
        for rate in [8_000, 22_050, 44_100, 96_000, 192_000] {
            let mut a = Analyser::new(rate);
            a.push(&[f32::NAN, f32::INFINITY, 1e30, -1e30]);
            a.push(&vec![0.1; 10_000]);
            let f = a.frame();
            assert!(
                f.bands
                    .iter()
                    .chain([&f.level, &f.onset])
                    .all(|v| v.is_finite())
            );
        }
    }
}
