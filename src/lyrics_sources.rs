//! The word-synced lyric sources BitChord asks first (its `data/lyrics` providers), and the
//! formats they answer in: Apple's TTML (lrc.red, BiniLyrics, BetterLyrics), QQ Music's
//! karaoke lines (BetterLyrics Portato) and enhanced LRC (Unison).
//!
//! Each returns its lines and, when it says, the length of the recording they're timed to.
//! None of them needs a key.

use std::time::Duration;

use serde_json::Value;

use crate::lyrics::{parse_lrc, Line, Word};

const AGENT: &str = "NabyTune (https://github.com/BillyNabil/NabyTune)";
/// Each request's cap, as BitChord's lyrics client has it.
const TIMEOUT: Duration = Duration::from_secs(6);

pub type Timed = (Vec<Line>, Option<Duration>);

fn get(http: &reqwest::blocking::Client, url: &str, query: &[(&str, String)]) -> Option<String> {
    let response = http.get(url).query(query).header("user-agent", AGENT).header("accept", "application/json").timeout(TIMEOUT).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.text().ok()
}

fn get_json(http: &reqwest::blocking::Client, url: &str, query: &[(&str, String)]) -> Option<Value> {
    serde_json::from_str(&get(http, url, query)?).ok()
}

// ---- Formats ------------------------------------------------------------------------------

