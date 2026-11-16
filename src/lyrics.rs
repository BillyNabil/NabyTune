//! Synced lyrics, from the same providers NabyTune's web player asks
//! (web/app/src/lyrics/providers.ts), in its priority order:
//!
//! 1. LyricsPlus: Apple Music timings syllable by syllable, on community mirrors.
//! 2. LRCLIB: whole lines, always up.
//! 3. KuGou: whole lines, strong outside the English catalogue (Japanese, Chinese, Korean).
//!
//! The three are asked at once; the best one that answers wins. YouTube Music's own,
//! unsynced lyrics are the last resort (youtube.rs).

use std::time::Duration;

use anyhow::Result;
use base64::Engine as _;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Line {
    pub at: Duration,
    /// Empty for an instrumental gap.
    pub text: String,
    /// Each word's own timing, where the source has it (LyricsPlus); empty for whole lines.
    pub words: Vec<Word>,
}

impl Line {
    pub fn new(at: Duration, text: String) -> Self {
        Self { at, text, words: Vec::new() }
    }
}

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub at: Duration,
    pub end: Duration,
    /// A space follows it. Japanese and Chinese run on without one.
    pub gap: bool,
}

/// Whether lyrics carry real timings for a song `secs` long. Sources answer with fakes:
/// every line at 0:00 (Apple Music's unsynced text, through LyricsPlus), or plain text
/// stamped one second apart (an LRCLIB upload), which would race through the song's first
/// half-minute.
pub fn timed(lines: &[Line], secs: u64) -> bool {
    let sung: Vec<Duration> = lines.iter().filter(|l| !l.text.is_empty()).map(|l| l.at).collect();
    let mut distinct = sung.clone();
    distinct.dedup();
    if sung.len() < 2 || distinct.len() * 2 < sung.len() || sung.last().is_none_or(|t| *t <= Duration::from_secs(1)) {
        return false;
    }
    // Stamped at an even step: a counter, not timings.
    let steps: Vec<Duration> = distinct.windows(2).map(|w| w[1] - w[0]).collect();
    if steps.len() >= 8 && steps.iter().all(|s| s.abs_diff(steps[0]) <= Duration::from_millis(20)) {
        return false;
    }
    // Lines that all fit in a sliver of the song.
    let span = (*distinct.last().unwrap() - distinct[0]).as_secs_f64();
    !(secs >= 60 && sung.len() >= 8 && span < secs as f64 * 0.3)
}

/// Lyrics as found: their lines, whether they're timed, and who had them.
#[derive(Clone, Debug)]
pub struct Found {
    pub lines: Vec<Line>,
    pub synced: bool,
    pub source: &'static str,
}

const AGENT: &str = "NabyTune prototype (https://github.com/BillyNabil)";
const LYRICSPLUS_MIRRORS: [&str; 2] = ["https://lyricsplus.binimum.org", "https://lyricsplus.prjktla.my.id"];

/// "(feat. …)", "(Official Video)", "[…]" and the like, which lyric databases' titles don't carry.
pub(crate) fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' | '[' | '（' | '【' => depth += 1,
            ')' | ']' | '）' | '】' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.split(" | ").next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
    if out.is_empty() { s.trim().to_string() } else { out }
}

/// `[mm:ss.xx]text` lines, a line may carry several stamps; tags like `[ar:…]` are skipped.
pub fn parse_lrc(lrc: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(body) = rest.strip_prefix('[') {
            let Some(end) = body.find(']') else { break };
            let stamp = &body[..end];
            rest = &body[end + 1..];
            let mut parts = stamp.splitn(2, ':');
            let (Some(m), Some(s)) = (parts.next(), parts.next()) else { continue };
            if let (Ok(m), Ok(s)) = (m.trim().parse::<u64>(), s.trim().replace(',', ".").parse::<f64>()) {
                stamps.push(Duration::from_secs(m * 60) + Duration::from_secs_f64(s));
            }
        }
        let text = rest.trim().to_string();
        for at in stamps {
            lines.push(Line::new(at, text.clone()));
        }
    }
    lines.sort_by_key(|l| l.at);
    // Collapse a run of empty lines into one gap.
    lines.dedup_by(|b, a| a.text.is_empty() && b.text.is_empty());
    lines
}

fn get_json(http: &reqwest::blocking::Client, url: &str, query: &[(&str, String)], timeout: Duration) -> Result<Value> {
    let response = http.get(url).query(query).header("user-agent", AGENT).timeout(timeout).send()?;
    if response.status().as_u16() == 404 {
        return Ok(Value::Null);
    }
    Ok(response.error_for_status()?.json()?)
}

