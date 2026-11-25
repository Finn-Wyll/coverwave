//! The Spotify desktop app through AppleScript (`osascript`), for when the
//! Web API cannot be reached: the app plays its downloaded songs offline and
//! still answers AppleScript, with no network. macOS only.
//!
//! The first use asks macOS for permission to control Spotify
//! ("Automation"), attributed to your terminal app.

use crate::{ArtRef, Command, NowPlaying, PlayRequest, SourceError};

/// One tab-separated line: state, name, artist, album, duration (ms),
/// position (s), track URI, volume, artwork URL. Or `notrunning` / `stopped`.
pub const STATE_SCRIPT: &str = r#"
if application "Spotify" is not running then return "notrunning"
tell application "Spotify"
    set st to player state as string
    if st is "stopped" then return "stopped"
    set t to current track
    set art to ""
    try
        set art to artwork url of t
    end try
    return st & tab & (name of t) & tab & (artist of t) & tab & (album of t) & tab & ((duration of t) as string) & tab & ((player position) as string) & tab & (id of t) & tab & ((sound volume) as string) & tab & art
end tell
"#;

/// Spotify's URIs to bare IDs ("spotify:track:abc" -> "abc"), matching the
/// Web API's `id`.
pub fn bare_id(uri: &str) -> &str {
    uri.rsplit(':').next().unwrap_or(uri)
}

/// Parses [`STATE_SCRIPT`]'s output (numbers may use a decimal comma).
pub fn parse_state(out: &str) -> Result<Option<NowPlaying>, SourceError> {
    let line = out.trim_end_matches(['\r', '\n']);
    match line.trim() {
        "notrunning" => {
            return Err(SourceError::Unavailable(
                "offline: open the Spotify app to play your downloads".into(),
            ));
        }
        "stopped" | "" => return Ok(None),
        _ => {}
    }
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 8 {
        return Err(SourceError::Failed(format!(
            "unexpected Spotify output: {line:?}"
        )));
    }
    let num = |s: &str| s.trim().replace(',', ".").parse::<f64>().ok();
    let text = |s: &str| {
        let s = s.trim();
        (!s.is_empty() && s != "missing value").then(|| s.to_string())
    };
    let uri = f[6].trim();
    Ok(Some(NowPlaying {
        track_id: bare_id(uri).to_string(),
        title: text(f[1]).unwrap_or_else(|| "Unknown".into()),
        artist: text(f[2]),
        album: text(f[3]),
        duration_secs: num(f[4]).filter(|d| *d > 0.0).map(|ms| ms / 1000.0),
        position_secs: num(f[5]).unwrap_or(0.0),
        playing: f[0].trim() == "playing",
        volume: num(f[7]).map(|v| v.clamp(0.0, 100.0) as u8),
        art: f
            .get(8)
            .and_then(|a| text(a))
            .map_or(ArtRef::None, ArtRef::Url),
        genre: None,
        isrc: None,
        bpm: None,
        device: Some("this computer, offline".into()),
    }))
}

/// AppleScript string literal.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The script for a command, or `None` when it means nothing to the app.
pub fn command_script(c: &Command) -> Result<Option<String>, SourceError> {
    let tell = |what: &str| Some(format!("tell application \"Spotify\" to {what}"));
    Ok(match c {
        Command::PlayPause => tell("playpause"),
        Command::Next => tell("next track"),
        Command::Previous => tell("previous track"),
        Command::Seek(s) => tell(&format!("set player position to {s}")),
        Command::Volume(v) => tell(&format!("set sound volume to {}", v.min(&100))),
        Command::Play(req) => Some(play_script(req)?),
        Command::Transfer(_) => {
            return Err(SourceError::Offline(
                "moving playback to another device needs the internet".into(),
            ));
        }
        Command::Login
        | Command::Logout
        | Command::Browse(_)
        | Command::OfflineMode(_)
        | Command::SaveAhead(_) => None,
    })
}

/// Plays a context at a track, a context, or the first of a track list.
pub fn play_script(req: &PlayRequest) -> Result<String, SourceError> {
    let play = match (&req.context_uri, &req.offset_uri, req.uris.first()) {
        (Some(ctx), Some(track), _) => {
            format!("play track {} in context {}", quoted(track), quoted(ctx))
        }
        (Some(ctx), None, _) => format!("play track {}", quoted(ctx)),
        (None, _, Some(track)) => format!("play track {}", quoted(track)),
        (None, _, None) => return Err(SourceError::Failed("nothing to play".into())),
    };
    Ok(format!("tell application \"Spotify\" to {play}"))
}

#[cfg(target_os = "macos")]
pub fn osascript(script: &str) -> Result<String, SourceError> {
    use std::process::Stdio;
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| SourceError::Failed(format!("osascript: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-1743") {
            return Err(SourceError::Failed(
                "allow your terminal to control Spotify: System Settings > Privacy & Security > Automation".into(),
            ));
        }
        return Err(SourceError::Failed(format!("osascript: {}", err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(target_os = "macos"))]
pub fn osascript(_script: &str) -> Result<String, SourceError> {
    Err(SourceError::Unavailable(
        "offline: following the Spotify app needs macOS".into(),
    ))
}

/// Reads what the Spotify app is playing.
pub fn poll() -> Result<Option<NowPlaying>, SourceError> {
    parse_state(&osascript(STATE_SCRIPT)?)
}

/// Sends a command to the Spotify app.
pub fn command(c: &Command) -> Result<(), SourceError> {
    match command_script(c)? {
        Some(script) => osascript(&script).map(|_| ()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_with_ms_duration_and_uri() {
        let out = "playing\tLet Down\tRadiohead\tOK Computer\t299000\t12,5\tspotify:track:abc\t70\thttps://i.scdn.co/image/x\n";
        let n = parse_state(out).unwrap().unwrap();
        assert_eq!(n.track_id, "abc");
        assert_eq!(n.duration_secs, Some(299.0));
        assert_eq!(n.position_secs, 12.5);
        assert_eq!(n.art, ArtRef::Url("https://i.scdn.co/image/x".into()));
        assert!(n.playing);
        let paused = parse_state("paused\tT\tA\tB\t1000\t0\tspotify:track:z\t10\t")
            .unwrap()
            .unwrap();
        assert!(!paused.playing && paused.art == ArtRef::None);
        assert_eq!(parse_state("stopped").unwrap(), None);
        assert!(matches!(
            parse_state("notrunning"),
            Err(SourceError::Unavailable(_))
        ));
    }

    #[test]
    fn play_scripts_keep_the_context() {
        let s = play_script(&PlayRequest {
            context_uri: Some("spotify:playlist:p".into()),
            offset_uri: Some("spotify:track:t".into()),
            uris: Vec::new(),
        })
        .unwrap();
        assert!(
            s.ends_with("play track \"spotify:track:t\" in context \"spotify:playlist:p\""),
            "{s}"
        );
        let s = play_script(&PlayRequest {
            uris: vec!["spotify:track:a".into(), "spotify:track:b".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(s.ends_with("play track \"spotify:track:a\""));
        assert!(play_script(&PlayRequest::default()).is_err());
        assert!(matches!(
            command_script(&Command::Transfer("d".into())),
            Err(SourceError::Offline(_))
        ));
        assert_eq!(bare_id("spotify:track:abc"), "abc");
    }
}
