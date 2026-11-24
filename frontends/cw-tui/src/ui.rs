use crate::app::{App, ArtCache, ColorMode, DISSOLVE_FRAMES, DebugPage, FADE_FRAMES, Mode};
use crate::cover::Slot;
use cw_audio::{Status, Track};
use cw_core::{CellGrid, Repeat, Rgb, ansi256};
use cw_render::{ArtParams, DEFAULT_LOOP_FRAMES, IdleStyle};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Widget, Wrap},
};

const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 12;

const BG_RGB: Rgb = Rgb(14, 15, 18);
const BG: Color = Color::Rgb(BG_RGB.0, BG_RGB.1, BG_RGB.2);
const FG: Color = Color::Rgb(196, 204, 214);
const DIM: Color = Color::Rgb(98, 104, 114);
const BORDER: Color = Color::Rgb(48, 52, 60);
const SELECTED_BG: Color = Color::Rgb(30, 33, 39);
/// Accent used when the playing track has no cover art.
const ACCENT: Color = Color::Rgb(214, 170, 110);
const ERROR: Color = Color::Rgb(206, 120, 110);

fn color(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// The one accent colour: from the shown cover's palette when there is one.
fn accent(app: &App) -> Color {
    app.shown_cover
        .as_ref()
        .map_or(ACCENT, |c| color(c.palette.accent))
}

fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(DIM))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG)), area);

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(f, area);
        return;
    }

    let [main, hud] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    match app.mode {
        Mode::Art => draw_art(f, app, main),
        Mode::Library => draw_library(f, app, main),
    }
    if app.mode == Mode::Art && app.show_lyrics {
        draw_lyrics(f, app, main);
    }
    draw_hud(f, app, hud);
    let bottom = Rect {
        y: main.bottom().saturating_sub(1),
        height: 1.min(main.height),
        ..main
    };
    if let Some(text) = &app.prompt {
        let line = Line::from(vec![
            Span::styled(" Save queue as playlist: ", Style::new().fg(FG)),
            Span::styled(text.clone(), Style::new().fg(accent(app))),
            Span::styled("_", Style::new().fg(DIM)),
            dim("   enter saves, esc cancels"),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::new().bg(SELECTED_BG)),
            bottom,
        );
    } else if let Some(msg) = app.status_text() {
        let line = Line::from(Span::styled(format!(" {msg} "), Style::new().fg(FG)));
        f.render_widget(
            Clear,
            Rect {
                width: (msg.chars().count() as u16 + 2).min(bottom.width),
                ..bottom
            },
        );
        f.render_widget(
            Paragraph::new(line).style(Style::new().bg(SELECTED_BG)),
            Rect {
                width: (msg.chars().count() as u16 + 2).min(bottom.width),
                ..bottom
            },
        );
    }
    match app.debug_page {
        DebugPage::Off => {}
        DebugPage::Stats => draw_debug(f, app, main),
        DebugPage::Recipe => draw_recipe(f, app, main),
    }
    if app.show_help {
        draw_help(f, main);
    }
    if app.color_mode == ColorMode::Ansi256 {
        to_ansi256(f.buffer_mut());
    }
}

/// Rewrites every 24-bit colour in the frame to its nearest xterm-256 index.
fn to_ansi256(buf: &mut Buffer) {
    let map = |c: Color| match c {
        Color::Rgb(r, g, b) => Color::Indexed(ansi256(Rgb(r, g, b))),
        other => other,
    };
    for cell in buf.content.iter_mut() {
        cell.fg = map(cell.fg);
        cell.bg = map(cell.bg);
    }
}

fn draw_too_small(f: &mut Frame, area: Rect) {
    let msg = Paragraph::new(vec![
        Line::from("Terminal too small"),
        Line::from(dim(format!(
            "need {MIN_WIDTH}x{MIN_HEIGHT}, have {}x{}",
            area.width, area.height
        ))),
    ])
    .style(Style::new().fg(FG))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    let y = area.y + area.height.saturating_sub(2) / 2;
    f.render_widget(
        msg,
        Rect {
            y,
            height: area.height.min(2),
            ..area
        },
    );
}

