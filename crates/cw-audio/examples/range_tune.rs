//! Compares meter behaviour at several loudness ranges on real music, to
//! tune `RANGE_DB` in `src/analysis.rs`.
//!
//! cargo run -p cw-audio --release --example range_tune -- FILE_OR_FOLDER...
//!
//! For each range it reports, over every analysed moment of every track:
//! - p10 / p50 / p90 of each meter (a healthy meter spans most of 0..1),
//! - `pinned`: share of time at or above 0.95 (stuck at the top),
//! - `floor`: share of time at or below 0.05 (stuck at the bottom),
//! - `move`: mean change per 1/48 s (how lively the meter looks).

use cw_audio::{Analyser, analyse_file_with, collect_audio_files};
use cw_core::AudioFrame;
use std::path::PathBuf;

const RATE: u32 = 48_000;
/// One reading per 1000 frames (about 21ms, close to a 30fps UI tick).
const EVERY: usize = 1_000;
const RANGES: &[f32] = &[12.0, 15.0, 18.0, 21.0, 24.0, 30.0];
/// Skip each track's first seconds while the ceilings settle.
const WARMUP_SECS: f64 = 3.0;

/// A meter's label and how to read it from a frame.
type Metric = (&'static str, fn(&AudioFrame) -> f32);

#[derive(Default)]
struct Series {
    values: Vec<f32>,
    moves: f64,
    move_n: usize,
}

impl Series {
    fn add(&mut self, track: &[f32]) {
        for w in track.windows(2) {
            self.moves += (w[1] - w[0]).abs() as f64;
            self.move_n += 1;
        }
        self.values.extend_from_slice(track);
    }

    fn report(&mut self) -> String {
        self.values.sort_by(f32::total_cmp);
        let n = self.values.len().max(1);
        let q = |p: f64| self.values[((n - 1) as f64 * p) as usize];
        let share = |f: &dyn Fn(f32) -> bool| {
            100.0 * self.values.iter().filter(|v| f(**v)).count() as f64 / n as f64
        };
        format!(
            "p10 {:.2}  p50 {:.2}  p90 {:.2}  pinned {:>4.1}%  floor {:>4.1}%  move {:.4}",
            q(0.10),
            q(0.50),
            q(0.90),
            share(&|v| v >= 0.95),
            share(&|v| v <= 0.05),
            self.moves / self.move_n.max(1) as f64
        )
    }
}

fn main() {
    let inputs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let files = collect_audio_files(&inputs);
    if files.is_empty() {
        eprintln!("usage: range_tune FILE_OR_FOLDER...");
        std::process::exit(2);
    }
    let warmup = (WARMUP_SECS * RATE as f64 / EVERY as f64) as usize;
    let metrics: [Metric; 4] = [
        ("level ", |f| f.level),
        ("bass  ", |f| f.bass()),
        ("mid   ", |f| f.mid()),
        ("treble", |f| f.treble()),
    ];
    let mut series: Vec<[Series; 4]> = RANGES.iter().map(|_| Default::default()).collect();
    // Per-track level spread (p90 - p10) at each range, to spot tracks
    // whose meters barely move.
    let mut spreads: Vec<(String, Vec<f32>)> = Vec::new();

    for (i, path) in files.iter().enumerate() {
        let analysers = RANGES
            .iter()
            .map(|&r| Analyser::new(RATE).with_range_db(r))
            .collect();
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        eprint!(
            "\r{}/{} {:<60}",
            i + 1,
            files.len(),
            name.chars().take(60).collect::<String>()
        );
        let Ok(per_range) = analyse_file_with(path, RATE, EVERY, analysers) else {
            continue;
        };
        let mut track_spreads = Vec::new();
        for (r, frames) in per_range.iter().enumerate() {
            let frames = frames.get(warmup..).unwrap_or_default();
            for (m, (_, get)) in metrics.iter().enumerate() {
                let v: Vec<f32> = frames.iter().map(get).collect();
                series[r][m].add(&v);
                if m == 0 {
                    let mut s = v.clone();
                    s.sort_by(f32::total_cmp);
                    let n = s.len();
                    track_spreads.push(if n > 10 {
                        s[n * 9 / 10] - s[n / 10]
                    } else {
                        0.0
                    });
                }
            }
        }
        spreads.push((name, track_spreads));
    }
    eprintln!();

    println!("{} tracks at {RATE} Hz\n", spreads.len());
    for (r, range) in RANGES.iter().enumerate() {
        println!("range {range:>4.0} dB");
        for (m, (label, _)) in metrics.iter().enumerate() {
            println!("  {label}  {}", series[r][m].report());
        }
    }

    println!("\nlevel spread (p90 - p10) per track, by range:");
    print!("{:<44}", "");
    for r in RANGES {
        print!("{r:>6.0}");
    }
    println!();
    spreads.sort_by(|a, b| {
        a.1.last()
            .copied()
            .unwrap_or(0.0)
            .total_cmp(&b.1.last().copied().unwrap_or(0.0))
    });
    let show: Vec<usize> = if spreads.len() <= 12 {
        (0..spreads.len()).collect()
    } else {
        // The six flattest and six liveliest at the current default.
        (0..6).chain(spreads.len() - 6..spreads.len()).collect()
    };
    for i in show {
        let (name, s) = &spreads[i];
        print!("{:<44}", name.chars().take(42).collect::<String>());
        for v in s {
            print!("{v:>6.2}");
        }
        println!();
    }
}
