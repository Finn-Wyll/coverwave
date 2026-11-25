//! The settings menu (`,`): the iOS app's settings, in the terminal's own
//! style. Rows change with left/right (or enter), apply at once, and are
//! saved to `config.toml` with its comments and layout kept (`toml_edit`).

use std::path::Path;

/// One setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    ShowLyrics,
    LyricStyle,
    LyricScroll,
    LyricSize,
    LyricColors,
    LyricsOnline,
    ArtStyle,
    Motion,
    FontSize,
    Source,
    SpotifyLogin,
    SpotifyOffline,
    SaveAhead,
    OfflineSaved,
    TempoOnline,
}

/// Sections and their rows, top to bottom.
pub const SECTIONS: &[(&str, &[Item])] = &[
    (
        "lyrics",
        &[
            Item::ShowLyrics,
            Item::LyricStyle,
            Item::LyricScroll,
            Item::LyricSize,
            Item::LyricColors,
            Item::LyricsOnline,
        ],
    ),
    ("art", &[Item::ArtStyle, Item::Motion, Item::FontSize]),
    ("sources", &[Item::Source, Item::SpotifyLogin]),
    (
        "offline",
        &[Item::SpotifyOffline, Item::SaveAhead, Item::OfflineSaved],
    ),
    ("rhythm", &[Item::TempoOnline]),
];

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::ShowLyrics => "show lyrics",
            Item::LyricStyle => "style",
            Item::LyricScroll => "scroll",
            Item::LyricSize => "size",
            Item::LyricColors => "colours",
            Item::LyricsOnline => "look up online",
            Item::ArtStyle => "art style",
            Item::Motion => "motion",
            Item::FontSize => "ghostty font size",
            Item::Source => "source",
            Item::SpotifyLogin => "spotify",
            Item::SpotifyOffline => "spotify offline",
            Item::SaveAhead => "save ahead",
            Item::OfflineSaved => "saved",
            Item::TempoOnline => "look up tempo online",
        }
    }

    /// One line on what it does, shown under the menu for the chosen row.
    pub fn help(self) -> &'static str {
        match self {
            Item::ShowLyrics => "lyrics take the cover's place; the motion keeps running (l)",
            Item::LyricStyle => "big line, all text, or only the sung line, big",
            Item::LyricScroll => "new lines rise from below (like Spotify) or come down",
            Item::LyricSize => "how big the sung line is; a smaller font makes it finer",
            Item::LyricColors => "accent, or the cover's own colours: letters as windows",
            Item::LyricsOnline => "find lyrics on lrclib.net (sends title, artist, album)",
            Item::ArtStyle => "how the cover is coloured (c)",
            Item::Motion => "cover motion on or off (m)",
            Item::FontSize => "coverwave's window font in Ghostty, reset on quit",
            Item::Source => "local files, Spotify or Apple Music (S)",
            Item::SpotifyLogin => "enter logs in (in your browser) or out",
            Item::SpotifyOffline => {
                "auto: follow the Spotify app when the internet is gone; always: never go online"
            }
            Item::SaveAhead => "while online, keep covers, genres, tempos and lyrics for offline",
            Item::OfflineSaved => {
                "songs with everything saved for offline; downloads stay in Spotify"
            }
            Item::TempoOnline => "beat clock tempo from Deezer when audio cannot be heard",
        }
    }
}

/// Ghostty font sizes offered, in points; 0 leaves the font alone.
pub const FONT_SIZES: [f32; 10] = [0.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0];

#[derive(Debug, Default)]
pub struct SettingsMenu {
    pub open: bool,
    pub cursor: usize,
}

impl SettingsMenu {
    /// Every row in order.
    pub fn items() -> Vec<Item> {
        SECTIONS
            .iter()
            .flat_map(|(_, items)| items.iter().copied())
            .collect()
    }

    pub fn selected(&self) -> Item {
        let items = Self::items();
        items[self.cursor.min(items.len() - 1)]
    }

    pub fn move_by(&mut self, delta: isize) {
        let n = Self::items().len() as isize;
        self.cursor = (self.cursor as isize + delta).rem_euclid(n) as usize;
    }
}

/// The next (or previous, `dir < 0`) entry of `list` after `current`.
pub fn step<T: PartialEq + Copy>(list: &[T], current: T, dir: i32) -> T {
    let i = list.iter().position(|v| *v == current).unwrap_or(0) as i32;
    list[(i + dir).rem_euclid(list.len() as i32) as usize]
}

/// A value to save: a string, a boolean or a number.
#[derive(Debug, Clone, PartialEq)]
pub enum Saved {
    Text(String),
    Flag(bool),
    Number(f64),
}

/// Writes `key` (in `[section]`, or at the top) into the config file,
/// keeping everything else as the user wrote it. Creates the file (from
/// the template) if it is missing.
pub fn save(path: &Path, section: Option<&str>, key: &str, value: Saved) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|_| crate::config::TEMPLATE.to_string());
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("config.toml: {e}"))?;
    let value = match value {
        Saved::Text(s) => toml_edit::value(s),
        Saved::Flag(b) => toml_edit::value(b),
        Saved::Number(n) if n.fract() == 0.0 => toml_edit::value(n as i64),
        Saved::Number(n) => toml_edit::value(n),
    };
    match section {
        None => set_keeping_comment(doc.as_table_mut(), key, value),
        Some(name) => {
            if !doc.contains_key(name) {
                doc[name] = toml_edit::table();
            }
            let table = doc[name]
                .as_table_mut()
                .ok_or_else(|| format!("config.toml: [{name}] is not a table"))?;
            set_keeping_comment(table, key, value);
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, doc.to_string()).map_err(|e| e.to_string())
}

/// Sets a value, keeping the end-of-line comment that was after the old one.
fn set_keeping_comment(table: &mut toml_edit::Table, key: &str, mut value: toml_edit::Item) {
    if let Some(old) = table.get(key).and_then(|i| i.as_value())
        && let Some(v) = value.as_value_mut()
    {
        *v.decor_mut() = old.decor().clone();
    }
    table[key] = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_wraps_and_steps_cycle() {
        let mut m = SettingsMenu::default();
        assert_eq!(m.selected(), Item::ShowLyrics);
        m.move_by(-1);
        assert_eq!(m.selected(), Item::TempoOnline);
        m.move_by(2);
        assert_eq!(m.selected(), Item::LyricStyle);
        assert_eq!(step(&[1, 2, 3], 3, 1), 1);
        assert_eq!(step(&[1, 2, 3], 1, -1), 3);
        assert_eq!(step(&FONT_SIZES, 9.0, 1), 10.0);
    }

    #[test]
    fn saves_keys_and_keeps_comments() {
        let dir = std::env::temp_dir().join(format!("cw-settings-{}", std::process::id()));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            "# mine\nmotion = true   # keep me\n\n[spotify]\nclient_id = \"abc\"\n",
        )
        .unwrap();
        save(&path, None, "motion", Saved::Flag(false)).unwrap();
        save(&path, Some("lyrics"), "style", Saved::Text("text".into())).unwrap();
        save(&path, None, "font_size", Saved::Number(8.0)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mine"), "{text}");
        assert!(text.contains("motion = false   # keep me"), "{text}");
        assert!(text.contains("client_id = \"abc\""));
        assert!(text.contains("[lyrics]") && text.contains("style = \"text\""));
        assert!(text.contains("font_size = 8"));
        // What was written still loads as a config.
        let (c, warning) = crate::config::Config::load(&path);
        assert_eq!(warning, None);
        assert!(!c.motion && c.lyrics.style == "text" && c.font_size == 8.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
