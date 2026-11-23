//! Tries the endless radio as a visitor: a song's radio and its chips, two continuations, a
//! chip's flavour, a refill past the point where continuations repeat, and an artist's radio.
//! `cargo run --example radio -- <videoId> "Artist name"`

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

use std::collections::HashSet;

use youtube::{RadioPage, SearchItem, YouTube};

fn show(tracks: &[SearchItem], n: usize) {
    for t in tracks.iter().take(n) {
        println!("    {} | {} | {} | {} ({})", t.video_id, t.title, t.artist, t.album, t.duration);
    }
}

fn chips(page: &RadioPage) -> String {
    page.chips.iter().map(|c| if c.selected { format!("[{}]", c.label) } else { c.label.clone() }).collect::<Vec<_>>().join(", ")
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let video_id = args.next().unwrap_or_else(|| "fHI8X4OXluQ".into());
    let name = args.next().unwrap_or_else(|| "The Weeknd".into());
    let yt = YouTube::new();

    // radio() as autoplay uses it.
    let auto = yt.radio(&video_id)?;
    println!("radio({video_id}): {} songs, seed left out: {}", auto.len(), !auto.iter().any(|t| t.video_id == video_id));

    // A song's radio.
    let start = yt.radio_start(Some(video_id.as_str()), &format!("RDAMVM{video_id}"), None)?;
    println!("\nradio_start: {} songs, playlist {}, continuation {}", start.tracks.len(), start.playlist_id, start.continuation.is_some());
    println!("  chips: {}", chips(&start));
    for c in &start.chips {
        println!("    {:<14} {} params {}", c.label, c.playlist_id, c.params.as_deref().map_or("-", |p| &p[..p.len().min(24)]));
    }
    show(&start.tracks, 6);

    // Two continuations: how many are new each time.
    let mut seen: Vec<String> = start.tracks.iter().map(|t| t.video_id.clone()).collect();
    let mut token = start.continuation.clone();
    for i in 1..=2 {
        let Some(t) = token.clone() else { println!("no continuation"); break };
        let more = yt.radio_more(&start.playlist_id, &t)?;
        let new: Vec<&SearchItem> = more.tracks.iter().filter(|t| !seen.contains(&t.video_id)).collect();
        println!("\nradio_more #{i}: {} songs, {} new, chips {}, continuation {}", more.tracks.len(), new.len(), more.chips.len(), more.continuation.is_some());
        for t in new.iter().take(4) {
            println!("    new: {} | {} | {}", t.video_id, t.title, t.artist);
        }
        seen.extend(new.iter().map(|t| t.video_id.clone()));
        token = more.continuation;
    }

    // A chip's flavour.
    let pick = start.chips.iter().find(|c| c.label == "Discover").or_else(|| start.chips.iter().find(|c| !c.selected));
    if let Some(chip) = pick {
        let flavoured = yt.radio_start(None, &chip.playlist_id, chip.params.as_deref())?;
        let all: HashSet<&str> = start.tracks.iter().map(|t| t.video_id.as_str()).collect();
        let shared = flavoured.tracks.iter().filter(|t| all.contains(t.video_id.as_str())).count();
        println!(
            "\nchip \"{}\": {} songs ({} also in the first \"All\" batch), playlist {}, chips {}, continuation {}",
            chip.label,
            flavoured.tracks.len(),
            shared,
            flavoured.playlist_id,
            flavoured.chips.len(),
            flavoured.continuation.is_some()
        );
        show(&flavoured.tracks, 5);
        if let Some(t) = &flavoured.continuation {
            let more = yt.radio_more(&flavoured.playlist_id, t)?;
            println!("  its continuation: {} songs, {} new", more.tracks.len(), more.tracks.iter().filter(|x| !flavoured.tracks.iter().any(|f| f.video_id == x.video_id)).count());
        }
    }

    // Refill until the continuations go round, then past it.
    println!("\nradio_refill, 20 at a time:");
    let (mut playlist, mut token, mut chip) = (start.playlist_id.clone(), token, None::<String>);
    for round in 1..=6 {
        let page = yt.radio_refill(&playlist, token.as_deref(), chip.as_deref(), &seen, 20)?;
        let restarted = !page.chips.is_empty();
        println!(
            "  #{round}: {} new, playlist {}{}",
            page.tracks.len(),
            page.playlist_id,
            if restarted { format!(", restarted, chips: {}", chips(&page)) } else { String::new() }
        );
        if let Some(t) = page.tracks.first() {
            println!("    first: {} | {}", t.title, t.artist);
        }
        let dupes = page.tracks.iter().filter(|t| seen.contains(&t.video_id)).count();
        assert_eq!(dupes, 0, "refill handed back a song already seen");
        seen.extend(page.tracks.iter().map(|t| t.video_id.clone()));
        playlist = page.playlist_id;
        token = page.continuation;
        // Steer to "Discover" after the third round.
        if round == 3 {
            chip = Some("Discover".into());
            println!("  (steering to Discover)");
        }
    }
    println!("  {} songs seen in all, none twice", seen.len());

    // An artist's radio.
    let artist = yt.find_artist(&name)?.ok_or_else(|| anyhow::anyhow!("no artist {name}"))?;
    let page = yt.artist(&artist)?;
    println!("\nartist {} ({artist}): radio {:?}, shuffle {:?}", page.name, page.radio, page.shuffle);
    println!("artist_radio = {:?}", yt.artist_radio(&artist)?);
    for (what, id) in [("radio", &page.radio), ("shuffle", &page.shuffle)] {
        if let Some(id) = id {
            let r = yt.radio_start(None, id, None)?;
            println!("  {what}: {} songs, chips {}, continuation {}", r.tracks.len(), chips(&r), r.continuation.is_some());
            show(&r.tracks, 4);
        }
    }
    Ok(())
}
