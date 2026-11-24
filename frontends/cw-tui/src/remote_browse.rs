//! Browsing a remote library (Spotify) in the Library view: a stack of
//! places, a cursor per place, a text filter, and a short-lived cache so
//! going back does not refetch.
//!
//! Listings arrive asynchronously in `RemoteState::library`; [`absorb`]
//! files each finished one into the cache.
//!
//! [`absorb`]: RemoteBrowser::absorb

use cw_sources::{ItemAction, LibraryItem, LibraryPath, LibraryView, PlayRequest};
use std::time::{Duration, Instant};

/// Cached listings are refetched after this long.
const CACHE_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteAction {
    None,
    Browse(LibraryPath),
    Play(PlayRequest),
}

struct Level {
    path: LibraryPath,
    cursor: usize,
    offset: usize,
}

pub struct RemoteBrowser {
    stack: Vec<Level>,
    /// Text filter over the current listing, while typing after `/`.
    pub filter: Option<String>,
    cache: Vec<(LibraryPath, Vec<LibraryItem>, Instant)>,
    /// The listing asked for and not yet arrived.
    requested: Option<LibraryPath>,
    /// The last error for the current place.
    pub error: Option<String>,
}

impl Default for RemoteBrowser {
    fn default() -> Self {
        RemoteBrowser::new()
    }
}

impl RemoteBrowser {
    pub fn new() -> RemoteBrowser {
        RemoteBrowser {
            stack: vec![Level {
                path: LibraryPath::Root,
                cursor: 0,
                offset: 0,
            }],
            filter: None,
            cache: Vec::new(),
            requested: None,
            error: None,
        }
    }

    fn level(&self) -> &Level {
        self.stack.last().expect("the stack always has the root")
    }

    fn level_mut(&mut self) -> &mut Level {
        self.stack
            .last_mut()
            .expect("the stack always has the root")
    }

    pub fn path(&self) -> &LibraryPath {
        &self.level().path
    }

    /// Places from the root to here, for the heading.
    pub fn breadcrumb(&self) -> Vec<&str> {
        self.stack.iter().map(|l| l.path.title()).collect()
    }

    pub fn cursor(&self) -> usize {
        self.level().cursor
    }

    pub fn scroll(&mut self, rows: usize) -> usize {
        let l = self.level_mut();
        if l.cursor < l.offset {
            l.offset = l.cursor;
        } else if rows > 0 && l.cursor >= l.offset + rows {
            l.offset = l.cursor + 1 - rows;
        }
        l.offset
    }

    fn cached(&self, path: &LibraryPath) -> Option<&Vec<LibraryItem>> {
        self.cache
            .iter()
            .find(|(p, _, at)| p == path && at.elapsed() < CACHE_TTL)
            .map(|(_, items, _)| items)
    }

    /// Takes in the source's latest listing.
    pub fn absorb(&mut self, view: Option<&LibraryView>) {
        let Some(view) = view else { return };
        if view.loading {
            return;
        }
        if self.requested.as_ref() == Some(&view.path) {
            self.requested = None;
        }
        if view.path == *self.path() {
            self.error = view.error.clone();
        }
        if view.error.is_none() {
            self.cache.retain(|(p, _, _)| *p != view.path);
            self.cache
                .push((view.path.clone(), view.items.clone(), Instant::now()));
        }
    }

    /// Whether the current place still has to be fetched (and is not on its
    /// way). Marks it as requested when it returns a path.
    pub fn wanted(&mut self) -> Option<LibraryPath> {
        let path = self.path().clone();
        if self.cached(&path).is_some()
            || self.error.is_some()
            || self.requested.as_ref() == Some(&path)
        {
            return None;
        }
        self.requested = Some(path.clone());
        Some(path)
    }

    pub fn is_loading(&self) -> bool {
        self.requested.as_ref() == Some(self.path())
    }

