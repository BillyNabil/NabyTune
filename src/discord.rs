//! Discord Rich Presence over the local IPC pipe.
//!
//! A Discord client running on this machine listens on `\\.\pipe\discord-ipc-0`
//! (to `-9`, so stable, PTB and Canary can run side by side). An application sets
//! a presence through it with nothing but its application id: no token, no login.
//!
//! Framing is a little-endian u32 opcode, a little-endian u32 length, and that many
//! bytes of JSON. The handshake is `{"v":1,"client_id":...}`, answered by a READY
//! dispatch; then each `SET_ACTIVITY` command (with the pid and a nonce) is answered
//! by a frame carrying the same nonce.
//!
//! Everything runs on one worker thread: it connects lazily (only once there is
//! something to show), retries every 15 s while no Discord answers, reconnects when
//! the pipe breaks, coalesces bursts so only the latest presence goes out, and keeps
//! to Discord's rate limit (one update per 2 s, five per 20 s). Same port as the
//! Kotlin desktop app's `DesktopDiscordIpc` / `DesktopDiscordRpc`.

use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What is playing, as handed to [`Discord::set`].
#[derive(Clone, Debug, PartialEq)]
pub struct Presence {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub position: Duration,
    pub length: Option<Duration>,
    pub playing: bool,
    /// music.youtube.com watch link, for the "Listen on YouTube Music" button.
    pub url: Option<String>,
}

/// Handle to the presence worker. Cheap to call [`Discord::set`] on every tick:
/// the worker drops updates that would not change what Discord shows.
pub struct Discord {
    tx: Sender<Update>,
}

impl Discord {
    /// Spawns the worker thread. Never blocks and never panics; if the thread
    /// cannot be spawned, `set` quietly does nothing.
    pub fn start(client_id: &str) -> Discord {
        let (tx, rx) = mpsc::channel();
        let client_id = client_id.to_owned();
        let spawned = std::thread::Builder::new()
            .name("discord-presence".into())
            .spawn(move || Worker::new(client_id, rx).run());
        if let Err(e) = spawned {
            log(&format!("discord: could not start worker: {e}"));
        }
        Discord { tx }
    }

    /// Shows `presence`, or clears the activity when `None`.
    pub fn set(&self, presence: Option<Presence>) {
        // Stamped here, not when the worker gets round to sending: a send held back
        // by the rate limit must not shift the progress bar by the time it waited.
        let _ = self.tx.send((presence, SystemTime::now()));
    }
}

// Dropping `Discord` drops the sender; the worker sees the channel disconnect,
// takes the presence down and exits.

type Update = (Option<Presence>, SystemTime);

/// Kotlin's approach: a paused track takes the presence down rather than showing
/// a frozen card. Set to false to keep the card up, without timestamps, instead.
const CLEAR_WHEN_PAUSED: bool = true;

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// Discord's activity type for "Listening to".
const ACTIVITY_LISTENING: u32 = 2;
/// Which line Discord repeats in the member list: 0 name, 1 state, 2 details.
/// Details is the song title (the Kotlin app's default).
const STATUS_DISPLAY_DETAILS: u32 = 2;

/// Discord rejects details/state/large_text shorter than 2 or longer than 128.
const TEXT_MIN: usize = 2;
const TEXT_MAX: usize = 128;
const BUTTON_LABEL_MAX: usize = 32;
const BUTTON_URL_MAX: usize = 512;
const IMAGE_URL_MAX: usize = 256;
const BUTTON_LABEL: &str = "Listen on YouTube Music";

const RETRY_EVERY: Duration = Duration::from_secs(15);
const MIN_GAP: Duration = Duration::from_secs(2);
const WINDOW: Duration = Duration::from_secs(20);
const WINDOW_SENDS: usize = 5;
/// How often an idle connection is checked for pings and a closed pipe.
const IDLE_POLL: Duration = Duration::from_secs(2);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// Timestamps closer than this to the ones already shown are not worth a resend.
const DRIFT_TOLERANCE_MS: u64 = 2_000;
/// Larger frames mean a desynchronised stream.
const MAX_FRAME: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

struct Worker {
    client_id: String,
    rx: Receiver<Update>,
    conn: Option<Conn>,
    /// The activity wanted now (`None`: nothing shown).
    desired: Option<Value>,
    /// Title of `desired`, for the log.
    desired_title: String,
    /// What the current connection last showed.
    sent: Option<Value>,
    /// `desired` differs from `sent`.
    dirty: bool,
    next_connect: Instant,
    sends: VecDeque<Instant>,
    /// A missing Discord is logged once, not every 15 s.
    reported_missing: bool,
}

