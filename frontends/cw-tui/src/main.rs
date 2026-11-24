//! coverwave terminal frontend.
//!
//! Usage: `cw-tui [FILE_OR_FOLDER ...]`. Folders are scanned recursively.

mod app;
mod browse;
mod config;
mod cover;
mod models;
mod recipes;
mod sources;
mod ui;

use anyhow::{Context, Result, bail};
use app::{App, ColorMode, Settings};
use cw_audio::{Player, Track, collect_audio_files};
use cw_core::ArtStyle;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The frozen style-DNA projection, fitted by `tools/pca-fit`.
const PCA_BIN: &[u8] = include_bytes!("../../../assets/pca.bin");

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
  --offline           never download the cover-analysis model. Without it,
                      covers get gentle default motion.
  -h, --help          show this help";

/// Splits arguments into settings and input paths.
/// Settings from the config file, before command-line overrides.
fn settings_from(config: &config::Config, warnings: &mut Vec<String>) -> Settings {
    let mut s = Settings {
        color_mode: ColorMode::detect(),
        ..Settings::default()
    };
    match ArtStyle::from_name(&config.art_style) {
        Some(a) => s.art_style = a,
        None => warnings.push(format!("config: unknown art_style {:?}", config.art_style)),
    }
    if config.color != "auto" {
        match ColorMode::from_name(&config.color) {
            Some(c) => s.color_mode = c,
            None => warnings.push(format!("config: unknown color {:?}", config.color)),
        }
    }
    s.cell_aspect = config.cell_aspect();
    s.offline = config.offline;
    s.motion = config.motion;
    s.fps = config.fps.clamp(10, 120);
    s
}

/// Applies command-line options on top of `settings`.
fn parse_args(args: &[String], mut settings: Settings) -> Result<(Settings, Vec<PathBuf>)> {
    let mut inputs = Vec::new();
    let mut it = args.iter();
    let mut offline = settings.offline;
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
            "--offline" => offline = true,
            "--" => inputs.extend(it.by_ref().map(PathBuf::from)),
            s if s.starts_with("--") => bail!("unknown option {s} (see --help)"),
            s => inputs.push(PathBuf::from(s)),
        }
    }
    settings.offline = offline;
    Ok((settings, inputs))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    // Config file first; flags override it. Write a commented template on
    // first run so it is easy to find.
    let config_path = models::config_dir().map(|d| d.join("config.toml"));
    let mut warnings = Vec::new();
    let config = match &config_path {
        Some(p) => {
            if !p.exists()
                && let Some(dir) = p.parent()
            {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(p, config::TEMPLATE);
            }
            let (c, warning) = config::Config::load(p);
            warnings.extend(warning);
            c
        }
        None => config::Config::default(),
    };
    let base = settings_from(&config, &mut warnings);
    let (settings, inputs) = match parse_args(&args, base) {
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
    let data = models::data_dir();
    let dirs = app::Dirs {
        playlists: data.as_ref().map(|d| d.join("playlists")),
        queue_file: data.as_ref().map(|d| d.join("queue.json")),
    };
    // With no files given, pick up where the last session stopped (paused).
    let saved = if inputs.is_empty() {
        dirs.queue_file
            .as_deref()
            .and_then(cw_library::state::SavedQueue::load)
    } else {
        None
    };
    let restore = saved.is_some();
    let (player, audio_error) = match Player::new(if restore { Vec::new() } else { tracks }, 0) {
        Ok(mut p) => {
            if let Some(q) = saved {
                let tracks: Vec<Track> = q
                    .paths
                    .iter()
                    .filter(|p| p.exists())
                    .cloned()
                    .map(Track::load)
                    .collect();
                if !tracks.is_empty() {
                    let current = q.current.min(tracks.len() - 1);
                    p.set_volume(q.volume);
                    p.replace(tracks, current, q.position_secs);
                    p.set_paused(true);
                    p.set_shuffle(q.shuffle);
                    p.set_repeat(match q.repeat.as_str() {
                        "all" => cw_core::Repeat::All,
                        "one" => cw_core::Repeat::One,
                        _ => cw_core::Repeat::Off,
                    });
                }
            }
            (Some(p), None)
        }
        Err(e) => (None, Some(e.to_string())),
    };

    // The library: the UI keeps one connection, a background thread scans
    // the music folders with another and reports when done.
    let db_path = data.as_ref().map(|d| d.join("library.db"));
    let library = db_path
        .as_deref()
        .and_then(|p| match cw_library::Library::open(p) {
            Ok(l) => Some(l),
            Err(e) => {
                warnings.push(e.to_string());
                None
            }
        });
    let roots: Vec<PathBuf> = config
        .music_dirs()
        .into_iter()
        .filter(|d| d.is_dir())
        .collect();
    let scan_rx = match (&db_path, library.is_some() && !roots.is_empty()) {
        (Some(p), true) => {
            let (tx, rx) = std::sync::mpsc::channel();
            let p = p.clone();
            let _ = std::thread::Builder::new()
                .name("cw-scan".into())
                .spawn(move || {
                    let result = cw_library::Library::open(&p)
                        .and_then(|mut l| l.scan(&roots))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(result);
                });
            Some(rx)
        }
        _ => None,
    };

    // `ratatui::init` enters the alternate screen + raw mode and installs a
    // panic hook that restores the terminal before the panic message prints.
    let mut terminal = ratatui::init();
    // The model loads (or downloads, once) in the background; covers get
    // neutral recipes until it is ready.
    let recipes = cover::RecipeContext {
        store: recipes::Store::default_dirs(),
        scenes: cw_analysis::ScenePrompts::shipped(),
        pca: cw_analysis::Pca::from_bytes(PCA_BIN).ok(),
        model: models::start(settings.offline),
    };
    let fps = settings.fps;
    let mut app = App::new(fps, player, audio_error, settings, recipes);
    app.attach_library(library, dirs, scan_rx);
    app.sources = sources::Sources::new(&config);
    if !warnings.is_empty() {
        app.say(warnings.join("; "));
    }
    app.measure_cell_aspect();
    let result = run(&mut terminal, &mut app);
    app.tick(); // a final snapshot for the saved queue
    app.save_queue();
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
