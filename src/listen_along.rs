//! Listen along: friends hear the same song at the same moment, each on their own PC.
//!
//! One listener hosts a session and shares its code; the others join with it. The host's
//! NabyTune says what it plays (the song, the one after it, where it is and whether it's
//! playing) whenever that changes; the others' NabyTune plays the same, from the same place,
//! and keeps within a couple of seconds of it. Friends can send the host songs for its queue.
//!
//! There's no NabyTune server: the messages go through a relay, an ntfy server (ntfy.sh unless
//! Settings names another), on two topics named after the session's code: the host's news,
//! and its inbox. The code is random and long enough that no one finds a session without
//! being told it. What's sent is only what's needed: the song, the time, and a first name.
//!
//! ntfy.sh takes a few hundred messages a day from each address, so the host says something
//! only when something changes (a new song, a pause, a seek), never on a timer; a friend
//! joining reads its latest news from the relay's cache instead of asking for it.

use std::io::BufRead as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// The relay used unless Settings says otherwise.
pub const DEFAULT_RELAY: &str = "https://ntfy.sh";
/// The code's letters: Crockford's base 32 (no I, L, O or U to misread).
const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// The code's length, without its dashes: 80 random bits.
const CODE_LEN: usize = 16;
/// A friend more than this far from the host is brought back to where the host is.
pub const DRIFT: Duration = Duration::from_millis(2500);
/// And no sooner than this after the last time (a seek takes a moment to settle).
pub const CORRECT_EVERY: Duration = Duration::from_secs(4);
/// The host waits this long between messages, so skipping through songs sends one.
pub const SETTLE: Duration = Duration::from_millis(1200);
/// A friend says it's still there this often; one not heard from for `GONE` is gone.
pub const HELLO_EVERY: Duration = Duration::from_secs(15 * 60);
pub const GONE: Duration = Duration::from_secs(35 * 60);

pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// 64 random bits, from the OS-seeded keys std's hash maps use.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.write_u32(std::process::id());
    h.finish()
}

/// A new session code: "K7Q2-M9XA-PD3F-W8RT".
pub fn new_code() -> String {
    let bits = ((random_u64() as u128) << 64) | random_u64() as u128;
    let chars: String = (0..CODE_LEN).map(|i| ALPHABET[((bits >> (i * 5)) & 31) as usize] as char).collect();
    format_code(&chars)
}

fn format_code(plain: &str) -> String {
    plain.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

/// A code typed or pasted (dashes, spaces and case as they come, misread letters taken for
/// what they look like), with the relay when the invite names one ("…@relay.example.com").
/// The code may sit anywhere in the text pasted, such as a whole invite message.
pub fn parse_invite(text: &str) -> Option<(String, Option<String>)> {
    let read = |token: &str| -> Option<(String, Option<String>)> {
        // Punctuation around it ("code: …." in a message) isn't part of it.
        let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let (code, relay) = match token.split_once('@') {
            Some((code, relay)) => (code, Some(relay)),
            None => (token, None),
        };
        let plain: String = code
            .chars()
            .filter(|c| *c != '-')
            .map(|c| match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            })
            .collect();
        if plain.len() != CODE_LEN || !plain.bytes().all(|b| ALPHABET.contains(&b)) {
            return None;
        }
        let relay = relay.map(|r| r.trim_end_matches('/')).filter(|r| !r.is_empty()).map(|r| {
            if r.starts_with("http://") || r.starts_with("https://") { r.to_string() } else { format!("https://{r}") }
        });
        Some((format_code(&plain), relay))
    };
    text.split_whitespace().find_map(read).or_else(|| read(&text.replace(' ', "")))
}

/// What a friend types to join: the code, and the relay too when it isn't the usual one.
pub fn invite_code(code: &str, relay: &str) -> String {
    if relay == DEFAULT_RELAY {
        code.to_string()
    } else {
        format!("{code}@{}", relay.trim_start_matches("https://"))
    }
}

/// The topic a Blend's two tastes meet on.
pub fn blend_topic(code: &str) -> String {
    format!("nabytune-blend-{}", code.replace('-', "").to_ascii_lowercase())
}

/// The topic the host's news goes to, and its inbox.
pub fn news_topic(code: &str) -> String {
    format!("nabytune-{}", code.replace('-', "").to_ascii_lowercase())
}
pub fn inbox_topic(code: &str) -> String {
    format!("{}-in", news_topic(code))
}