fn lrclib(http: &reqwest::blocking::Client, title: &str, artist: &str, secs: u64) -> Option<Vec<Line>> {
    let t = Duration::from_secs(8);
    // Anyone can upload to LRCLIB, so an entry is taken only if its timings hold up.
    let usable = |lrc: Option<&str>| lrc.filter(|s| !s.trim().is_empty()).map(parse_lrc).filter(|lines| timed(lines, secs));
    let exact = get_json(http, "https://lrclib.net/api/get", &[("track_name", title.into()), ("artist_name", artist.into()), ("duration", secs.to_string())], t).ok()?;
    if let Some(lines) = usable(exact["syncedLyrics"].as_str()) {
        return Some(lines);
    }
    // The closest-length hit with usable timings, within 10 s.
    let pick = |hits: &Value| -> Option<Vec<Line>> {
        let mut hits: Vec<(&Value, f64)> =
            hits.as_array()?.iter().map(|h| (h, (h["duration"].as_f64().unwrap_or(0.0) - secs as f64).abs())).filter(|(_, off)| *off <= 10.0).collect();
        hits.sort_by(|a, b| a.1.total_cmp(&b.1));
        hits.into_iter().find_map(|(h, _)| usable(h["syncedLyrics"].as_str()))
    };
    let hits = get_json(http, "https://lrclib.net/api/search", &[("track_name", title.into()), ("artist_name", artist.into())], t).ok()?;
    if let Some(lines) = pick(&hits) {
        return Some(lines);
    }
    let hits = get_json(http, "https://lrclib.net/api/search", &[("q", format!("{title} {artist}"))], t).ok()?;
    if let Some(lines) = pick(&hits) {
        return Some(lines);
    }
    // "好きでいて - Suki de Ite": YouTube's title carries the reading after a dash; LRCLIB's
    // carries the title alone.
    let (short, _) = title.split_once(" - ")?;
    let hits = get_json(http, "https://lrclib.net/api/search", &[("track_name", short.trim().into()), ("artist_name", artist.into())], t).ok()?;
    pick(&hits)
}

/// Syllables ("I've ", "been ", or "jus", "ti", "fy ") into words: a syllable ending in a
/// space closes its word. Japanese and Chinese have no spaces to go by, so each of their
/// syllables stays its own unit and lights on its own. Times in milliseconds.
fn words_of(syllables: &[Value]) -> Vec<Word> {
    let ms = |v: f64| Duration::from_secs_f64(v.max(0.0) / 1000.0);
    let mut words: Vec<Word> = Vec::new();
    let mut open = false;
    for s in syllables {
        let (Some(text), Some(time)) = (s["text"].as_str(), s["time"].as_f64()) else { continue };
        let end = ms(time + s["duration"].as_f64().unwrap_or(0.0));
        let gap = text.ends_with(char::is_whitespace);
        let unspaced = text.chars().any(|c| (c as u32) >= 0x2E80);
        match words.last_mut() {
            Some(word) if open && !unspaced => {
                word.text.push_str(text.trim_end());
                word.end = end;
                word.gap = gap;
            }
            _ => words.push(Word { text: text.trim().to_string(), at: ms(time), end, gap }),
        }
        open = !gap && !unspaced;
    }
    words.retain(|w| !w.text.is_empty());
    words
}

