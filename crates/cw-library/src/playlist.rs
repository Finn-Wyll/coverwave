//! `.m3u8` playlists (UTF-8 M3U), so other players can read them.

use cw_audio::Track;
use std::io;
use std::path::{Path, PathBuf};

/// Writes `tracks` as `<dir>/<name>.m3u8` with `#EXTINF` lines and absolute
/// paths. The name is made filesystem-safe. Returns the file's path.
pub fn save(dir: &Path, name: &str, tracks: &[Track]) -> io::Result<PathBuf> {
    let safe: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "playlist".to_string()
    } else {
        safe
    };
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{safe}.m3u8"));
    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        let secs = t.info.duration_secs.map_or(-1, |d| d.round() as i64);
        let label = match &t.info.artist {
            Some(a) => format!("{a} - {}", t.info.title),
            None => t.info.title.clone(),
        };
        out.push_str(&format!("#EXTINF:{secs},{label}\n{}\n", t.path.display()));
    }
    let tmp = path.with_extension("m3u8.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(tmp, &path)?;
    Ok(path)
}

/// Paths listed in an M3U/M3U8 file, relative entries resolved against the
/// playlist's folder. Comments, blank lines and URLs are skipped.
pub fn load(path: &Path) -> io::Result<Vec<PathBuf>> {
    let text = std::fs::read_to_string(path)?;
    let base = path.parent().unwrap_or(Path::new("."));
    Ok(text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.contains("://"))
        .map(|l| {
            let p = PathBuf::from(l);
            if p.is_absolute() { p } else { base.join(p) }
        })
        .collect())
}

/// Playlists in `dir` as (name, path), sorted by name.
pub fn list(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("m3u8") || e.eq_ignore_ascii_case("m3u"))
        })
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect();
    out.sort_by_key(|a| a.0.to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_audio::TrackInfo;

    #[test]
    fn save_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("cw-pl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tracks: Vec<Track> = ["/music/a.flac", "/music/b c.mp3"]
            .iter()
            .map(|p| Track {
                path: PathBuf::from(p),
                info: TrackInfo {
                    title: "T".into(),
                    artist: Some("A".into()),
                    duration_secs: Some(61.4),
                    ..Default::default()
                },
            })
            .collect();
        let path = save(&dir, "road/trip", &tracks).unwrap();
        assert_eq!(path.file_name().unwrap(), "road_trip.m3u8");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("#EXTM3U\n#EXTINF:61,A - T\n/music/a.flac\n"),
            "{text}"
        );
        assert_eq!(
            load(&path).unwrap(),
            [
                PathBuf::from("/music/a.flac"),
                PathBuf::from("/music/b c.mp3")
            ]
        );
        assert_eq!(list(&dir), [("road_trip".to_string(), path.clone())]);

        // Relative entries and BOMs from other players.
        std::fs::write(
            dir.join("x.m3u"),
            "\u{feff}#EXTM3U\nsub/one.mp3\n\nhttp://radio/stream\n",
        )
        .unwrap();
        assert_eq!(load(&dir.join("x.m3u")).unwrap(), [dir.join("sub/one.mp3")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
