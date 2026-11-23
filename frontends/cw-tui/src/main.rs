//! coverwave terminal frontend.
//!
//! M0: a fixed-rate render loop with switchable Art / Library modes, a HUD
//! line, a debug overlay, and clean terminal restore on exit or panic.

mod app;
mod ui;

use anyhow::Result;
use app::App;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::{Duration, Instant};

const DEFAULT_FPS: u32 = 30;

fn main() -> Result<()> {
    // `ratatui::init` enters the alternate screen + raw mode and installs a
    // panic hook that restores the terminal before the panic message prints.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let mut app = App::new(DEFAULT_FPS);
    let frame_time = Duration::from_secs_f64(1.0 / app.fps as f64);
    let mut next_frame = Instant::now();

    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        app.on_frame_drawn(Instant::now());

        // Fixed timestep: schedule from the previous deadline, not from "now",
        // so frame pacing does not drift. If we fall far behind (e.g. the
        // terminal was suspended), resynchronise instead of fast-forwarding.
        next_frame += frame_time;
        let now = Instant::now();
        if next_frame + frame_time * 4 < now {
            next_frame = now;
        }

        // Handle all input that arrives before the next frame is due.
        loop {
            let timeout = next_frame.saturating_duration_since(Instant::now());
            if !event::poll(timeout)? {
                break;
            }
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                _ => {} // resize is picked up on the next draw
            }
            if app.should_quit {
                break;
            }
        }
    }
    Ok(())
}
