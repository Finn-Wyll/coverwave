//! coverwave terminal frontend.
//!
//! Usage: `cw-tui [FILE_OR_FOLDER ...]`. Folders are scanned recursively.

mod app;
mod cover;
mod ui;

use anyhow::{Context, Result, bail};
use app::{App, ColorMode, Settings};
use cw_audio::{Player, Track, collect_audio_files};
use cw_core::ArtStyle;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEFAULT_FPS: u32 = 30;

const USAGE: &str = "\
coverwave: terminal music player

usage: cw-tui [OPTIONS] [FILE_OR_FOLDER ...]

Folders are scanned recursively for audio files.
Inside the app, press ? for keys.

options:
  --art-style STYLE   cover colouring: palette (default), truecolor, duotone.
                      Press c in the app to cycle.
  --color MODE        truecolor or 256. Default: truecolor when COLORTERM
                      says so, else 256.
  --cell-aspect R     terminal cell width / height (default: measured, else
                      0.5). Raise it if covers look too tall.
  -h, --help          show this help";

/// Splits arguments into settings and input paths.
fn parse_args(args: &[String]) -> Result<(Settings, Vec<PathBuf>)> {
    let mut settings = Settings {
        color_mode: ColorMode::detect(),
        ..Settings::default()
    };
    let mut inputs = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--art-style" => {
                let v = value("--art-style")?;
                settings.art_style = ArtStyle::from_name(v).with_context(|| {
                    format!("unknown art style {v:?} (palette, truecolor, duotone)")
                })?;
            }
            "--color" => {
                let v = value("--color")?;
                settings.color_mode = ColorMode::from_name(v)
                    .with_context(|| format!("unknown colour mode {v:?} (truecolor, 256)"))?;
            }
            "--cell-aspect" => {
                let v = value("--cell-aspect")?;
                let r: f32 = v
                    .parse()
                    .ok()
                    .filter(|r: &f32| (0.2..=2.0).contains(r))
                    .with_context(|| format!("--cell-aspect wants a number 0.2..2, got {v:?}"))?;
                settings.cell_aspect = Some(r);
            }
            "--" => inputs.extend(it.by_ref().map(PathBuf::from)),
            s if s.starts_with("--") => bail!("unknown option {s} (see --help)"),
            s => inputs.push(PathBuf::from(s)),
        }
    }
    Ok((settings, inputs))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let (settings, inputs) = match parse_args(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(2);
        }
    };
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
    let mut app = App::new(DEFAULT_FPS, player, audio_error, settings);
    app.measure_cell_aspect();
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
                // The new size is picked up on the next draw; the font (and so
                // the cell shape) may have changed too.
                Event::Resize(..) => app.measure_cell_aspect(),
                _ => {}
            }
            if app.should_quit {
                break;
            }
        }
    }
    Ok(())
}
