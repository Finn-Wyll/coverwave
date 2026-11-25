//! Lyrics in place of the cover: a square the size of the cover's art,
//! centred where the cover was. The words become the art: [`lyric_base`]
//! draws them into the square each frame and [`lyric_scene`] runs the
//! cover's own recipe over that, so every effect (light, tint, sway,
//! ripple, glitch, particles) acts on the letters as it did on the cover.
//!
//! - [`LyricStyle::BigCurrent`]: the sung line in large letters, the lines
//!   around it as text fading with distance.
//! - [`LyricStyle::Text`]: every line as text, the sung one in the accent.
//! - [`LyricStyle::BigOnly`]: only the sung line, large and centred.
//!
//! Large letters are drawn from a real typeface (Space Mono Bold, SIL Open
//! Font License, `assets/fonts/`), rasterised with `fontdue` and sampled
//! into the grid: each cell's ink coverage picks a dense glyph in the
//! letter's core and lighter ones on its edges. Their height is a share of
//! the square ([`LyricSize`]), so a finer grid draws the same letters with
//! more, smaller characters: more detail, same size.
//!
//! Lines scroll up (new ones rise from below, like Spotify) or down. The
//! glide between lines takes [`GLIDE_SECS`] and is a pure function of the
//! playback position, so frames stay deterministic and seeks just work.
//! The audio only brightens the sung line on beats (amplitude, never time).

use crate::Rect;
use cw_core::lyrics::Lyrics;
use cw_core::{AudioFrame, Cell, CellGrid, Rgb};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LyricStyle {
    BigCurrent,
    Text,
    #[default]
    BigOnly,
}

impl LyricStyle {
    pub fn from_name(s: &str) -> Option<LyricStyle> {
        match s.trim().to_ascii_lowercase().as_str() {
            "big" | "big_current" => Some(LyricStyle::BigCurrent),
            "text" => Some(LyricStyle::Text),
            "big_only" => Some(LyricStyle::BigOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LyricScroll {
    /// New lines rise from below; the sung line sits above the middle.
    #[default]
    Up,
    /// New lines come down from above.
    Down,
}

impl LyricScroll {
    pub fn from_name(s: &str) -> Option<LyricScroll> {
        match s.trim().to_ascii_lowercase().as_str() {
            "up" => Some(LyricScroll::Up),
            "down" => Some(LyricScroll::Down),
            _ => None,
        }
    }
}

/// How tall the large letters are, as a share of the square.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LyricSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl LyricSize {
    pub fn from_name(s: &str) -> Option<LyricSize> {
        match s.trim().to_ascii_lowercase().as_str() {
            "small" => Some(LyricSize::Small),
            "medium" => Some(LyricSize::Medium),
            "large" => Some(LyricSize::Large),
            _ => None,
        }
    }

    /// Capital height over square height.
    fn fraction(self) -> f32 {
        match self {
            LyricSize::Small => 0.075,
            LyricSize::Medium => 0.11,
            LyricSize::Large => 0.16,
        }
    }
}

/// Where the lyrics' colours come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LyricPaint {
    /// The cover's accent for the sung line, a light palette colour for the
    /// rest.
    #[default]
    Accent,
    /// The cover itself: each letter shows the cover's colour where it
    /// stands, as if the lyrics were windows onto the art.
    Cover,
}

impl LyricPaint {
    pub fn from_name(s: &str) -> Option<LyricPaint> {
        match s.trim().to_ascii_lowercase().as_str() {
            "accent" => Some(LyricPaint::Accent),
            "cover" | "window" => Some(LyricPaint::Cover),
            _ => None,
        }
    }
}

/// How the lyrics look; all are user settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LyricView {
    pub style: LyricStyle,
    pub scroll: LyricScroll,
    pub size: LyricSize,
    pub paint: LyricPaint,
}

/// The cover the lyrics stand in for: its still art and this frame of its
/// motion, cell for cell on the same grid. The lyrics take their light from
/// it: every tint, flicker, sweep, glow or shadow the recipe puts on the
/// cover falls on the letters in the same place.
#[derive(Debug, Clone, Copy)]
pub struct CoverLight<'a> {
    pub base: &'a CellGrid,
    pub frame: &'a CellGrid,
}

/// Marks a cell covered by the wide character before it (CJK, emoji):
/// frontends draw nothing there.
pub const WIDE_CONTINUATION: char = '\0';

/// Colours from the cover: its accent (the sung line), a light palette
/// colour (other lines) and the page background (what edges fade to).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LyricColors {
    pub accent: Rgb,
    pub text: Rgb,
    pub background: Rgb,
}

/// How long a new line takes to glide into place.
pub const GLIDE_SECS: f64 = 0.35;
/// Columns kept clear inside each side of the square.
const MARGIN: u16 = 2;
/// Sub-samples per cell height when rasterising (width follows the cell
/// aspect).
const SUB: f32 = 8.0;
/// Space Mono's capital height, as a share of its em.
const CAP_HEIGHT: f32 = 0.66;