impl Worker {
    fn new(client_id: String, rx: Receiver<Update>) -> Worker {
        Worker {
            client_id,
            rx,
            conn: None,
            desired: None,
            desired_title: String::new(),
            sent: None,
            dirty: false,
            next_connect: Instant::now(),
            sends: VecDeque::new(),
            reported_missing: false,
        }
    }

    fn run(mut self) {
        loop {
            let received = match self.next_wake(Instant::now()) {
                None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(wait) => self.rx.recv_timeout(wait),
            };
            match received {
                Ok(update) => {
                    self.accept(update);
                    // Coalesce a burst: only the newest matters.
                    loop {
                        match self.rx.try_recv() {
                            Ok(update) => self.accept(update),
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                self.shutdown();
                                return;
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.shutdown();
                    return;
                }
            }
            self.tick(Instant::now());
        }
    }

    fn accept(&mut self, (presence, stamp): Update) {
        self.desired_title = presence.as_ref().map(|p| p.title.clone()).unwrap_or_default();
        self.desired = presence.as_ref().and_then(|p| activity_json(p, stamp));
        self.dirty = !same_activity(self.desired.as_ref(), self.sent.as_ref());
    }

    /// How long the loop may sleep before it has work; `None` blocks until a message.
    fn next_wake(&self, now: Instant) -> Option<Duration> {
        if self.dirty {
            if self.conn.is_none() {
                if self.desired.is_none() {
                    return Some(Duration::ZERO);
                }
                return Some(self.next_connect.saturating_duration_since(now));
            }
            return Some(self.rate_wait(now).unwrap_or(Duration::ZERO));
        }
        if self.conn.is_some() {
            return Some(IDLE_POLL);
        }
        None
    }

    fn tick(&mut self, now: Instant) {
        if let Some(conn) = self.conn.as_mut() {
            if let Err(e) = conn.drain() {
                self.lost(&e);
            }
        }
        if !self.dirty {
            return;
        }
        if self.conn.is_none() {
            if self.desired.is_none() {
                // Nothing is shown without a connection, so nothing to clear.
                self.sent = None;
                self.dirty = false;
                return;
            }
            if now < self.next_connect || !self.connect(now) {
                return;
            }
        }
        if self.rate_wait(now).is_some() {
            return;
        }
        let Some(conn) = self.conn.as_mut() else { return };
        self.sends.push_back(now);
        while self.sends.len() > WINDOW_SENDS {
            self.sends.pop_front();
        }
        match conn.set_activity(self.desired.as_ref()) {
            Ok(()) => {
                self.sent = self.desired.clone();
                self.dirty = false;
                if self.desired.is_some() {
                    log(&format!("discord: presence set: {}", self.desired_title));
                } else {
                    log("discord: presence cleared");
                }
            }
            Err(e) => {
                self.lost(&e);
                // Discord may have restarted: look once more straight away.
                self.connect(Instant::now());
            }
        }
    }

    /// Tries every pipe; on failure holds off the next try for [`RETRY_EVERY`].
    fn connect(&mut self, now: Instant) -> bool {
        match Conn::connect(&self.client_id) {
            Some(conn) => {
                log(&format!("discord: connected on {}", conn.label));
                self.conn = Some(conn);
                self.sent = None;
                self.dirty = self.desired.is_some();
                self.reported_missing = false;
                true
            }
            None => {
                if !self.reported_missing {
                    log("discord: no client answering; retrying every 15 s");
                    self.reported_missing = true;
                }
                self.next_connect = now + RETRY_EVERY;
                false
            }
        }
    }

    fn lost(&mut self, e: &io::Error) {
        if let Some(conn) = self.conn.take() {
            log(&format!("discord: lost {}: {e}", conn.label));
        }
        self.sent = None;
        self.dirty = self.desired.is_some();
        self.next_connect = Instant::now();
    }

    /// Time left before another SET_ACTIVITY is allowed, if any.
    fn rate_wait(&self, now: Instant) -> Option<Duration> {
        let mut until = self.sends.back().map(|&last| last + MIN_GAP);
        if self.sends.len() >= WINDOW_SENDS {
            let window_end = self.sends[self.sends.len() - WINDOW_SENDS] + WINDOW;
            until = Some(until.map_or(window_end, |u| u.max(window_end)));
        }
        until.filter(|&t| t > now).map(|t| t - now)
    }

    fn shutdown(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            if self.sent.is_some() {
                let _ = conn.set_activity(None);
            }
            let _ = conn.send(OP_CLOSE, "{}");
        }
    }
}

// ---------------------------------------------------------------------------
// Activity JSON
// ---------------------------------------------------------------------------

/// The activity for SET_ACTIVITY, or `None` when the presence should come down.
fn activity_json(p: &Presence, stamp: SystemTime) -> Option<Value> {
    if !p.playing && CLEAR_WHEN_PAUSED {
        return None;
    }
    let mut a = Map::new();
    a.insert("type".into(), json!(ACTIVITY_LISTENING));
    if let Some(details) = clamp_text(&p.title, TEXT_MAX) {
        a.insert("details".into(), json!(details));
    }
    if let Some(state) = clamp_text(&p.artist, TEXT_MAX) {
        a.insert("state".into(), json!(state));
    }
    a.insert("status_display_type".into(), json!(STATUS_DISPLAY_DETAILS));

    if p.playing {
        let now_ms = stamp.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let start = now_ms.saturating_sub(p.position.as_millis() as u64);
        let mut ts = Map::new();
        ts.insert("start".into(), json!(start));
        if let Some(len) = p.length.filter(|l| !l.is_zero() && *l > p.position) {
            ts.insert("end".into(), json!(start + len.as_millis() as u64));
        }
        a.insert("timestamps".into(), Value::Object(ts));
    }

    let mut assets = Map::new();
    if let Some(cover) = p.cover_url.as_deref().map(str::trim).filter(|u| is_http(u) && u.len() <= IMAGE_URL_MAX) {
        assets.insert("large_image".into(), json!(cover));
    }
    if let Some(album) = p.album.as_deref().and_then(|a| clamp_text(a, TEXT_MAX)) {
        assets.insert("large_text".into(), json!(album));
    }
    if !assets.is_empty() {
        a.insert("assets".into(), Value::Object(assets));
    }

    if let Some(url) = p.url.as_deref().map(str::trim).filter(|u| is_http(u) && u.len() <= BUTTON_URL_MAX) {
        let label = clamp_text(BUTTON_LABEL, BUTTON_LABEL_MAX).unwrap_or_default();
        a.insert("buttons".into(), json!([{ "label": label, "url": url }]));
    }
    Some(Value::Object(a))
}

fn is_http(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Fits text to Discord's limits, counted in UTF-16 units as Discord's JavaScript
/// does: trimmed, cut to `max` with an ellipsis, and padded with a zero-width space
/// when shorter than 2. `None` for blank text, which is left out.
fn clamp_text(text: &str, max: usize) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut out = if text.encode_utf16().count() <= max {
        text.to_owned()
    } else {
        let mut cut = String::new();
        let mut units = 0;
        for c in text.chars() {
            if units + c.len_utf16() > max - 1 {
                break;
            }
            units += c.len_utf16();
            cut.push(c);
        }
        let mut cut = cut.trim_end().to_owned();
        cut.push('\u{2026}');
        cut
    };
    while out.encode_utf16().count() < TEXT_MIN {
        out.push('\u{200B}');
    }
    Some(out)
}

/// Equal apart from timestamps that moved by less than [`DRIFT_TOLERANCE_MS`].
fn same_activity(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let (mut a, mut b) = (a.clone(), b.clone());
            let ta = a.as_object_mut().and_then(|o| o.remove("timestamps"));
            let tb = b.as_object_mut().and_then(|o| o.remove("timestamps"));
            a == b && timestamps_close(ta.as_ref(), tb.as_ref())
        }
        _ => false,
    }
}

