//! Headless check of the music analyser: decodes a file and prints the
//! analysis 30 times a second as text meters (one line per frame).
//!
//! cargo run -p cw-audio --release --example analyse -- FILE [FPS]

use cw_core::audio::BAND_NAMES;

fn bar(v: f32, width: usize) -> String {
    let n = ((v.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    format!("{}{}", "#".repeat(n), ".".repeat(width - n))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: analyse FILE [FPS]");
        std::process::exit(2);
    };
    let fps: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let rate = 48_000;
    let frames = match cw_audio::analyse_file(path.as_ref(), rate, rate as usize / fps) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let header: Vec<String> = BAND_NAMES.iter().map(|n| format!("{n:<7}")).collect();
    println!("  time  {} level   onset", header.join(" "));
    let mut last_count = 0;
    for (i, f) in frames.iter().enumerate() {
        let bands: Vec<String> = f.bands.iter().map(|b| bar(*b, 7)).collect();
        let hit = if f.onset_count > last_count {
            "  HIT"
        } else {
            ""
        };
        last_count = f.onset_count;
        println!(
            "{:>6.2}  {} {} {}{hit}",
            (i + 1) as f32 / fps as f32,
            bands.join(" "),
            bar(f.level, 7),
            bar(f.onset, 5),
        );
    }
}