/// The art with the lyrics in it, for one moment: the cover's square
/// cleared to the page and the lines drawn in, coloured from the still
/// cover. Prepare a scene over it with [`lyric_scene`] and render that: the
/// recipe then animates the letters like the cover itself.
#[allow(clippy::too_many_arguments)]
pub fn lyric_base(
    cover_base: &CellGrid,
    art: Rect,
    cell_aspect: f32,
    lyrics: &Lyrics,
    position: f64,
    duration: Option<f64>,
    view: LyricView,
    colors: LyricColors,
    audio: &AudioFrame,
) -> CellGrid {
    let mut grid = blank_square(cover_base, art, colors.background);
    let still = CoverLight {
        base: cover_base,
        frame: cover_base,
    };
    overlay(
        &mut grid,
        art,
        cell_aspect,
        lyrics,
        position,
        duration,
        view,
        colors,
        audio,
        Some(still),
    );
    grid
}

/// The cover's recipe over a [`lyric_base`]: nothing protected, so every
/// layer (light, tint, sway, ripple, glitch, particles) works on the words.
/// Cheap enough to make each frame (about 0.1ms for a usual recipe at
/// 240x70). Render it without `limit_changes`, which would hold back the
/// scrolling lines.
pub fn lyric_scene(
    recipe: &cw_core::Recipe,
    base: CellGrid,
    art: Rect,
    ramp: &[char],
) -> crate::Scene {
    let mut open = recipe.clone();
    open.subject_mask = cw_core::recipe::Mask::from_bits(0, 0, &[]);
    crate::prepare(&open, base, art, ramp)
}

/// `base` with the cover's square cleared to the page.
pub fn blank_square(base: &CellGrid, art: Rect, background: Rgb) -> CellGrid {
    let mut out = base.clone();
    let bg = base.cells().first().and_then(|c| c.bg);
    for y in art.y..art.y + art.h {
        for x in art.x..art.x + art.w {
            out.set(
                x,
                y,
                Cell {
                    ch: ' ',
                    fg: background,
                    bg,
                },
            );
        }
    }
    out
}

/// Draws `lyrics` at `position` (seconds into the track; `duration` paces
/// unsynced lyrics) inside `art`, the cover's square on `grid`.
/// `cell_aspect` is a cell's width over its height.
#[allow(clippy::too_many_arguments)]
pub fn overlay(
    grid: &mut CellGrid,
    art: Rect,
    cell_aspect: f32,
    lyrics: &Lyrics,
    position: f64,
    duration: Option<f64>,
    view: LyricView,
    colors: LyricColors,
    audio: &AudioFrame,
    cover: Option<CoverLight<'_>>,
) {
    if lyrics.lines.is_empty() || art.w < 12 || art.h < 6 {
        return;
    }
    let painter = Painter {
        cover,
        paint: view.paint,
        background: colors.background,
    };
    let (current, glide) = progress(lyrics, position, duration);
    let inner = art.w.saturating_sub(2 * MARGIN).max(1);
    // Fractional on purpose: it only sets the font's pixel size, so letters
    // keep the same share of the square at every grid size.
    let cap_rows = (view.size.fraction() * art.h as f32).max(3.0);
    let font = BigFont::new(cap_rows, cell_aspect);

    let shown: Vec<usize> = match view.style {
        LyricStyle::BigOnly => current.into_iter().collect(),
        _ => (0..lyrics.lines.len()).collect(),
    };
    let blocks: Vec<(usize, Vec<Row>)> = shown
        .iter()
        .map(|&i| {
            let text = lyrics.lines[i].text.trim();
            let big = Some(i) == current && view.style != LyricStyle::Text && font.can_draw(text);
            let rows = if big {
                font.rows(text, inner)
            } else {
                text_rows(text, inner)
            };
            (i, rows)
        })
        .collect();
    let height = |b: &(usize, Vec<Row>)| b.1.iter().map(Row::height).sum::<u16>() + 1;

    // Where a line's block starts, counted from the first block.
    let top_of = |idx: Option<usize>| -> i32 {
        match idx.and_then(|i| blocks.iter().position(|b| b.0 == i)) {
            Some(k) => blocks[..k].iter().map(|b| height(b) as i32).sum(),
            // Before the first line: as if a line before it were sung.
            None => -2,
        }
    };
    let now = top_of(current);
    let before = match current {
        Some(i) if i > 0 => top_of(Some(i - 1)),
        _ => -2,
    };
    let eased = glide * glide * (3.0 - 2.0 * glide);
    let offset = (before as f32 + (now - before) as f32 * eased).round() as i32;
    let total: i32 = blocks.iter().map(|b| height(b) as i32).sum();

    let anchor = art.y as i32
        + match (view.style, view.scroll) {
            (LyricStyle::BigOnly, _) => {
                let hb = blocks.first().map_or(0, |b| height(b) as i32);
                (art.h as i32 - hb) / 2
            }
            (_, LyricScroll::Up) => (art.h as f32 * 0.3) as i32,
            (_, LyricScroll::Down) => (art.h as f32 * 0.5) as i32,
        };
    let beat = (0.6 * audio.onset + 0.4 * audio.bass()).clamp(0.0, 1.0);
    let clip = (art.y as i32, (art.y + art.h) as i32);

    let mut run = 0i32;
    let order: Vec<&(usize, Vec<Row>)> = match view.scroll {
        LyricScroll::Up => blocks.iter().collect(),
        // Down: later lines above earlier ones.
        LyricScroll::Down => blocks.iter().rev().collect(),
    };
    for block in order {
        let (i, rows) = block;
        let hb = height(block) as i32;
        let top = match view.scroll {
            LyricScroll::Up => anchor + run - offset,
            // Mirrored: the sung block's top stays at the anchor.
            LyricScroll::Down => anchor + run - (total - offset - hb),
        };
        run += hb;
        if top >= clip.1 || top + hb <= clip.0 {
            continue;
        }
        let distance = match current {
            Some(c) => (*i as i32 - c as i32).unsigned_abs(),
            None => *i as u32 + 1,
        };
        let sung = Some(*i) == current;
        let tone = Tone {
            color: if sung { colors.accent } else { colors.text },
            fade: if sung {
                0.0
            } else {
                (0.2 * distance as f32).min(0.85)
            },
            lift: if sung { 0.25 * beat } else { 0.0 },
        };
        let mut y = top;
        for row in rows {
            row.draw(grid, art, y, clip, tone, &painter);
            y += row.height() as i32;
        }
    }
}