fn timestamps_close(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => ["start", "end"].iter().all(|k| match (a[k].as_u64(), b[k].as_u64()) {
            (None, None) => true,
            (Some(x), Some(y)) => x.abs_diff(y) <= DRIFT_TOLERANCE_MS,
            _ => false,
        }),
        _ => false,
    }
}

fn activity_payload(activity: Option<&Value>, nonce: &str) -> String {
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": activity },
        "nonce": nonce,
    })
    .to_string()
}

fn handshake_payload(client_id: &str) -> String {
    json!({ "v": 1, "client_id": client_id }).to_string()
}

fn next_nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}-{:x}-{}", std::process::id(), nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// One frame: little-endian opcode, little-endian length, JSON.
fn encode_frame(opcode: u32, payload: &str) -> Vec<u8> {
    let body = payload.as_bytes();
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&opcode.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

/// Opcode and length from a frame header.
fn decode_header(header: [u8; 8]) -> (u32, usize) {
    let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    (op, len)
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

struct Conn {
    pipe: File,
    label: String,
}

impl Conn {
    /// The first Discord that completes the handshake.
    fn connect(client_id: &str) -> Option<Conn> {
        for path in pipe_candidates() {
            let Ok(pipe) = std::fs::OpenOptions::new().read(true).write(true).open(&path) else { continue };
            let mut conn = Conn { pipe, label: path };
            match conn.handshake(client_id) {
                Ok(()) => return Some(conn),
                Err(e) => log(&format!("discord: handshake on {} failed: {e}", conn.label)),
            }
        }
        None
    }

    fn handshake(&mut self, client_id: &str) -> io::Result<()> {
        self.send(OP_HANDSHAKE, &handshake_payload(client_id))?;
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        loop {
            let Some((op, body)) = self.read_frame(deadline)? else {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no READY"));
            };
            match op {
                OP_FRAME if body["evt"] == "READY" => return Ok(()),
                OP_CLOSE => return Err(closed(&body)),
                OP_PING => self.send(OP_PONG, &body.to_string())?,
                _ => {}
            }
        }
    }

    /// Sends SET_ACTIVITY and waits briefly for its reply, logging a rejection.
    fn set_activity(&mut self, activity: Option<&Value>) -> io::Result<()> {
        let nonce = next_nonce();
        self.send(OP_FRAME, &activity_payload(activity, &nonce))?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        while let Some((op, body)) = self.read_frame(deadline)? {
            if self.handle(op, &body)? && body["nonce"] == nonce.as_str() {
                return Ok(());
            }
        }
        log("discord: no reply to SET_ACTIVITY");
        Ok(())
    }

    /// Reads whatever Discord has sent, without blocking.
    fn drain(&mut self) -> io::Result<()> {
        while self.available()? >= 8 {
            let deadline = Instant::now() + Duration::from_secs(1);
            match self.read_frame(deadline)? {
                Some((op, body)) => {
                    self.handle(op, &body)?;
                }
                None => break,
            }
        }
        Ok(())
    }

    /// Answers pings, logs errors, and fails on CLOSE. True for an ordinary frame.
    fn handle(&mut self, op: u32, body: &Value) -> io::Result<bool> {
        match op {
            OP_FRAME => {
                if body["evt"] == "ERROR" {
                    log(&format!("discord: {} rejected: {}", body["cmd"], body["data"]));
                }
                Ok(true)
            }
            OP_CLOSE => Err(closed(body)),
            OP_PING => {
                self.send(OP_PONG, &body.to_string())?;
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn send(&mut self, opcode: u32, payload: &str) -> io::Result<()> {
        self.pipe.write_all(&encode_frame(opcode, payload))?;
        self.pipe.flush()
    }

    /// One frame, or `None` if none arrived by `deadline`. Polls rather than blocks,
    /// so a Discord that never answers cannot hang the worker.
    fn read_frame(&mut self, deadline: Instant) -> io::Result<Option<(u32, Value)>> {
        if !self.wait_for(8, deadline)? {
            return Ok(None);
        }
        let mut header = [0u8; 8];
        self.pipe.read_exact(&mut header)?;
        let (op, len) = decode_header(header);
        if len > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes")));
        }
        // The header is already consumed, so a body that never comes is a broken stream.
        if !self.wait_for(len, deadline.max(Instant::now() + Duration::from_secs(1)))? {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "frame body never arrived"));
        }
        let mut body = vec![0u8; len];
        self.pipe.read_exact(&mut body)?;
        let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        Ok(Some((op, value)))
    }

    fn wait_for(&self, bytes: usize, deadline: Instant) -> io::Result<bool> {
        loop {
            if self.available()? >= bytes {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn available(&self) -> io::Result<usize> {
        sys::available(&self.pipe)
    }
}

fn closed(body: &Value) -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        format!("closed by Discord: {} {}", body["code"], body["message"]),
    )
}

#[cfg(windows)]
fn pipe_candidates() -> Vec<String> {
    (0..10).map(|i| format!(r"\\.\pipe\discord-ipc-{i}")).collect()
}

#[cfg(not(windows))]
fn pipe_candidates() -> Vec<String> {
    Vec::new()
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    extern "system" {
        fn PeekNamedPipe(
            pipe: *mut c_void,
            buffer: *mut c_void,
            buffer_size: u32,
            bytes_read: *mut u32,
            total_available: *mut u32,
            bytes_left_this_message: *mut u32,
        ) -> i32;
    }

    /// Bytes waiting in the pipe. Fails (ERROR_BROKEN_PIPE) once Discord has gone.
    pub fn available(pipe: &File) -> io::Result<usize> {
        let mut total = 0u32;
        // SAFETY: a valid pipe handle owned by `pipe`; no buffer, so only the counts are written.
        let ok = unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut total,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(total as usize)
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use std::fs::File;
    use std::io;

    pub fn available(_pipe: &File) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Discord IPC is Windows-only here"))
    }
}

/// Diagnostics to %TEMP%\nabytune-proto.log, quietly.
fn log(line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("nabytune-proto.log"))
    {
        let _ = writeln!(f, "{:?} {}", SystemTime::now(), line);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Presence {
        Presence {
            title: "Blinding Lights".into(),
            artist: "The Weeknd".into(),
            album: Some("After Hours".into()),
            cover_url: Some("https://lh3.googleusercontent.com/abc=w544-h544".into()),
            position: Duration::from_secs(30),
            length: Some(Duration::from_secs(200)),
            playing: true,
            url: Some("https://music.youtube.com/watch?v=4NRXx6U8ABQ".into()),
        }
    }

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn frame_is_little_endian_opcode_length_json() {
        let frame = encode_frame(1, r#"{"a":"é"}"#);
        assert_eq!(&frame[0..4], &[1, 0, 0, 0]);
        assert_eq!(&frame[4..8], &[10, 0, 0, 0]); // é is two bytes
        assert_eq!(&frame[8..], r#"{"a":"é"}"#.as_bytes());
        let header: [u8; 8] = frame[0..8].try_into().unwrap();
        assert_eq!(decode_header(header), (1, 10));
        assert_eq!(decode_header([2, 0, 0, 0, 0x00, 0x01, 0, 0]), (2, 256));
    }

    #[test]
    fn handshake_payload_shape() {
        let v: Value = serde_json::from_str(&handshake_payload("1571674595331670026")).unwrap();
        assert_eq!(v, json!({ "v": 1, "client_id": "1571674595331670026" }));
    }

    #[test]
    fn set_activity_payload_carries_pid_nonce_and_activity() {
        let activity = activity_json(&song(), at(1_000_000)).unwrap();
        let v: Value = serde_json::from_str(&activity_payload(Some(&activity), "n1")).unwrap();
        assert_eq!(v["cmd"], "SET_ACTIVITY");
        assert_eq!(v["nonce"], "n1");
        assert_eq!(v["args"]["pid"], std::process::id());
        assert_eq!(v["args"]["activity"], activity);

        let cleared: Value = serde_json::from_str(&activity_payload(None, "n2")).unwrap();
        assert!(cleared["args"]["activity"].is_null());
        assert!(cleared["args"].as_object().unwrap().contains_key("activity"));
    }

    #[test]
    fn playing_activity_fields() {
        let a = activity_json(&song(), at(1_000_000)).unwrap();
        assert_eq!(a["type"], 2);
        assert_eq!(a["details"], "Blinding Lights");
        assert_eq!(a["state"], "The Weeknd");
        assert_eq!(a["status_display_type"], 2);
        assert_eq!(a["timestamps"]["start"], 970_000);
        assert_eq!(a["timestamps"]["end"], 1_170_000);
        assert_eq!(a["assets"]["large_image"], "https://lh3.googleusercontent.com/abc=w544-h544");
        assert_eq!(a["assets"]["large_text"], "After Hours");
        assert_eq!(a["buttons"][0]["label"], "Listen on YouTube Music");
        assert_eq!(a["buttons"][0]["url"], "https://music.youtube.com/watch?v=4NRXx6U8ABQ");
    }

    #[test]
    fn paused_follows_kotlin() {
        let p = Presence { playing: false, ..song() };
        let a = activity_json(&p, at(1_000_000));
        if CLEAR_WHEN_PAUSED {
            assert!(a.is_none());
        } else {
            assert!(a.unwrap().get("timestamps").is_none());
        }
    }

    #[test]
    fn optional_fields_left_out() {
        let p = Presence {
            album: None,
            cover_url: Some("file:///C:/cover.jpg".into()),
            length: None,
            url: Some("not a url".into()),
            artist: "   ".into(),
            ..song()
        };
        let a = activity_json(&p, at(1_000_000)).unwrap();
        assert!(a.get("assets").is_none());
        assert!(a.get("buttons").is_none());
        assert!(a.get("state").is_none());
        assert_eq!(a["timestamps"], json!({ "start": 970_000 }));

        let long_cover = Presence { cover_url: Some(format!("https://x/{}", "a".repeat(300))), ..song() };
        assert!(activity_json(&long_cover, at(0)).unwrap()["assets"].get("large_image").is_none());

        // Position past the end: no end time in the past.
        let over = Presence { position: Duration::from_secs(250), ..song() };
        assert!(activity_json(&over, at(1_000_000)).unwrap()["timestamps"].get("end").is_none());
    }

    #[test]
    fn clamp_pads_short_and_cuts_long() {
        assert_eq!(clamp_text("", 128), None);
        assert_eq!(clamp_text("   ", 128), None);
        assert_eq!(clamp_text("X", 128).unwrap(), "X\u{200B}");
        assert_eq!(clamp_text(" Hi ", 128).unwrap(), "Hi");

        let exact = "a".repeat(128);
        assert_eq!(clamp_text(&exact, 128).unwrap(), exact);

        let long = clamp_text(&"a".repeat(300), 128).unwrap();
        assert_eq!(long.encode_utf16().count(), 128);
        assert!(long.ends_with('\u{2026}'));

        // Surrogate pairs count as two, and are never split.
        let emoji = clamp_text(&"\u{1F3B5}".repeat(100), 128).unwrap();
        assert!(emoji.encode_utf16().count() <= 128);
        assert!(emoji.ends_with('\u{2026}'));
        assert_eq!(clamp_text("\u{1F3B5}", 128).unwrap(), "\u{1F3B5}");

        // No trailing space before the ellipsis.
        let spaced = clamp_text(&format!("{} tail", "b".repeat(126)), 128).unwrap();
        assert_eq!(spaced, format!("{}\u{2026}", "b".repeat(126)));

        assert!(clamp_text(&"y".repeat(40), BUTTON_LABEL_MAX).unwrap().encode_utf16().count() <= 32);
    }

    #[test]
    fn small_drift_is_the_same_activity() {
        let a = activity_json(&song(), at(1_000_000));
        // Same song a second later, one second further in: same start.
        let b = activity_json(&Presence { position: Duration::from_secs(31), ..song() }, at(1_001_000));
        assert!(same_activity(a.as_ref(), b.as_ref()));
        // A seek moves start by 60 s.
        let c = activity_json(&Presence { position: Duration::from_secs(90), ..song() }, at(1_001_000));
        assert!(!same_activity(a.as_ref(), c.as_ref()));
        let d = activity_json(&Presence { title: "Other".into(), ..song() }, at(1_000_000));
        assert!(!same_activity(a.as_ref(), d.as_ref()));
        assert!(same_activity(None, None));
        assert!(!same_activity(a.as_ref(), None));
    }

    #[test]
    fn rate_limit_spacing_and_window() {
        let (_tx, rx) = mpsc::channel();
        let mut w = Worker::new("0".into(), rx);
        let t0 = Instant::now();
        assert_eq!(w.rate_wait(t0), None);
        w.sends.push_back(t0);
        assert_eq!(w.rate_wait(t0 + Duration::from_millis(500)), Some(Duration::from_millis(1500)));
        assert_eq!(w.rate_wait(t0 + Duration::from_secs(2)), None);
        // Five sends 2 s apart: the sixth waits until 20 s after the first.
        w.sends = (0..5).map(|i| t0 + Duration::from_secs(2 * i)).collect();
        assert_eq!(w.rate_wait(t0 + Duration::from_secs(10)), Some(Duration::from_secs(10)));
        assert_eq!(w.rate_wait(t0 + Duration::from_secs(20)), None);
    }

    #[test]
    fn nonces_are_unique() {
        assert_ne!(next_nonce(), next_nonce());
    }

    #[test]
    fn drop_ends_worker_without_discord_work() {
        // With nothing ever set, the worker never connects and exits on drop.
        let d = Discord::start("0");
        drop(d);
    }
}
