//! Finds lyrics for a song the way the app does, then romanises a few lines.
//! `cargo run --release --example lyrics -- "Title" "Artist" seconds`

#[path = "../src/lyrics.rs"]
mod lyrics;
#[path = "../src/romaji.rs"]
mod romaji;

use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let title = args.next().expect("title");
    let artist = args.next().expect("artist");
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    // Optional: show the lines around this second instead of the first few.
    let around: Option<f64> = args.next().and_then(|s| s.parse().ok());
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(20)).build().unwrap();

    let started = Instant::now();
    let all = lyrics::fetch(&http, &title, &artist, Duration::from_secs(secs));
    println!("in {:?}: {}", started.elapsed(), all.iter().map(|f| format!("{} ({} lines)", f.source, f.lines.len())).collect::<Vec<_>>().join(", "));
    let Some(found) = all.into_iter().next() else {
        println!("no synced lyrics");
        return;
    };

    let texts: Vec<String> = found.lines.iter().map(|l| l.text.clone()).collect();
    let started = Instant::now();
    let romaji = romaji::romanize(&texts).expect("romanize");
    println!("romanised in {:?}", started.elapsed());
    let from = around.map_or(0.0, |t| t - 6.0);
    for (line, r) in found.lines.iter().zip(&romaji).filter(|(l, _)| !l.text.is_empty() && l.at.as_secs_f64() >= from).take(8) {
        let words = if line.words.is_empty() { String::new() } else { format!("  [{} timed units]", line.words.len()) };
        println!("  {:>6.2}s  {}{words}", line.at.as_secs_f64(), line.text);
        if let Some(r) = r {
            println!("           {r}");
        }
    }
}