    /// The rows to show: the cached listing for here, filtered.
    pub fn rows(&self) -> Vec<&LibraryItem> {
        let Some(items) = self.cached(self.path()) else {
            return Vec::new();
        };
        match self.filter.as_deref().map(str::to_lowercase) {
            Some(f) if !f.is_empty() => items
                .iter()
                .filter(|i| {
                    i.title.to_lowercase().contains(&f)
                        || i.subtitle
                            .as_deref()
                            .is_some_and(|s| s.to_lowercase().contains(&f))
                })
                .collect(),
            _ => items.iter().collect(),
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let n = self.rows().len();
        let l = self.level_mut();
        l.cursor = if n == 0 {
            0
        } else {
            (l.cursor as isize + delta).clamp(0, n as isize - 1) as usize
        };
    }

    /// `enter`: open the row or play it.
    pub fn enter(&mut self) -> RemoteAction {
        let Some(item) = self.rows().get(self.cursor()).map(|i| (*i).clone()) else {
            return RemoteAction::None;
        };
        match item.action {
            ItemAction::Open(path) => {
                self.filter = None;
                self.error = None;
                self.stack.push(Level {
                    path: path.clone(),
                    cursor: 0,
                    offset: 0,
                });
                match self.wanted() {
                    Some(p) => RemoteAction::Browse(p),
                    None => RemoteAction::None,
                }
            }
            ItemAction::Play(req) => RemoteAction::Play(req),
        }
    }

    pub fn at_root(&self) -> bool {
        self.stack.len() == 1
    }

    /// `enter` while typing at the top level: search the whole catalogue
    /// for the typed text instead of filtering.
    pub fn search(&mut self) -> RemoteAction {
        let query = self.filter.take().unwrap_or_default().trim().to_string();
        if query.is_empty() {
            return RemoteAction::None;
        }
        let path = LibraryPath::Search { query };
        // Searches are fresh each time.
        self.cache.retain(|(p, _, _)| *p != path);
        self.error = None;
        self.stack.push(Level {
            path,
            cursor: 0,
            offset: 0,
        });
        match self.wanted() {
            Some(p) => RemoteAction::Browse(p),
            None => RemoteAction::None,
        }
    }

    /// `backspace`/`esc`: clear the filter, else go up a level. Returns
    /// false at the root with no filter (the key is not used).
    pub fn back(&mut self) -> bool {
        if self.filter.take().is_some() {
            self.move_by(0);
            return true;
        }
        if self.stack.len() > 1 {
            self.stack.pop();
            self.error = None;
            return true;
        }
        false
    }

    /// Drops the cached listing for here so it is fetched again.
    pub fn refresh(&mut self) {
        let path = self.path().clone();
        self.cache.retain(|(p, _, _)| *p != path);
        self.error = None;
        self.requested = None;
    }

    pub fn start_filter(&mut self) {
        self.filter = Some(String::new());
        self.level_mut().cursor = 0;
    }

    pub fn type_char(&mut self, c: char) {
        if let Some(f) = self.filter.as_mut() {
            f.push(c);
            self.level_mut().cursor = 0;
        }
    }

    pub fn delete_char(&mut self) {
        if let Some(f) = self.filter.as_mut() {
            if f.pop().is_none() {
                self.filter = None;
            }
            self.level_mut().cursor = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, action: ItemAction) -> LibraryItem {
        LibraryItem {
            title: title.into(),
            subtitle: None,
            count: None,
            duration_secs: None,
            action,
        }
    }

    fn arrive(b: &mut RemoteBrowser, path: LibraryPath, items: Vec<LibraryItem>) {
        b.absorb(Some(&LibraryView {
            path,
            items,
            loading: false,
            error: None,
        }));
    }

    #[test]
    fn fetches_once_opens_plays_and_goes_back_from_cache() {
        let mut b = RemoteBrowser::new();
        assert_eq!(b.wanted(), Some(LibraryPath::Root));
        assert_eq!(b.wanted(), None, "already requested");
        assert!(b.is_loading());
        arrive(
            &mut b,
            LibraryPath::Root,
            vec![
                item("Liked Songs", ItemAction::Open(LibraryPath::LikedSongs)),
                item("Albums", ItemAction::Open(LibraryPath::Albums)),
            ],
        );
        assert_eq!(b.rows().len(), 2);
        b.move_by(1);
        assert_eq!(b.enter(), RemoteAction::Browse(LibraryPath::Albums));
        assert_eq!(b.breadcrumb(), ["Library", "Albums"]);
        let play = PlayRequest {
            context_uri: Some("spotify:album:x".into()),
            ..Default::default()
        };
        arrive(
            &mut b,
            LibraryPath::Albums,
            vec![item("Kid A", ItemAction::Play(play.clone()))],
        );
        assert_eq!(b.enter(), RemoteAction::Play(play));
        assert!(b.back());
        assert_eq!(b.path(), &LibraryPath::Root);
        assert_eq!(b.cursor(), 1, "cursor kept per level");
        assert_eq!(b.wanted(), None, "root still cached");
        assert!(!b.back(), "root with no filter");
    }

    #[test]
    fn filter_narrows_rows_and_back_clears_it() {
        let mut b = RemoteBrowser::new();
        let none = ItemAction::Play(PlayRequest::default());
        arrive(
            &mut b,
            LibraryPath::Root,
            vec![item("Karma Police", none.clone()), item("Nude", none)],
        );
        b.start_filter();
        for c in "nu".chars() {
            b.type_char(c);
        }
        let titles: Vec<&str> = b.rows().iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Nude"]);
        assert!(b.back());
        assert_eq!(b.rows().len(), 2);
    }

    #[test]
    fn enter_at_the_root_searches_the_catalogue() {
        let mut b = RemoteBrowser::new();
        arrive(&mut b, LibraryPath::Root, vec![]);
        b.start_filter();
        for c in "kid a".chars() {
            b.type_char(c);
        }
        let path = LibraryPath::Search {
            query: "kid a".into(),
        };
        assert_eq!(b.search(), RemoteAction::Browse(path.clone()));
        assert_eq!(b.path(), &path);
        assert_eq!(b.filter, None);
        assert!(b.back());
        assert!(b.at_root());
        // An empty search does nothing.
        b.start_filter();
        assert_eq!(b.search(), RemoteAction::None);
    }

    #[test]
    fn errors_stop_refetching_until_refresh() {
        let mut b = RemoteBrowser::new();
        assert!(b.wanted().is_some());
        b.absorb(Some(&LibraryView {
            path: LibraryPath::Root,
            items: Vec::new(),
            loading: false,
            error: Some("nope".into()),
        }));
        assert_eq!(b.error.as_deref(), Some("nope"));
        assert_eq!(b.wanted(), None);
        b.refresh();
        assert_eq!(b.wanted(), Some(LibraryPath::Root));
    }
}