/// A TTML or LRC time: "27.395", "1:05.20", "1:02:03.4", "12.5s", "500ms".
fn clock(text: &str) -> Option<Duration> {
    let t = text.trim();
    if let Some(ms) = t.strip_suffix("ms") {
        return ms.trim().parse::<f64>().ok().map(|v| Duration::from_secs_f64(v / 1000.0));
    }
    let t = t.strip_suffix('s').unwrap_or(t);
    let mut secs = 0.0;
    for part in t.split(':') {
        secs = secs * 60.0 + part.trim().parse::<f64>().ok()?;
    }
    (secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

/// A syllable as sources time it: its text, when it's sung, and whether a space follows.
#[derive(Clone, Debug)]
pub(crate) struct Syllable {
    pub text: String,
    pub at: Duration,
    pub end: Duration,
    pub gap: bool,
}

/// Han, kana and Thai syllables stand alone: those scripts run on without spaces, and each
/// syllable lights on its own.
fn unspaced(text: &str) -> bool {
    text.chars().any(|c| (c as u32) >= 0x2E80 || ('\u{0E00}'..='\u{0E7F}').contains(&c))
}

/// Syllables into words: a syllable with no space after it joins the next one.
pub(crate) fn words_from(syllables: &[Syllable]) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut open = false;
    for s in syllables {
        let lone = unspaced(&s.text);
        match words.last_mut() {
            Some(word) if open && !lone => {
                word.text.push_str(s.text.trim_end());
                word.end = s.end;
                word.gap = s.gap;
            }
            _ => words.push(Word { text: s.text.trim().to_string(), at: s.at, end: s.end, gap: s.gap }),
        }
        open = !s.gap && !lone;
    }
    words.retain(|w| !w.text.is_empty());
    words
}

/// A line's text from its words, spaced as they're sung.
fn text_of(words: &[Word]) -> String {
    let mut text = String::new();
    for w in words {
        text.push_str(&w.text);
        if w.gap {
            text.push(' ');
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A role attribute, by its local name (TTML's `ttm:role`).
fn role<'a>(node: roxmltree::Node<'a, 'a>) -> Option<&'a str> {
    node.attributes().find(|a| a.name() == "role").map(|a| a.value())
}

/// Apple's TTML: each `<p>` a line, each timed `<span>` in it a syllable; spaces between
/// spans part words. Translations and romanisations ride along as roles and are left out,
/// and so are background vocals. A `<p>` with no timed spans is a line-synced line.
pub fn parse_ttml(text: &str) -> Vec<Line> {
    let Ok(doc) = roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }) else {
        return Vec::new();
    };
    fn collect<'a>(node: roxmltree::Node<'a, 'a>, out: &mut Vec<Syllable>) {
        for child in node.children() {
            if child.is_text() {
                // Whitespace between syllables ends the one before it.
                if child.text().is_some_and(|t| t.chars().any(char::is_whitespace)) {
                    if let Some(last) = out.last_mut() {
                        last.gap = true;
                    }
                }
                continue;
            }
            if !child.is_element() || child.tag_name().name() != "span" {
                continue;
            }
            if matches!(role(child), Some("x-translation" | "x-roman" | "x-bg")) {
                continue;
            }
            let timed = child.attribute("begin").is_some() && !child.children().any(|c| c.is_element() && c.attribute("begin").is_some());
            if timed {
                let text: String = child.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect();
                let (Some(at), end) = (child.attribute("begin").and_then(clock), child.attribute("end").and_then(clock)) else { continue };
                let gap = text.ends_with(char::is_whitespace);
                out.push(Syllable { text: text.trim_end().to_string(), at, end: end.unwrap_or(at + Duration::from_millis(300)), gap });
            } else {
                collect(child, out);
            }
        }
    }
    let mut lines = Vec::new();
    for p in doc.descendants().filter(|n| n.is_element() && n.tag_name().name() == "p") {
        let begin = p.attribute("begin").and_then(clock);
        let mut syllables = Vec::new();
        collect(p, &mut syllables);
        if syllables.is_empty() {
            let text: String = p.descendants().filter(|d| d.is_text() && !d.ancestors().any(|a| matches!(role(a), Some("x-translation" | "x-roman" | "x-bg")))).filter_map(|d| d.text()).collect();
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some(at) = begin {
                lines.push(Line { at, text, words: Vec::new() });
            }
            continue;
        }
        let words = words_from(&syllables);
        let at = begin.map_or(syllables[0].at, |b| b.min(syllables[0].at));
        lines.push(Line { at, text: text_of(&words), words });
    }
    lines.sort_by_key(|l| l.at);
    lines
}

/// QQ Music's karaoke lines (QRC): `[start,length]word(start,length)word(start,length)…`,
/// all in milliseconds.
pub fn parse_qrc(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let raw = raw.trim();
        let Some(rest) = raw.strip_prefix('[') else { continue };
        let Some((head, body)) = rest.split_once(']') else { continue };
        let Some((start, _)) = head.split_once(',') else { continue };
        let Ok(start) = start.trim().parse::<u64>() else { continue };
        let mut syllables = Vec::new();
        let mut text_so_far = String::new();
        let mut rest = body;
        while let Some(open) = rest.find('(') {
            let Some(close) = rest[open..].find(')').map(|c| open + c) else { break };
            let stamp = &rest[open + 1..close];
            let mut nums = stamp.split(',').map(|n| n.trim().parse::<u64>());
            match (nums.next(), nums.next()) {
                (Some(Ok(at)), Some(Ok(len))) => {
                    text_so_far.push_str(&rest[..open]);
                    let word = std::mem::take(&mut text_so_far);
                    if !word.trim().is_empty() {
                        syllables.push(Syllable {
                            gap: word.ends_with(' '),
                            text: word.trim().to_string(),
                            at: Duration::from_millis(at),
                            end: Duration::from_millis(at + len),
                        });
                    } else if let Some(last) = syllables.last_mut() {
                        last.gap = true;
                    }
                }
                // Brackets that are part of the words.
                _ => text_so_far.push_str(&rest[..=close]),
            }
            rest = &rest[close + 1..];
        }
        let words = words_from(&syllables);
        let text = text_of(&words);
        if text.is_empty() {
            continue;
        }
        lines.push(Line { at: Duration::from_millis(start), text, words });
    }
    lines
}

/// Enhanced LRC: `[mm:ss.xx]<mm:ss.xx>word <mm:ss.xx>word …`. A word ends where the next
/// begins; a line's last word where the next line does (or 800 ms on, at the end).
pub fn parse_enhanced_lrc(text: &str) -> Vec<Line> {
    let mut lines: Vec<(Duration, Vec<(Duration, String)>)> = Vec::new();
    for raw in text.lines() {
        let raw = raw.trim();
        let Some(rest) = raw.strip_prefix('[') else { continue };
        let Some((stamp, body)) = rest.split_once(']') else { continue };
        let Some(at) = clock(stamp) else { continue };
        let mut words = Vec::new();
        for piece in body.split('<').skip(1) {
            let Some((stamp, word)) = piece.split_once('>') else { continue };
            if let Some(wat) = clock(stamp) {
                words.push((wat, word.to_string()));
            }
        }
        lines.push((at, words));
    }
    if !lines.iter().any(|(_, w)| !w.is_empty()) {
        return Vec::new();
    }
    let starts: Vec<Duration> = lines.iter().map(|(at, _)| *at).collect();
    lines
        .into_iter()
        .enumerate()
        .map(|(i, (at, stamped))| {
            let next_line = starts.get(i + 1).copied();
            let syllables: Vec<Syllable> = stamped
                .iter()
                .enumerate()
                .filter(|(_, (_, w))| !w.trim().is_empty())
                .map(|(j, (wat, w))| Syllable {
                    text: w.trim().to_string(),
                    at: *wat,
                    end: stamped.get(j + 1).map(|(n, _)| *n).or(next_line).unwrap_or(*wat + Duration::from_millis(800)),
                    gap: w.ends_with(' '),
                })
                .collect();
            let words = words_from(&syllables);
            Line { at, text: text_of(&words), words }
        })
        .collect()
}

/// A provider's answer, whatever it's wrapped in: JSON objects are unwrapped (refusing one
/// that says it's an error) to the first of the keys sources put lyrics under, and the text
/// is then read as TTML, QQ karaoke, enhanced LRC or plain LRC, as it looks.
pub fn parse_any(raw: &str) -> Vec<Line> {
    fn unwrap(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => match serde_json::from_str::<Value>(s) {
                Ok(inner @ (Value::Object(_) | Value::Array(_))) => unwrap(&inner),
                _ => Some(s.clone()),
            },
            Value::Array(items) => {
                let parts: Vec<String> = items.iter().filter_map(unwrap).collect();
                (!parts.is_empty()).then(|| parts.join("\n"))
            }
            Value::Object(map) => {
                if map.get("isError").and_then(Value::as_bool) == Some(true) || map.get("ok").and_then(Value::as_bool) == Some(false) {
                    return None;
                }
                if map.get("error").is_some_and(|e| !(e.is_null() || e == &Value::Bool(false) || e.as_str() == Some(""))) {
                    return None;
                }
                for key in ["ttml", "ttmlContent", "lyrics", "lrc", "content", "text", "syncedLyrics", "lines", "lyric", "data", "result", "response"] {
                    if let Some(found) = map.get(key).and_then(unwrap) {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }
    let text = raw.trim_start_matches('\u{feff}').trim();
    let text = match serde_json::from_str::<Value>(text) {
        Ok(v @ (Value::Object(_) | Value::Array(_))) => match unwrap(&v) {
            Some(t) => t,
            None => return Vec::new(),
        },
        _ => text.to_string(),
    };
    let text = text.replace("&lt;tt", "<tt");
    if text.contains("<tt") {
        return parse_ttml(&text);
    }
    let karaoke = text.lines().any(|l| {
        let l = l.trim();
        l.starts_with('[') && l.split_once(']').is_some_and(|(h, b)| h.contains(',') && b.contains('('))
    });
    if karaoke {
        return parse_qrc(&text);
    }
    if text.contains('<') && text.lines().any(|l| l.contains("]<")) {
        let lines = parse_enhanced_lrc(&text);
        if !lines.is_empty() {
            return lines;
        }
    }
    parse_lrc(&text)
}

// ---- Providers ----------------------------------------------------------------------------

/// Letters and digits only, lower case, accents dropped where they're plain Latin ones.
fn fold(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A title's core: no brackets, nothing after " - ".
fn core_title(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in title.chars() {
        match c {
            '(' | '[' | '（' | '【' => depth += 1,
            ')' | ']' | '）' | '】' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    fold(out.split(" - ").next().unwrap_or(""))
}

/// The version words a title carries (Live, Remix, Acoustic…): two recordings must agree.
fn version_words(title: &str) -> Vec<&'static str> {
    const WORDS: [&str; 21] = [
        "live", "remix", "remixed", "mix", "acoustic", "unplugged", "instrumental", "karaoke", "cappella", "acapella", "demo", "edit", "version", "cover",
        "sped", "slowed", "reverb", "nightcore", "lofi", "orchestral", "extended",
    ];
    let folded = fold(title);
    let words: Vec<&str> = folded.split_whitespace().collect();
    WORDS.iter().copied().filter(|w| words.contains(w)).collect()
}

/// Two credits share an artist (split on commas, "&", "and", "x", "feat").
fn shared_artist(a: &str, b: &str) -> bool {
    let names = |s: &str| -> Vec<String> {
        let folded = format!(" {} ", fold(s));
        let mut parts = vec![folded];
        for sep in [" and ", " x ", " with ", " feat ", " ft "] {
            parts = parts.iter().flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>()).collect();
        }
        parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
    };
    let (a, b) = (names(&a.replace([',', '&', ';', '/'], " and ")), names(&b.replace([',', '&', ';', '/'], " and ")));
    a.iter().any(|x| b.iter().any(|y| x == y || x.contains(y.as_str()) || y.contains(x.as_str())))
}

/// lrc.red: Apple's word-timed TTML, found by search (title, artist, its ISRC and length).
pub fn lrc_red(http: &reqwest::blocking::Client, title: &str, artist: &str, secs: u64) -> Option<Timed> {
    let hits = get_json(http, "https://lrc.red/search.json", &[("q", format!("{title} {artist}"))])?;
    let wanted_title = core_title(title);
    let wanted_versions = version_words(title);
    let mut best: Option<(f64, String, f64)> = None;
    for hit in hits["hits"].as_array()? {
        let (Some(t), Some(a), Some(isrc), Some(d)) = (hit["title"].as_str(), hit["artist"].as_str(), hit["isrc"].as_str(), hit["duration"].as_f64()) else { continue };
        let isrc = isrc.to_uppercase();
        let valid = isrc.len() == 12 && isrc.chars().take(2).all(|c| c.is_ascii_uppercase()) && isrc.chars().skip(5).all(|c| c.is_ascii_digit());
        let off = (d - secs as f64).abs();
        if !valid || off > 3.0 || core_title(t) != wanted_title || version_words(t) != wanted_versions || !shared_artist(a, artist) {
            continue;
        }
        if best.as_ref().is_none_or(|(o, _, _)| off < *o) {
            best = Some((off, isrc, d));
        }
    }
    let (_, isrc, length) = best?;
    let ttml = get(http, &format!("https://lrc.red/s/{isrc}.ttml"), &[])?;
    let lines = parse_ttml(&ttml);
    (!lines.is_empty()).then(|| (lines, Some(Duration::from_secs_f64(length))))
}

/// BiniLyrics: its index of lrc.red's documents, by title, artist and length.
pub fn bini_lyrics(http: &reqwest::blocking::Client, title: &str, artist: &str, secs: u64) -> Option<Timed> {
    let found = get_json(http, "https://lyrics-api.binimum.org/", &[("track", title.into()), ("artist", artist.into()), ("duration", secs.to_string())])?;
    let hit = &found["results"][0];
    let url = hit["lyricsUrl"].as_str()?;
    let lines = parse_ttml(&get(http, url, &[])?);
    (!lines.is_empty()).then(|| (lines, hit["duration"].as_f64().filter(|d| *d > 0.0).map(Duration::from_secs_f64)))
}

/// BetterLyrics: Apple's TTML inside JSON (`/getLyrics`), or QQ Music's karaoke lines
/// (`/qq/getLyrics`, "Portato").
pub fn better_lyrics(http: &reqwest::blocking::Client, title: &str, artist: &str, secs: u64, qq: bool) -> Option<Timed> {
    let url = if qq { "https://lyrics-api.boidu.dev/qq/getLyrics" } else { "https://lyrics-api.boidu.dev/getLyrics" };
    let raw = get(http, url, &[("s", title.into()), ("a", artist.into()), ("d", secs.to_string())])?;
    let lines = parse_any(&raw);
    (!lines.is_empty()).then_some((lines, None))
}

/// Unison: community-voted lyrics, TTML or (enhanced) LRC.
pub fn unison(http: &reqwest::blocking::Client, title: &str, artist: &str, secs: u64) -> Option<Timed> {
    let found = get_json(http, "https://unison.boidu.dev/lyrics", &[("song", title.into()), ("artist", artist.into()), ("duration", secs.to_string())])?;
    if found["success"].as_bool() != Some(true) {
        return None;
    }
    let data = &found["data"];
    let text = data["lyrics"].as_str()?;
    if data["syncType"].as_str() == Some("plain") {
        return None;
    }
    let lines = if data["format"].as_str() == Some("ttml") {
        parse_ttml(text)
    } else {
        let enhanced = parse_enhanced_lrc(text);
        if enhanced.is_empty() { parse_lrc(text) } else { enhanced }
    };
    (!lines.is_empty()).then_some((lines, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ttml_words_and_lines() {
        let ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata"><body><div>
            <p begin="27.395" end="30.0"><span begin="27.395" end="27.6">I've</span> <span begin="27.7" end="27.9">been</span> <span begin="28.0" end="28.2">try</span><span begin="28.2" end="28.5">ing</span><span ttm:role="x-translation">ignored</span></p>
            <p begin="1:05.20" end="1:07">A whole line</p>
        </div></body></tt>"#;
        let lines = parse_ttml(ttml);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "I've been trying");
        assert_eq!(lines[0].words.len(), 3);
        assert_eq!(lines[0].words[2].text, "trying");
        assert_eq!(lines[0].at, Duration::from_millis(27_395));
        assert_eq!(lines[1].text, "A whole line");
        assert_eq!(lines[1].at, Duration::from_millis(65_200));
        assert!(lines[1].words.is_empty());
    }

    #[test]
    fn reads_qq_karaoke_and_enhanced_lrc() {
        let qrc = "[1000,2000]夢(1000,300)な(1300,200)ら(1500,200)\n[3000,1500]Hello (3000,500)world(3500,500)";
        let lines = parse_qrc(qrc);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "夢なら");
        assert_eq!(lines[0].words.len(), 3);
        assert_eq!(lines[1].text, "Hello world");
        assert_eq!(lines[1].words[1].at, Duration::from_millis(3500));

        let lrc = "[00:12.00]<00:12.00>Hello <00:12.50>there\n[00:14.00]<00:14.00>Bye";
        let lines = parse_enhanced_lrc(lrc);
        assert_eq!(lines[0].text, "Hello there");
        assert_eq!(lines[0].words[1].end, Duration::from_secs(14));
        assert_eq!(parse_any(r#"{"ttml":"<tt><body><div><p begin=\"1\" end=\"2\">Hi</p></div></body></tt>"}"#)[0].text, "Hi");
        assert!(parse_any(r#"{"error":"not found"}"#).is_empty());
    }

    #[test]
    fn matches_recordings() {
        assert_eq!(core_title("Pretender (Official Video) - from X"), "pretender");
        assert_eq!(version_words("Blinding Lights (Live)"), vec!["live"]);
        assert!(shared_artist("Official HIGE DANdism", "Official Hige Dandism"));
        assert!(shared_artist("YOASOBI & Ayase", "YOASOBI"));
        assert!(!shared_artist("José González", "Rie Takahashi"));
    }
}
