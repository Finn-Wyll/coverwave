//! `~/.config/coverwave/config.toml` (PLAN.md section 12). Every key is
//! optional; command-line flags override it. A missing file is the
//! defaults; a broken one is reported and the defaults are used.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Folders indexed into the library. `~` is expanded.
    pub music_dirs: Vec<String>,
    pub fps: u32,
    /// "auto" | "truecolor" | "256"
    pub color: String,
    /// "palette" | "truecolor" | "duotone"
    pub art_style: String,
    /// "auto" or a number (cell width / height).
    pub cell_aspect: toml::Value,
    /// Never download the cover-analysis model.
    pub offline: bool,
    /// Start with cover motion on.
    pub motion: bool,
    /// Ghostty: coverwave's window uses this font size (points) while it
    /// runs, for finer art. 0 leaves the font alone.
    pub font_size: f32,
    /// Kept from the planned config; reserved.
    pub change_budget: Option<f32>,
    pub charset: Option<String>,
    pub lyrics: Lyrics,
    pub spotify: Spotify,
    pub capture: Capture,
    pub tempo: Tempo,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Tempo {
    /// Look up a remote track's tempo on Deezer (sends artist, title or
    /// ISRC) so the beat clock matches the song when audio is not captured.
    pub online_lookup: bool,
}

impl Default for Tempo {
    fn default() -> Self {
        Tempo {
            online_lookup: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Spotify {
    /// The public client ID of your Spotify app (developer.spotify.com).
    /// Empty: no Spotify source.
    pub client_id: String,
    /// Register `http://127.0.0.1:<port>/callback` in that app.
    pub redirect_port: u16,
    /// "auto" (follow the Spotify app only when the internet is gone) |
    /// "always" (offline mode: never use the internet for Spotify)
    pub offline: String,
    /// Saved ahead for offline: "off" | "liked" | "all" (liked + playlists)
    pub save_ahead: String,
}

impl Default for Spotify {
    fn default() -> Self {
        Spotify {
            client_id: String::new(),
            redirect_port: 8898,
            offline: "auto".into(),
            save_ahead: "liked".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Capture {
    /// Capture the remote player's audio so its cover reacts to the music.
    pub enabled: bool,
}

impl Default for Capture {
    fn default() -> Self {
        Capture { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Lyrics {
    /// Look lyrics up on LRCLIB (sends artist, title, album, length).
    pub online_lookup: bool,
    /// "big_only" (only the sung line, big; default) | "big" (sung line
    /// big, the others as text) | "text"
    pub style: String,
    /// "up" (new lines rise, like Spotify) | "down"
    pub scroll: String,
    /// Big letters: "small" | "medium" | "large" (same at any font size)
    pub size: String,
    /// "accent" | "cover" (letters as windows onto the cover)
    pub colors: String,
}

impl Default for Lyrics {
    fn default() -> Self {
        Lyrics {
            online_lookup: true,
            style: "big_only".into(),
            scroll: "up".into(),
            size: "medium".into(),
            colors: "accent".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            music_dirs: vec!["~/Music".into()],
            fps: 30,
            color: "auto".into(),
            art_style: "palette".into(),
            cell_aspect: toml::Value::String("auto".into()),
            offline: false,
            motion: true,
            font_size: 9.0,
            change_budget: None,
            charset: None,
            lyrics: Lyrics::default(),
            spotify: Spotify::default(),
            capture: Capture::default(),
            tempo: Tempo::default(),
        }
    }
}

impl Config {
    /// Reads `path`. Returns the defaults plus a warning if it is broken.
    pub fn load(path: &Path) -> (Config, Option<String>) {
        match std::fs::read_to_string(path) {
            Err(_) => (Config::default(), None),
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => (c, None),
                Err(e) => (
                    Config::default(),
                    Some(format!("{}: {}", path.display(), e.message())),
                ),
            },
        }
    }

    pub fn music_dirs(&self) -> Vec<PathBuf> {
        self.music_dirs.iter().map(|d| expand_tilde(d)).collect()
    }

    pub fn cell_aspect(&self) -> Option<f32> {
        match &self.cell_aspect {
            toml::Value::Float(f) => Some(*f as f32),
            toml::Value::Integer(i) => Some(*i as f32),
            _ => None,
        }
        .filter(|r| (0.2..=2.0).contains(r))
    }
}

pub fn expand_tilde(p: &str) -> PathBuf {
    crate::paths::expand_tilde(p)
}

/// The default config file, written on first run so users can find it.
pub const TEMPLATE: &str = r#"# coverwave settings. Command-line flags override these.

# Folders indexed into the library (searched recursively).
music_dirs = ["~/Music"]

fps = 30
color = "auto"          # auto | truecolor | 256
art_style = "palette"   # palette | truecolor | duotone  (c cycles in the app)
cell_aspect = "auto"    # auto (measured) or a number such as 0.5
offline = false         # true: never download the cover-analysis model
motion = true           # false: start with cover motion off (m toggles)
font_size = 9           # Ghostty: font size while coverwave runs, for finer
                        # art; reset on quit. 0 leaves the font alone.

[spotify]
# Your Spotify app's public client ID (developer.spotify.com > Dashboard).
# In the app, add the redirect URI http://127.0.0.1:8898/callback
client_id = ""
redirect_port = 8898
# Offline: coverwave follows the Spotify app (macOS), which plays your
# downloaded songs, and uses what it saved: covers, genres, tempos, lyrics
# and your library's listings.
offline = "auto"        # auto (when the internet is gone) | always
save_ahead = "liked"    # save ahead for offline: off | liked | all (+ playlists)

[capture]
# Listen to Spotify / Apple Music so their covers react to the music.
enabled = true

[lyrics]
# l shows lyrics instead of the cover (the cover keeps moving behind).
online_lookup = true    # look lyrics up on LRCLIB (sends artist, title, album)
style = "big_only"      # big_only (sung line, big) | big (+ others as text) | text
scroll = "up"           # up (new lines rise, like Spotify) | down
size = "medium"         # big letters: small | medium | large
colors = "accent"       # accent | cover (letters as windows onto the cover)

[tempo]
# When a remote track's audio cannot be captured, the cover moves to a beat
# clock. true: look up the track's tempo on Deezer (sends only its artist,
# title or ISRC). false: use a typical tempo for the genre.
online_lookup = true
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        let c: Config = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(c.cell_aspect(), None);
        assert_eq!(c.font_size, 9.0, "Ghostty zoom on by default");
        let off: Config = toml::from_str("font_size = 0").unwrap();
        assert_eq!(off.font_size, 0.0);
    }

    #[test]
    fn partial_and_numeric_values() {
        let c: Config =
            toml::from_str("music_dirs = ['/x', '~/y']\ncell_aspect = 0.55\nmotion = false")
                .unwrap();
        assert_eq!(c.cell_aspect(), Some(0.55));
        assert!(!c.motion);
        assert_eq!(c.music_dirs()[0], PathBuf::from("/x"));
        assert!(toml::from_str::<Config>("bogus = 1").is_err());
    }
}