/// LyricsPlus: lines with syllable timings; the line starts at its first syllable.
fn lyricsplus(http: &reqwest::blocking::Client, title: &str, artist: &str, secs: u64) -> Option<Vec<Line>> {
    for host in LYRICSPLUS_MIRRORS {
        let Ok(body) = get_json(
            http,
            &format!("{host}/v2/lyrics/get"),
            &[("title", title.into()), ("artist", artist.into()), ("duration", secs.to_string())],
            Duration::from_secs(7),
        ) else {
            continue;
        };
        let mut lines: Vec<Line> = body["lyrics"]
            .as_array()
            .map(|lyrics| {
                lyrics
                    .iter()
                    .filter_map(|l| {
                        let time = l["time"].as_f64()?;
                        let syllables: String = l["syllabus"]
                            .as_array()
                            .map(|s| s.iter().filter_map(|x| x["text"].as_str()).collect())
                            .unwrap_or_default();
                        let text = if syllables.trim().is_empty() { l["text"].as_str().unwrap_or("").to_string() } else { syllables };
                        let start = l["syllabus"][0]["time"].as_f64().map_or(time, |s| s.min(time));
                        let words = l["syllabus"].as_array().map(|s| words_of(s)).unwrap_or_default();
                        Some(Line {
                            at: Duration::from_secs_f64(start.max(0.0) / 1000.0),
                            text: text.split_whitespace().collect::<Vec<_>>().join(" "),
                            words,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        lines.sort_by_key(|l| l.at);
        if lines.iter().any(|l| !l.text.is_empty()) {
            return Some(lines);
        }
    }
    None
}

/// Whether two artist credits name the same artist: one inside the other, or a shared word
/// of three letters or more. A song of the same title and length by someone else (KuGou's
/// "Stay Alive" by José González for Rie Takahashi's) must not pass.
pub(crate) fn same_artist(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>();
    let (a, b) = (norm(a), norm(b));
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.contains(b) || b.contains(a) {
        return true;
    }
    a.split_whitespace().filter(|w| w.chars().count() >= 3).any(|w| b.split_whitespace().any(|v| v == w))
}

/// KuGou opens and closes its files with credit lines ("作词: …") stamped like lyrics.
fn strip_kugou_credits(lrc: &str) -> String {
    let stamped: Vec<&str> = lrc.lines().filter(|l| l.len() > 10 && l.starts_with('[') && l.as_bytes().get(3) == Some(&b':')).collect();
    let credit = |l: &str| l.split_once(']').is_some_and(|(_, rest)| rest.contains(':') || rest.contains('：'));
    // Each end is searched only within its own half, so a short file's closing credits
    // aren't taken for opening ones (which would drop every line in between).
    let head = stamped.iter().take(30.min(stamped.len() / 2)).rposition(|l| credit(l)).map_or(0, |i| i + 1);
    let body = &stamped[head.min(stamped.len())..];
    let tail = body.iter().rev().take(30.min(body.len() / 2)).position(|l| credit(l)).map_or(0, |i| i + 1);
    body[..body.len() - tail.min(body.len())].join("\n")
}

fn kugou(http: &reqwest::blocking::Client, title: &str, artist: &str, secs: u64) -> Option<Vec<Line>> {
    let t = Duration::from_secs(8);
    let keyword = format!("{title} - {artist}");
    let songs = get_json(
        http,
        "https://mobileservice.kugou.com/api/v3/search/song",
        &[("version", "9108".into()), ("plat", "0".into()), ("pagesize", "8".into()), ("showtype", "0".into()), ("keyword", keyword.clone())],
        t,
    )
    .ok()?;
    let mut hashes: Vec<(u64, String)> = songs["data"]["info"]
        .as_array()
        .map(|info| {
            info.iter()
                .filter_map(|s| {
                    let duration = s["duration"].as_u64()?;
                    let hash = s["hash"].as_str()?;
                    let off = duration.abs_diff(secs);
                    // The right song, not just the right title: the artist has to match too.
                    let singer = s["singername"].as_str().unwrap_or("");
                    (off <= 8 && same_artist(singer, artist)).then(|| (off, hash.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    hashes.sort();
    let search = |query: &[(&str, String)]| -> Option<(String, String)> {
        let found = get_json(http, "https://lyrics.kugou.com/search", query, t).ok()?;
        let c = &found["candidates"][0];
        // A keyword search can land on anyone's song of that name: check whose it is.
        if c["singer"].as_str().is_some_and(|singer| !same_artist(singer, artist)) {
            return None;
        }
        Some((c["id"].as_str()?.to_string(), c["accesskey"].as_str()?.to_string()))
    };
    let base = || vec![("ver", "1".to_string()), ("man", "yes".to_string()), ("client", "pc".to_string())];
    let candidate = hashes
        .iter()
        .find_map(|(_, hash)| search(&[base(), vec![("hash", hash.clone())]].concat()))
        .or_else(|| search(&[base(), vec![("keyword", keyword.clone()), ("duration", (secs * 1000).to_string())]].concat()))?;
    let download = get_json(
        http,
        "https://lyrics.kugou.com/download",
        &[
            ("fmt", "lrc".into()),
            ("charset", "utf8".into()),
            ("client", "pc".into()),
            ("ver", "1".into()),
            ("id", candidate.0),
            ("accesskey", candidate.1),
        ],
        t,
    )
    .ok()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(download["content"].as_str()?).ok()?;
    let lrc = strip_kugou_credits(&String::from_utf8_lossy(&bytes));
    let lines = parse_lrc(&lrc);
    // "纯音乐，请欣赏": KuGou's notice for an instrumental, not lyrics.
    if lines.iter().all(|l| l.text.is_empty() || (l.text.contains("纯音乐") && l.text.contains("欣赏"))) {
        return None;
    }
    Some(lines)
}

/// The three synced sources at once: every one whose lines are really timed, best first
/// (LyricsPlus, LRCLIB, KuGou). The first is shown; the rest can be picked instead.
pub fn fetch(http: &reqwest::blocking::Client, title: &str, artist: &str, length: Duration) -> Vec<Found> {
    let (title, artist) = (clean(title), clean(artist));
    let secs = length.as_secs();
    let (plus, lrc, kg) = std::thread::scope(|s| {
        let plus = s.spawn(|| lyricsplus(http, &title, &artist, secs));
        let lrc = s.spawn(|| lrclib(http, &title, &artist, secs));
        let kg = s.spawn(|| kugou(http, &title, &artist, secs));
        (plus.join().ok().flatten(), lrc.join().ok().flatten(), kg.join().ok().flatten())
    });
    [(plus, "LyricsPlus"), (lrc, "LRCLIB"), (kg, "KuGou")]
        .into_iter()
        .filter_map(|(lines, source)| lines.filter(|l| timed(l, secs)).map(|lines| Found { lines: one_per_time(lines), synced: true, source }))
        .collect()
}

/// Japanese, Chinese or Korean letters in it.
fn east_asian(text: &str) -> bool {
    text.chars().any(|c| (c as u32) >= 0x2E80)
}

/// Lyric files often carry a second line at the same time stamp: the line's romaji, or a
/// translation. Shown as lines of their own, the companion would take the highlight from the
/// sung line. One line per time stamp: the one in the song's own script.
fn one_per_time(lines: Vec<Line>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    for line in lines {
        match out.last_mut() {
            Some(kept) if !kept.text.is_empty() && !line.text.is_empty() && line.at.abs_diff(kept.at) <= Duration::from_millis(30) => {
                if east_asian(&line.text) && !east_asian(&kept.text) {
                    *kept = line;
                }
            }
            _ => out.push(line),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stamps_tags_and_gaps() {
        let lines = parse_lrc("[ar:Someone]\n[00:01.50]Hello\n[00:03.00][00:10.25]Again\n[00:05.00]\n[00:06.00]\n[00:07.00]World");
        let got: Vec<(u128, &str)> = lines.iter().map(|l| (l.at.as_millis(), l.text.as_str())).collect();
        assert_eq!(got, vec![(1500, "Hello"), (3000, "Again"), (5000, ""), (7000, "World"), (10250, "Again")]);
    }

    #[test]
    fn rejects_untimed_lyrics() {
        let at = |s: u64, t: &str| Line::new(Duration::from_secs(s), t.into());
        assert!(!timed(&[at(0, "Hmm"), at(0, "Stay here"), at(0, "Hold me now")], 200));
        assert!(timed(&[at(0, ""), at(12, "Hello"), at(15, "Again"), at(19, "World")], 0));
        assert!(!timed(&[at(0, "Only one")], 200));
    }

    #[test]
    fn rejects_counter_stamps() {
        // Plain text stamped 0:00, 0:01, 0:02…, as one LRCLIB upload of a four-minute song is.
        let counted: Vec<Line> = (0..34).map(|s| Line::new(Duration::from_secs(s), format!("line {s}"))).collect();
        assert!(!timed(&counted, 244));
        // Real timings: uneven, across the song.
        let real: Vec<Line> = [13.2, 17.9, 21.4, 26.0, 30.5, 36.1, 41.8, 92.2, 120.4, 180.0, 215.3]
            .iter()
            .map(|s| Line::new(Duration::from_secs_f64(*s), "sung".into()))
            .collect();
        assert!(timed(&real, 244));
    }

    #[test]
    fn joins_syllables_into_words() {
        let syllables: Value = serde_json::json!([
            { "text": "I've ", "time": 30189, "duration": 207 },
            { "text": "jus", "time": 30396, "duration": 100 },
            { "text": "ti", "time": 30496, "duration": 100 },
            { "text": "fied", "time": 30596, "duration": 200 },
        ]);
        let words = words_of(syllables.as_array().unwrap());
        let got: Vec<(&str, u128, u128)> = words.iter().map(|w| (w.text.as_str(), w.at.as_millis(), w.end.as_millis())).collect();
        assert_eq!(got, vec![("I've", 30189, 30396), ("justified", 30396, 30796)]);
    }

    #[test]
    fn keeps_one_line_per_time_stamp() {
        let lines = parse_lrc(
            "[01:13.14]その瞳がその言葉が\n[01:13.14]Sono hitomi ga sono kotoba ga\n[01:15.52]Uso demo sore wa kanzen na ai\n[01:15.52]嘘でもそれは完全なアイ\n[01:18.47]はいはいあの子は特別です",
        );
        let got: Vec<String> = one_per_time(lines).into_iter().map(|l| l.text).collect();
        assert_eq!(got, vec!["その瞳がその言葉が", "嘘でもそれは完全なアイ", "はいはいあの子は特別です"]);
    }

    #[test]
    fn keeps_japanese_syllables_apart() {
        let syllables: Value = serde_json::json!([
            { "text": "夢", "time": 1000, "duration": 300 },
            { "text": "な", "time": 1300, "duration": 200 },
            { "text": "ら ", "time": 1500, "duration": 200 },
            { "text": "ば", "time": 1700, "duration": 400 },
        ]);
        let words = words_of(syllables.as_array().unwrap());
        let got: Vec<(&str, bool)> = words.iter().map(|w| (w.text.as_str(), w.gap)).collect();
        assert_eq!(got, vec![("夢", false), ("な", false), ("ら", true), ("ば", false)]);
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean("Stay Alive (From \"Re:Zero\")"), "Stay Alive");
        assert_eq!(clean("エミリア（CV:高橋李依）"), "エミリア");
        assert_eq!(clean("Song [Official Video] | Extra"), "Song");
    }

    #[test]
    fn matches_artists() {
        assert!(same_artist("Kenshi Yonezu", "Kenshi Yonezu"));
        assert!(same_artist("米津玄師", "米津玄師、Kenshi Yonezu"));
        assert!(same_artist("Ade Govinda, Gloria Jessica", "Gloria Jessica"));
        assert!(!same_artist("José González", "Rie Takahashi"));
        assert!(!same_artist("", "Hindia"));
    }

    #[test]
    fn strips_kugou_credits() {
        let lrc = "[00:00.00]Title - Artist\n[00:01.00]作词：Someone\n[00:02.00]作曲：Someone\n[00:10.00]First line\n[00:20.00]Second line\n[03:00.00]制作人：Someone";
        assert_eq!(strip_kugou_credits(lrc), "[00:10.00]First line\n[00:20.00]Second line");
    }
}


// ---- Kept on disk ------------------------------------------------------------------------

/// Every source a cached version can name (cached versions come back with these).
const SOURCES: [&str; 4] = ["LyricsPlus", "LRCLIB", "KuGou", "YouTube Music"];

/// How long found lyrics are kept before they're asked for again.
const KEEP: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

fn cache_file(video_id: &str) -> Option<std::path::PathBuf> {
    let safe: String = video_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (!safe.is_empty()).then(|| Some(std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("NabyTune").join("lyrics").join(format!("{safe}.json")))).flatten()
}

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

/// The lyrics found for a song before, if they're recent enough (none found counts too:
/// an empty list, so a song without lyrics isn't asked about every time).
pub fn cached(video_id: &str) -> Option<Vec<Found>> {
    let path = cache_file(video_id)?;
    let age = std::fs::metadata(&path).ok()?.modified().ok()?.elapsed().unwrap_or_default();
    let keep = if std::fs::read_to_string(&path).ok()?.contains("\"versions\":[]") { KEEP / 10 } else { KEEP };
    if age > keep {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    let at = |v: &serde_json::Value, k: &str| Duration::from_millis(v[k].as_u64().unwrap_or(0));
    v["versions"]
        .as_array()?
        .iter()
        .map(|f| {
            let source = SOURCES.iter().find(|s| Some(**s) == f["source"].as_str()).copied()?;
            let lines = f["lines"]
                .as_array()?
                .iter()
                .map(|l| Line {
                    at: at(l, "at"),
                    text: l["text"].as_str().unwrap_or("").to_string(),
                    words: l["words"]
                        .as_array()
                        .map(|w| w.iter().map(|w| Word { text: w["text"].as_str().unwrap_or("").to_string(), at: at(w, "at"), end: at(w, "end"), gap: w["gap"].as_bool().unwrap_or(false) }).collect())
                        .unwrap_or_default(),
                })
                .collect();
            Some(Found { lines, synced: f["synced"].as_bool().unwrap_or(false), source })
        })
        .collect()
}

/// Keeps what was found for a song, for next time it plays.
pub fn keep(video_id: &str, found: &[Found]) {
    let Some(path) = cache_file(video_id) else { return };
    let versions: Vec<serde_json::Value> = found
        .iter()
        .map(|f| {
            serde_json::json!({
                "source": f.source,
                "synced": f.synced,
                "lines": f.lines.iter().map(|l| serde_json::json!({
                    "at": ms(l.at),
                    "text": l.text,
                    "words": l.words.iter().map(|w| serde_json::json!({ "text": w.text, "at": ms(w.at), "end": ms(w.end), "gap": w.gap })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::json!({ "versions": versions }).to_string());
}
