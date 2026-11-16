//! Lossless addons: HTTP servers the listener adds by URL, which find a song in their own
//! catalogue and hand back a lossless stream (FLAC, ALAC). The same protocol as NabyTune's
//! earlier apps:
//!
//! - `GET {base}/manifest.json`: `{id, name, version, resources, settings[{key, default, options}]}`
//! - `GET {base}/search?q=…&quality=…&<settings' defaults>`: `{tracks: [{id, title, artist,
//!   album, duration, audioQuality, format}]}`
//! - `GET {base}/stream/{id}?quality=…`: `{url, manifest: none|hls|dash, codec, sampleRate,
//!   bitDepth, encrypted, error}`
//!
//! No addon is built in: there are only the ones the listener adds. A song plays from the
//! first addon that has the same recording (title, artist, version and length agreeing);
//! anything else plays from YouTube as before.

use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use reqwest::blocking::Client;
use reqwest::Url;
use serde_json::Value;

use crate::lyrics::{clean, same_artist};

const AGENT: &str = "NabyTune/0.1 (lossless addon client)";

/// The quality asked of an addon.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tier {
    /// CD quality: 16-bit lossless.
    Lossless,
    /// The best there is: 24-bit, up to 192 kHz.
    HiRes,
}

impl Tier {
    pub fn key(self) -> &'static str {
        match self {
            Tier::Lossless => "LOSSLESS",
            Tier::HiRes => "HI_RES_LOSSLESS",
        }
    }
    pub fn from_key(key: &str) -> Self {
        if key == "HI_RES_LOSSLESS" { Tier::HiRes } else { Tier::Lossless }
    }
    /// Words an addon's quality options use for this tier.
    fn words(self) -> &'static [&'static str] {
        match self {
            Tier::Lossless => &["lossless", "flac", "hifi", "cd", "16"],
            Tier::HiRes => &["hi_res", "hi-res", "hires", "max", "24", "best", "master"],
        }
    }
}

/// An addon, as its manifest describes it.
#[derive(Clone, Debug)]
pub struct Addon {
    /// Where it lives, without `/manifest.json` or a trailing slash.
    pub base: String,
    pub name: String,
    pub version: String,
    /// Every declared setting's default, sent with each request.
    defaults: Vec<(String, String)>,
    /// What its `quality` setting offers.
    qualities: Vec<String>,
}

/// One song in an addon's catalogue.
#[derive(Clone, Debug)]
pub struct AddonTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: Option<f64>,
    pub quality: String,
}

/// What a stream request answers.
#[derive(Clone, Debug, Default)]
pub struct AddonStream {
    pub url: String,
    /// "hls", "dash", or empty for a plain file.
    pub transport: String,
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
}

impl AddonStream {
    /// A short label for the player: "FLAC 24-bit/96 kHz", "FLAC", "ALAC 16-bit/44.1 kHz".
    pub fn label(&self) -> String {
        let codec = if self.codec.is_empty() { "Lossless".to_string() } else { self.codec.to_uppercase() };
        match (self.bit_depth, self.sample_rate) {
            (Some(bits), Some(rate)) => {
                let khz = rate as f64 / 1000.0;
                let khz = if khz.fract() == 0.0 { format!("{khz:.0}") } else { format!("{khz:.1}") };
                format!("{codec} {bits}-bit/{khz} kHz")
            }
            _ => codec,
        }
    }
}

/// The base URL from whatever was pasted: `…/manifest.json` and trailing slashes off.
pub fn normalize_base(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((head, last)) if last.to_ascii_lowercase().ends_with(".json") && trimmed.contains("://") => head.trim_end_matches('/').to_string(),
        _ => trimmed.to_string(),
    }
}

