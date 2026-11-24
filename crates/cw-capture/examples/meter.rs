//! Captures system audio for a few seconds and prints the analysis, to check
//! capture works on this machine without the TUI.
//!
//! cargo run -p cw-capture --example meter -- [SECONDS] [BUNDLE_ID ...]
//! (macOS: e.g. `com.spotify.client` or `com.apple.Music`; no IDs = whole system)

fn bar(v: f32) -> String {
    let n = (v.clamp(0.0, 1.0) * 10.0).round() as usize;
    format!("{}{}", "#".repeat(n), ".".repeat(10 - n))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let secs: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let ids: Vec<String> = args.collect();
    let target = if ids.is_empty() {
        cw_capture::Target::System
    } else {
        cw_capture::Target::Apps(ids)
    };
    let (tx, rx) = rtrb::RingBuffer::new(96_000);
    let capture = match cw_capture::Capture::start(&target, tx) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("capturing {target:?} at {} Hz", capture.sample_rate());
    let analyser =
        cw_audio::AnalyserHandle::spawn(rx, capture.sample_rate()).expect("analyser thread");
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < secs {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let f = analyser.frame();
        println!(
            "{:5.2}s  bass {}  mid {}  treble {}  level {}  onsets {}",
            start.elapsed().as_secs_f64(),
            bar(f.bass()),
            bar(f.mid()),
            bar(f.treble()),
            bar(f.level),
            f.onset_count
        );
    }
}