/// The sung line and how far its glide has got (0..1). Unsynced lyrics
/// move through the song at an even pace.
fn progress(lyrics: &Lyrics, position: f64, duration: Option<f64>) -> (Option<usize>, f32) {
    if lyrics.synced {
        let Some(i) = lyrics.index_at(position) else {
            return (None, 1.0);
        };
        let start = lyrics.lines[i].time.unwrap_or(0.0);
        let g = ((position - start) / GLIDE_SECS).clamp(0.0, 1.0) as f32;
        return (Some(i), g);
    }
    let n = lyrics.lines.len();
    match duration.filter(|d| *d > 0.0) {
        Some(d) => {
            let x = (position / d).clamp(0.0, 0.9999) * n as f64;
            let i = x.floor() as usize;
            let g = ((x - i as f64) * d / n as f64 / GLIDE_SECS).clamp(0.0, 1.0) as f32;
            (Some(i.min(n - 1)), g)
        }
        None => (Some(0), 1.0),
    }
}

/// One row of a block: a line of text, or a band of large letters as
/// per-cell ink coverage (0..1).
enum Row {
    Text(String),
    Big { cols: u16, rows: u16, ink: Vec<f32> },
}

impl Row {
    fn height(&self) -> u16 {
        match self {
            Row::Text(_) => 1,
            Row::Big { rows, .. } => *rows,
        }
    }