/// An addon's URL for showing: scheme and host only, as its path may hold a private token.
pub fn redact(base: &str) -> String {
    match Url::parse(base) {
        Ok(url) => format!("{}://{}{}", url.scheme(), url.host_str().unwrap_or(""), if url.path().len() > 1 { "/…" } else { "" }),
        Err(_) => "(not a URL)".into(),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn get_json(http: &Client, url: Url) -> Result<Value> {
    let response = http.get(url).header("user-agent", AGENT).timeout(Duration::from_secs(15)).send()?;
    if response.status().as_u16() == 404 {
        bail!("not found");
    }
    Ok(response.error_for_status()?.json()?)
}

/// Reads the addon at `raw` (as pasted): its manifest, or why it isn't one.
pub fn connect(http: &Client, raw: &str) -> Result<Addon> {
    let base = normalize_base(raw);
    let url = Url::parse(&base).context("That isn't a web address")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("That isn't a web address");
    }
    let manifest = get_json(http, Url::parse(&format!("{base}/manifest.json"))?).context("No addon manifest at that address")?;
    let id = text(&manifest["id"]);
    if id.is_empty() {
        bail!("That address answered, but not with an addon manifest");
    }
    let resources: Vec<String> = manifest["resources"].as_array().map(|r| r.iter().map(text).collect()).unwrap_or_default();
    if !resources.is_empty() && !resources.iter().any(|r| r.eq_ignore_ascii_case("search")) {
        bail!("That addon can't search for songs");
    }
    let mut defaults = Vec::new();
    let mut qualities = Vec::new();
    for setting in manifest["settings"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let key = text(&setting["key"]);
        if key.is_empty() {
            continue;
        }
        let default = text(&setting["default"]);
        if !default.is_empty() {
            defaults.push((key.clone(), default));
        }
        if key == "quality" {
            qualities = setting["options"]
                .as_array()
                .map(|o| o.iter().map(|o| if o.is_object() { text(&o["value"]) } else { text(o) }).filter(|v| !v.is_empty()).collect())
                .unwrap_or_default();
        }
    }
    let name = text(&manifest["name"]);
    Ok(Addon { base, name: if name.is_empty() { id } else { name }, version: text(&manifest["version"]), defaults, qualities })
}

impl Addon {
    /// The query every request carries: the settings' defaults, then the quality asked for,
    /// in the addon's own words when it declares them.
    fn params(&self, tier: Tier) -> Vec<(String, String)> {
        let mut params: Vec<(String, String)> = self.defaults.iter().filter(|(k, _)| k != "quality").cloned().collect();
        let quality = self
            .qualities
            .iter()
            .find(|q| q.eq_ignore_ascii_case(tier.key()))
            .or_else(|| self.qualities.iter().find(|q| tier.words().iter().any(|w| q.to_lowercase().contains(w))))
            .cloned()
            .unwrap_or_else(|| tier.key().to_string());
        params.push(("quality".into(), quality));
        params
    }

    fn endpoint(&self, segments: &[&str], params: &[(String, String)]) -> Result<Url> {
        let mut url = Url::parse(&self.base)?;
        url.path_segments_mut().map_err(|_| anyhow::anyhow!("That isn't a usable address"))?.pop_if_empty().extend(segments);
        url.query_pairs_mut().extend_pairs(params);
        Ok(url)
    }

    pub fn search(&self, http: &Client, query: &str, tier: Tier) -> Result<Vec<AddonTrack>> {
        let mut params = self.params(tier);
        params.push(("q".into(), query.to_string()));
        let found = get_json(http, self.endpoint(&["search"], &params)?)?;
        Ok(found["tracks"]
            .as_array()
            .map(|tracks| {
                tracks
                    .iter()
                    .filter_map(|t| {
                        let id = text(&t["id"]);
                        (!id.is_empty()).then(|| AddonTrack {
                            id,
                            title: text(&t["title"]),
                            artist: text(&t["artist"]),
                            duration: t["duration"].as_f64().or_else(|| text(&t["duration"]).parse().ok()).filter(|d| *d > 0.0),
                            quality: format!("{} {}", text(&t["audioQuality"]), text(&t["format"])).trim().to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn stream(&self, http: &Client, id: &str, tier: Tier) -> Result<AddonStream> {
        let answer = get_json(http, self.endpoint(&["stream", id], &self.params(tier))?)?;
        let error = text(&answer["error"]);
        let url = text(&answer["url"]);
        if url.is_empty() {
            bail!("{}", if error.is_empty() { "the addon gave no stream".to_string() } else { error });
        }
        // DRM-protected streams can't be played here.
        let encrypted = match &answer["encrypted"] {
            Value::Bool(b) => *b,
            Value::String(s) => !s.is_empty() && !s.eq_ignore_ascii_case("false") && !s.eq_ignore_ascii_case("none"),
            _ => false,
        };
        if encrypted {
            bail!("the addon's stream is encrypted");
        }
        let transport = [&answer["manifest"], &answer["mediaType"], &answer["format"]]
            .iter()
            .map(|v| text(v).to_lowercase())
            .find_map(|t| match t.as_str() {
                "hls" | "m3u8" | "application/x-mpegurl" | "application/vnd.apple.mpegurl" => Some("hls"),
                "dash" | "mpd" | "application/dash+xml" => Some("dash"),
                _ => None,
            })
            .unwrap_or("")
            .to_string();
        let number = |k: &str| answer[k].as_f64().or_else(|| text(&answer[k]).parse().ok()).filter(|n| *n > 0.0);
        Ok(AddonStream {
            url,
            transport,
            codec: [text(&answer["codec"]), text(&answer["fileCodec"])].into_iter().find(|c| !c.is_empty()).unwrap_or_default(),
            // Some addons state kHz where Hz is asked for.
            sample_rate: number("sampleRate").map(|r| if r < 1000.0 { (r * 1000.0) as u32 } else { r as u32 }),
            bit_depth: number("bitDepth").map(|b| b as u32),
        })
    }
}

/// The whole stream's bytes: the file, or every segment of an HLS or DASH rendition joined
/// (initialisation first), which is one fragmented MP4 to the decoder.
pub fn download(http: &Client, stream: &AddonStream) -> Result<Vec<u8>> {
    let base = Url::parse(&stream.url).context("the addon's stream URL is not a URL")?;
    let parts: Vec<Url> = match stream.transport.as_str() {
        "hls" => hls_parts(http, base)?,
        "dash" => {
            let mpd = fetch_text(http, &base)?;
            dash_parts(&mpd, &base)?
        }
        _ => vec![base],
    };
    let mut bytes = Vec::new();
    for part in parts {
        let response = http.get(part).header("user-agent", AGENT).timeout(Duration::from_secs(60)).send()?.error_for_status()?;
        bytes.extend_from_slice(&response.bytes()?);
    }
    if bytes.is_empty() {
        bail!("the addon's stream was empty");
    }
    Ok(bytes)
}

fn fetch_text(http: &Client, url: &Url) -> Result<String> {
    Ok(http.get(url.clone()).header("user-agent", AGENT).timeout(Duration::from_secs(15)).send()?.error_for_status()?.text()?)
}

/// An HLS playlist's files in order: the best variant's initialisation map, then segments.
fn hls_parts(http: &Client, url: Url) -> Result<Vec<Url>> {
    let mut url = url;
    let mut playlist = fetch_text(http, &url)?;
    // A master playlist: take the variant with the most bandwidth (lossless is the biggest).
    if playlist.contains("#EXT-X-STREAM-INF") {
        let mut best: Option<(u64, Url)> = None;
        let mut lines = playlist.lines().peekable();
        while let Some(line) = lines.next() {
            if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
                let bandwidth = attr(attrs, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0);
                if let Some(uri) = lines.next().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                    let variant = url.join(uri)?;
                    if best.as_ref().is_none_or(|(b, _)| bandwidth >= *b) {
                        best = Some((bandwidth, variant));
                    }
                }
            }
        }
        url = best.context("the HLS playlist lists no variants")?.1;
        playlist = fetch_text(http, &url)?;
    }
    hls_media_parts(&playlist, &url)
}

/// A media playlist's files: `#EXT-X-MAP` first, then each segment.
fn hls_media_parts(playlist: &str, url: &Url) -> Result<Vec<Url>> {
    if playlist.contains("#EXT-X-KEY") && !playlist.contains("METHOD=NONE") {
        bail!("the addon's stream is encrypted");
    }
    let mut parts = Vec::new();
    for line in playlist.lines().map(str::trim) {
        if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            if let Some(uri) = attr(attrs, "URI") {
                parts.push(url.join(&uri)?);
            }
        } else if !line.is_empty() && !line.starts_with('#') {
            parts.push(url.join(line)?);
        }
    }
    if parts.is_empty() {
        bail!("the HLS playlist has no segments");
    }
    Ok(parts)
}

/// `KEY=value` or `KEY="value"` in an attribute list.
fn attr(list: &str, key: &str) -> Option<String> {
    let at = list.find(&format!("{key}="))? + key.len() + 1;
    let rest = &list[at..];
    Some(if let Some(quoted) = rest.strip_prefix('"') { quoted.split('"').next()?.to_string() } else { rest.split(',').next()?.to_string() })
}

/// `$Name$` or `$Name%05d$` (zero-padded to 5) in a DASH template, filled with `value`.
fn substitute(pattern: &str, name: &str, value: u64) -> String {
    let open = format!("${name}");
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(at) = rest.find(&open) {
        out.push_str(&rest[..at]);
        let after = &rest[at + open.len()..];
        let Some(end) = after.find('$') else { break };
        let width = after[..end].strip_prefix("%0").and_then(|f| f.strip_suffix('d')).and_then(|w| w.parse::<usize>().ok()).unwrap_or(0);
        out.push_str(&format!("{value:0width$}"));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// `name="value"` on an XML tag.
fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    Some(tag[at..].split('"').next()?.replace("&amp;", "&"))
}

/// Seconds in an ISO 8601 duration ("PT3M45.2S").
fn iso_seconds(text: &str) -> Option<f64> {
    let t = text.strip_prefix("PT")?;
    let mut seconds = 0.0;
    let mut number = String::new();
    for c in t.chars() {
        match c {
            'H' => seconds += number.parse::<f64>().ok()? * 3600.0,
            'M' => seconds += number.parse::<f64>().ok()? * 60.0,
            'S' => seconds += number.parse::<f64>().ok()?,
            _ => {
                number.push(c);
                continue;
            }
        }
        number.clear();
    }
    Some(seconds)
}

/// A DASH manifest's files for its best audio representation: one BaseURL file, or a
/// SegmentTemplate's initialisation and numbered (or timed) segments.
fn dash_parts(mpd: &str, url: &Url) -> Result<Vec<Url>> {
    if mpd.contains("<ContentProtection") {
        bail!("the addon's stream is encrypted");
    }
    // The representation with the most bandwidth: lossless renditions come last and largest.
    let mut best: Option<(u64, usize)> = None;
    let mut search = 0;
    while let Some(found) = mpd[search..].find("<Representation") {
        let at = search + found;
        let tag = &mpd[at..at + mpd[at..].find('>').unwrap_or(0)];
        let bandwidth = xml_attr(tag, "bandwidth").and_then(|b| b.parse().ok()).unwrap_or(0);
        if best.is_none_or(|(b, _)| bandwidth >= b) {
            best = Some((bandwidth, at));
        }
        search = at + 1;
    }
    let (_, at) = best.context("the DASH manifest has no representations")?;
    let rep_tag = &mpd[at..at + mpd[at..].find('>').unwrap_or(0)];
    let rep_id = xml_attr(rep_tag, "id").unwrap_or_default();
    let bandwidth = xml_attr(rep_tag, "bandwidth").unwrap_or_default();
    // The representation's own element, else the adaptation set's (templates are shared).
    let rep_end = mpd[at..].find("</Representation>").map_or(mpd.len(), |e| at + e);
    let rep_body = &mpd[at..rep_end];
    let set_start = mpd[..at].rfind("<AdaptationSet").unwrap_or(0);
    let scope = |inner: &'static str| -> Option<&str> {
        rep_body.find(inner).map(|i| &rep_body[i..]).or_else(|| mpd[set_start..at].find(inner).map(|i| &mpd[set_start + i..at]))
    };
    let mut base = url.clone();
    if let Some(b) = scope("<BaseURL>") {
        let value = b["<BaseURL>".len()..].split("</BaseURL>").next().unwrap_or("").trim().replace("&amp;", "&");
        base = url.join(&value)?;
    }
    let Some(template) = scope("<SegmentTemplate") else {
        // A single file.
        return Ok(vec![base]);
    };
    let tag_end = template.find('>').unwrap_or(template.len());
    let tag = &template[..tag_end];
    let fill = |pattern: &str, number: u64, time: u64| {
        let named = pattern.replace("$RepresentationID$", &rep_id).replace("$Bandwidth$", &bandwidth);
        substitute(&substitute(&named, "Number", number), "Time", time)
    };
    let mut parts = Vec::new();
    if let Some(init) = xml_attr(tag, "initialization") {
        parts.push(base.join(&fill(&init, 0, 0))?);
    }
    let media = xml_attr(tag, "media").context("the DASH template names no segments")?;
    let start: u64 = xml_attr(tag, "startNumber").and_then(|n| n.parse().ok()).unwrap_or(1);
    let timescale: f64 = xml_attr(tag, "timescale").and_then(|n| n.parse().ok()).unwrap_or(1.0);
    let template_end = template.find("</SegmentTemplate>").unwrap_or(tag_end);
    let body = &template[..template_end];
    if let Some(timeline) = body.find("<SegmentTimeline") {
        // <S t d r>: each entry is a segment, repeated r more times.
        let mut number = start;
        let mut time = 0u64;
        let mut rest = &body[timeline..];
        while let Some(s) = rest.find("<S ") {
            let s_tag = &rest[s..s + rest[s..].find('>').unwrap_or(0)];
            if let Some(t) = xml_attr(s_tag, "t").and_then(|t| t.parse().ok()) {
                time = t;
            }
            let d: u64 = xml_attr(s_tag, "d").and_then(|d| d.parse().ok()).context("a DASH segment without a length")?;
            let r: i64 = xml_attr(s_tag, "r").and_then(|r| r.parse().ok()).unwrap_or(0);
            for _ in 0..=r.max(0) {
                parts.push(base.join(&fill(&media, number, time))?);
                number += 1;
                time += d;
            }
            rest = &rest[s + 1..];
        }
    } else {
        // Fixed-length segments: as many as fit the presentation.
        let duration: f64 = xml_attr(tag, "duration").and_then(|d| d.parse().ok()).context("the DASH template gives no segment length")?;
        let total = mpd
            .find("mediaPresentationDuration=\"")
            .and_then(|i| iso_seconds(mpd[i + 27..].split('"').next()?))
            .context("the DASH manifest gives no length")?;
        let count = (total / (duration / timescale)).ceil() as u64;
        for n in 0..count {
            parts.push(base.join(&fill(&media, start + n, 0))?);
        }
    }
    Ok(parts)
}

/// Version words that make a different recording: a remix is not the album take.
const VERSIONS: [&str; 10] = ["live", "remix", "acoustic", "instrumental", "karaoke", "sped up", "slowed", "remaster", "demo", "cover"];

fn plain(text: &str) -> String {
    text.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How well an addon's track answers for the YouTube song (higher is better), or None when
/// it is another recording: the title and an artist must agree, so must version words, and
/// the length within 30 s when both are known.
pub fn score(title: &str, artist: &str, seconds: Option<f64>, candidate: &AddonTrack) -> Option<f64> {
    let (want, have) = (plain(&clean(title)), plain(&clean(&candidate.title)));
    if want.is_empty() || have.is_empty() {
        return None;
    }
    let title_score = if want == have {
        1.0
    } else if have.contains(&want) || want.contains(&have) {
        0.7
    } else {
        return None;
    };
    if !same_artist(&candidate.artist, artist) {
        return None;
    }
    let (full_want, full_have) = (title.to_lowercase(), candidate.title.to_lowercase());
    if VERSIONS.iter().any(|v| full_want.contains(v) != full_have.contains(v)) {
        return None;
    }
    let length_score = match (seconds, candidate.duration) {
        (Some(a), Some(b)) if (a - b).abs() > 30.0 => return None,
        (Some(a), Some(b)) => 1.0 - (a - b).abs() / 30.0,
        _ => 0.5,
    };
    Some(title_score * 2.0 + length_score)
}

/// The best match for a YouTube song in this addon's catalogue, if it has one.
pub fn find(http: &Client, addon: &Addon, title: &str, artist: &str, seconds: Option<f64>, tier: Tier) -> Result<Option<AddonTrack>> {
    let primary = artist.split([',', '&']).next().unwrap_or(artist).trim();
    let mut best: Option<(f64, AddonTrack)> = None;
    for query in [format!("{} {primary}", clean(title)), clean(title)] {
        for candidate in addon.search(http, &query, tier)? {
            if let Some(s) = score(title, artist, seconds, &candidate) {
                if best.as_ref().is_none_or(|(b, _)| s > *b) {
                    best = Some((s, candidate));
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    Ok(best.map(|(_, t)| t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str, duration: f64) -> AddonTrack {
        AddonTrack { id: "1".into(), title: title.into(), artist: artist.into(), duration: Some(duration), quality: "LOSSLESS".into() }
    }

    #[test]
    fn matches_the_same_recording_only() {
        assert!(score("Lemon", "Kenshi Yonezu", Some(255.0), &track("Lemon", "Kenshi Yonezu", 256.0)).is_some());
        // Another artist, another version, another length.
        assert!(score("Lemon", "Kenshi Yonezu", Some(255.0), &track("Lemon", "Someone Else", 255.0)).is_none());
        assert!(score("Lemon", "Kenshi Yonezu", Some(255.0), &track("Lemon (Live)", "Kenshi Yonezu", 255.0)).is_none());
        assert!(score("Lemon", "Kenshi Yonezu", Some(255.0), &track("Lemon", "Kenshi Yonezu", 330.0)).is_none());
        // Closer length scores higher.
        let near = score("Lemon", "Kenshi Yonezu", Some(255.0), &track("Lemon", "Kenshi Yonezu", 255.0)).unwrap();
        let far = score("Lemon", "Kenshi Yonezu", Some(255.0), &track("Lemon", "Kenshi Yonezu", 270.0)).unwrap();
        assert!(near > far);
    }

    #[test]
    fn reads_addresses() {
        assert_eq!(normalize_base("https://addon.example/abc/manifest.json"), "https://addon.example/abc");
        assert_eq!(normalize_base(" https://addon.example/ "), "https://addon.example");
        assert_eq!(redact("https://addon.example/secret-token"), "https://addon.example/…");
        assert_eq!(redact("https://addon.example"), "https://addon.example");
    }

    #[test]
    fn lists_dash_segments() {
        let mpd = r#"<MPD mediaPresentationDuration="PT8S"><Period><AdaptationSet mimeType="audio/mp4">
            <Representation id="aac" bandwidth="320000"/>
            <Representation id="flac" bandwidth="1411000" codecs="flac">
              <SegmentTemplate initialization="init-$RepresentationID$.mp4" media="seg-$RepresentationID$-$Number$.m4s" startNumber="1" timescale="44100">
                <SegmentTimeline><S t="0" d="176400" r="1"/><S d="88200"/></SegmentTimeline>
              </SegmentTemplate>
            </Representation></AdaptationSet></Period></MPD>"#;
        let base = Url::parse("https://cdn.example/a/b.mpd").unwrap();
        let parts: Vec<String> = dash_parts(mpd, &base).unwrap().into_iter().map(|u| u.to_string()).collect();
        assert_eq!(
            parts,
            vec![
                "https://cdn.example/a/init-flac.mp4",
                "https://cdn.example/a/seg-flac-1.m4s",
                "https://cdn.example/a/seg-flac-2.m4s",
                "https://cdn.example/a/seg-flac-3.m4s"
            ]
        );
        assert_eq!(iso_seconds("PT3M45.5S"), Some(225.5));
        assert_eq!(substitute("chunk-$Number%05d$.m4s", "Number", 7), "chunk-00007.m4s");
        assert_eq!(substitute("seg-$Number$.m4s", "Number", 12), "seg-12.m4s");
    }

    #[test]
    fn lists_hls_segments() {
        let playlist = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\nseg1.m4s\n#EXTINF:4,\nseg2.m4s\n#EXT-X-ENDLIST\n";
        let base = Url::parse("https://cdn.example/x/play.m3u8").unwrap();
        let parts: Vec<String> = hls_media_parts(playlist, &base).unwrap().into_iter().map(|u| u.to_string()).collect();
        assert_eq!(parts, vec!["https://cdn.example/x/init.mp4", "https://cdn.example/x/seg1.m4s", "https://cdn.example/x/seg2.m4s"]);
    }
}