fn draw_art(f: &mut Frame, app: &mut App, area: Rect) {
    let grid = match app.shown_cover.clone() {
        Some(entry) => {
            let fresh = app.art_cache.as_ref().is_some_and(|c| {
                c.entry == entry.id
                    && (c.width, c.height) == (area.width, area.height)
                    && c.style == app.art_style
                    && c.cell_aspect == app.cell_aspect
            });
            if !fresh {
                // Only on cover, recipe, size, style or aspect changes.
                let recipe = &entry.recipe.recipe;
                let ramp = recipe.global.charset.ramp();
                let params = ArtParams {
                    cell_aspect: app.cell_aspect,
                    style: app.art_style,
                    background: BG_RGB,
                    ramp,
                };
                let (w, h) = (area.width, area.height);
                let base = cw_render::base_art(&entry.image, &entry.palette, w, h, params);
                let art = cw_render::art_rect(
                    entry.image.width,
                    entry.image.height,
                    w,
                    h,
                    app.cell_aspect,
                )
                .unwrap_or_default();
                let scene = cw_render::prepare(recipe, base.clone(), art, ramp);
                if app.fade_entry != entry.id {
                    app.fade_entry = entry.id;
                    app.fade_start = app.frame;
                    // Dissolve from whatever was on screen (the last cover
                    // or the idle scene) instead of hard-cutting.
                    app.dissolve = app.prev_art.clone().map(|g| (g, app.frame));
                }
                app.prev_out = None;
                app.art_cache = Some(ArtCache {
                    entry: entry.id,
                    width: w,
                    height: h,
                    style: app.art_style,
                    cell_aspect: app.cell_aspect,
                    base,
                    scene,
                });
            }
            match app.art_cache.as_ref() {
                None => CellGrid::new(0, 0),
                Some(c) if !app.motion => c.base.clone(),
                Some(c) => {
                    let age = app.frame.wrapping_sub(app.fade_start);
                    let t = (age as f32 / FADE_FRAMES as f32).min(1.0);
                    // Eased fade-in, times the pause/resume settle level.
                    let fade = t * t * (3.0 - 2.0 * t) * app.motion_level;
                    let input = cw_render::Input {
                        frame: app.frame,
                        audio: &app.audio,
                        fade,
                    };
                    let out = cw_render::render(&c.scene, input);
                    let out = match app.prev_out.take() {
                        Some(prev) => cw_render::limit_changes(&c.scene, &prev, out),
                        None => out,
                    };
                    let grid = out.grid.clone();
                    app.prev_out = Some(out);
                    grid
                }
            }
        }
        None => cw_render::idle(
            area.width,
            area.height,
            app.frame,
            DEFAULT_LOOP_FRAMES,
            app.idle_seed,
            IdleStyle::default(),
        ),
    };
    let grid = match &app.dissolve {
        Some((from, start)) => {
            let age = app.frame.wrapping_sub(*start);
            if age < DISSOLVE_FRAMES {
                cw_render::dissolve(from, &grid, age as f32 / DISSOLVE_FRAMES as f32)
            } else {
                app.dissolve = None;
                grid
            }
        }
        None => grid,
    };
    let total = (grid.width() as usize * grid.height() as usize).max(1);
    app.last_change_pct = match &app.prev_art {
        Some(prev) => 100.0 * grid.diff_count(prev) as f32 / total as f32,
        None => 100.0,
    };
    f.render_widget(GridWidget(&grid), area);
    app.prev_art = Some(grid);
}

/// Three lyric lines (previous, current, next) centred above the HUD. They
/// are drawn after the art, so motion never runs through them.
fn draw_lyrics(f: &mut Frame, app: &App, main: Rect) {
    let Some(lyrics) = app.current_lyrics() else {
        return;
    };
    let Some(snap) = app.snapshot.as_ref() else {
        return;
    };
    let n = lyrics.lines.len();
    if n == 0 || main.height < 6 {
        return;
    }
    let pos = snap.position_secs + app.lyrics_offset;
    let current = if lyrics.synced {
        lyrics.index_at(pos)
    } else {
        // Unsynced: scroll evenly through the song.
        snap.duration_secs
            .filter(|d| *d > 0.0)
            .map(|d| ((pos / d).clamp(0.0, 0.999) * n as f64) as usize)
    };
    let rows = [
        current.and_then(|i| i.checked_sub(1)),
        current,
        current.map_or(Some(0), |i| (i + 1 < n).then_some(i + 1)),
    ];
    let top = main.bottom().saturating_sub(4);
    for (k, idx) in rows.iter().enumerate() {
        let Some(i) = idx else { continue };
        let text = lyrics.lines[*i].text.trim();
        if text.is_empty() {
            continue;
        }
        let text = fit(text, main.width.saturating_sub(4) as usize);
        let w = text.chars().count() as u16 + 2;
        let area = Rect {
            x: main.x + main.width.saturating_sub(w) / 2,
            y: top + k as u16,
            width: w.min(main.width),
            height: 1,
        };
        let style = if k == 1 {
            Style::new().fg(FG).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(DIM)
        };
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {text} "), style)).style(Style::new().bg(BG)),
            area,
        );
    }
}

