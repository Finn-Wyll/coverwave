//! coverwave terminal frontend.
//!
//! Usage: `cw-tui [FILE_OR_FOLDER ...]`. Folders are scanned recursively.

mod app;
mod ui;

use anyhow::Result;
use app::App;
use cw_audio::{Player, Track, collect_audio_files};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEFAULT_FPS: u32 = 30;

const USAGE: &str = "\
coverwave: terminal music player

usage: cw-tui [FILE_OR_FOLDER ...]

Folders are scanned recursively for audio files.
Inside the app, press ? for keys.";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let inputs: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let missing: Vec<&PathBuf> = inputs.iter().filter(|p| !p.exists()).collect();
    if !missing.is_empty() {
        for p in missing {
            eprintln!("not found: {}", p.display());
        }
        std::process::exit(2);
    }

    // Read tags before entering the alternate screen so slow disks do not
    // leave a blank terminal.
    let tracks: Vec<Track> = collect_audio_files(&inputs)
        .into_iter()
        .map(Track::load)
        .collect();
    if !inputs.is_empty() && tracks.is_empty() {
        eprintln!("no playable audio files found");
        std::process::exit(2);
    }
    let (player, audio_error) = match Player::new(tracks, 0) {
        Ok(p) => (Some(p), None),
        Err(e) => (None, Some(e.to_string())),
    };

    // `ratatui::init` enters the alternate screen + raw mode and installs a
    // panic hook that restores the terminal before the panic message prints.
    let mut terminal = ratatui::init();
    let mut app = App::new(DEFAULT_FPS, player, audio_error);
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let frame_time = Duration::from_secs_f64(1.0 / app.fps as f64);
    let mut next_frame = Instant::now();

    while !app.should_quit {
        app.tick();
        terminal.draw(|f| ui::draw(f, app))?;
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