    fn draw(
        &self,
        grid: &mut CellGrid,
        art: Rect,
        y: i32,
        clip: (i32, i32),
        tone: Tone,
        painter: &Painter,
    ) {
        match self {
            Row::Text(text) => {
                if y < clip.0 || y >= clip.1 {
                    return;
                }
                let y = y as u16;
                let mut x = art.x + art.w.saturating_sub(str_width(text)) / 2;
                for ch in text.chars() {
                    let w = char_width(ch);
                    if x + w > art.x + art.w {
                        break;
                    }
                    if let Some(under) = grid.get(x, y).copied() {
                        let fg = painter.color(x, y, tone, 1.0);
                        grid.set(
                            x,
                            y,
                            Cell {
                                ch,
                                fg,
                                bg: under.bg,
                            },
                        );
                        if w == 2 {
                            grid.set(
                                x + 1,
                                y,
                                Cell {
                                    ch: WIDE_CONTINUATION,
                                    fg,
                                    bg: under.bg,
                                },
                            );
                        }
                    }
                    x += w;
                }
            }
            Row::Big { cols, rows, ink } => {
                let x0 = art.x + art.w.saturating_sub(*cols) / 2;
                for ry in 0..*rows {
                    let yy = y + ry as i32;
                    if yy < clip.0 || yy >= clip.1 {
                        continue;
                    }
                    for rx in 0..*cols {
                        let c = ink[ry as usize * *cols as usize + rx as usize];
                        let Some(ch) = shade(c, rx, ry) else { continue };
                        let x = x0 + rx;
                        // A window: the cover's own character shows through
                        // the letter (its shape still from the ink).
                        let ch = painter.window_char(x, yy as u16).unwrap_or(ch);
                        if let Some(under) = grid.get(x, yy as u16).copied() {
                            // Faint edges lean toward the page.
                            let fg = painter.color(x, yy as u16, tone, (0.45 + 0.55 * c).min(1.0));
                            grid.set(
                                x,
                                yy as u16,
                                Cell {
                                    ch,
                                    fg,
                                    bg: under.bg,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

/// A line's colouring before light: its colour, how far it fades toward
/// the page (lines away from the sung one), and how much a beat lifts it.
#[derive(Debug, Clone, Copy)]
struct Tone {
    color: Rgb,
    fade: f32,
    lift: f32,
}

/// Colours each lyric cell from its line's tone, the paint mode and the
/// cover's light at that cell.
struct Painter<'a> {
    cover: Option<CoverLight<'a>>,
    paint: LyricPaint,
    background: Rgb,
}

impl Painter<'_> {
    /// With cover paint, the cover's character at (x, y), if it draws
    /// anything there (blank cells keep the letter's own shading).
    fn window_char(&self, x: u16, y: u16) -> Option<char> {
        if self.paint != LyricPaint::Cover {
            return None;
        }
        let c = self.cover?.frame.get(x, y)?.ch;
        (!c.is_whitespace() && c != WIDE_CONTINUATION).then_some(c)
    }

    /// The colour of the lyric cell at (x, y); `ink` < 1 fades a letter's
    /// edges toward the page.
    fn color(&self, x: u16, y: u16, tone: Tone, ink: f32) -> Rgb {
        let cell_color = |g: &CellGrid| g.get(x, y).map(|c| c.bg.unwrap_or(c.fg));
        let lifted = |c: Rgb| c.lerp(Rgb(255, 255, 255), tone.lift);
        let base = match (self.paint, self.cover) {
            // A window onto the moving cover: its colour here. Where the
            // cover is too dark to read (black covers), the line's own
            // colour fills in, wholly on black. The cover's motion lights it.
            (LyricPaint::Cover, Some(cover)) => {
                let c = cell_color(cover.frame).unwrap_or(tone.color);
                let lum = luminance(c);
                let c = if lum < 110.0 {
                    c.lerp(tone.color, 1.0 - lum / 110.0)
                } else {
                    c
                };
                lifted(c)
            }
            // The line's colour, under the cover's light at this cell.
            (_, cover) => {
                let c = lifted(tone.color);
                match cover.and_then(|cv| Some((cell_color(cv.base)?, cell_color(cv.frame)?))) {
                    Some((still, moving)) => relight(c, still, moving),
                    None => c,
                }
            }
        };
        self.background
            .lerp(base, (ink * (1.0 - tone.fade)).clamp(0.0, 1.0))
    }
}

/// Applies the change from `still` to `moving` (the cover's colour at one
/// cell, without and with motion) to `c`: a cell the recipe brightens,
/// tints or shades does the same to the letter on it.
fn relight(c: Rgb, still: Rgb, moving: Rgb) -> Rgb {
    let ch = |c: u8, s: u8, m: u8| {
        let ratio = ((m as f32 + 24.0) / (s as f32 + 24.0)).clamp(0.3, 2.2);
        (c as f32 * ratio).round().clamp(0.0, 255.0) as u8
    };
    Rgb(
        ch(c.0, still.0, moving.0),
        ch(c.1, still.1, moving.1),
        ch(c.2, still.2, moving.2),
    )
}

fn luminance(c: Rgb) -> f32 {
    0.2126 * c.0 as f32 + 0.7152 * c.1 as f32 + 0.0722 * c.2 as f32
}

/// Terminal cells a character takes: 2 for East Asian wide and emoji.
pub fn char_width(c: char) -> u16 {
    let v = c as u32;
    let wide = matches!(v,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

fn str_width(s: &str) -> u16 {
    s.chars().map(char_width).sum()
}

/// Ink coverage to a glyph: dense in the core, lighter on the edges. The
/// choice among equals varies by position, a stable texture like the art's.
fn shade(c: f32, x: u16, y: u16) -> Option<char> {
    let pick = |set: &[char]| set[(x as usize * 7 + y as usize * 13) % set.len()];
    if c >= 0.62 {
        Some(pick(&['@', '#', '%', '@']))
    } else if c >= 0.38 {
        Some(pick(&['*', '+', '=']))
    } else if c >= 0.16 {
        Some(pick(&[':', '-', '.']))
    } else {
        None
    }
}

/// Wraps text into rows at most `width` cells wide, on spaces; words
/// wider than a row are split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let w = |s: &str| str_width(s) as usize;
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while w(&word) > width {
            if !row.is_empty() {
                rows.push(std::mem::take(&mut row));
            }
            let mut head = String::new();
            for c in word.chars() {
                if w(&head) + char_width(c) as usize > width {
                    break;
                }
                head.push(c);
            }
            if head.is_empty() {
                break;
            }
            word = word[head.len()..].to_string();
            rows.push(head);
        }
        if row.is_empty() {
            row = word;
        } else if w(&row) + 1 + w(&word) <= width {
            row.push(' ');
            row.push_str(&word);
        } else {
            rows.push(std::mem::replace(&mut row, word));
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

fn text_rows(text: &str, width: u16) -> Vec<Row> {
    wrap(text, width as usize)
        .into_iter()
        .map(Row::Text)
        .collect()
}

fn font() -> &'static fontdue::Font {
    static FONT: OnceLock<fontdue::Font> = OnceLock::new();
    FONT.get_or_init(|| {
        fontdue::Font::from_bytes(
            include_bytes!("../../../assets/fonts/SpaceMono-Bold.ttf") as &[u8],
            fontdue::FontSettings::default(),
        )
        .expect("the bundled font parses")
    })
}

/// The large-letter typeface at one size, in grid units.
struct BigFont {
    /// Pixel size the font is rasterised at.
    px: f32,
    /// Sub-samples per cell, across and down.
    sub_x: f32,
    sub_y: f32,
    /// Rows per line of large text (ascent to descent, plus a gap).
    rows: u16,
    ascent: f32,
}

impl BigFont {
    fn new(cap_rows: f32, cell_aspect: f32) -> BigFont {
        let sub_y = SUB;
        let sub_x = (SUB * cell_aspect.clamp(0.2, 2.0)).max(1.0);
        let px = cap_rows * sub_y / CAP_HEIGHT;
        let lm = font().horizontal_line_metrics(px);
        let (ascent, descent) = lm.map_or((px * 0.9, -px * 0.25), |m| (m.ascent, m.descent));
        let rows = ((ascent - descent) / sub_y).ceil() as u16 + 1;
        BigFont {
            px,
            sub_x,
            sub_y,
            rows,
            ascent,
        }
    }

    /// The same typeface at `factor` of this size.
    fn scaled(&self, factor: f32) -> BigFont {
        let px = self.px * factor;
        let lm = font().horizontal_line_metrics(px);
        let (ascent, descent) = lm.map_or((px * 0.9, -px * 0.25), |m| (m.ascent, m.descent));
        BigFont {
            px,
            sub_x: self.sub_x,
            sub_y: self.sub_y,
            rows: ((ascent - descent) / self.sub_y).ceil() as u16 + 1,
            ascent,
        }
    }

    /// Whether the typeface has most of the line's characters (else it is
    /// shown as text, e.g. Japanese).
    fn can_draw(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        let known = chars
            .iter()
            .filter(|c| font().lookup_glyph_index(**c) != 0)
            .count();
        !chars.is_empty() && known * 10 >= chars.len() * 9
    }

    fn advance(&self, c: char) -> f32 {
        font().metrics(c, self.px).advance_width
    }

    /// The line as rows of large letters, wrapped on words to `width` cells.
    fn rows(&self, text: &str, width: u16) -> Vec<Row> {
        let max_px = width as f32 * self.sub_x;
        // A word wider than the square: shrink the line's letters to fit it
        // instead of cutting it off.
        let widest = text
            .split_whitespace()
            .map(|w| w.chars().map(|c| self.advance(c)).sum::<f32>())
            .fold(0.0, f32::max);
        if widest > max_px {
            return self.scaled(max_px / widest * 0.97).rows(text, width);
        }
        let mut lines: Vec<String> = Vec::new();
        let mut line = String::new();
        for word in text.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            let w: f32 = candidate.chars().map(|c| self.advance(c)).sum();
            if w <= max_px || line.is_empty() {
                line = candidate;
            } else {
                lines.push(std::mem::replace(&mut line, word.to_string()));
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines.iter().map(|l| self.raster(l, width)).collect()
    }

    /// Rasterises one line and samples it into cells.
    fn raster(&self, text: &str, width: u16) -> Row {
        let line_px: f32 = text.chars().map(|c| self.advance(c)).sum();
        let cols = ((line_px / self.sub_x).ceil() as u16).clamp(1, width);
        let (bw, bh) = (
            (cols as f32 * self.sub_x).ceil() as usize,
            (self.rows as f32 * self.sub_y).ceil() as usize,
        );
        let mut bitmap = vec![0u8; bw * bh];
        let baseline = self.ascent + self.sub_y * 0.5;
        let mut pen = 0.0f32;
        for c in text.chars() {
            let (m, glyph) = font().rasterize(c, self.px);
            let left = (pen + m.xmin as f32).round() as i32;
            let top = (baseline - (m.ymin as f32 + m.height as f32)).round() as i32;
            for gy in 0..m.height {
                for gx in 0..m.width {
                    let (x, y) = (left + gx as i32, top + gy as i32);
                    if x >= 0 && y >= 0 && (x as usize) < bw && (y as usize) < bh {
                        let v = glyph[gy * m.width + gx];
                        let dst = &mut bitmap[y as usize * bw + x as usize];
                        *dst = (*dst).max(v);
                    }
                }
            }
            pen += m.advance_width;
        }
        // Average coverage per cell.
        let mut ink = vec![0.0f32; cols as usize * self.rows as usize];
        for cy in 0..self.rows as usize {
            let y0 = (cy as f32 * self.sub_y) as usize;
            let y1 = (((cy + 1) as f32 * self.sub_y) as usize).min(bh);
            for cx in 0..cols as usize {
                let x0 = (cx as f32 * self.sub_x) as usize;
                let x1 = (((cx + 1) as f32 * self.sub_x) as usize).min(bw);
                let mut sum = 0u32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += bitmap[y * bw + x] as u32;
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)).max(1) as f32;
                ink[cy * cols as usize + cx] = sum as f32 / (255.0 * n);
            }
        }
        Row::Big {
            cols,
            rows: self.rows,
            ink,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::lyrics::parse_lrc;

    const ACCENT: Rgb = Rgb(240, 120, 40);
    const BG: Rgb = Rgb(14, 15, 18);
    const COLORS: LyricColors = LyricColors {
        accent: ACCENT,
        text: Rgb(200, 200, 210),
        background: BG,
    };
    /// A 60x40 grid with a 40x40-cell... square-ish art box in the middle.
    const ART: Rect = Rect {
        x: 10,
        y: 0,
        w: 40,
        h: 40,
    };

    fn song() -> Lyrics {
        parse_lrc(
            "[00:01.00]first line here\n[00:05.00]second line\n[00:09.00]third one\n[00:13.00]fourth",
            "test",
        )
        .unwrap()
    }

    fn cover() -> CellGrid {
        let mut g = CellGrid::new(60, 40);
        for y in 0..40 {
            for x in 0..60 {
                g.set(
                    x,
                    y,
                    Cell {
                        ch: '.',
                        fg: Rgb(90, 90, 90),
                        bg: None,
                    },
                );
            }
        }
        g
    }

    fn view(style: LyricStyle, scroll: LyricScroll, size: LyricSize) -> LyricView {
        LyricView {
            style,
            scroll,
            size,
            paint: LyricPaint::Accent,
        }
    }

    fn render_with(v: LyricView, pos: f64, aspect: f32) -> CellGrid {
        let mut g = cover();
        overlay(
            &mut g,
            ART,
            aspect,
            &song(),
            pos,
            Some(20.0),
            v,
            COLORS,
            &AudioFrame::default(),
            None,
        );
        g
    }

    fn render(style: LyricStyle, scroll: LyricScroll, pos: f64) -> CellGrid {
        render_with(view(style, scroll, LyricSize::Medium), pos, 0.5)
    }

    fn text_of(g: &CellGrid, y: u16) -> String {
        (0..g.width()).map(|x| g.get(x, y).unwrap().ch).collect()
    }

    /// Rows holding accent-tinted cells (large letters, or the accent text).
    fn accent_rows(g: &CellGrid) -> Vec<u16> {
        (0..g.height())
            .filter(|&y| {
                (0..g.width()).any(|x| {
                    let c = g.get(x, y).unwrap();
                    c.fg.0 > 150 && c.fg.2 < 100
                })
            })
            .collect()
    }

    #[test]
    fn draws_only_inside_the_square_with_a_big_sung_line() {
        let g = render(LyricStyle::BigCurrent, LyricScroll::Up, 6.0);
        // Outside the square nothing changes.
        for y in 0..40 {
            for x in (0..10).chain(50..60) {
                assert_eq!(g.get(x, y).unwrap().ch, '.', "({x},{y})");
            }
        }
        // The sung line is several rows of large letters in the accent.
        let rows = accent_rows(&g);
        assert!(rows.len() >= 4, "{rows:?}");
        // Text neighbours: the previous line above, the next below.
        let prev = (0..40)
            .find(|&y| text_of(&g, y).contains("first line here"))
            .unwrap();
        let next = (0..40)
            .find(|&y| text_of(&g, y).contains("third one"))
            .unwrap();
        assert!(
            prev < rows[0] && next > *rows.last().unwrap(),
            "{prev} {rows:?} {next}"
        );
    }

    #[test]
    fn size_sets_letter_height_and_detail_keeps_it() {
        // Finer detail scales the grid both ways: a square of art_h rows
        // is 2 * art_h columns of half-width cells.
        let height = |size, art_h: u16| {
            let art = Rect {
                x: 0,
                y: 0,
                w: 2 * art_h,
                h: art_h,
            };
            let mut g = CellGrid::new(2 * art_h, art_h);
            let v = view(LyricStyle::BigOnly, LyricScroll::Up, size);
            overlay(
                &mut g,
                art,
                0.5,
                &song(),
                6.0,
                None,
                v,
                COLORS,
                &AudioFrame::default(),
                None,
            );
            accent_rows(&g).len() as f32 / art_h as f32
        };
        let (s, m, l) = (
            height(LyricSize::Small, 60),
            height(LyricSize::Medium, 60),
            height(LyricSize::Large, 60),
        );
        assert!(s < m && m < l, "{s} {m} {l}");
        // Twice the rows (finer detail): the same share of the square.
        let fine = height(LyricSize::Medium, 120);
        assert!((fine - m).abs() < 0.03, "{m} vs {fine}");
    }

    #[test]
    fn styles_and_directions() {
        // Text: the sung line is plain text in the accent.
        let t = render(LyricStyle::Text, LyricScroll::Up, 6.0);
        let y = accent_rows(&t);
        assert_eq!(y.len(), 1);
        assert!(text_of(&t, y[0]).contains("second line"));
        // Big only: no other lines, roughly centred in the square.
        let b = render(LyricStyle::BigOnly, LyricScroll::Up, 6.0);
        assert!((0..40).all(|y| !text_of(&b, y).contains("third")));
        let rows = accent_rows(&b);
        assert!(rows[0] > 8 && *rows.last().unwrap() < 32, "{rows:?}");
        // Down: the next line sits above the sung one.
        let d = render(LyricStyle::Text, LyricScroll::Down, 6.0);
        let sung = accent_rows(&d)[0];
        let next = (0..40)
            .find(|&y| text_of(&d, y).contains("third one"))
            .unwrap();
        assert!(next < sung, "{next} {sung}");
    }

    #[test]
    fn glides_between_lines_and_is_deterministic() {
        let at = |p| accent_rows(&render(LyricStyle::Text, LyricScroll::Up, p))[0];
        let (start, mid, end) = (at(5.0), at(5.0 + GLIDE_SECS / 2.0), at(6.0));
        assert!(start > mid && mid >= end, "{start} {mid} {end}");
        assert_eq!(
            render(LyricStyle::BigCurrent, LyricScroll::Down, 7.3),
            render(LyricStyle::BigCurrent, LyricScroll::Down, 7.3)
        );
    }

    /// A cover whose left half is red and right half blue, and a moving
    /// frame of it with the top half lit twice as bright.
    fn two_tone() -> (CellGrid, CellGrid) {
        let mut base = CellGrid::new(60, 40);
        let mut frame = CellGrid::new(60, 40);
        for y in 0..40 {
            for x in 0..60 {
                // Bright enough that window paint shows them as they are.
                let c = if x < 30 {
                    Rgb(255, 120, 120)
                } else {
                    Rgb(120, 120, 255)
                };
                base.set(
                    x,
                    y,
                    Cell {
                        ch: '#',
                        fg: c,
                        bg: None,
                    },
                );
                let lit = if y < 20 {
                    c.lerp(Rgb(255, 255, 255), 0.5)
                } else {
                    c
                };
                frame.set(
                    x,
                    y,
                    Cell {
                        ch: if x < 30 { 'L' } else { 'R' },
                        fg: lit,
                        bg: None,
                    },
                );
            }
        }
        (base, frame)
    }

    #[test]
    fn window_paint_shows_the_cover_and_light_reaches_the_letters() {
        let (base, frame) = two_tone();
        let lit_cover = CoverLight {
            base: &base,
            frame: &frame,
        };
        let mut v = view(LyricStyle::BigOnly, LyricScroll::Up, LyricSize::Large);
        v.paint = LyricPaint::Cover;
        let mut g = cover();
        overlay(
            &mut g,
            ART,
            0.5,
            &song(),
            6.0,
            None,
            v,
            COLORS,
            &AudioFrame::default(),
            Some(lit_cover),
        );
        // Letters on the left are reddish, on the right bluish.
        let lyric = |g: &CellGrid, x: u16, y: u16| {
            let c = g.get(x, y).unwrap();
            (c.ch != '.').then_some(c.fg)
        };
        let left: Vec<Rgb> = (0..40)
            .flat_map(|y| (10..30).filter_map(move |x| (x, y).into()))
            .filter_map(|(x, y)| lyric(&g, x, y))
            .collect();
        let right: Vec<Rgb> = (0..40)
            .flat_map(|y| (30..50).filter_map(move |x| (x, y).into()))
            .filter_map(|(x, y)| lyric(&g, x, y))
            .collect();
        assert!(!left.is_empty() && !right.is_empty());
        assert!(left.iter().all(|c| c.0 > c.2) && right.iter().all(|c| c.2 > c.0));
        // The letters are made of the cover's own characters.
        let chars = |xs: std::ops::Range<u16>| -> Vec<char> {
            (0..40)
                .flat_map(|y| xs.clone().map(move |x| (x, y)))
                .map(|(x, y)| g.get(x, y).unwrap().ch)
                .filter(|c| *c != '.')
                .collect()
        };
        assert!(
            chars(10..30).iter().all(|c| *c == 'L'),
            "{:?}",
            chars(10..30)
        );
        assert!(chars(30..50).iter().all(|c| *c == 'R'));

        // Accent paint: the same letters, lit where the cover is lit.
        let still = CoverLight {
            base: &base,
            frame: &base,
        };
        let render = |cv| {
            let mut g = cover();
            let v = view(LyricStyle::Text, LyricScroll::Up, LyricSize::Medium);
            overlay(
                &mut g,
                ART,
                0.5,
                &song(),
                6.0,
                Some(20.0),
                v,
                COLORS,
                &AudioFrame::default(),
                Some(cv),
            );
            g
        };
        let (dark, lit) = (render(still), render(lit_cover));
        let y = accent_rows(&dark)[0];
        assert!(y < 20, "the sung line is in the lit half");
        let x = (10..50)
            .find(|&x| dark.get(x, y).unwrap().ch != '.')
            .unwrap();
        let sum = |c: Rgb| c.0 as u32 + c.1 as u32 + c.2 as u32;
        assert!(
            sum(lit.get(x, y).unwrap().fg) > sum(dark.get(x, y).unwrap().fg),
            "brighter where lit"
        );
    }

    #[test]
    fn long_words_shrink_and_wide_text_stays_inside() {
        let long = parse_lrc("[00:01.00]Supercalifragilisticexpialidocious", "t").unwrap();
        let mut g = cover();
        let v = view(LyricStyle::BigOnly, LyricScroll::Up, LyricSize::Large);
        overlay(
            &mut g,
            ART,
            0.5,
            &long,
            2.0,
            None,
            v,
            COLORS,
            &AudioFrame::default(),
            None,
        );
        assert!(!accent_rows(&g).is_empty());
        // Nothing drawn outside the square.
        for y in 0..40 {
            for x in (0..10).chain(50..60) {
                assert_eq!(g.get(x, y).unwrap().ch, '.', "({x},{y})");
            }
        }
        // Wide characters take two cells and wrap within the square.
        let jp = parse_lrc(
            "[00:01.00]こんにちは世界、こんにちは世界、こんにちは世界",
            "t",
        )
        .unwrap();
        let mut g = cover();
        let v = view(LyricStyle::Text, LyricScroll::Up, LyricSize::Medium);
        overlay(
            &mut g,
            ART,
            0.5,
            &jp,
            2.0,
            None,
            v,
            COLORS,
            &AudioFrame::default(),
            None,
        );
        let row = accent_rows(&g)[0];
        assert!((0..60).any(|x| g.get(x, row).unwrap().ch == WIDE_CONTINUATION));
        assert!(
            (0..10)
                .chain(50..60)
                .all(|x| g.get(x, row).unwrap().ch == '.')
        );
        assert_eq!(wrap("日本語テキスト", 6), ["日本語", "テキス", "ト"]);
    }

    #[test]
    fn blank_square_and_edge_cases() {
        let b = blank_square(&cover(), ART, BG);
        assert_eq!(b.get(20, 20).unwrap().ch, ' ');
        assert_eq!(b.get(5, 20).unwrap().ch, '.');
        // Before the first line: no accent, the first line waiting below.
        let g = render(LyricStyle::BigCurrent, LyricScroll::Up, 0.2);
        assert!(accent_rows(&g).is_empty());
        assert!((0..40).any(|y| text_of(&g, y).contains("first line here")));
        // Unsynced: paced through the song.
        let plain = parse_lrc("one\ntwo\nthree\nfour", "t").unwrap();
        let mut g = cover();
        let v = view(LyricStyle::Text, LyricScroll::Up, LyricSize::Medium);
        overlay(
            &mut g,
            ART,
            0.5,
            &plain,
            11.0,
            Some(20.0),
            v,
            COLORS,
            &AudioFrame::default(),
            None,
        );
        assert!(text_of(&g, accent_rows(&g)[0]).contains("three"));
        // A line the typeface cannot draw stays text even in big styles.
        let jp = parse_lrc("[00:01.00]こんにちは", "t").unwrap();
        let mut g = cover();
        let v = view(LyricStyle::BigCurrent, LyricScroll::Up, LyricSize::Medium);
        overlay(
            &mut g,
            ART,
            0.5,
            &jp,
            2.0,
            None,
            v,
            COLORS,
            &AudioFrame::default(),
            None,
        );
        assert_eq!(accent_rows(&g).len(), 1);
        assert_eq!(wrap("a bb ccc dddd", 6), ["a bb", "ccc", "dddd"]);
    }

    #[test]
    fn window_paint_on_a_black_cover_fills_with_the_line_colour() {
        let mut black = CellGrid::new(60, 40);
        for y in 0..40 {
            for x in 0..60 {
                black.set(
                    x,
                    y,
                    Cell {
                        ch: ' ',
                        fg: Rgb(0, 0, 0),
                        bg: None,
                    },
                );
            }
        }
        let still = CoverLight {
            base: &black,
            frame: &black,
        };
        let mut v = view(LyricStyle::BigOnly, LyricScroll::Up, LyricSize::Large);
        v.paint = LyricPaint::Cover;
        let mut g = cover();
        overlay(
            &mut g,
            ART,
            0.5,
            &song(),
            6.0,
            None,
            v,
            COLORS,
            &AudioFrame::default(),
            Some(still),
        );
        // The sung line's colour (the accent) at full ink, not black.
        let bright = (0..40)
            .flat_map(|y| (0..60).map(move |x| (x, y)))
            .filter_map(|(x, y)| g.get(x, y).filter(|c| c.ch != '.').map(|c| c.fg))
            .any(|c| c == COLORS.accent);
        assert!(bright, "letters on black take the line colour");
    }
}
