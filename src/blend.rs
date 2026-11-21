//! Blend: two listeners' tastes in one playlist, and how alike they are.
//!
//! Each side's taste is worked out on its own PC: the songs it played most lately (from the
//! Stats log) with its hearts, and the artists behind them. The two meet through the same
//! relay as Listen along (listen_along.rs): the one who starts a Blend leaves their taste
//! under a short code; the friend who enters it reads it, leaves theirs, and both PCs mix
//! the same playlist. A taste is a few dozen songs and names, nothing more.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::stats::{self, Listen};

/// How far back the songs played count.
const RECENT_DAYS: u64 = 180;
/// Songs and artists a taste carries.
const SONGS: usize = 30;
const ARTISTS: usize = 20;
/// How long the Blend's playlist runs.
pub const LENGTH: usize = 50;
/// A taste fits one relay message (ntfy takes 4 KB), with room to spare.
const MAX_BYTES: usize = 3800;

/// A song in a taste, and how much it's played (hearts count too).
#[derive(Clone, Debug, PartialEq)]
pub struct TasteSong {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub thumbnail: Option<String>,
    pub weight: u32,
}

/// One listener's taste.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Taste {
    /// Random, per taste: the two sides sort by it, so both mix the same playlist.
    pub id: String,
    pub name: String,
    pub songs: Vec<TasteSong>,
    /// Artists, most played first, with their weight.
    pub artists: Vec<(String, u32)>,
}

/// A song as `liked` hands it over: (id, title, artist, thumbnail).
pub type LikedSong = (String, String, String, Option<String>);

impl Taste {
    /// The taste in `listens` (the Stats log) and `liked` (newest first), as of `now`
    /// (Unix seconds). This PC's own files are left out: a friend can't play them.
    pub fn from_listening(id: String, name: String, listens: &[Listen], liked: &[LikedSong], now: u64) -> Self {
        let since = now.saturating_sub(RECENT_DAYS * 86_400);
        let mut songs: HashMap<String, TasteSong> = HashMap::new();
        let mut artists: HashMap<String, (String, u32)> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        let mut add = |id: &str, title: &str, artist: &str, thumbnail: &Option<String>, weight: u32| {
            if id.is_empty() || id.starts_with("file:") {
                return;
            }
            let entry = songs.entry(id.to_string()).or_insert_with(|| {
                order.push(id.to_string());
                TasteSong { id: id.to_string(), title: title.to_string(), artist: artist.to_string(), thumbnail: thumbnail.clone(), weight: 0 }
            });
            entry.weight += weight;
            if entry.thumbnail.is_none() {
                entry.thumbnail = thumbnail.clone();
            }
            for name in stats::split_artists(artist) {
                let slot = artists.entry(name.to_lowercase()).or_insert_with(|| (name.clone(), 0));
                slot.1 += weight;
            }
        };
        for l in listens.iter().rev().filter(|l| l.at >= since && stats::counts(l)) {
            add(&l.video_id, &l.title, &l.artist, &l.thumbnail, 1);
        }
        // A heart is worth a few plays: newer ones a little more.
        for (i, (id, title, artist, thumbnail)) in liked.iter().enumerate().take(60) {
            add(id, title, artist, thumbnail, if i < 20 { 3 } else { 2 });
        }
        let mut songs: Vec<TasteSong> = order.into_iter().filter_map(|id| songs.remove(&id)).collect();
        // Most weight first; between equals, what came first (most recent) stays first.
        songs.sort_by_key(|s| std::cmp::Reverse(s.weight));
        songs.truncate(SONGS);
        let mut artists: Vec<(String, u32)> = artists.into_values().collect();
        artists.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        artists.truncate(ARTISTS);
        Self { id, name, songs, artists }
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }

    /// As sent: titles and names cut short, and as many artworks as fit one message.
    pub fn to_json(&self) -> String {
        let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
        let build = |songs: usize, thumbs: usize| {
            json!({
                "v": 1, "kind": "taste", "id": self.id, "name": clip(&self.name, 40),
                "songs": self.songs.iter().take(songs).enumerate().map(|(i, s)| {
                    let mut v = json!([s.id, clip(&s.title, 48), clip(&s.artist, 40), s.weight]);
                    if let (true, Some(t)) = (i < thumbs, &s.thumbnail) {
                        v.as_array_mut().unwrap().push(json!(short_thumbnail(t)));
                    }
                    v
                }).collect::<Vec<_>>(),
                "artists": self.artists.iter().map(|(a, w)| json!([clip(a, 40), w])).collect::<Vec<_>>(),
            })
            .to_string()
        };
        let mut thumbs = self.songs.len();
        let mut songs = self.songs.len();
        loop {
            let text = build(songs, thumbs);
            if text.len() <= MAX_BYTES || songs == 0 {
                return text;
            }
            if thumbs > 0 {
                thumbs = thumbs.saturating_sub(4);
            } else {
                songs -= 1;
            }
        }
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        if v["kind"].as_str() != Some("taste") {
            return None;
        }
        let songs = v["songs"]
            .as_array()?
            .iter()
            .filter_map(|s| {
                let id = s[0].as_str().filter(|id| !id.is_empty() && !id.starts_with("file:"))?;
                Some(TasteSong {
                    id: id.to_string(),
                    title: s[1].as_str().unwrap_or("").to_string(),
                    artist: s[2].as_str().unwrap_or("").to_string(),
                    weight: s[3].as_u64().unwrap_or(1).min(10_000) as u32,
                    thumbnail: s[4].as_str().map(long_thumbnail),
                })
            })
            .take(SONGS)
            .collect();
        let artists = v["artists"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| Some((x[0].as_str()?.to_string(), x[1].as_u64().unwrap_or(1).min(10_000) as u32))).take(ARTISTS).collect())
            .unwrap_or_default();
        Some(Self { id: v["id"].as_str().unwrap_or("").to_string(), name: v["name"].as_str().unwrap_or("").chars().take(40).collect(), songs, artists })
    }
}

