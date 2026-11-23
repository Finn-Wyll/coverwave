use lofty::prelude::*;
use std::path::{Path, PathBuf};

/// File extensions the decoder can play.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "wav", "wave", "m4a", "mp4", "aac", "aiff", "aif", "caf", "mka",
    "webm",
];

#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub path: PathBuf,
    pub info: TrackInfo,
}

impl Track {
    /// Reads tags; missing or unreadable tags fall back to the file name.
    pub fn load(path: PathBuf) -> Track {
        let fallback = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut info = TrackInfo {
            title: fallback,
            ..Default::default()
        };
        if let Ok(tagged) = lofty::read_from_path(&path) {
            info.duration_secs =
                Some(tagged.properties().duration().as_secs_f64()).filter(|d| *d > 0.0);
            if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
                let clean = |s: Option<std::borrow::Cow<'_, str>>| {
                    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
                };
                if let Some(t) = clean(tag.title()) {
                    info.title = t;
                }
                info.artist = clean(tag.artist());
                info.album = clean(tag.album());
            }
        }
        Track { path, info }
    }
}

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

/// Expands files and folders (recursively) into a sorted list of audio
/// files. Folders are sorted by path, so albums play in track-file order.
pub fn collect_audio_files(inputs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if p.is_dir() {
                walk(&p, out);
            } else if is_audio_file(&p) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for input in inputs {
        if input.is_dir() {
            walk(input, &mut out);
        } else if is_audio_file(input) {
            out.push(input.clone());
        }
    }
    out
}