/// A song as the messages carry it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub detail: String,
    pub duration: String,
    pub thumbnail: Option<String>,
}

impl Song {
    fn to_json(&self) -> Value {
        json!({ "id": self.id, "title": self.title, "artist": self.artist, "detail": self.detail, "duration": self.duration, "thumbnail": self.thumbnail })
    }
    fn from_json(v: &Value) -> Option<Self> {
        let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let id = v["id"].as_str().filter(|id| !id.is_empty() && !id.starts_with("file:"))?.to_string();
        Some(Self { id, title: text("title"), artist: text("artist"), detail: text("detail"), duration: text("duration"), thumbnail: v["thumbnail"].as_str().map(str::to_string) })
    }
}

/// Where the host is: what plays (None: nothing, or a file of the host's own, `local`),
/// what's next, whether it plays, and where it was at `at` (Unix ms, the host's clock).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct HostState {
    pub host: String,
    pub song: Option<Song>,
    /// The host plays a file from their PC: its name only, nothing to play along to.
    pub local: Option<String>,
    pub next: Option<Song>,
    pub playing: bool,
    pub position_ms: u64,
    pub at: u64,
    /// Friends may send songs for the queue.
    pub requests: bool,
}

impl HostState {
    /// Where the host is at `now` (Unix ms), as far as can be told.
    pub fn position_at(&self, now: u64) -> Duration {
        let ms = if self.playing { self.position_ms + now.saturating_sub(self.at) } else { self.position_ms };
        Duration::from_millis(ms)
    }
}

/// A message on a session's topics.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// The host's news.
    State(HostState),
    /// The host ended the session.
    End,
    /// A friend joined (or is still there), or left: their id and name.
    Hello { from: String, name: String },
    Bye { from: String },
    /// A friend's song for the host's queue.
    Add { name: String, song: Song },
}

impl Message {
    pub fn to_json(&self) -> String {
        let v = match self {
            Message::State(s) => json!({
                "v": 1, "kind": "state", "host": s.host,
                "song": s.song.as_ref().map(Song::to_json), "local": s.local, "next": s.next.as_ref().map(Song::to_json),
                "playing": s.playing, "pos": s.position_ms, "at": s.at, "requests": s.requests,
            }),
            Message::End => json!({ "v": 1, "kind": "end" }),
            Message::Hello { from, name } => json!({ "v": 1, "kind": "hello", "from": from, "name": name }),
            Message::Bye { from } => json!({ "v": 1, "kind": "bye", "from": from }),
            Message::Add { name, song } => json!({ "v": 1, "kind": "add", "name": name, "song": song.to_json() }),
        };
        v.to_string()
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        let text = |k: &str| v[k].as_str().unwrap_or("").chars().take(60).collect::<String>();
        Some(match v["kind"].as_str()? {
            "state" => Message::State(HostState {
                host: text("host"),
                song: Song::from_json(&v["song"]),
                local: v["local"].as_str().map(|s| s.chars().take(120).collect()),
                next: Song::from_json(&v["next"]),
                playing: v["playing"].as_bool().unwrap_or(false),
                position_ms: v["pos"].as_u64().unwrap_or(0),
                at: v["at"].as_u64().unwrap_or(0),
                requests: v["requests"].as_bool().unwrap_or(false),
            }),
            "end" => Message::End,
            "hello" => Message::Hello { from: text("from"), name: text("name") },
            "bye" => Message::Bye { from: text("from") },
            "add" => Message::Add { name: text("name"), song: Song::from_json(&v["song"])? },
            _ => return None,
        })
    }
}

/// What the reading thread hears.
pub enum Heard {
    Connected,
    /// A message's text, as sent (a `Message`, or a Blend's taste).
    Message(String),
    /// The relay can't be reached, or turned the request away (why).
    Trouble(String),
}

/// A topic being read, on a thread of its own, until this is dropped.
pub struct Reader {
    stop: Arc<AtomicBool>,
    pub heard: Receiver<Heard>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        // The thread notices within one keepalive (under a minute) and lets go.
        self.stop.store(true, Ordering::Release);
    }
}

pub fn client() -> reqwest::blocking::Client {
    // Long enough to wait out the relay's keepalives (every 45 s on ntfy) between messages.
    reqwest::blocking::Client::builder().timeout(Duration::from_secs(90)).build().unwrap_or_default()
}

