//! Counts the songs in the signed-in account's playlists (and its liked songs), every page of
//! them, to check long playlists come back whole.
//! `cargo run --example playlists`

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
#[path = "../src/dsp.rs"]
#[allow(dead_code)]
mod dsp;
#[path = "../src/eq.rs"]
#[allow(dead_code)]
mod eq;

fn main() -> anyhow::Result<()> {
    let yt = youtube::YouTube::new();
    let Some(cookie) = account::load() else {
        println!("not signed in");
        return Ok(());
    };
    yt.set_account(Some(cookie));
    // `playlists dump <browseId>`: the page's JSON, and its first continuation's, to look at.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("dump") {
        let id = args.get(1).cloned().unwrap_or_else(|| "VLLM".into());
        let page = yt.browse_raw(serde_json::json!({ "browseId": id }))?;
        std::fs::write(std::env::temp_dir().join("naby-page.json"), serde_json::to_string_pretty(&page)?)?;
        println!("wrote {}", std::env::temp_dir().join("naby-page.json").display());
        return Ok(());
    }
    // `playlists search <query>` and `playlists resolve <videoId>`: what a search finds, and
    // whether a song's stream can be had.
    if args.first().map(String::as_str) == Some("search") {
        for item in yt.search(&args[1..].join(" "))?.iter().take(8) {
            println!("{} | {} | {} | {}", item.video_id, item.title, item.artist, item.duration);
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("resolve") {
        match yt.resolve(&args[1]) {
            Ok(s) => match yt.download(&s) {
                Ok(bytes) => println!("ok: {} of {} bytes fetched, {:?}", bytes.len(), s.content_length, s.length),
                Err(e) => println!("resolved, but fetching failed: {e:#}"),
            },
            Err(e) => println!("failed: {e:#}"),
        }
        return Ok(());
    }
    // `playlists home`: Home's shelves, every page of them.
    if args.first().map(String::as_str) == Some("home") {
        let started = std::time::Instant::now();
        let shelves = yt.home()?;
        for shelf in &shelves {
            println!("{}: {} items", shelf.title, shelf.items.len());
        }
        println!("{} shelves in {:.1} s", shelves.len(), started.elapsed().as_secs_f32());
        return Ok(());
    }
    // `playlists queue <playlistId>`: the playlist as a player's queue (the `next` endpoint),
    // page by page.
    if args.first().map(String::as_str) == Some("queue") {
        let id = args.get(1).cloned().unwrap_or_else(|| "LM".into());
        let mut page = yt.radio_start(None, &id, None)?;
        let mut ids: Vec<String> = page.tracks.iter().map(|t| t.video_id.clone()).collect();
        println!("page 1: {} songs, continuation {}", page.tracks.len(), page.continuation.is_some());
        for n in 2..=20 {
            let Some(token) = page.continuation.clone() else { break };
            page = yt.radio_more(&id, &token)?;
            let new = page.tracks.iter().filter(|t| !ids.contains(&t.video_id)).count();
            ids.extend(page.tracks.iter().map(|t| t.video_id.clone()));
            println!("page {n}: {} songs, {new} new, continuation {}", page.tracks.len(), page.continuation.is_some());
        }
        ids.sort();
        ids.dedup();
        println!("distinct: {}", ids.len());
        return Ok(());
    }
    let started = std::time::Instant::now();
    let liked = yt.liked_songs()?;
    println!("Liked music: {} songs ({:.1} s)", liked.len(), started.elapsed().as_secs_f32());
    for card in yt.account_playlists()? {
        let Some(id) = card.browse_id.clone() else { continue };
        let started = std::time::Instant::now();
        // YouTube Music's own count ("41 songs • 2 hours"), and how many rows its page has.
        let raw = yt.browse_raw(serde_json::json!({ "browseId": id }))?;
        let text = raw.to_string();
        fn find<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
            match v {
                serde_json::Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find(x, key))),
                serde_json::Value::Array(a) => a.iter().find_map(|x| find(x, key)),
                _ => None,
            }
        }
        let count = find(&raw, "secondSubtitle")
            .and_then(|s| s["runs"].as_array())
            .map(|runs| runs.iter().filter_map(|r| r["text"].as_str()).collect::<String>())
            .unwrap_or_else(|| "?".into());
        let rows = text.matches("\"musicResponsiveListItemRenderer\"").count();
        match yt.collection(&id, &card) {
            Ok((_, tracks)) => println!("{} [{id}]: {} shown · {rows} rows · YouTube says \"{count}\" ({:.1} s)", card.title, tracks.len(), started.elapsed().as_secs_f32()),
            Err(e) => println!("{} [{id}]: {e} · {rows} rows · YouTube says \"{count}\"", card.title),
        }
    }
    Ok(())
}
