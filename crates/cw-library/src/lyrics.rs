//! Lyrics for local files: a sidecar `.lrc` next to the audio file first,
//! then lyrics embedded in the tags (often LRC text too). Parsing lives in
//! `cw_core::lyrics`; online lookup in `cw_sources::lrclib`.
//!
//! Not supported: ID3 `SYLT` frames (lofty exposes them only as raw binary).

pub use cw_core::lyrics::{LyricLine, Lyrics, parse_lrc};
use std::path::Path;

/// Lyrics for an audio file, if any.
pub fn load(audio: &Path) -> Option<Lyrics> {
    sidecar(audio).or_else(|| embedded(audio))
}

fn sidecar(audio: &Path) -> Option<Lyrics> {
    let dir = audio.parent()?;
    let stem = audio.file_stem()?.to_string_lossy().to_lowercase();
    let found = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lrc"))
                && p.file_stem()
                    .is_some_and(|s| s.to_string_lossy().to_lowercase() == stem)
        })?;
    let bytes = std::fs::read(found).ok()?;
    parse_lrc(&String::from_utf8_lossy(&bytes), "lrc file")
}

fn embedded(audio: &Path) -> Option<Lyrics> {
    use lofty::prelude::*;
    use lofty::tag::ItemKey;
    let tagged = lofty::read_from_path(audio).ok()?;
    tagged.tags().iter().find_map(|tag| {
        [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
            .into_iter()
            .find_map(|k| tag.get_string(k).map(str::to_string))
            .and_then(|text| parse_lrc(&text, "tags"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_found_case_insensitively() {
        let dir = std::env::temp_dir().join(format!("cw-lrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("01 Song.flac");
        std::fs::write(&audio, b"not audio").unwrap();
        assert_eq!(load(&audio), None);
        std::fs::write(dir.join("01 song.LRC"), "[00:02.00]hi").unwrap();
        let l = load(&audio).unwrap();
        assert_eq!((l.source, l.lines[0].time), ("lrc file", Some(2.0)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
