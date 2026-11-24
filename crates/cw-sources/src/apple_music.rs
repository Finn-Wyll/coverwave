//! Apple Music (Music.app) through AppleScript (`osascript`). macOS only:
//! elsewhere the backend reports that it is unavailable.
//!
//! The first use asks macOS for permission to control Music ("Automation"),
//! attributed to your terminal app.
//!
//! The output parsing is portable and tested; the scripts themselves are
//! UNTESTED (written in a Linux cloud session). See LOCAL_TODO.md.

use crate::{ArtRef, Backend, Command, NowPlaying, SourceError};
use std::path::PathBuf;

/// Prints one tab-separated line: state, name, artist, album, duration,
/// position, persistent ID, volume, genre. Or `notrunning` / `stopped`.
pub const STATE_SCRIPT: &str = r#"
if application "Music" is not running then return "notrunning"
tell application "Music"
    set st to player state as string
    if st is "stopped" then return "stopped"
    set t to current track
    set d to duration of t
    set p to player position
    return st & tab & (name of t) & tab & (artist of t) & tab & (album of t) & tab & (d as string) & tab & (p as string) & tab & (persistent ID of t) & tab & (sound volume as string) & tab & (genre of t)
end tell
"#;

/// Writes the current track's first artwork to the given POSIX path and
/// prints the path, or prints nothing when there is no artwork.
pub const ART_SCRIPT: &str = r#"
on run argv
    set outPath to item 1 of argv
    tell application "Music"
        set t to current track
        if (count of artworks of t) is 0 then return ""
        set d to raw data of artwork 1 of t
    end tell
    set f to open for access (POSIX file outPath) with write permission
    set eof f to 0
    write d to f
    close access f
    return outPath
end run
"#;

/// Parses [`STATE_SCRIPT`]'s output. Numbers may use a decimal comma
/// (AppleScript follows the system locale).
pub fn parse_state(out: &str) -> Result<Option<NowPlaying>, SourceError> {
    let line = out.trim_end_matches(['\r', '\n']);
    match line.trim() {
        "notrunning" => return Err(SourceError::Unavailable("Music is not running".into())),
        "stopped" | "" => return Ok(None),
        _ => {}
    }
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 8 {
        return Err(SourceError::Failed(format!(
            "unexpected Music.app output: {line:?}"
        )));
    }
    let num = |s: &str| s.trim().replace(',', ".").parse::<f64>().ok();
    let text = |s: &str| {
        let s = s.trim();
        (!s.is_empty() && s != "missing value").then(|| s.to_string())
    };
    Ok(Some(NowPlaying {
        track_id: f[6].trim().to_string(),
        title: text(f[1]).unwrap_or_else(|| "Unknown".into()),
        artist: text(f[2]),
        album: text(f[3]),
        duration_secs: num(f[4]).filter(|d| *d > 0.0),
        position_secs: num(f[5]).unwrap_or(0.0),
        playing: f[0].trim() == "playing",
        volume: num(f[7]).map(|v| v.clamp(0.0, 100.0) as u8),
        art: ArtRef::None,
        genre: f.get(8).and_then(|g| text(g)),
        isrc: None,
        bpm: None,
    }))
}

pub struct AppleMusic {
    /// Persistent ID of the track whose artwork was exported last.
    art_for: Option<String>,
    art_path: PathBuf,
}

impl AppleMusic {
    pub fn new() -> AppleMusic {
        AppleMusic {
            art_for: None,
            art_path: std::env::temp_dir()
                .join(format!("coverwave-music-art-{}", std::process::id())),
        }
    }
}

impl Default for AppleMusic {
    fn default() -> Self {
        AppleMusic::new()
    }
}

