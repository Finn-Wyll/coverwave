//! Estimates the tempo of audio files (e.g. 30-second previews).
//!
//! cargo run -p cw-audio --release --example tempo -- FILE...

fn main() {
    for path in std::env::args().skip(1) {
        let rate = 44_100;
        match cw_audio::decode_mono(path.as_ref(), rate) {
            Ok(samples) => match cw_audio::estimate_tempo(&samples, rate) {
                Some(t) => println!("{path}: {:.1} BPM (confidence {:.2})", t.bpm, t.confidence),
                None => println!("{path}: no clear pulse"),
            },
            Err(e) => println!("{path}: {e}"),
        }
    }
}
