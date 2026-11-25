//! Prints one lyrics frame as text, to eyeball the layout, and times it:
//! `cargo run -p cw-render --release --example lyrics_preview -- [big|text|big_only] [up|down] [small|medium|large] [ROWS] [SECONDS]`

use cw_core::lyrics::parse_lrc;
use cw_core::{AudioFrame, Cell, CellGrid, Rgb};
use cw_render::Rect;
use cw_render::lyrics::{LyricColors, LyricScroll, LyricSize, LyricStyle, LyricView, overlay};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    let view = LyricView {
        style: LyricStyle::from_name(arg(0)).unwrap_or_default(),
        scroll: LyricScroll::from_name(arg(1)).unwrap_or_default(),
        size: LyricSize::from_name(arg(2)).unwrap_or_default(),
        paint: Default::default(),
    };
    let rows: u16 = arg(3).parse().unwrap_or(40);
    let pos: f64 = arg(4).parse().unwrap_or(6.2);
    let lyrics = parse_lrc(
        "[00:01.00]The first line of a song\n[00:05.00]Sung right now\n\
         [00:09.00]Then the next one waits\n[00:13.00]And another after that\n[00:17.00]Last",
        "demo",
    )
    .expect("lyrics");
    // A square of `rows` rows is twice as many half-width columns.
    let (w, h) = (rows * 2 + 8, rows);
    let art = Rect {
        x: 4,
        y: 0,
        w: rows * 2,
        h: rows,
    };
    let mut grid = CellGrid::new(w, h);
    for y in 0..h {
        for x in 0..w {
            grid.set(
                x,
                y,
                Cell {
                    ch: ' ',
                    fg: Rgb(14, 15, 18),
                    bg: None,
                },
            );
        }
    }
    let colors = LyricColors {
        accent: Rgb(255, 140, 60),
        text: Rgb(220, 220, 230),
        background: Rgb(14, 15, 18),
    };
    let audio = AudioFrame::default();
    let t = std::time::Instant::now();
    let runs = 200;
    let mut out = grid.clone();
    for _ in 0..runs {
        out = grid.clone();
        overlay(
            &mut out,
            art,
            0.5,
            &lyrics,
            pos,
            Some(20.0),
            view,
            colors,
            &audio,
            None,
        );
    }
    for y in 0..h {
        let line: String = (0..w).map(|x| out.get(x, y).expect("cell").ch).collect();
        println!("{}", line.trim_end());
    }
    eprintln!("{w}x{h}: {:?} per frame", t.elapsed() / runs);
}
