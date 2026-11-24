use lofty::prelude::*;
use std::path::{Path, PathBuf};

/// File extensions the decoder can play.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "wav", "wave", "m4a", "mp4", "aac", "aiff", "aif", "caf", "mka",
    "webm",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackInfo {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    /// As tagged, possibly several ("Rock; Pop").
    pub genre: Option<String>,
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
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
                info.album_artist = clean(
                    tag.get_string(lofty::tag::ItemKey::AlbumArtist)
                        .map(Into::into),
                );
                info.track_no = tag.track();
                info.genre = clean(tag.genre());
                info.disc_no = tag.disk();
            }
        }
        Track { path, info }
    }
}

/// Image file stems checked, in order, when a track has no embedded art.
const COVER_STEMS: &[&str] = &["cover", "folder", "front", "album", "albumart"];
const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];

/// Raw cover image bytes for a track: the embedded front cover (or the first
/// embedded picture), else `cover.jpg`, `folder.png` and friends in the
/// track's folder, matched case-insensitively. Reads the disk, so call it off
/// the UI thread.
pub fn cover_art(path: &Path) -> Option<Vec<u8>> {
    embedded_art(path).or_else(|| folder_art(path))
}

fn embedded_art(path: &Path) -> Option<Vec<u8>> {
    use lofty::picture::PictureType;
    let tagged = lofty::read_from_path(path).ok()?;
    let pictures = || tagged.tags().iter().flat_map(|t| t.pictures());
    pictures()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures().next())
        .map(|p| p.data().to_vec())
        .filter(|d| !d.is_empty())
}

fn folder_art(path: &Path) -> Option<Vec<u8>> {
    let dir = path.parent()?;
    let mut candidates: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_str()?.to_ascii_lowercase();
            let ext = p.extension()?.to_str()?.to_ascii_lowercase();
            if !COVER_EXTENSIONS.contains(&ext.as_str()) {
                return None;
            }
            let rank = COVER_STEMS.iter().position(|s| *s == stem)?;
            Some((rank, p))
        })
        .collect();
    // Best stem first; the path breaks ties so the choice is stable.
    candidates.sort();
    candidates
        .into_iter()
        .find_map(|(_, p)| std::fs::read(p).ok().filter(|d| !d.is_empty()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_art_prefers_cover_and_ignores_case() {
        let dir = std::env::temp_dir().join(format!("cw-cover-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let track = dir.join("01 song.wav");
        std::fs::write(&track, b"not really audio").unwrap();
        assert_eq!(cover_art(&track), None);

        std::fs::write(dir.join("Folder.PNG"), b"folder").unwrap();
        std::fs::write(dir.join("notes.jpg"), b"unrelated").unwrap();
        assert_eq!(cover_art(&track).as_deref(), Some(&b"folder"[..]));

        std::fs::write(dir.join("cover.jpg"), b"cover").unwrap();
        assert_eq!(cover_art(&track).as_deref(), Some(&b"cover"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
