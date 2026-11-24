//! Looks up a track's tempo the way remote sources do (Deezer, then preview
//! analysis).
//!
//! cargo run -p cw-sources --release --example tempo_lookup -- "ARTIST" "TITLE" [ISRC]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: tempo_lookup ARTIST TITLE [ISRC]");
        std::process::exit(2);
    }
    let now = cw_sources::NowPlaying {
        track_id: "cli".into(),
        title: args[1].clone(),
        artist: Some(args[0].clone()),
        album: None,
        duration_secs: None,
        position_secs: 0.0,
        playing: true,
        volume: None,
        art: cw_sources::ArtRef::None,
        genre: None,
        isrc: args.get(2).cloned(),
        bpm: None,
    };
    match cw_sources::tempo::lookup(&now) {
        Some(bpm) => println!("{} - {}: {bpm:.1} BPM", args[0], args[1]),
        None => println!("{} - {}: no tempo found", args[0], args[1]),
    }
}
