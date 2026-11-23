//! Headless check: reads tags and fully decodes each file at 48 kHz.
//!
//! cargo run -p cw-audio --example probe -- FILE_OR_FOLDER...

use cw_audio::{Track, collect_audio_files, decode_to_end};
use std::path::PathBuf;

fn main() {
    let inputs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    for path in collect_audio_files(&inputs) {
        let t = Track::load(path.clone());
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match decode_to_end(&path, 48_000) {
            Ok(frames) => println!(
                "{name}: \"{}\" by {:?}, tag duration {:.2?}s, decoded {:.3}s",
                t.info.title,
                t.info.artist,
                t.info.duration_secs,
                frames as f64 / 48_000.0
            ),
            Err(e) => println!("{name}: ERROR {e}"),
        }
    }
}