/// What a reader hears of what was sent before it started.
#[derive(Clone, Copy, PartialEq)]
pub enum CatchUp {
    /// Only what's new.
    Nothing,
    /// The latest message (where the host is already).
    Latest,
    /// Everything the relay keeps (a Blend's tastes).
    All,
}

/// Reads `topic` on `relay`, after what `catch_up` asks for of the past, every new message.
/// Reconnects by itself, missing nothing in between.
pub fn read(relay: &str, topic: &str, catch_up: CatchUp) -> Reader {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, heard) = channel();
    let (relay, topic, flag) = (relay.trim_end_matches('/').to_string(), topic.to_string(), stop.clone());
    let _ = std::thread::Builder::new().name("nabytune-listen-along".into()).spawn(move || run_reader(&relay, &topic, catch_up, &tx, &flag));
    Reader { stop, heard }
}

/// One line of a topic's JSON stream: its id, and the text of the message it carries.
fn parse_line(line: &str) -> Option<(String, Option<String>)> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["event"].as_str() != Some("message") {
        return None;
    }
    let id = v["id"].as_str()?.to_string();
    Some((id, v["message"].as_str().map(str::to_string)))
}

fn run_reader(relay: &str, topic: &str, catch_up: CatchUp, tx: &Sender<Heard>, stop: &AtomicBool) {
    let http = client();
    let mut since: Option<String> = None;
    let past = match catch_up {
        CatchUp::Nothing => None,
        CatchUp::Latest => Some("latest"),
        CatchUp::All => Some("all"),
    };
    if let Some(past) = past {
        match http.get(format!("{relay}/{topic}/json?poll=1&since={past}")).send() {
            Ok(response) if response.status().is_success() => {
                for line in std::io::BufReader::new(response).lines().map_while(Result::ok) {
                    if let Some((id, message)) = parse_line(&line) {
                        since = Some(id);
                        if let Some(m) = message {
                            if tx.send(Heard::Message(m)).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            Ok(response) => {
                let _ = tx.send(Heard::Trouble(format!("HTTP {}", response.status().as_u16())));
            }
            Err(e) => {
                let _ = tx.send(Heard::Trouble(short(&e)));
            }
        }
    }
    let mut wait = Duration::from_secs(1);
    while !stop.load(Ordering::Acquire) {
        let url = match &since {
            Some(id) => format!("{relay}/{topic}/json?since={id}"),
            None => format!("{relay}/{topic}/json"),
        };
        match http.get(url).send() {
            Ok(response) if response.status().is_success() => {
                if tx.send(Heard::Connected).is_err() {
                    return;
                }
                wait = Duration::from_secs(1);
                for line in std::io::BufReader::new(response).lines() {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    // A read that times out (no keepalive) or breaks: connect again.
                    let Ok(line) = line else { break };
                    if let Some((id, message)) = parse_line(&line) {
                        since = Some(id);
                        if let Some(m) = message {
                            if tx.send(Heard::Message(m)).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            Ok(response) => {
                let code = response.status().as_u16();
                let why = if code == 429 { "the relay's limit for today is reached".to_string() } else { format!("HTTP {code}") };
                if tx.send(Heard::Trouble(why)).is_err() {
                    return;
                }
                wait = wait.max(Duration::from_secs(30));
            }
            Err(e) => {
                if tx.send(Heard::Trouble(short(&e))).is_err() {
                    return;
                }
            }
        }
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        wait = (wait * 2).min(Duration::from_secs(30));
    }
}

fn short(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "the relay didn't answer".into()
    } else if e.is_connect() {
        "can't reach the relay".into()
    } else {
        e.to_string()
    }
}

/// Sends `message` to `topic` on `relay` (blocking: call it off the UI thread).
pub fn send(http: &reqwest::blocking::Client, relay: &str, topic: &str, message: &Message) -> anyhow::Result<()> {
    send_text(http, relay, topic, message.to_json())
}

/// Sends `text` as it is (a Blend's taste).
pub fn send_text(http: &reqwest::blocking::Client, relay: &str, topic: &str, text: String) -> anyhow::Result<()> {
    let response = http.post(format!("{}/{topic}", relay.trim_end_matches('/'))).body(text).send()?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        429 => anyhow::bail!("the relay's message limit for today is reached"),
        code => anyhow::bail!("the relay said HTTP {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_random_and_read_back() {
        let a = new_code();
        let b = new_code();
        assert_ne!(a, b);
        assert_eq!(a.len(), CODE_LEN + 3);
        assert_eq!(parse_invite(&a), Some((a.clone(), None)));
        // Typed loosely: lower case, no dashes, an O for a zero, inside a whole message.
        assert_eq!(parse_invite("k7q2m9xapd3fw8rt"), Some(("K7Q2-M9XA-PD3F-W8RT".into(), None)));
        assert_eq!(parse_invite("Join me! Code: K7Q2-M9XA-PD3F-W8RT."), Some(("K7Q2-M9XA-PD3F-W8RT".into(), None)));
        assert_eq!(parse_invite("K7Q2-M9XA-PD3F-W8RO").map(|c| c.0), Some("K7Q2-M9XA-PD3F-W8R0".into()));
        // A relay of their own comes along.
        let invite = invite_code("K7Q2-M9XA-PD3F-W8RT", "https://relay.example.com");
        assert_eq!(invite, "K7Q2-M9XA-PD3F-W8RT@relay.example.com");
        assert_eq!(parse_invite(&invite), Some(("K7Q2-M9XA-PD3F-W8RT".into(), Some("https://relay.example.com".into()))));
        assert_eq!(invite_code("K7Q2-M9XA-PD3F-W8RT", DEFAULT_RELAY), "K7Q2-M9XA-PD3F-W8RT");
        assert_eq!(parse_invite("hello there"), None);
        assert_eq!(parse_invite("K7Q2-M9XA"), None);
        assert_eq!(news_topic("K7Q2-M9XA-PD3F-W8RT"), "nabytune-k7q2m9xapd3fw8rt");
        assert_eq!(inbox_topic("K7Q2-M9XA-PD3F-W8RT"), "nabytune-k7q2m9xapd3fw8rt-in");
    }

    #[test]
    fn messages_round_trip() {
        let song = Song { id: "dQw4w9WgXcQ".into(), title: "Song".into(), artist: "Artist".into(), detail: "Artist • Album".into(), duration: "3:33".into(), thumbnail: Some("https://x/y.jpg".into()) };
        let state = HostState { host: "Billy".into(), song: Some(song.clone()), local: None, next: None, playing: true, position_ms: 61_000, at: 1_700_000_000_000, requests: true };
        for m in [
            Message::State(state.clone()),
            Message::End,
            Message::Hello { from: "abc".into(), name: "Ana".into() },
            Message::Bye { from: "abc".into() },
            Message::Add { name: "Ana".into(), song: song.clone() },
        ] {
            assert_eq!(Message::from_json(&m.to_json()), Some(m.clone()));
            assert!(m.to_json().len() < 4096, "fits one ntfy message");
        }
        // Where the host is, a moment later: on if it plays, still if it's paused.
        assert_eq!(state.position_at(state.at + 2_500), Duration::from_millis(63_500));
        let paused = HostState { playing: false, ..state.clone() };
        assert_eq!(paused.position_at(paused.at + 2_500), Duration::from_millis(61_000));
        // A file on the host's PC is never something to play: no path ever comes through.
        let local = json!({ "kind": "state", "song": { "id": "file:C:\\Music\\a.mp3", "title": "a" } }).to_string();
        assert!(matches!(Message::from_json(&local), Some(Message::State(HostState { song: None, .. }))));
        assert_eq!(Message::from_json("{\"kind\":\"what\"}"), None);
        assert_eq!(Message::from_json("not json"), None);
    }

    #[test]
    fn reads_the_relay_stream() {
        let inner = Message::Hello { from: "a".into(), name: "Ana".into() }.to_json();
        let line = json!({ "id": "hwQ2YpKdmg", "time": 1635528741, "event": "message", "topic": "t", "message": inner }).to_string();
        let (id, text) = parse_line(&line).unwrap();
        assert_eq!(id, "hwQ2YpKdmg");
        assert_eq!(text.as_deref().and_then(Message::from_json), Some(Message::Hello { from: "a".into(), name: "Ana".into() }));
        assert_eq!(parse_line(r#"{"id":"x","time":1,"event":"keepalive","topic":"t"}"#), None);
        // Someone else's message on the topic: its id still counts, it's just not ours.
        let (_, text) = parse_line(r#"{"id":"y","time":1,"event":"message","topic":"t","message":"hi"}"#).unwrap();
        assert_eq!(text.as_deref().and_then(Message::from_json), None);
    }
}