#[cfg(target_os = "macos")]
fn osascript(script: &str, args: &[&str]) -> Result<String, SourceError> {
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .args(args)
        .output()
        .map_err(|e| SourceError::Failed(format!("osascript: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        // -1743: the user has not allowed controlling Music.
        if err.contains("-1743") {
            return Err(SourceError::Failed(
                "allow your terminal to control Music: System Settings > Privacy & Security > Automation".into(),
            ));
        }
        return Err(SourceError::Failed(format!("osascript: {}", err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn osascript(_script: &str, _args: &[&str]) -> Result<String, SourceError> {
    Err(SourceError::Unavailable(
        "Apple Music control needs macOS".into(),
    ))
}

impl Backend for AppleMusic {
    fn name(&self) -> &'static str {
        "Apple Music"
    }

    fn poll(&mut self) -> Result<Option<NowPlaying>, SourceError> {
        let Some(mut now) = parse_state(&osascript(STATE_SCRIPT, &[])?)? else {
            return Ok(None);
        };
        // Export the artwork once per track; reuse the file after that.
        if self.art_for.as_deref() != Some(now.track_id.as_str()) {
            let path = self.art_path.to_string_lossy().into_owned();
            let exported = osascript(ART_SCRIPT, &[&path]).map(|o| !o.trim().is_empty());
            self.art_for = Some(now.track_id.clone());
            if !matches!(exported, Ok(true)) {
                let _ = std::fs::remove_file(&self.art_path);
            }
        }
        if self.art_path.exists() {
            now.art = ArtRef::File(self.art_path.clone());
        }
        Ok(Some(now))
    }

    fn command(&mut self, c: &Command) -> Result<(), SourceError> {
        let script = match c {
            Command::PlayPause => "tell application \"Music\" to playpause".to_string(),
            Command::Next => "tell application \"Music\" to next track".to_string(),
            Command::Previous => "tell application \"Music\" to previous track".to_string(),
            Command::Seek(s) => format!("tell application \"Music\" to set player position to {s}"),
            Command::Volume(v) => format!(
                "tell application \"Music\" to set sound volume to {}",
                v.min(&100)
            ),
            Command::Login | Command::Logout | Command::Browse(_) => return Ok(()),
            Command::Play(_) => {
                return Err(SourceError::Unavailable(
                    "pick music in the Music app itself".into(),
                ));
            }
        };
        osascript(&script, &[]).map(|_| ())
    }
}

impl Drop for AppleMusic {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.art_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playing_track_with_decimal_commas() {
        let out = "playing\tLow Tide\tLumen\tTides\t245,5\t12,25\tABCDEF0123\t65\n";
        let n = parse_state(out).unwrap().unwrap();
        assert_eq!(
            (n.title.as_str(), n.artist.as_deref(), n.album.as_deref()),
            ("Low Tide", Some("Lumen"), Some("Tides"))
        );
        assert_eq!(
            (n.duration_secs, n.position_secs, n.volume),
            (Some(245.5), 12.25, Some(65))
        );
        assert!(n.playing);
        assert_eq!(n.track_id, "ABCDEF0123");
        assert_eq!(
            n.genre, None,
            "older output without a genre field still parses"
        );
        let g = parse_state("playing\tT\tA\tB\t10\t1\tID\t50\tAlternative\n")
            .unwrap()
            .unwrap();
        assert_eq!(g.genre.as_deref(), Some("Alternative"));
    }

    #[test]
    fn paused_missing_fields_stopped_and_not_running() {
        let n = parse_state("paused\tRadio\tmissing value\t\t0\t3.0\tID\t40")
            .unwrap()
            .unwrap();
        assert!(!n.playing);
        assert_eq!((n.artist, n.album, n.duration_secs), (None, None, None));
        assert_eq!(parse_state("stopped\n").unwrap(), None);
        assert!(matches!(
            parse_state("notrunning"),
            Err(SourceError::Unavailable(_))
        ));
        assert!(parse_state("playing\tonly two").is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unavailable_off_macos() {
        let mut m = AppleMusic::new();
        assert!(matches!(m.poll(), Err(SourceError::Unavailable(_))));
    }
}
