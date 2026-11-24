//! Where coverwave keeps its files.
//!
//! | | macOS / Linux | Windows |
//! |---|---|---|
//! | config (`config.toml`, recipe overrides) | `~/.config/coverwave` | `%APPDATA%\coverwave` |
//! | data (library, playlists, queue) | `~/.local/share/coverwave` | `%LOCALAPPDATA%\coverwave` |
//! | cache (model, recipes) | `~/.cache/coverwave` | `%LOCALAPPDATA%\coverwave\cache` |
//!
//! `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME` override these
//! on every platform (handy for tests and portable setups). macOS uses the
//! XDG layout on purpose: it is what terminal users expect, and it keeps the
//! files easy to find and edit.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Config,
    Data,
    Cache,
}

pub fn dir(kind: Kind) -> Option<PathBuf> {
    resolve(kind, cfg!(windows), &|k| std::env::var_os(k))
}

/// The user's home folder: `HOME`, else `USERPROFILE` (Windows).
pub fn home() -> Option<PathBuf> {
    home_from(&|k| std::env::var_os(k))
}

fn home_from(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    get("HOME").or_else(|| get("USERPROFILE"))
}

/// `kind`'s folder under the given environment (pure, for tests).
pub fn resolve(
    kind: Kind,
    windows: bool,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let xdg = match kind {
        Kind::Config => "XDG_CONFIG_HOME",
        Kind::Data => "XDG_DATA_HOME",
        Kind::Cache => "XDG_CACHE_HOME",
    };
    if let Some(base) = get(xdg) {
        return Some(base.join("coverwave"));
    }
    if windows {
        let roaming = get("APPDATA");
        let local = get("LOCALAPPDATA").or_else(|| roaming.clone());
        return match kind {
            Kind::Config => roaming.map(|b| b.join("coverwave")),
            Kind::Data => local.map(|b| b.join("coverwave")),
            Kind::Cache => local.map(|b| b.join("coverwave").join("cache")),
        };
    }
    let home = home_from(env)?;
    Some(match kind {
        Kind::Config => home.join(".config").join("coverwave"),
        Kind::Data => home.join(".local").join("share").join("coverwave"),
        Kind::Cache => home.join(".cache").join("coverwave"),
    })
}

/// Expands a leading `~` (`~/Music`, `~\Music`, `~`) to the home folder.
pub fn expand_tilde(p: &str) -> PathBuf {
    expand_with(p, home())
}

fn expand_with(p: &str, home: Option<PathBuf>) -> PathBuf {
    let rest = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\"));
    match (rest, home) {
        (Some(rest), Some(h)) => h.join(rest),
        (None, Some(h)) if p == "~" => h,
        _ => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn unix_defaults_and_xdg_overrides() {
        let e = env(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/tmp/c")]);
        assert_eq!(
            resolve(Kind::Config, false, &e),
            Some(PathBuf::from("/home/u/.config/coverwave"))
        );
        assert_eq!(
            resolve(Kind::Data, false, &e),
            Some(PathBuf::from("/home/u/.local/share/coverwave"))
        );
        assert_eq!(
            resolve(Kind::Cache, false, &e),
            Some(PathBuf::from("/tmp/c/coverwave"))
        );
        assert_eq!(resolve(Kind::Config, false, &env(&[])), None);
    }

    #[test]
    fn windows_uses_appdata() {
        let e = env(&[
            ("APPDATA", r"C:\Users\u\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
            ("USERPROFILE", r"C:\Users\u"),
        ]);
        let c = resolve(Kind::Config, true, &e).unwrap();
        assert!(c.starts_with(r"C:\Users\u\AppData\Roaming") && c.ends_with("coverwave"));
        let d = resolve(Kind::Data, true, &e).unwrap();
        assert!(d.starts_with(r"C:\Users\u\AppData\Local") && d.ends_with("coverwave"));
        assert!(resolve(Kind::Cache, true, &e).unwrap().ends_with("cache"));
        // XDG still wins (tests, portable setups).
        let x = env(&[("APPDATA", "a"), ("XDG_CONFIG_HOME", "x")]);
        assert_eq!(
            resolve(Kind::Config, true, &x),
            Some(PathBuf::from("x/coverwave"))
        );
        // Without LOCALAPPDATA, data falls back to roaming.
        assert!(resolve(Kind::Data, true, &env(&[("APPDATA", "a")])).is_some());
    }

    #[test]
    fn home_falls_back_to_userprofile_and_tilde_expands() {
        assert_eq!(
            home_from(&env(&[("USERPROFILE", "/w")])),
            Some(PathBuf::from("/w"))
        );
        assert_eq!(
            home_from(&env(&[("HOME", "/h"), ("USERPROFILE", "/w")])),
            Some(PathBuf::from("/h"))
        );
        let h = Some(PathBuf::from("/h"));
        assert_eq!(expand_with("~/Music", h.clone()), PathBuf::from("/h/Music"));
        assert_eq!(
            expand_with("~\\Music", h.clone()),
            PathBuf::from("/h").join("Music")
        );
        assert_eq!(expand_with("~", h.clone()), PathBuf::from("/h"));
        assert_eq!(expand_with("/abs", h.clone()), PathBuf::from("/abs"));
        assert_eq!(expand_with("~/x", None), PathBuf::from("~/x"));
    }
}
