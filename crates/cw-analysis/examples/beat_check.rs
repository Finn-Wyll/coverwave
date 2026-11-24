//! Checks the Rust port of Beat This! against PyTorch's own output.
//!
//! ```sh
//! python ios/tools/export_beat_model.py MODEL.onnx --reference REF_DIR
//! cargo run -p cw-analysis --release --example beat_check -- MODEL.onnx REF_DIR
//! ```
//!
//! Compares the log-mel features, the logits (tract vs PyTorch) and the
//! resulting beats and rhythm map on the reference clip.

use cw_analysis::rhythm::{self, BeatModel, CHUNK, N_MELS, Spectrum};
use std::path::Path;

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() >= 3 && args[1] == "--wav" {
        // beat_check MODEL --wav CLIP.wav...: maps of real clips (mono f32
        // WAV, e.g. `afconvert -f WAVE -d LEF32@22050 -c 1 in.m4a out.wav`).
        let model =
            BeatModel::from_onnx_bytes(&std::fs::read(&args[0]).expect("model")).expect("load");
        for path in &args[2..] {
            let bytes = std::fs::read(path).expect("wav");
            let data = bytes
                .windows(4)
                .position(|w| w == b"data")
                .expect("data chunk")
                + 8;
            let sr = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
            let samples: Vec<f32> = bytes[data..]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            if std::env::var("BEAT_DEBUG").is_ok() {
                let (b, _) = model.track(&samples, sr).expect("track");
                let d: Vec<String> = b
                    .beats
                    .windows(2)
                    .map(|w| format!("{:.2}", w[1] - w[0]))
                    .collect();
                println!(
                    "{} beats, {} downbeats; intervals {}",
                    b.beats.len(),
                    b.downbeats.len(),
                    d.join(" ")
                );
                println!("grid {:?}", rhythm::fit_grid(&b.beats).map(|g| g.0));
            }
            let t = std::time::Instant::now();
            match model.map(&samples, sr) {
                Ok(m) => println!(
                    "{:5.1} BPM {}/4 downbeat {:.2}s fit {:.2} ({:?})  {}",
                    m.bpm,
                    m.beats_per_bar,
                    m.first_downbeat,
                    m.confidence,
                    t.elapsed(),
                    Path::new(path).file_name().unwrap().to_string_lossy()
                ),
                Err(e) => println!("{e}  {path}"),
            }
        }
        return;
    }
    let [model_path, dir] = args.as_slice() else {
        eprintln!("usage: beat_check MODEL.onnx REF_DIR");
        std::process::exit(2);
    };
    let dir = Path::new(dir);
    let clip = read_f32(&dir.join("clip_22050.f32"));
    let ref_mel = read_f32(&dir.join("logmel.f32"));
    let ref_beat = read_f32(&dir.join("beat.f32"));
    let ref_down = read_f32(&dir.join("downbeat.f32"));

    let spec = Spectrum::of(&clip);
    println!(
        "frames: rust {} torch {}",
        spec.frames,
        ref_mel.len() / N_MELS
    );
    let n = spec.log_mel.len().min(ref_mel.len());
    let mel_diff = max_diff(&spec.log_mel[..n], &ref_mel[..n]);
    let mel_mean = ref_mel.iter().sum::<f32>() / ref_mel.len() as f32;
    println!("log-mel max |rust - torch| = {mel_diff:.4} (mean value {mel_mean:.3})");
    let (mut worst, mut at) = (0.0f32, 0);
    let mut sum = 0.0f32;
    for (i, (a, b)) in spec.log_mel.iter().zip(&ref_mel).enumerate() {
        let d = (a - b).abs();
        sum += d;
        if d > worst {
            worst = d;
            at = i;
        }
    }
    println!(
        "  mean diff {:.5}; worst at frame {} mel {} (rust {:.3} torch {:.3})",
        sum / n as f32,
        at / N_MELS,
        at % N_MELS,
        spec.log_mel[at],
        ref_mel[at]
    );
    let per_mel: Vec<f32> = (0..N_MELS)
        .map(|m| {
            (0..n / N_MELS)
                .map(|f| (spec.log_mel[f * N_MELS + m] - ref_mel[f * N_MELS + m]).abs())
                .sum::<f32>()
                / (n / N_MELS) as f32
        })
        .collect();
    println!(
        "  mean diff by mel (every 8th): {:?}",
        per_mel
            .iter()
            .step_by(8)
            .map(|v| (v * 1000.0).round() / 1000.0)
            .collect::<Vec<_>>()
    );

    let t = std::time::Instant::now();
    let model =
        BeatModel::from_onnx_bytes(&std::fs::read(model_path).expect("model")).expect("load");
    println!("model load {:?}", t.elapsed());
    let mut input = vec![0.0; CHUNK * N_MELS];
    let used = spec.frames.min(CHUNK);
    input[..used * N_MELS].copy_from_slice(&spec.log_mel[..used * N_MELS]);
    let t = std::time::Instant::now();
    let (beat, down) = model.logits(&input).expect("run");
    println!("inference {:?}", t.elapsed());
    println!(
        "logits max |rust - torch|: beat {:.3}, downbeat {:.3}",
        max_diff(&beat, &ref_beat),
        max_diff(&down, &ref_down)
    );
    let ours = rhythm::peaks(&beat[..used]);
    let theirs = rhythm::peaks(&ref_beat[..used]);
    println!("beats: rust {} torch {}", ours.len(), theirs.len());
    let worst = ours
        .iter()
        .map(|a| {
            theirs
                .iter()
                .map(|b| (a - b).abs())
                .fold(f32::MAX, f32::min)
        })
        .fold(0.0, f32::max);
    println!("worst beat offset vs torch: {:.3}s", worst);

    let map = model.map(&clip, rhythm::SAMPLE_RATE).expect("map");
    println!(
        "map: {:.2} BPM, {}/4, first downbeat {:.3}s, confidence {:.2}",
        map.bpm, map.beats_per_bar, map.first_downbeat, map.confidence
    );
    let fmt = |v: &[f32]| {
        v.iter()
            .map(|x| format!("{x:.1}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    println!("accents: {}", fmt(&map.accents));
    let bass: Vec<f32> = map.hits.iter().map(|h| h[0].max(h[1])).collect();
    let treble: Vec<f32> = map.hits.iter().map(|h| h[5]).collect();
    println!("bass:    {}", fmt(&bass));
    println!("treble:  {}", fmt(&treble));
}
