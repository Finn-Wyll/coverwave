//! Looks a song up on LRCLIB and prints the first lines:
//! `cargo run -p cw-sources --example lyrics_lookup -- "ARTIST" "TITLE" [SECONDS]`

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(artist), Some(title)) = (args.first(), args.get(1)) else {
        eprintln!("usage: lyrics_lookup ARTIST TITLE [SECONDS]");
        std::process::exit(2);
    };
    let secs = args.get(2).and_then(|s| s.parse().ok());
    match cw_sources::lrclib::fetch(artist, title, None, secs) {
        Some(cw_sources::lrclib::Found::Lyrics(text)) => {
            for line in text.lines().take(6) {
                println!("{line}");
            }
            println!("... ({} lines)", text.lines().count());
        }
        Some(cw_sources::lrclib::Found::Instrumental) => println!("instrumental"),
        None => println!("not found"),
    }
}