/// YouTube Music's artwork links are long; most of one is the same every time.
const ART_HOST: &str = "https://lh3.googleusercontent.com/";

fn short_thumbnail(url: &str) -> String {
    // The size asked for goes too: it's asked for again when shown.
    let url = url.rsplit_once("=w").map_or(url, |(base, _)| base);
    url.strip_prefix(ART_HOST).map_or_else(|| url.to_string(), |rest| format!("~{rest}"))
}

fn long_thumbnail(short: &str) -> String {
    match short.strip_prefix('~') {
        Some(rest) => format!("{ART_HOST}{rest}=w120-h120-l90-rj"),
        None => short.to_string(),
    }
}

/// The two tastes mixed.
#[derive(Clone, Debug, PartialEq)]
pub struct Blend {
    /// How alike the two are, 0 to 100.
    pub score: u32,
    /// Songs both play, then turn about from each, LENGTH at most.
    pub songs: Vec<TasteSong>,
    /// Artists both play, most shared first.
    pub shared_artists: Vec<String>,
    /// How many songs both play.
    pub shared_songs: usize,
}

/// Mixes `a` and `b` (the same playlist whichever side does it).
pub fn mix(a: &Taste, b: &Taste) -> Blend {
    let (a, b) = if (a.id.as_str(), a.name.as_str()) <= (b.id.as_str(), b.name.as_str()) { (a, b) } else { (b, a) };
    let in_b: HashMap<&str, &TasteSong> = b.songs.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut common: Vec<(u32, TasteSong)> =
        a.songs.iter().filter_map(|s| in_b.get(s.id.as_str()).map(|t| (s.weight.min(t.weight) * 2 + s.weight.max(t.weight), s.clone()))).collect();
    common.sort_by_key(|c| std::cmp::Reverse(c.0));
    let shared_songs = common.len();
    let mut seen: HashSet<String> = HashSet::new();
    let mut songs: Vec<TasteSong> = Vec::new();
    for (_, s) in common {
        if seen.insert(s.id.clone()) {
            songs.push(s);
        }
    }
    // Then in turns, so each side has as many: a song of an artist both like goes first.
    let artists_a: HashSet<String> = a.artists.iter().map(|(n, _)| n.to_lowercase()).collect();
    let artists_b: HashSet<String> = b.artists.iter().map(|(n, _)| n.to_lowercase()).collect();
    let liked_by_other = |s: &TasteSong, other: &HashSet<String>| stats::split_artists(&s.artist).iter().any(|n| other.contains(&n.to_lowercase()));
    let ordered = |taste: &Taste, other: &HashSet<String>| -> Vec<TasteSong> {
        let (mut first, mut rest): (Vec<TasteSong>, Vec<TasteSong>) = taste.songs.iter().cloned().partition(|s| liked_by_other(s, other));
        first.append(&mut rest);
        first
    };
    let (from_a, from_b) = (ordered(a, &artists_b), ordered(b, &artists_a));
    let (mut ia, mut ib) = (from_a.into_iter(), from_b.into_iter());
    while songs.len() < LENGTH {
        let (x, y) = (ia.next(), ib.next());
        if x.is_none() && y.is_none() {
            break;
        }
        for s in [x, y].into_iter().flatten() {
            if songs.len() < LENGTH && seen.insert(s.id.clone()) {
                songs.push(s);
            }
        }
    }
    // Alike: how much the two lean on the same artists (cosine of their plays), with a
    // little more for songs both play.
    let weights = |t: &Taste| -> HashMap<String, f64> { t.artists.iter().map(|(n, w)| (n.to_lowercase(), *w as f64)).collect() };
    let (wa, wb) = (weights(a), weights(b));
    let dot: f64 = wa.iter().filter_map(|(n, x)| wb.get(n).map(|y| x * y)).sum();
    let norm = |w: &HashMap<String, f64>| w.values().map(|x| x * x).sum::<f64>().sqrt();
    let cosine = if norm(&wa) > 0.0 && norm(&wb) > 0.0 { dot / (norm(&wa) * norm(&wb)) } else { 0.0 };
    let song_share = shared_songs as f64 / a.songs.len().min(b.songs.len()).max(1) as f64;
    // Square roots lift the middle: two people who share a fifth of their listening already
    // have a fair bit in common.
    let score = (100.0 * (0.75 * cosine.sqrt() + 0.25 * song_share.sqrt())).round().clamp(0.0, 100.0) as u32;
    let mut shared: Vec<(String, f64)> = a
        .artists
        .iter()
        .filter_map(|(n, x)| wb.get(&n.to_lowercase()).map(|y| (n.clone(), (*x as f64).min(*y))))
        .collect();
    shared.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    Blend { score, songs, shared_artists: shared.into_iter().map(|(n, _)| n).take(5).collect(), shared_songs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listen(id: &str, artist: &str, at: u64) -> Listen {
        Listen { video_id: id.into(), title: format!("Song {id}"), artist: artist.into(), at, seconds: 200, length: 200, ..Default::default() }
    }

    fn taste(id: &str, name: &str, songs: &[(&str, &str, u32)]) -> Taste {
        let mut artists: HashMap<String, u32> = HashMap::new();
        for (_, a, w) in songs {
            *artists.entry(a.to_string()).or_default() += w;
        }
        let mut artists: Vec<(String, u32)> = artists.into_iter().collect();
        artists.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Taste {
            id: id.into(),
            name: name.into(),
            songs: songs.iter().map(|(s, a, w)| TasteSong { id: s.to_string(), title: s.to_string(), artist: a.to_string(), thumbnail: None, weight: *w }).collect(),
            artists,
        }
    }

    #[test]
    fn taste_counts_plays_and_hearts_lately() {
        let now = 1_800_000_000;
        let listens = vec![
            listen("a", "Tulus", now - 100),
            listen("a", "Tulus", now - 90),
            listen("b", "Hindia", now - 80),
            // Too long ago, a file of this PC's, and too short to count.
            listen("old", "Tulus", now - 400 * 86_400),
            listen("file:C:\\x.mp3", "Me", now - 10),
            Listen { seconds: 5, ..listen("skip", "Tulus", now - 5) },
        ];
        let liked = vec![("c".to_string(), "Song c".to_string(), "Nadin Amizah".to_string(), None)];
        let t = Taste::from_listening("x".into(), "Ana".into(), &listens, &liked, now);
        let ids: Vec<&str> = t.songs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
        assert_eq!(t.artists[0].0, "Nadin Amizah");
        assert!(t.artists.iter().any(|(n, w)| n == "Tulus" && *w == 2));
    }

    #[test]
    fn tastes_travel_in_one_message() {
        let mut t = taste("x", "Ana", &[("dQw4w9WgXcQ", "Rick Astley", 5)]);
        t.songs[0].thumbnail = Some("https://lh3.googleusercontent.com/abcDEF123=w120-h120-l90-rj".into());
        let back = Taste::from_json(&t.to_json()).unwrap();
        assert_eq!(back, t);
        // A big one still fits: artwork goes first, then songs, never the message limit.
        let many: Vec<(String, String)> = (0..30).map(|i| (format!("id{i:09}"), "A very long artist name for the test".to_string())).collect();
        let mut big = taste("y", "Budi", &many.iter().map(|(id, a)| (id.as_str(), a.as_str(), 1)).collect::<Vec<_>>());
        for s in &mut big.songs {
            s.title = "A rather long song title, as some songs have them".into();
            s.thumbnail = Some(format!("https://lh3.googleusercontent.com/{}=w120-h120", "x".repeat(110)));
        }
        let text = big.to_json();
        assert!(text.len() <= MAX_BYTES, "{} bytes", text.len());
        assert!(Taste::from_json(&text).unwrap().songs.len() >= 20);
        assert_eq!(Taste::from_json("{\"kind\":\"other\"}"), None);
    }

    #[test]
    fn mixes_both_sides_the_same_way() {
        let a = taste("1", "Ana", &[("s1", "Tulus", 9), ("s2", "Tulus", 5), ("s3", "Hindia", 4), ("a1", "Raisa", 2)]);
        let b = taste("2", "Budi", &[("s2", "Tulus", 7), ("b1", "Sheila on 7", 6), ("b2", "Tulus", 3), ("b3", "Dewa 19", 1)]);
        let blend = mix(&a, &b);
        assert_eq!(blend, mix(&b, &a), "the same playlist on both PCs");
        // The song both play leads; then turns, songs of shared artists first.
        let ids: Vec<&str> = blend.songs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids[0], "s2");
        assert_eq!(ids.len(), 7);
        assert_eq!(blend.shared_songs, 1);
        assert_eq!(blend.shared_artists, vec!["Tulus".to_string()]);
        assert!(blend.score > 20 && blend.score < 100, "score {}", blend.score);
        // The same taste: a full match. Nothing alike: none.
        assert_eq!(mix(&a, &Taste { id: "3".into(), ..a.clone() }).score, 100);
        let c = taste("4", "Cici", &[("c1", "Mozart", 3)]);
        assert_eq!(mix(&a, &c).score, 0);
    }
}