/// Copies a `CellGrid` into the ratatui buffer.
struct GridWidget<'a>(&'a CellGrid);

impl Widget for GridWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let g = self.0;
        for y in 0..area.height.min(g.height()) {
            for x in 0..area.width.min(g.width()) {
                let Some(cell) = g.get(x, y) else { continue };
                let target = &mut buf[(area.x + x, area.y + y)];
                target.set_char(cell.ch).set_fg(color(cell.fg));
                if let Some(bg) = cell.bg {
                    target.set_bg(color(bg));
                }
            }
        }
    }
}

fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(BORDER))
        .title(Span::styled(format!(" {title} "), Style::new().fg(FG)))
        .style(Style::new().bg(BG).fg(FG))
        .padding(Padding::new(1, 1, 0, 0))
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Truncates to `width` columns, ending with "..." when cut.
fn fit(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count <= width {
        s.to_string()
    } else if width <= 3 {
        s.chars().take(width).collect()
    } else {
        let mut out: String = s.chars().take(width - 3).collect();
        out.push_str("...");
        out
    }
}

fn track_label(t: &Track) -> String {
    match &t.info.artist {
        Some(a) => format!("{} - {a}", t.info.title),
        None => t.info.title.clone(),
    }
}

/// The Spotify library: a breadcrumb, the current listing (filterable),
/// and what Spotify is playing.
fn draw_remote_library(f: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area);
    let accent = accent(app);
    let crumbs = app.remote_browser.breadcrumb().join(" / ");
    let title = match &app.remote_browser.filter {
        Some(q) => format!("Spotify / {crumbs}   / {q}_"),
        None => format!("Spotify / {crumbs}"),
    };
    let block = panel(&title);
    let inner = block.inner(left);
    f.render_widget(block, left);

    let state = app.sources.state.clone();
    let auth = state.as_ref().map(|s| s.auth.clone());
    let message = match auth {
        Some(cw_sources::Auth::NeedsLogin) => Some(("Press L to log in to Spotify.", FG)),
        Some(cw_sources::Auth::LoggingIn) => Some(("Finish logging in in your browser.", FG)),
        None => Some(("Connecting to Spotify.", DIM)),
        Some(cw_sources::Auth::Ready) => None,
    };
    let rows_h = inner.height as usize;
    let width = inner.width as usize;
    if let Some((msg, color)) = message {
        f.render_widget(
            Paragraph::new(Span::styled(msg, Style::new().fg(color))),
            inner,
        );
    } else if let Some(err) = app.remote_browser.error.clone() {
        let p = Paragraph::new(vec![
            Line::from(Span::styled(err, Style::new().fg(ERROR))),
            Line::from(""),
            Line::from(dim("r tries again, backspace goes back")),
        ])
        .wrap(Wrap { trim: false });
        f.render_widget(p, inner);
    } else if app.remote_browser.rows().is_empty() {
        let msg = if app.remote_browser.is_loading() {
            "loading from Spotify"
        } else if app.remote_browser.filter.is_some() {
            "nothing matches"
        } else {
            "nothing here"
        };
        f.render_widget(Paragraph::new(dim(msg)), inner);
    } else {
        let offset = app.remote_browser.scroll(rows_h);
        let cursor = app.remote_browser.cursor();
        let side_w = 8usize;
        let lines: Vec<Line> = app
            .remote_browser
            .rows()
            .into_iter()
            .enumerate()
            .skip(offset)
            .take(rows_h)
            .map(|(i, item)| {
                let selected = i == cursor;
                let side = match (item.duration_secs, item.count) {
                    (Some(d), _) => fmt_time(d as f64),
                    (None, Some(n)) => format!("{n}"),
                    _ => String::new(),
                };
                let opens = matches!(item.action, cw_sources::ItemAction::Open(_));
                let marker = if opens { "> " } else { "  " };
                let main_w = width.saturating_sub(side_w + 2);
                let title = fit(&item.title, main_w);
                let rest = main_w.saturating_sub(title.chars().count());
                let sub = item
                    .subtitle
                    .as_deref()
                    .filter(|_| rest > 6)
                    .map(|s| fit(&format!("  {s}"), rest))
                    .unwrap_or_default();
                let pad = main_w.saturating_sub(title.chars().count() + sub.chars().count());
                let bg = if selected { SELECTED_BG } else { BG };
                let title_style = if item.title == "Play all" {
                    Style::new().fg(accent)
                } else if selected {
                    Style::new().fg(FG).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(FG)
                };
                Line::from(vec![
                    Span::styled(marker, Style::new().fg(accent).bg(bg)),
                    Span::styled(title, title_style.bg(bg)),
                    Span::styled(sub, Style::new().fg(DIM).bg(bg)),
                    Span::styled(" ".repeat(pad), Style::new().bg(bg)),
                    Span::styled(format!("{side:>side_w$}"), Style::new().fg(DIM).bg(bg)),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    // Right: what Spotify is playing, and the keys for this view.
    let block = panel("Now playing");
    let inner = block.inner(right);
    f.render_widget(block, right);
    let now = state.as_ref().and_then(|s| s.now.clone());
    let w = inner.width as usize;
    let mut lines = match &now {
        Some(n) => vec![
            Line::from(Span::styled(
                fit(&n.title, w),
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            )),
            Line::from(dim(fit(n.artist.as_deref().unwrap_or(""), w))),
            Line::from(dim(fit(n.album.as_deref().unwrap_or(""), w))),
        ],
        None => vec![Line::from(dim("nothing playing yet"))],
    };
    if let Some(e) = state.as_ref().and_then(|s| s.error.clone()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(fit(&e, w), Style::new().fg(ERROR))));
    }
    lines.push(Line::from(""));
    for (k, what) in [
        ("enter", "open / play"),
        ("bksp", "back"),
        ("/", "filter"),
        ("r", "refresh"),
        ("space", "play / pause"),
        ("n  p", "next / previous"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("{k:>6}  "), Style::new().fg(FG)),
            dim(what),
        ]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_library(f: &mut Frame, app: &mut App, area: Rect) {
    if app.browsing_remote() {
        draw_remote_library(f, app, area);
        return;
    }
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area);

    let title = match app.browser.levels.len() {
        1 if !app.scan_status.is_empty() => format!("Library · {}", app.scan_status),
        _ => app.browser.current().title.clone(),
    };
    let block = panel(&title);
    let inner = block.inner(left);
    f.render_widget(block, left);
    let playing = app.snapshot.as_ref().and_then(|s| s.track);
    let accent = accent(app);
    draw_browser(f, app, inner);
    // Queue: the playing track, then what follows in play order.
    let block = panel("Queue");
    let inner = block.inner(right);
    f.render_widget(block, right);
    let (Some(snap), Some(player)) = (app.snapshot.as_ref(), app.player.as_ref()) else {
        return;
    };
    let tracks = player.tracks();
    let mut lines = Vec::new();
    let start = playing
        .and_then(|t| snap.order.iter().position(|&o| o == t))
        .unwrap_or(0);
    let width = inner.width as usize;
    for (i, &t) in snap
        .order
        .iter()
        .skip(start)
        .take(inner.height as usize)
        .enumerate()
    {
        let label = fit(&track_label(&tracks[t]), width.saturating_sub(2));
        if i == 0 && playing.is_some() {
            lines.push(Line::from(vec![
                Span::styled("> ", Style::new().fg(accent)),
                Span::styled(label, Style::new().fg(FG).add_modifier(Modifier::BOLD)),
            ]));
        } else {
            lines.push(Line::from(vec![dim("  "), dim(label)]));
        }
    }
    if let Some(err) = &snap.last_error
        && (inner.height as usize) > lines.len() + 1
    {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            fit(err, width),
            Style::new().fg(ERROR),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// The library browser: the current level's entries, a search line when
/// searching, and hints when there is nothing to show.
fn draw_browser(f: &mut Frame, app: &mut App, area: Rect) {
    let accent = accent(app);
    let playing = app.current_path();
    let searching = app.browser.in_search();
    let mut area = area;
    if searching {
        let line = Line::from(vec![
            Span::styled("/ ", Style::new().fg(accent)),
            Span::styled(app.browser.query.clone(), Style::new().fg(FG)),
            Span::styled("_", Style::new().fg(DIM)),
        ]);
        f.render_widget(Paragraph::new(line), Rect { height: 1, ..area });
        area.y += 2;
        area.height = area.height.saturating_sub(2);
    }
    let rows = area.height as usize;
    let level = app.browser.levels.last_mut().expect("root level");
    if level.entries.is_empty() {
        let hint: Vec<Line> = if searching {
            vec![Line::from(dim(if app.browser.query.is_empty() {
                "Type to search titles, artists and albums."
            } else {
                "No matches."
            }))]
        } else if app.library.is_none() {
            vec![
                Line::from("No library database."),
                Line::from(dim("Files given on the command line still play.")),
            ]
        } else {
            vec![
                Line::from("Nothing here yet."),
                Line::from(""),
                Line::from(dim("coverwave indexes the folders in")),
                Line::from(Span::styled(
                    "  ~/.config/coverwave/config.toml",
                    Style::new().fg(FG),
                )),
                Line::from(dim("(music_dirs, default ~/Music). Or start it with")),
                Line::from(Span::styled(
                    "  cw-tui ~/Music/Some\\ Album",
                    Style::new().fg(FG),
                )),
            ]
        };
        f.render_widget(Paragraph::new(hint).wrap(Wrap { trim: false }), area);
        return;
    }
    if rows == 0 {
        return;
    }
    // Keep the cursor visible, scrolling as little as possible.
    if level.cursor < level.offset {
        level.offset = level.cursor;
    } else if level.cursor >= level.offset + rows {
        level.offset = level.cursor + 1 - rows;
    }
    let width = area.width as usize;
    let detail_w = level
        .entries
        .iter()
        .skip(level.offset)
        .take(rows)
        .map(|e| e.detail.chars().count())
        .max()
        .unwrap_or(0)
        .min(width / 3);
    let lines: Vec<Line> = level
        .entries
        .iter()
        .enumerate()
        .skip(level.offset)
        .take(rows)
        .map(|(i, e)| {
            let is_playing = match e.kind {
                crate::browse::EntryKind::Track(t) => {
                    playing.as_ref().is_some_and(|p| *p == level.tracks[t].path)
                }
                _ => false,
            };
            let marker = match (&e.kind, is_playing) {
                (_, true) => Span::styled("> ", Style::new().fg(accent)),
                (crate::browse::EntryKind::Open(_), _) => dim("+ "),
                _ => Span::raw("  "),
            };
            let label_w = width.saturating_sub(3 + detail_w);
            let label = format!("{:<label_w$}", fit(&e.label, label_w));
            let mut style = Style::new().fg(FG);
            if i == level.cursor {
                style = style.bg(SELECTED_BG);
            }
            Line::from(vec![
                marker,
                Span::styled(label, style),
                Span::styled(
                    format!(" {:>detail_w$}", fit(&e.detail, detail_w)),
                    style.fg(DIM),
                ),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_hud(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Block::new().style(Style::new().bg(BG)), area);
    if app.sources.is_remote() {
        draw_remote_hud(f, app, area);
        return;
    }

    let key = |k: &'static str| Span::styled(k, Style::new().fg(FG).add_modifier(Modifier::BOLD));
    let accent = accent(app);
    let snap = app.snapshot.as_ref();
    let current = snap
        .zip(app.player.as_ref())
        .and_then(|(s, p)| s.track.map(|t| (s, &p.tracks()[t])));

    let Some((snap, track)) = current else {
        let left = match &app.audio_error {
            Some(e) => Span::styled(format!(" {e}"), Style::new().fg(ERROR)),
            None => dim(" nothing playing"),
        };
        let hints = Line::from(vec![
            key("tab"),
            dim(" library   "),
            key("?"),
            dim(" keys   "),
            key("q"),
            dim(" quit "),
        ]);
        let w = hints.width() as u16;
        let [l, r] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(w)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(left)), l);
        f.render_widget(Paragraph::new(hints), r);
        return;
    };

    // Right side: progress, time, volume, modes.
    let pos = snap.position_secs;
    let time = match snap.duration_secs {
        Some(d) => format!("{} / {}", fmt_time(pos), fmt_time(d)),
        None => fmt_time(pos),
    };
    let bar_w = (area.width / 5).clamp(10, 40) as usize;
    let bar = progress_bar(pos, snap.duration_secs, bar_w);
    let mut right = vec![
        Span::styled(bar.0, Style::new().fg(accent)),
        dim(bar.1),
        Span::raw("  "),
        Span::styled(time, Style::new().fg(FG)),
        dim(format!("   vol {:>3}", snap.volume)),
    ];
    if snap.shuffle {
        right.push(Span::styled("  shuffle", Style::new().fg(FG)));
    }
    match snap.repeat {
        Repeat::Off => {}
        Repeat::All => right.push(Span::styled("  repeat", Style::new().fg(FG))),
        Repeat::One => right.push(Span::styled("  repeat one", Style::new().fg(FG))),
    }
    right.push(Span::raw(" "));
    let right = Line::from(right);
    let right_w = right.width() as u16;

    let glyph = match snap.status {
        Status::Playing => Span::styled(" > ", Style::new().fg(accent)),
        Status::Paused => Span::styled(" ||", Style::new().fg(FG)),
        Status::Stopped => dim(" []"),
    };
    let [l, r] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
    let title_w = (l.width as usize).saturating_sub(5);
    let mut left = vec![glyph, Span::raw(" ")];
    let title = fit(&track.info.title, title_w);
    let rest = title_w.saturating_sub(title.chars().count());
    left.push(Span::styled(
        title,
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    ));
    if let Some(artist) = &track.info.artist
        && rest > 6
    {
        left.push(dim(fit(&format!("  {artist}"), rest)));
    }
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(right), r);
}

/// The HUD while following Spotify or Apple Music.
fn draw_remote_hud(f: &mut Frame, app: &App, area: Rect) {
    let accent = accent(app);
    let name = app.sources.active.name();
    let tag = Span::styled(format!(" {name} "), Style::new().fg(BG).bg(DIM));
    let state = app.sources.state.as_ref();
    let Some((state, now)) = state.and_then(|s| s.now.as_ref().map(|n| (s, n))) else {
        let msg = match state.map(|s| (&s.auth, &s.error)) {
            Some((cw_sources::Auth::NeedsLogin, _)) => format!("{name}: press L to log in"),
            Some((cw_sources::Auth::LoggingIn, _)) => {
                "finish logging in in your browser".to_string()
            }
            Some((_, Some(e))) => e.clone(),
            Some(_) => format!("nothing playing in {name}"),
            None => "connecting".to_string(),
        };
        let line = Line::from(vec![
            Span::raw(" "),
            tag,
            Span::raw("  "),
            dim(fit(&msg, area.width.saturating_sub(16) as usize)),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    };
    let pos = state.position_now(std::time::Instant::now());
    let time = match now.duration_secs {
        Some(d) => format!("{} / {}", fmt_time(pos), fmt_time(d)),
        None => fmt_time(pos),
    };
    let bar_w = (area.width / 5).clamp(10, 40) as usize;
    let bar = progress_bar(pos, now.duration_secs, bar_w);
    let mut right = vec![
        Span::styled(bar.0, Style::new().fg(accent)),
        dim(bar.1),
        Span::raw("  "),
        Span::styled(time, Style::new().fg(FG)),
    ];
    if let Some(v) = now.volume {
        right.push(dim(format!("   vol {v:>3}")));
    }
    right.push(Span::raw(" "));
    let right = Line::from(right);
    let right_w = right.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
    let glyph = if now.playing {
        Span::styled(" > ", Style::new().fg(accent))
    } else {
        Span::styled(" ||", Style::new().fg(FG))
    };
    let title_w = (l.width as usize).saturating_sub(name.len() + 8);
    let title = fit(&now.title, title_w);
    let rest = title_w.saturating_sub(title.chars().count());
    let mut left = vec![glyph, Span::raw(" "), tag, Span::raw(" ")];
    left.push(Span::styled(
        title,
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    ));
    if let Some(artist) = &now.artist
        && rest > 6
    {
        left.push(dim(fit(&format!("  {artist}"), rest)));
    }
    if let Some(e) = &state.error
        && l.width > 60
    {
        left.push(Span::styled(
            format!("  {}", fit(e, 30)),
            Style::new().fg(ERROR),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(right), r);
}

/// Returns (played, remaining) parts of an ASCII progress bar.
fn progress_bar(pos: f64, duration: Option<f64>, width: usize) -> (String, String) {
    let frac = duration
        .filter(|d| *d > 0.0)
        .map_or(0.0, |d| (pos / d).clamp(0.0, 1.0));
    let filled = ((frac * width as f64).round() as usize).min(width);
    ("=".repeat(filled), "-".repeat(width - filled))
}

fn draw_debug(f: &mut Frame, app: &App, main: Rect) {
    let mut lines = vec![
        Line::from(format!("fps     {:>5.1} / {}", app.measured_fps(), app.fps)),
        Line::from(format!(
            "frame   {:>3} / {}",
            app.frame % DEFAULT_LOOP_FRAMES,
            DEFAULT_LOOP_FRAMES
        )),
        Line::from(format!("size    {}x{}", main.width, main.height)),
        Line::from(format!(
            "changed {:>5.1}%",
            if app.mode == Mode::Art {
                app.last_change_pct
            } else {
                0.0
            }
        )),
    ];
    if app.sources.is_remote() {
        let state = app.sources.state.as_ref();
        let status = match state.and_then(|s| s.now.as_ref()) {
            Some(n) if n.playing => "Playing",
            Some(_) => "Paused",
            None => "Stopped",
        };
        let pos = state.map_or(0.0, |s| s.position_now(std::time::Instant::now()));
        lines.push(Line::from(format!("status  {status}")));
        lines.push(Line::from(format!("pos     {pos:.2}s")));
    } else if let Some(s) = &app.snapshot {
        lines.push(Line::from(format!("status  {:?}", s.status)));
        lines.push(Line::from(format!("pos     {:.2}s", s.position_secs)));
    }
    if let Some(log) = &app.stderr_log {
        // Native messages that would have scribbled on the screen go here.
        let name = log
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        lines.push(Line::from(format!("stderr  {name}")));
    }
    lines.push(Line::from(format!(
        "style   {} / {}",
        app.art_style.name(),
        app.color_mode.name()
    )));
    lines.push(Line::from(format!(
        "cell    {:.2} {}",
        app.cell_aspect,
        if app.cell_aspect_measured {
            "measured"
        } else {
            "assumed"
        }
    )));
    let playing = app.current_path();
    let slot = if app.sources.is_remote() {
        app.shown_cover.clone().map(crate::cover::Slot::Ready)
    } else {
        playing.as_deref().and_then(|p| app.covers.get(p)).cloned()
    };
    let cover = match slot.as_ref() {
        None => "-".to_string(),
        Some(Slot::Loading) => "loading".to_string(),
        Some(Slot::Missing(why)) => why.clone(),
        Some(Slot::Ready(c)) => {
            format!("{}x{} {:08x}", c.image.width, c.image.height, c.seed >> 32)
        }
    };
    lines.push(Line::from(format!("cover   {}", fit(&cover, 18))));
    lines.push(Line::from(format!("source  {}", app.sources.active.name())));
    if app.sources.is_remote() {
        lines.push(Line::from(format!(
            "capture {}",
            fit(&app.sources.capture_description(), 20)
        )));
    }
    if let Some(c) = &app.shown_cover {
        let mut swatches = vec![Span::raw("palette ")];
        for &col in &c.palette.colors {
            swatches.push(Span::styled("  ", Style::new().bg(color(col))));
        }
        swatches.push(Span::raw(" "));
        swatches.push(Span::styled("  ", Style::new().bg(color(c.palette.accent))));
        lines.push(Line::from(swatches));
    }
    lines.push(Line::from(""));
    lines.extend(audio_lines(app));
    draw_overlay(f, main, "debug", lines);
}

/// A titled panel in the top-right corner of `main`.
fn draw_overlay(f: &mut Frame, main: Rect, title: &str, lines: Vec<Line>) {
    let w = OVERLAY_W.min(main.width);
    let h = (lines.len() as u16 + 2).min(main.height);
    let area = Rect {
        x: main.right().saturating_sub(w + 1),
        y: main.y + 1,
        width: w,
        height: h,
    };
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(FG))
            .block(panel(title)),
        area,
    );
}

const OVERLAY_W: u16 = 32;
/// Text width inside an overlay (borders and padding take 4).
const OVERLAY_TEXT_W: usize = OVERLAY_W as usize - 4;

/// The second `D` page: the shown cover's motion recipe.
fn draw_recipe(f: &mut Frame, app: &App, main: Rect) {
    let mut lines = vec![Line::from(format!(
        "model   {}",
        fit(&app.covers.model().status().describe(), OVERLAY_TEXT_W - 8)
    ))];
    let Some(entry) = &app.shown_cover else {
        lines.push(Line::from(dim("no cover shown")));
        draw_overlay(f, main, "recipe", lines);
        return;
    };
    let loaded = &entry.recipe;
    let r = &loaded.recipe;
    lines.push(Line::from(format!(
        "source  {}{}",
        loaded.source.name(),
        if app.motion { "" } else { ", motion off" }
    )));
    if let Some(w) = &loaded.warning {
        lines.push(Line::from(Span::styled(
            fit(w, OVERLAY_TEXT_W),
            Style::new().fg(ERROR),
        )));
    }
    lines.push(Line::from(dim(fit(&r.cover_hash, OVERLAY_TEXT_W))));

    lines.push(Line::from(""));
    let mut scenes: Vec<(&String, &f32)> = r.scene_weights.iter().collect();
    scenes.sort_by(|a, b| b.1.total_cmp(a.1));
    if scenes.is_empty() {
        lines.push(Line::from(dim("scenes  (none)")));
    }
    for (name, w) in scenes.iter().take(3) {
        lines.push(meter(&fit(name, 7), **w, FG));
    }

    lines.push(Line::from(""));
    for l in &r.layers {
        lines.push(meter(&fit(l.kind(), 7), l.intensity(), accent(app)));
    }

    lines.push(Line::from(""));
    let g = &r.global;
    lines.push(Line::from(format!(
        "speed   {:.2}  sway {:.2}",
        g.speed, g.sway
    )));
    lines.push(Line::from(format!(
        "density {:.2}  wind {:.0}",
        g.density, g.wind_deg
    )));
    lines.push(Line::from(
        format!("chars   {:?}", g.charset).to_lowercase(),
    ));
    lines.push(Line::from(format!(
        "energy  {:.2}  mask {:.0}%",
        r.features.energy,
        100.0 * r.subject_mask.coverage()
    )));
    for half in r.style_dna.chunks(4) {
        let vals: Vec<String> = half.iter().map(|d| format!("{d:+.1}")).collect();
        let label = if lines
            .last()
            .is_some_and(|l| l.to_string().starts_with("dna"))
        {
            "       "
        } else {
            "dna    "
        };
        lines.push(Line::from(format!("{label} {}", vals.join(" "))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(dim("R reloads (overrides too)")));
    draw_overlay(f, main, "recipe", lines);
}

/// Width of the analysis bars in the debug overlay.
const METER_W: usize = 18;

/// One meter row: label, then `#` for the value and `-` for the rest.
fn meter(label: &str, v: f32, fill: Color) -> Line<'static> {
    let n = ((v.clamp(0.0, 1.0) * METER_W as f32).round() as usize).min(METER_W);
    Line::from(vec![
        Span::raw(format!("{label:<8}")),
        Span::styled("#".repeat(n), Style::new().fg(fill)),
        dim("-".repeat(METER_W - n)),
    ])
}

/// Live analysis for the debug overlay: six bands, level, and onsets.
fn audio_lines(app: &App) -> Vec<Line<'static>> {
    let a = &app.audio;
    let accent = accent(app);
    let mut lines: Vec<Line> = cw_core::audio::BAND_NAMES
        .iter()
        .zip(a.bands)
        .map(|(name, v)| meter(name, v, FG))
        .collect();
    lines.push(meter("level", a.level, FG));
    let pulse = (a.onset * METER_W as f32).round() as usize;
    lines.push(Line::from(vec![
        Span::raw("onset   "),
        Span::styled("*".repeat(pulse.min(METER_W)), Style::new().fg(accent)),
        dim(format!(
            "{:>w$}",
            a.onset_count,
            w = METER_W.saturating_sub(pulse.min(METER_W))
        )),
    ]));
    lines
}

const KEYS: &[(&str, &str)] = &[
    ("space", "play / pause"),
    ("n  p", "next / previous"),
    ("<-  ->", "seek 5s (shift: 30s)"),
    ("+  -", "volume"),
    ("s", "shuffle"),
    ("r", "repeat: off, all, one"),
    ("tab", "art / library"),
    ("/", "search (enter plays, tab adds)"),
    ("enter", "open, or play from this track"),
    ("a", "add to queue"),
    ("bksp esc", "back"),
    ("P", "save queue as playlist"),
    ("S", "source: local, Spotify, Apple Music"),
    ("L", "log in to Spotify"),
    ("l", "lyrics on / off"),
    ("[  ]", "lyrics earlier / later"),
    ("c", "cover style (palette, true, duo)"),
    ("m", "motion on / off"),
    ("up  down", "move in library (or j k)"),
    ("D", "debug overlay: stats, recipe, off"),
    ("R", "reload the cover's recipe"),
    ("ctrl-l", "redraw the screen"),
    ("?", "close this help"),
    ("q", "quit"),
];

fn draw_help(f: &mut Frame, main: Rect) {
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(
                    format!("{k:>9}  "),
                    Style::new().fg(FG).add_modifier(Modifier::BOLD),
                ),
                dim(*what),
            ])
        })
        .collect();
    let w = 48.min(main.width);
    let h = (lines.len() as u16 + 2).min(main.height);
    let area = Rect {
        x: main.x + (main.width - w) / 2,
        y: main.y + (main.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(panel("keys")), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formatting() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(65.9), "1:05");
        assert_eq!(fmt_time(3725.0), "1:02:05");
    }

    #[test]
    fn fit_truncates_with_dots() {
        assert_eq!(fit("hello", 10), "hello");
        assert_eq!(fit("hello world", 8), "hello...");
        assert_eq!(fit("hello", 2), "he");
    }

    #[test]
    fn progress_bar_widths() {
        assert_eq!(
            progress_bar(5.0, Some(10.0), 10),
            ("=====".into(), "-----".into())
        );
        assert_eq!(progress_bar(5.0, None, 4), ("".into(), "----".into()));
        assert_eq!(
            progress_bar(50.0, Some(10.0), 4),
            ("====".into(), "".into())
        );
    }
}
