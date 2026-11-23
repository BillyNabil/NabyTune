//! Together: the side panel for listening with friends. Listen along (one hosts, the
//! others hear the same song at the same moment) and Blend (two tastes in one playlist).
//! The relay and the messages are in listen_along.rs; mixing tastes is in blend.rs. This is
//! the app's side: who's in the session, what the host says and the friends do about it.

use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, rgb, rgba, AnyElement, ClipboardItem, Context, FontWeight, SharedString};

use crate::appearance::{accent, on_accent};
use crate::blend::{self, Taste};
use crate::i18n::{t, tf};
use crate::listen_along::{self as la, CatchUp, Heard, HostState, Message, Reader, Song};
use crate::{chrome, library, local, settings, stats, NabyTune, Tab, Track, SECONDARY, STROKE, SUBTLE, SUBTLE_STRONG, TERTIARY};

/// Segoe Fluent Icons' People.
pub(crate) const PEOPLE: &str = "\u{E716}";

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Role {
    Host,
    Friend,
}

/// A friend in the host's session.
pub(crate) struct Friend {
    id: String,
    pub name: String,
    seen: Instant,
}

/// A listen-along session, hosted here or joined.
pub(crate) struct Session {
    pub role: Role,
    pub code: String,
    pub relay: String,
    /// This PC in the session, and the name the others see.
    me: String,
    pub name: String,
    reader: Reader,
    http: reqwest::blocking::Client,
    pub connected: bool,
    /// Why the relay isn't answering, while it isn't.
    pub trouble: Option<String>,
    // ---- Hosting
    pub friends: Vec<Friend>,
    /// Friends may send songs for the queue.
    pub requests: bool,
    /// What the last news said, and when it went.
    sent: Option<HostState>,
    sent_at: Option<Instant>,
    // ---- Joined
    /// Where the host is, as last heard.
    pub host: Option<HostState>,
    /// Playing what the host plays (off once the friend picks music of their own).
    pub following: bool,
    /// The host ended the session.
    pub ended: bool,
    corrected: Instant,
    hello_at: Instant,
}

/// A Blend started here, waiting for the friend's taste.
pub(crate) struct BlendWait {
    pub code: String,
    relay: String,
    mine: Taste,
    reader: Reader,
}

/// A Blend made: with whom, how alike, and its playlist.
#[derive(Clone)]
pub(crate) struct BlendMade {
    pub friend: String,
    pub score: u32,
    pub shared: Vec<String>,
    pub playlist: String,
}

pub(crate) fn track_of(song: &Song) -> Track {
    Track {
        video_id: song.id.clone(),
        title: song.title.clone().into(),
        artist: song.artist.clone().into(),
        detail: if song.detail.is_empty() { song.artist.clone().into() } else { song.detail.clone().into() },
        duration: song.duration.clone().into(),
        thumbnail: song.thumbnail.clone(),
        local: None,
    }
}

fn song_of(track: &Track) -> Song {
    Song {
        id: track.video_id.clone(),
        title: track.title.to_string(),
        artist: track.artist.to_string(),
        detail: track.detail.to_string(),
        duration: track.duration.to_string(),
        thumbnail: track.thumbnail.clone(),
    }
}

fn apart(a: Duration, b: Duration) -> Duration {
    a.abs_diff(b)
}

/// The relay Settings names (ntfy.sh unless another).
pub(crate) fn relay() -> String {
    settings::get("listen_relay").map(|r| r.trim().trim_end_matches('/').to_string()).filter(|r| r.starts_with("http")).unwrap_or_else(|| la::DEFAULT_RELAY.to_string())
}

