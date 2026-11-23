use crate::app::{App, Mode};
use cw_core::{CellGrid, Rgb};
use cw_render::{DEFAULT_LOOP_FRAMES, IdleStyle};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap},
};

const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 12;

const BG: Color = Color::Rgb(14, 15, 18);
const FG: Color = Color::Rgb(196, 204, 214);
const DIM: Color = Color::Rgb(98, 104, 114);
const BORDER: Color = Color::Rgb(48, 52, 60);

fn color(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
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
        Mode::Library => draw_library(f, main),
    }
    draw_hud(f, app, hud);
    if app.show_debug {
        draw_debug(f, app, main);
    }
}

fn draw_too_small(f: &mut Frame, area: Rect) {
    let msg = Paragraph::new(vec![
        Line::from("Terminal too small"),
        Line::from(Span::styled(
            format!(
                "need {MIN_WIDTH}x{MIN_HEIGHT}, have {}x{}",
                area.width, area.height
            ),
            Style::new().fg(DIM),
        )),
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
    let grid = cw_render::idle(
        area.width,
        area.height,
        app.frame,
        DEFAULT_LOOP_FRAMES,
        app.idle_seed,
        IdleStyle::default(),
    );
    let total = (grid.width() as usize * grid.height() as usize).max(1);
    app.last_change_pct = match &app.prev_art {
        Some(prev) => 100.0 * grid.diff_count(prev) as f32 / total as f32,
        None => 100.0,
    };
    f.render_widget(GridWidget(&grid), area);
    app.prev_art = Some(grid);
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
}

fn draw_library(f: &mut Frame, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area);

    let empty = Paragraph::new(vec![
        Line::from("No music yet."),
        Line::from(""),
        Line::from(Span::styled(
            "Library scanning arrives in milestone M6.",
            Style::new().fg(DIM),
        )),
        Line::from(Span::styled(
            "You will add folders in ~/.config/coverwave/config.toml:",
            Style::new().fg(DIM),
        )),
        Line::from(Span::styled(
            "  music_dirs = [\"~/Music\"]",
            Style::new().fg(FG),
        )),
    ])
    .wrap(Wrap { trim: false })
    .block(panel("Library").padding(ratatui::widgets::Padding::new(2, 2, 1, 1)));
    f.render_widget(empty, left);

    let queue = Paragraph::new(Line::from(Span::styled(
        "Queue is empty.",
        Style::new().fg(DIM),
    )))
    .block(panel("Queue").padding(ratatui::widgets::Padding::new(2, 2, 1, 1)));
    f.render_widget(queue, right);
}

fn draw_hud(f: &mut Frame, app: &App, area: Rect) {
    let key = |k: &'static str| Span::styled(k, Style::new().fg(FG).add_modifier(Modifier::BOLD));
    let label = |l: &'static str| Span::styled(l, Style::new().fg(DIM));
    let other = match app.mode {
        Mode::Art => "  library",
        Mode::Library => "  art",
    };
    let hints = Line::from(vec![
        key("tab"),
        label(other),
        label("   "),
        key("D"),
        label("  debug"),
        label("   "),
        key("q"),
        label("  quit "),
    ]);
    let hints_width = hints.width() as u16;
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(hints_width)]).areas(area);

    f.render_widget(
        Paragraph::new(Line::from(label(" nothing playing"))).style(Style::new().bg(BG)),
        left,
    );
    f.render_widget(Paragraph::new(hints).style(Style::new().bg(BG)), right);
}

fn draw_debug(f: &mut Frame, app: &App, main: Rect) {
    let lines = vec![
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
    let w = 26.min(main.width);
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
            .block(panel("debug")),
        area,
    );
}
