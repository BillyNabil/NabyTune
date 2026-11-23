//! Tries the artist and album pages the way the app opens them, as a visitor.
//! `cargo run --example browse -- <videoId> "Artist name"`

#[path = "../src/account.rs"]
#[allow(dead_code)]
mod account;
#[path = "../src/lyrics_sources.rs"]
#[allow(dead_code)]
mod lyrics_sources;
#[path = "../src/lyrics.rs"]
#[allow(dead_code)]
mod lyrics;
#[path = "../src/youtube.rs"]
#[allow(dead_code)]
mod youtube;
#[path = "../src/audio.rs"]
#[allow(dead_code)]
mod audio;
#[path = "../src/decoder.rs"]
#[allow(dead_code)]
mod decoder;
#[path = "../src/prefetch.rs"]
#[allow(dead_code)]
mod prefetch;
#[path = "../src/dsp.rs"]
#[allow(dead_code)]
mod dsp;
#[path = "../src/eq.rs"]
#[allow(dead_code)]
mod eq;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let video_id = args.next().unwrap_or_else(|| "fHI8X4OXluQ".into());
    let name = args.next().unwrap_or_else(|| "The Weeknd".into());
    let yt = youtube::YouTube::new();
    let links = yt.song_links(&video_id)?;
    println!("song_links({video_id}) = {links:?}");
    let artist_id = match links.0.clone() {
        Some(id) => id,
        None => yt.find_artist(&name)?.expect("an artist"),
    };
    println!("find_artist({name}) = {:?}", yt.find_artist(&name)?);
    let page = yt.artist(&artist_id)?;
    println!("artist: {} / {} / thumb {}", page.name, page.subtitle, page.thumbnail.is_some());
    for s in page.songs.iter().take(5) {
        println!("  song: {} â€” {} ({}) {}", s.title, s.artist, s.duration, s.video_id);
    }
    for (title, cards) in &page.shelves {
        println!("  shelf {title}: {} cards, first {:?}", cards.len(), cards.first().map(|c| (&c.title, &c.subtitle, &c.browse_id)));
    }
    if let Some(album) = links.1.or_else(|| page.shelves.iter().find(|(t, _)| t == "Albums").and_then(|(_, c)| c.first()).and_then(|c| c.browse_id.clone())) {
        let card = youtube::Card { title: "?".into(), subtitle: String::new(), thumbnail: None, browse_id: Some(album.clone()), video_id: None };
        let (header, tracks) = yt.collection(&album, &card)?;
        println!("album {album}: {header:?}");
        for t in tracks.iter().take(4) {
            println!("  {} â€” {} ({}) thumb {}", t.title, t.artist, t.duration, t.thumbnail.is_some());
        }
    }
    Ok(())
}