impl NabyTune {
    /// The name friends see: the account's first name, else Windows' user name.
    fn my_name(&self) -> String {
        self.account
            .as_ref()
            .and_then(|a| a.name.split_whitespace().next().map(str::to_string))
            .or_else(|| std::env::var("USERNAME").ok())
            .map(|n| n.trim().chars().take(30).collect::<String>())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| t("A friend").to_string())
    }

    /// Playing what a host plays, as a friend in their session.
    pub(crate) fn following(&self) -> bool {
        self.together.as_ref().is_some_and(|s| s.role == Role::Friend && s.following && !s.ended)
    }

    fn new_session(&self, role: Role, code: String, relay: String, reader: Reader) -> Session {
        Session {
            role,
            code,
            relay,
            me: format!("{:x}", la::unix_ms() ^ ((std::process::id() as u64) << 20)),
            name: self.my_name(),
            reader,
            http: la::client(),
            connected: false,
            trouble: None,
            friends: Vec::new(),
            requests: settings::get("listen_requests").is_none_or(|v| v != "0"),
            sent: None,
            sent_at: None,
            host: None,
            following: true,
            ended: false,
            corrected: Instant::now(),
            hello_at: Instant::now(),
        }
    }

    /// Hosts a new session: friends join with its code.
    pub(crate) fn host_session(&mut self, cx: &mut Context<Self>) {
        self.leave_session(cx);
        let code = la::new_code();
        let relay = relay();
        let reader = la::read(&relay, &la::inbox_topic(&code), CatchUp::Nothing);
        self.together = Some(self.new_session(Role::Host, code, relay, reader));
        self.together_error = None;
        cx.notify();
    }

    /// Joins the session in `text` (a code, or an invite with one in it).
    pub(crate) fn join_session(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some((code, relay_named)) = la::parse_invite(text) else {
            self.together_error = Some(t("That doesn't look like a session code").into());
            cx.notify();
            return;
        };
        self.leave_session(cx);
        let relay = relay_named.unwrap_or_else(relay);
        // The latest news first: where the host is already.
        let reader = la::read(&relay, &la::news_topic(&code), CatchUp::Latest);
        let session = self.new_session(Role::Friend, code, relay, reader);
        let hello = Message::Hello { from: session.me.clone(), name: session.name.clone() };
        let inbox = la::inbox_topic(&session.code);
        self.together = Some(session);
        self.together_error = None;
        self.join_field.update(cx, |f, cx| f.clear_text(cx));
        self.post(inbox, hello, cx);
        cx.notify();
    }

    /// Leaves the session (a host ends it for everyone).
    pub(crate) fn leave_session(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.together.take() else { return };
        let (topic, message) = match session.role {
            Role::Host => (la::news_topic(&session.code), Message::End),
            Role::Friend => (la::inbox_topic(&session.code), Message::Bye { from: session.me.clone() }),
        };
        if !session.ended {
            let (http, relay) = (session.http.clone(), session.relay.clone());
            cx.background_executor().spawn(async move { la::send(&http, &relay, &topic, &message) }).detach();
        }
        cx.notify();
    }

    /// Plays what the host plays again, after music of one's own.
    pub(crate) fn follow_again(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.together.as_mut().filter(|s| s.role == Role::Friend) {
            s.following = true;
            s.corrected = Instant::now() - la::CORRECT_EVERY;
        }
        self.on_host_news(cx);
        cx.notify();
    }

    /// The friend picked music of their own: they stay in the session, not following it.
    pub(crate) fn stop_following(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.together.as_mut().filter(|s| s.role == Role::Friend && s.following) {
            s.following = false;
            self.note(t("Playing your own music: Together › Follow again goes back to the session"), cx);
        }
    }

    /// Copies the invite, to paste to friends.
    pub(crate) fn copy_session_invite(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.together else { return };
        let code = la::invite_code(&s.code, &s.relay);
        cx.write_to_clipboard(ClipboardItem::new_string(tf("Listen along with me on NabyTune! Open Together (the people button), choose Join and enter: {}", &[&code])));
        self.note(t("Invite copied: paste it to your friends"), cx);
    }

    /// A friend's song for the host's queue.
    pub(crate) fn suggest_to_session(&mut self, track: &Track, cx: &mut Context<Self>) {
        let Some(s) = self.together.as_ref().filter(|s| s.role == Role::Friend && !s.ended) else { return };
        if local::is_local(&track.video_id) {
            self.note(t("Songs from this PC can't go to the session"), cx);
            return;
        }
        let host = s.host.as_ref().map(|h| h.host.clone()).unwrap_or_default();
        let message = Message::Add { name: s.name.clone(), song: song_of(track) };
        let inbox = la::inbox_topic(&s.code);
        self.post(inbox, message, cx);
        self.note(if host.is_empty() { tf("Sent “{}” to the session", &[&track.title]) } else { tf("Sent “{}” to {}", &[&track.title, &host]) }, cx);
    }

    /// Whether the track menu offers sending a song to the session.
    pub(crate) fn can_suggest(&self) -> bool {
        self.together.as_ref().is_some_and(|s| s.role == Role::Friend && !s.ended && s.host.as_ref().is_some_and(|h| h.requests))
    }

    /// Sends `message` to `topic` of the session, in the background.
    fn post(&mut self, topic: String, message: Message, cx: &mut Context<Self>) {
        let Some(s) = &self.together else { return };
        let (http, relay) = (s.http.clone(), s.relay.clone());
        let news = matches!(message, Message::State(_));
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { la::send(&http, &relay, &topic, &message) }).await;
            let _ = this.update(cx, |app, cx| {
                if let (Some(s), Err(e)) = (app.together.as_mut(), result) {
                    s.trouble = Some(format!("{e:#}"));
                    if news {
                        // Said again in a while (not at once: the relay may be counting).
                        s.sent = None;
                        s.sent_at = Some(Instant::now() + Duration::from_secs(20));
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Each tick: what the relay brought, then the host's news or the friend's catching up.
    pub(crate) fn together_tick(&mut self, cx: &mut Context<Self>) {
        self.blend_tick(cx);
        let Some(session) = self.together.as_mut() else { return };
        let mut heard = Vec::new();
        while let Ok(h) = session.reader.heard.try_recv() {
            heard.push(h);
        }
        for h in heard {
            match h {
                Heard::Connected => {
                    if let Some(s) = self.together.as_mut() {
                        s.connected = true;
                        s.trouble = None;
                    }
                    cx.notify();
                }
                Heard::Trouble(why) => {
                    if let Some(s) = self.together.as_mut() {
                        s.connected = false;
                        s.trouble = Some(why);
                    }
                    cx.notify();
                }
                Heard::Message(text) => {
                    if let Some(m) = Message::from_json(&text) {
                        self.on_session_message(m, cx);
                    }
                }
            }
        }
        match self.together.as_ref().map(|s| s.role) {
            Some(Role::Host) => self.host_tick(cx),
            Some(Role::Friend) => self.friend_tick(cx),
            None => {}
        }
    }

    fn on_session_message(&mut self, message: Message, cx: &mut Context<Self>) {
        let Some(s) = self.together.as_mut() else { return };
        match (s.role, message) {
            (Role::Host, Message::Hello { from, name }) => {
                match s.friends.iter_mut().find(|f| f.id == from) {
                    Some(f) => f.seen = Instant::now(),
                    None => {
                        s.friends.push(Friend { id: from, name: name.clone(), seen: Instant::now() });
                        self.note(tf("{} is listening along", &[&name]), cx);
                    }
                }
                cx.notify();
            }
            (Role::Host, Message::Bye { from }) => {
                s.friends.retain(|f| f.id != from);
                cx.notify();
            }
            (Role::Host, Message::Add { name, song }) if s.requests => {
                let track = track_of(&song);
                self.fetch_art(&track, cx);
                self.add_to_queue(track, cx);
                self.note(tf("{} added “{}” to the queue", &[&name, &song.title]), cx);
            }
            (Role::Friend, Message::State(state)) => {
                s.host = Some(state);
                self.on_host_news(cx);
                cx.notify();
            }
            (Role::Friend, Message::End) => {
                s.ended = true;
                s.following = false;
                let host = s.host.as_ref().map(|h| h.host.clone()).unwrap_or_default();
                self.note(if host.is_empty() { t("The session ended").to_string() } else { tf("{} ended the session", &[&host]) }, cx);
            }
            _ => {}
        }
    }

    /// Where this PC stands, as the host's news says it (None while a song loads: where it
    /// is isn't known yet).
    fn host_snapshot(&self) -> Option<HostState> {
        let s = self.together.as_ref()?;
        let current = self.current();
        if self.loading || (current.is_some() && self.error.is_none() && self.audio.status().generation != self.generation) {
            return None;
        }
        let (song, local) = match current {
            Some(t) if local::is_local(&t.video_id) => (None, Some(format!("{} · {}", t.title, t.artist))),
            Some(t) => (Some(song_of(t)), None),
            None => (None, None),
        };
        let next = self
            .next_index()
            .filter(|n| Some(*n) != self.index)
            .and_then(|n| self.queue.get(n))
            .filter(|t| !local::is_local(&t.video_id))
            .map(song_of);
        Some(HostState {
            host: s.name.clone(),
            song,
            local,
            next,
            playing: self.playing && current.is_some(),
            position_ms: self.now().as_millis() as u64,
            at: la::unix_ms(),
            requests: s.requests,
        })
    }

    /// The host says something when what plays changes: a new song, a pause, a seek, the
    /// next song. Never on a timer (the relay counts messages).
    fn host_tick(&mut self, cx: &mut Context<Self>) {
        let Some(now) = self.host_snapshot() else { return };
        let Some(s) = self.together.as_mut() else { return };
        s.friends.retain(|f| f.seen.elapsed() < la::GONE);
        let id = |song: &Option<Song>| song.as_ref().map(|x| x.id.clone());
        let changed = match &s.sent {
            None => true,
            Some(last) => {
                id(&last.song) != id(&now.song)
                    || last.local != now.local
                    || last.playing != now.playing
                    || last.requests != now.requests
                    || id(&last.next) != id(&now.next)
                    || apart(last.position_at(now.at), Duration::from_millis(now.position_ms)) > la::DRIFT
            }
        };
        if !changed || s.sent_at.is_some_and(|at| at.elapsed() < la::SETTLE) {
            return;
        }
        s.sent = Some(now.clone());
        s.sent_at = Some(Instant::now());
        let topic = la::news_topic(&s.code);
        self.post(topic, Message::State(now), cx);
    }

    /// The host's news came: the same song as the host, playing or paused as it is.
    fn on_host_news(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.together.as_ref().filter(|s| s.role == Role::Friend && s.following && !s.ended).and_then(|s| s.host.clone()) else {
            return;
        };
        let now = la::unix_ms();
        let Some(song) = state.song.clone() else {
            // Nothing to play along to: nothing plays there, or a file of the host's own.
            if self.playing && self.index.is_some() {
                self.toggle(cx);
            }
            return;
        };
        // News from a session whose host went away mid-song long ago: nothing to join yet.
        let length = chrome::clock_seconds(&song.duration).map(Duration::from_secs_f64);
        if state.playing && length.is_some_and(|l| state.position_at(now) > l + Duration::from_secs(30)) {
            return;
        }
        if self.current().is_none_or(|t| t.video_id != song.id) {
            if let Some(s) = self.together.as_mut() {
                s.corrected = Instant::now() - la::CORRECT_EVERY;
            }
            // The host's song, and the one after it, so it runs on into that in time.
            let mut list = vec![track_of(&song)];
            if let Some(next) = &state.next {
                list.push(track_of(next));
            }
            for track in &list {
                self.fetch_art(track, cx);
            }
            self.radio = None;
            self.queue = list;
            self.index = Some(0);
            self.playing = state.playing;
            self.start(cx);
            return;
        }
        if let (Some(i), Some(next)) = (self.index, &state.next) {
            if self.queue.get(i + 1).map(|t| t.video_id.as_str()) != Some(next.id.as_str()) {
                self.queue.truncate(i + 1);
                let track = track_of(next);
                self.fetch_art(&track, cx);
                self.queue.push(track);
                self.queue_changed(cx);
            }
        }
        if self.playing != state.playing {
            self.toggle(cx);
        }
        // Both paused: at the same spot, ready to go on together.
        if !state.playing && !self.loading {
            let want = state.position_at(now);
            if apart(want, self.now()) > la::DRIFT {
                self.seek_to(want);
            }
        }
    }

    /// A friend keeps within a couple of seconds of the host, and says now and then that
    /// they're still there.
    fn friend_tick(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.together.as_mut() else { return };
        if !s.ended && s.hello_at.elapsed() >= la::HELLO_EVERY {
            s.hello_at = Instant::now();
            let hello = Message::Hello { from: s.me.clone(), name: s.name.clone() };
            let inbox = la::inbox_topic(&s.code);
            self.post(inbox, hello, cx);
            return;
        }
        if !s.following || s.ended || s.corrected.elapsed() < la::CORRECT_EVERY {
            return;
        }
        let Some(state) = s.host.clone().filter(|h| h.playing) else { return };
        let Some(song) = &state.song else { return };
        let playing_it = self.current().is_some_and(|t| t.video_id == song.id);
        if !playing_it || !self.playing || self.loading || self.audio.status().generation != self.generation || self.length.is_zero() {
            return;
        }
        let want = state.position_at(la::unix_ms());
        if want < self.length && apart(want, self.now()) > la::DRIFT {
            if let Some(s) = self.together.as_mut() {
                s.corrected = Instant::now();
            }
            self.seek_to(want);
        }
    }

    // ---- Blend ------------------------------------------------------------------------

    /// The taste here: what's been played lately and the hearts. Reads the Stats log.
    fn my_taste(&self, cx: &mut Context<Self>) -> gpui::Task<Taste> {
        let liked: Vec<blend::LikedSong> =
            self.library.liked.iter().map(|s| (s.video_id.clone(), s.title.clone(), s.artist.clone(), s.thumbnail.clone())).collect();
        let name = self.my_name();
        let known = self.stats.listens.clone();
        cx.background_executor().spawn(async move {
            let listens = known.unwrap_or_else(|| stats::default_path().map(|p| stats::load(&p)).unwrap_or_default());
            let now = la::unix_ms() / 1000;
            Taste::from_listening(la::new_code(), name, &listens, &liked, now)
        })
    }

    /// Starts a Blend: this taste goes to the relay under a new code, for a friend to answer.
    pub(crate) fn start_blend(&mut self, cx: &mut Context<Self>) {
        if self.blend_busy {
            return;
        }
        self.blend_busy = true;
        self.blend_error = None;
        let task = self.my_taste(cx);
        cx.spawn(async move |this, cx| {
            let mine = task.await;
            let _ = this.update(cx, |app, cx| {
                app.blend_busy = false;
                if mine.is_empty() {
                    app.blend_error = Some(t("Play some music first: a Blend is made from what you listen to").into());
                    cx.notify();
                    return;
                }
                let code = la::new_code();
                let relay = relay();
                let (http, topic, text) = (la::client(), la::blend_topic(&code), mine.to_json());
                let send_relay = relay.clone();
                let sent = cx.background_executor().spawn(async move { la::send_text(&http, &send_relay, &topic, text) });
                settings::set("blend_waiting", &format!("{code} {relay}"));
                settings::set("blend_mine", &mine.to_json());
                let reader = la::read(&relay, &la::blend_topic(&code), CatchUp::All);
                app.blend_wait = Some(BlendWait { code, relay, mine, reader });
                cx.spawn(async move |this, cx| {
                    if let Err(e) = sent.await {
                        let _ = this.update(cx, |app, cx| {
                            app.blend_error = Some(tf("The Blend couldn't be shared: {}", &[&format!("{e:#}")]).into());
                            app.blend_wait = None;
                            settings::set("blend_waiting", "");
                            cx.notify();
                        });
                    }
                })
                .detach();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A Blend started here before NabyTune last closed: still waiting for the friend.
    pub(crate) fn resume_blend(&mut self) {
        let waiting = settings::get("blend_waiting").unwrap_or_default();
        let Some((code, relay)) = waiting.split_once(' ') else { return };
        let Some(mine) = settings::get("blend_mine").and_then(|m| Taste::from_json(&m)) else { return };
        let reader = la::read(relay, &la::blend_topic(code), CatchUp::All);
        self.blend_wait = Some(BlendWait { code: code.to_string(), relay: relay.to_string(), mine, reader });
    }

    pub(crate) fn cancel_blend(&mut self, cx: &mut Context<Self>) {
        self.blend_wait = None;
        settings::set("blend_waiting", "");
        settings::set("blend_mine", "");
        cx.notify();
    }

    pub(crate) fn copy_blend_invite(&mut self, cx: &mut Context<Self>) {
        let Some(w) = &self.blend_wait else { return };
        let code = la::invite_code(&w.code, &w.relay);
        cx.write_to_clipboard(ClipboardItem::new_string(tf("Let's make a Blend on NabyTune! Open Together (the people button), and under Blend enter: {}", &[&code])));
        self.note(t("Blend invite copied: paste it to your friend"), cx);
    }

    /// Answers a friend's Blend: reads their taste, leaves this one, and mixes the two.
    pub(crate) fn join_blend(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some((code, relay_named)) = la::parse_invite(text) else {
            self.blend_error = Some(t("That doesn't look like a Blend code").into());
            cx.notify();
            return;
        };
        if self.blend_busy {
            return;
        }
        self.blend_busy = true;
        self.blend_error = None;
        let relay = relay_named.unwrap_or_else(relay);
        let task = self.my_taste(cx);
        cx.spawn(async move |this, cx| {
            let mine = task.await;
            let topic = la::blend_topic(&code);
            let (read_relay, read_topic) = (relay.clone(), topic.clone());
            // The tastes left there so far, oldest (the one who started it) first.
            let theirs = cx
                .background_executor()
                .spawn(async move {
                    let http = la::client();
                    let response = http.get(format!("{read_relay}/{read_topic}/json?poll=1&since=all")).send()?;
                    anyhow::ensure!(response.status().is_success(), "the relay said HTTP {}", response.status().as_u16());
                    let text = response.text()?;
                    Ok::<_, anyhow::Error>(text.lines().filter_map(|line| {
                        let v: serde_json::Value = serde_json::from_str(line).ok()?;
                        Taste::from_json(v["message"].as_str()?)
                    }).next())
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.blend_busy = false;
                match theirs {
                    Ok(Some(theirs)) => {
                        if !mine.is_empty() {
                            let (http, text) = (la::client(), mine.to_json());
                            cx.background_executor().spawn(async move { la::send_text(&http, &relay, &topic, text) }).detach();
                        }
                        app.blend_field.update(cx, |f, cx| f.clear_text(cx));
                        app.make_blend(&mine, &theirs, cx);
                    }
                    Ok(None) => app.blend_error = Some(t("No Blend under that code (they last a few hours: ask for a new one)").into()),
                    Err(e) => app.blend_error = Some(tf("The Blend didn't come: {}", &[&format!("{e:#}")]).into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// While a Blend started here waits: the friend's taste, once it's there.
    fn blend_tick(&mut self, cx: &mut Context<Self>) {
        let Some(wait) = self.blend_wait.as_mut() else { return };
        let mut theirs = None;
        while let Ok(h) = wait.reader.heard.try_recv() {
            if let Heard::Message(text) = h {
                if let Some(taste) = Taste::from_json(&text).filter(|t| t.id != wait.mine.id) {
                    theirs.get_or_insert(taste);
                }
            }
        }
        if let Some(theirs) = theirs {
            let mine = wait.mine.clone();
            self.blend_wait = None;
            settings::set("blend_waiting", "");
            settings::set("blend_mine", "");
            self.make_blend(&mine, &theirs, cx);
        }
    }

    /// The two tastes mixed, kept as a playlist in Library.
    fn make_blend(&mut self, mine: &Taste, theirs: &Taste, cx: &mut Context<Self>) {
        let mixed = blend::mix(mine, theirs);
        let friend = if theirs.name.is_empty() { t("A friend").to_string() } else { theirs.name.clone() };
        let me = if mine.name.is_empty() { self.my_name() } else { mine.name.clone() };
        let name = tf("Blend · {} + {}", &[&me, &friend]);
        let tracks: Vec<library::Saved> = mixed
            .songs
            .iter()
            .map(|s| library::Saved {
                video_id: s.id.clone(),
                title: s.title.clone(),
                artist: s.artist.clone(),
                detail: s.artist.clone(),
                duration: String::new(),
                thumbnail: s.thumbnail.clone(),
            })
            .collect();
        self.library.playlists.retain(|p| p.name != name);
        self.library.playlists.push(library::Playlist { name: name.clone(), tracks });
        self.library.save();
        self.blend_made = Some(BlendMade { friend: friend.clone(), score: mixed.score, shared: mixed.shared_artists, playlist: name });
        self.note(tf("Your Blend with {} is ready: {}% alike", &[&friend, &mixed.score]), cx);
    }

    /// Opens the Blend's playlist in Library.
    fn open_blend(&mut self, cx: &mut Context<Self>) {
        let Some(made) = &self.blend_made else { return };
        if let Some(i) = self.library.playlists.iter().position(|p| p.name == made.playlist) {
            self.go_to(Tab::Library);
            self.open_playlist = Some(i);
            self.renaming = false;
            self.library_motion.clear();
            cx.notify();
        }
    }

    // ---- The panel --------------------------------------------------------------------

    pub(crate) fn together_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let body = div()
            .id("together-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.listen_along_card(cx))
            .child(self.blend_card(cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_none().px_4().pt_4().pb_2().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child(t("Together")))
            .child(body)
            .into_any_element()
    }

    fn listen_along_card(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let mut card = panel_card("Listen along");
        match &self.together {
            None => {
                card = card
                    .child(hint(t("Hear the same song at the same moment as your friends, each on your own PC. One of you hosts; the others join with the code.")))
                    .child(button("host-session", t("Host a session"), true).on_click(cx.listener(|this, _, _, cx| this.host_session(cx))))
                    .child(field_row(self.join_field.clone(), button("join-session", t("Join"), false).on_click(cx.listener(|this, _, _, cx| {
                        let text = this.join_field.read(cx).text().to_string();
                        this.join_session(&text, cx);
                    }))));
                if let Some(e) = &self.together_error {
                    card = card.child(warning(e.clone()));
                }
            }
            Some(s) if s.role == Role::Host => {
                let listening: SharedString = match s.friends.len() {
                    0 => t("No one has joined yet: share the code").into(),
                    _ => tf("Listening along: {}", &[&s.friends.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")]).into(),
                };
                card = card
                    .child(hint(t("You're hosting. Friends hear what you play, from where you are.")))
                    .child(code_view(&la::invite_code(&s.code, &s.relay)))
                    .child(button("copy-invite", t("Copy invite"), true).on_click(cx.listener(|this, _, _, cx| this.copy_session_invite(cx))))
                    .child(div().text_size(px(13.0)).child(listening))
                    .child(toggle_row("session-requests", t("Friends can add songs to your queue"), s.requests).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = this.together.as_mut() {
                            s.requests = !s.requests;
                            settings::set("listen_requests", if s.requests { "1" } else { "0" });
                        }
                        cx.notify();
                    })))
                    .child(button("end-session", t("End session"), false).on_click(cx.listener(|this, _, _, cx| this.leave_session(cx))));
            }
            Some(s) => {
                let host = s.host.as_ref().map(|h| h.host.clone()).filter(|h| !h.is_empty()).unwrap_or_else(|| t("the host").to_string());
                let what: SharedString = match (&s.host, s.ended) {
                    (_, true) => t("The session has ended.").into(),
                    (None, _) => t("Waiting to hear from the host…").into(),
                    (Some(h), _) => match (&h.song, &h.local) {
                        (Some(song), _) => tf("{} is playing “{}” by {}", &[&host, &song.title, &song.artist]).into(),
                        (None, Some(_)) => tf("{} is playing a song from their PC, which can't be shared", &[&host]).into(),
                        _ => tf("{} isn't playing anything right now", &[&host]).into(),
                    },
                };
                card = card.child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).child(tf("Listening along with {}", &[&host]))).child(hint(what));
                if !s.ended && !s.following {
                    card = card.child(button("follow-again", t("Follow again"), true).on_click(cx.listener(|this, _, _, cx| this.follow_again(cx))));
                }
                if !s.ended && s.host.as_ref().is_some_and(|h| h.requests) {
                    card = card.child(hint(t("Right-click any song › Send to the session, to add it to their queue.")));
                }
                card = card.child(button("leave-session", if s.ended { t("Close") } else { t("Leave") }, false).on_click(cx.listener(|this, _, _, cx| this.leave_session(cx))));
            }
        }
        if let Some(s) = &self.together {
            let status: SharedString = match (&s.trouble, s.connected) {
                (Some(why), _) => tf("The relay isn't answering ({}); trying again…", &[why]).into(),
                (None, false) => t("Connecting…").into(),
                (None, true) => tf("Connected through {}", &[&s.relay.trim_start_matches("https://")]).into(),
            };
            card = card.child(div().text_size(px(11.0)).text_color(rgba(if s.trouble.is_some() { 0xff8a80ff } else { TERTIARY })).child(status));
        }
        card.child(div().text_size(px(11.0)).text_color(rgba(TERTIARY)).child(t(
            "It goes through a public relay: what plays and your first name, under the session's secret code. Nothing else leaves your PC.",
        )))
    }

    fn blend_card(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let mut card = panel_card("Blend").child(hint(t(
            "Your taste and a friend's in one playlist: the songs you both love, then turn about from each of you. And how alike you are.",
        )));
        if let Some(made) = self.blend_made.clone() {
            let shared: SharedString = if made.shared.is_empty() {
                t("No artists in common yet: a chance to discover each other's").into()
            } else {
                tf("You both play {}", &[&made.shared.join(", ")]).into()
            };
            card = card.child(
                div()
                    .p(px(12.0))
                    .rounded(px(10.0))
                    .bg(rgba(SUBTLE_STRONG))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_size(px(28.0)).font_weight(FontWeight::BOLD).text_color(rgb(accent())).child(format!("{}%", made.score)))
                    .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).child(tf("alike with {}", &[&made.friend])))
                    .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(shared))
                    .child(button("open-blend", t("Open the playlist"), true).mt(px(6.0)).on_click(cx.listener(|this, _, _, cx| this.open_blend(cx)))),
            );
        }
        match &self.blend_wait {
            Some(w) => {
                card = card
                    .child(hint(t("Waiting for your friend to enter this code. Your Blend appears here as soon as they do.")))
                    .child(code_view(&la::invite_code(&w.code, &w.relay)))
                    .child(button("copy-blend", t("Copy invite"), true).on_click(cx.listener(|this, _, _, cx| this.copy_blend_invite(cx))))
                    .child(button("cancel-blend", t("Cancel"), false).on_click(cx.listener(|this, _, _, cx| this.cancel_blend(cx))));
            }
            None => {
                card = card
                    .child(
                        button("start-blend", if self.blend_busy { t("Working it out…") } else { t("Start a Blend") }, true)
                            .on_click(cx.listener(|this, _, _, cx| this.start_blend(cx))),
                    )
                    .child(field_row(self.blend_field.clone(), button("join-blend", t("Join"), false).on_click(cx.listener(|this, _, _, cx| {
                        let text = this.blend_field.read(cx).text().to_string();
                        this.join_blend(&text, cx);
                    }))));
            }
        }
        if let Some(e) = &self.blend_error {
            card = card.child(warning(e.clone()));
        }
        card
    }
}

fn panel_card(title: &'static str) -> gpui::Div {
    div()
        .p(px(14.0))
        .rounded(px(12.0))
        .bg(rgba(SUBTLE))
        .border_1()
        .border_color(rgba(STROKE))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).child(t(title)))
}

fn hint(text: impl Into<SharedString>) -> gpui::Div {
    div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(text.into())
}

fn warning(text: SharedString) -> gpui::Div {
    div().text_size(px(12.0)).text_color(rgba(0xff8a80ff)).child(text)
}

fn button(id: &'static str, label: impl Into<SharedString>, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(32.0))
        .px(px(14.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .map(|d| {
            if primary {
                d.bg(rgb(accent())).text_color(rgb(on_accent())).hover(|s| s.opacity(0.9))
            } else {
                d.bg(rgba(SUBTLE_STRONG)).border_1().border_color(rgba(STROKE)).hover(|s| s.bg(rgba(0xffffff26)))
            }
        })
        .child(label.into())
}

/// A code, big enough to read out.
fn code_view(code: &str) -> gpui::Div {
    div()
        .py(px(10.0))
        .rounded(px(8.0))
        .bg(rgba(0x0000_0033))
        .flex()
        .justify_center()
        .text_size(px(if code.len() > 20 { 13.0 } else { 18.0 }))
        .font_weight(FontWeight::BOLD)
        .child(code.to_string())
}

fn field_row(field: gpui::Entity<crate::search_box::SearchBox>, join: gpui::Stateful<gpui::Div>) -> gpui::Div {
    div().flex().items_center().gap_2().child(div().flex_1().min_w_0().child(field)).child(join)
}

fn toggle_row(id: &'static str, label: impl Into<SharedString>, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .child(div().flex_1().text_size(px(13.0)).child(label.into()))
        .child(crate::settings_page::switch("together-switch", on))
}
