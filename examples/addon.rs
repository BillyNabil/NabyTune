//! Plays a song through a lossless addon the way the app does, without the window:
//! manifest, match, stream, download, decode.
//! `cargo run --release --example addon -- <addon url> "Title" "Artist" seconds`

#[path = "../src/addon.rs"]
mod addon;
#[path = "../src/decoder.rs"]
mod decoder;
#[path = "../src/lyrics.rs"]
mod lyrics;

use std::time::{Duration, Instant};

use rodio::Source;

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("addon url");
    let title = args.next().expect("title");
    let artist = args.next().expect("artist");
    let seconds: Option<f64> = args.next().and_then(|s| s.parse().ok());
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build().unwrap();

    let started = Instant::now();
    let found = addon::connect(&http, &url).expect("connect");
    println!("addon: {} v{} at {}", found.name, found.version, addon::redact(&found.base));
    for tier in [addon::Tier::Lossless, addon::Tier::HiRes] {
        let hit = addon::find(&http, &found, &title, &artist, seconds, tier).expect("search").expect("no match");
        let stream = found.stream(&http, &hit.id, tier).expect("stream");
        let bytes = addon::download(&http, &stream).expect("download");
        let source = decoder::AudioSource::new(bytes.clone()).expect("decode");
        println!(
            "{tier:?}: matched \"{}\" ({}), {} via {}, {} MB; decoder: {} Hz, {} ch, {:?}",
            hit.title,
            hit.id,
            stream.label(),
            if stream.transport.is_empty() { "file" } else { &stream.transport },
            bytes.len() / 1_000_000,
            source.sample_rate(),
            source.channels(),
            source.total_duration(),
        );
        // Decoded to the end: how long it really is.
        let frames = decoder::AudioSource::new(bytes).unwrap().count() / 2;
        println!("    decoded {:.1} s", frames as f64 / source.sample_rate() as f64);
    }
    println!("in {:?}", started.elapsed());
}
