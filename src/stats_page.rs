//! Stats: what the listener played, per week, month, year or all time (stats.rs counts it),
//! said plainly first ("You listened for 12 hours this month"), then the top songs and
//! artists, when the music plays, what's new, and a "Wrapped" card, in one of ten styles,
//! to save as a picture. Every listen is logged as it ends: the seconds really heard,
//! pauses and skipped parts left out. Songs and artists can be hidden from all of it, and
//! the history cleared.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    canvas, div, img, linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba, AnyElement, Bounds, BoxShadow, Context, FontWeight, ObjectFit,
    Pixels, RenderImage, ScrollHandle, SharedString, StyledImage, Window,
};

use crate::appearance::{accent, accent_wash, on_accent};
use crate::i18n::{self, plural, t, tf};
use crate::stats::{self, ArtistStat, Hidden, Listen, Period, SongStat, Summary};
use crate::{
    artwork, brand_mark, local, settings, snapshot, NabyTune, Panel, Tab, Track, DISPLAY, ICONS, PANEL, SECONDARY, STROKE, SUBTLE, SUBTLE_STRONG,
    TERTIARY,
};

/// The song being listened to: the log's entry so far, when it was last counted, and the
/// seconds heard (with their fractions).
pub(crate) struct Listening {
    listen: Listen,
    counted: Instant,
    heard: f32,
}

/// The Wrapped card's size: 4:5, as a post, the same in every style.
const CARD_W: f32 = 480.0;
const CARD_H: f32 = 600.0;
/// How many of each top list show until "Show top 10".
const SHORT_LIST: usize = 5;

// The cards' typefaces, all on every Windows 10 and 11.
const IMPACT: &str = "Impact";
const MONO: &str = "Consolas";
const TYPEWRITER: &str = "Courier New";
const SERIF: &str = "Georgia";
const HAND: &str = "Ink Free";
const ROUNDED: &str = "Bahnschrift";

/// How the Wrapped card looks. Each is its own layout, palette and type, not a recolour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WrappedStyle {
    /// Dark, lit by the accent colour (follows Settings › Appearance).
    Signature,
    /// A bold poster: one loud colour, huge condensed type.
    Poster,
    /// Black on white, light type and thin rules.
    Minimal,
    /// A till receipt on paper, typewriter type and a barcode.
    Receipt,
    /// Glowing signs on a night-purple ground.
    Neon,
    /// Northern lights behind frosted glass.
    Aurora,
    /// A 70s mixtape: side A the songs, side B the artists.
    Cassette,
    /// The top song's cover as an instant photo, with a handwritten caption.
    Polaroid,
    /// A grid of big numbers on colour blocks.
    Numbers,
    /// A magazine cover: the top song's cover full-bleed, under a serif masthead.
    Magazine,
}

impl WrappedStyle {
    pub const ALL: [WrappedStyle; 10] = [
        WrappedStyle::Signature,
        WrappedStyle::Poster,
        WrappedStyle::Minimal,
        WrappedStyle::Receipt,
        WrappedStyle::Neon,
        WrappedStyle::Aurora,
        WrappedStyle::Cassette,
        WrappedStyle::Polaroid,
        WrappedStyle::Numbers,
        WrappedStyle::Magazine,
    ];

    /// What Settings keeps ("wrapped_style").
    pub fn key(self) -> &'static str {
        match self {
            WrappedStyle::Signature => "signature",
            WrappedStyle::Poster => "poster",
            WrappedStyle::Minimal => "minimal",
            WrappedStyle::Receipt => "receipt",
            WrappedStyle::Neon => "neon",
            WrappedStyle::Aurora => "aurora",
            WrappedStyle::Cassette => "cassette",
            WrappedStyle::Polaroid => "polaroid",
            WrappedStyle::Numbers => "numbers",
            WrappedStyle::Magazine => "magazine",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// Its name, in English (shown through `t`).
    fn name(self) -> &'static str {
        match self {
            WrappedStyle::Signature => "Signature",
            WrappedStyle::Poster => "Poster",
            WrappedStyle::Minimal => "Minimal",
            WrappedStyle::Receipt => "Receipt",
            WrappedStyle::Neon => "Neon",
            WrappedStyle::Aurora => "Aurora",
            WrappedStyle::Cassette => "Cassette",
            WrappedStyle::Polaroid => "Polaroid",
            WrappedStyle::Numbers => "Big numbers",
            WrappedStyle::Magazine => "Magazine",
        }
    }

    /// Whether it shows the top song's cover large (and so fetches a bigger one).
    fn big_cover(self) -> bool {
        matches!(self, WrappedStyle::Polaroid | WrappedStyle::Magazine)
    }
}

/// What was hidden last, to take back.
#[derive(Clone)]
enum Undo {
    Song { id: String, title: String },
    Artist(String),
}

/// What the summary on screen was worked out from.
#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    period: Period,
    back: u32,
    listens: usize,
    /// Five-minute steps of the clock (for "today" and the period's bounds).
    now: u64,
    /// The song playing: when it started, and ten-second steps of what's been heard.
    playing: Option<(u64, u32)>,
}

struct Cached {
    key: CacheKey,
    summary: Rc<Summary>,
    /// The first listen that counts, if any: there's nothing to page back to before it.
    earliest: Option<u64>,
    offset: i64,
}

/// The Stats page's state.
pub(crate) struct StatsView {
    pub period: Period,
    /// How many periods back (0: this one).
    pub back: u32,
    /// The whole log, once read.
    pub listens: Option<Vec<Listen>>,
    /// Songs and artists left out of everything here (stats_hidden.json).
    pub hidden: Hidden,
    /// The Wrapped card's look (Settings' "wrapped_style").
    pub style: WrappedStyle,
    /// Where the Wrapped card is on screen, for saving it.
    pub card: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Where the last picture went, or why it didn't.
    pub note: Option<SharedString>,
    /// The top lists show ten, not five.
    expanded: bool,
    /// The list of hidden songs and artists is open.
    show_hidden: bool,
    /// "Clear history" was pressed once: the choice and the warning show.
    clearing: bool,
    /// What was hidden last, for Undo.
    undo: Option<Undo>,
    /// What the last clear did, or why it didn't.
    data_note: Option<SharedString>,
    /// The page's scroll, to bring the card into view before saving it.
    scroll: ScrollHandle,
    /// The summary on screen, worked out again only when what it shows changed.
    cache: Option<Cached>,
}

impl Default for StatsView {
    fn default() -> Self {
        Self {
            period: Period::Month,
            back: 0,
            listens: None,
            hidden: Hidden::default(),
            style: settings::get("wrapped_style").and_then(|k| WrappedStyle::from_key(&k)).unwrap_or(WrappedStyle::Signature),
            card: Rc::new(Cell::new(None)),
            note: None,
            expanded: false,
            show_hidden: false,
            clearing: false,
            undo: None,
            data_note: None,
            scroll: ScrollHandle::new(),
            cache: None,
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl NabyTune {
    // ---- The log -------------------------------------------------------------------

    /// A song started: the one before is logged, this one starts counting.
    pub(crate) fn listen_start(&mut self) {
        self.listen_flush();
        let Some(track) = self.current() else { return };
        let listen = Listen {
            video_id: track.video_id.clone(),
            title: track.title.to_string(),
            artist: track.artist.to_string(),
            album: track.detail.split(" • ").nth(1).unwrap_or("").to_string(),
            thumbnail: track.thumbnail.clone(),
            at: unix_now(),
            seconds: 0,
            length: self.length.as_secs() as u32,
        };
        self.listening = Some(Listening { listen, counted: Instant::now(), heard: 0.0 });
    }

    /// Counts the time since the last tick, while the song is really playing. A long gap
    /// (the computer slept) counts as two seconds at most.
    pub(crate) fn listen_tick(&mut self) {
        let playing = self.playing && !self.loading;
        let Some(l) = self.listening.as_mut() else { return };
        let gap = l.counted.elapsed().min(Duration::from_secs(2));
        l.counted = Instant::now();
        if playing {
            l.heard += gap.as_secs_f32();
        }
    }

    /// Logs the song being listened to (if any of it was heard).
    pub(crate) fn listen_flush(&mut self) {
        self.listen_tick();
        let Some(mut l) = self.listening.take() else { return };
        l.listen.seconds = l.heard as u32;
        if l.listen.seconds == 0 {
            return;
        }
        if let Some(path) = stats::default_path() {
            if let Err(e) = stats::record(&path, &l.listen) {
                crate::audio::log(format!("stats: couldn't log a listen: {e:#}"));
            }
        }
        if let Some(all) = self.stats.listens.as_mut() {
            all.push(l.listen);
        }
    }

    /// Opens Stats, reading the log (and what's hidden) the first time.
    pub(crate) fn open_stats(&mut self, cx: &mut Context<Self>) {
        self.go_to(Tab::Stats);
        if self.stats.listens.is_none() {
            cx.spawn(async move |this, cx| {
                let (listens, hidden) = cx
                    .background_executor()
                    .spawn(async move {
                        let listens = stats::default_path().map(|p| stats::load(&p)).unwrap_or_default();
                        let hidden = stats::hidden_path().map(|p| Hidden::load(&p)).unwrap_or_default();
                        (listens, hidden)
                    })
                    .await;
                let _ = this.update(cx, |app, cx| {
                    app.stats.listens = Some(listens);
                    app.stats.hidden = hidden;
                    app.stats.cache = None;
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// The summary of the period on screen, of the listens that count (what's playing now
    /// included), worked out again only when something in it changed.
    fn stats_summary(&mut self) -> (Rc<Summary>, Option<u64>, i64) {
        let now = unix_now();
        let key = CacheKey {
            period: self.stats.period,
            back: self.stats.back,
            listens: self.stats.listens.as_ref().map_or(0, Vec::len),
            now: now / 300,
            playing: self.listening.as_ref().map(|l| (l.listen.at, l.heard as u32 / 10)),
        };
        if let Some(c) = self.stats.cache.as_ref().filter(|c| c.key == key) {
            return (c.summary.clone(), c.earliest, c.offset);
        }
        let offset = stats::local_offset();
        let mut all = stats::visible(self.stats.listens.as_deref().unwrap_or_default(), &self.stats.hidden);
        if let Some(l) = &self.listening {
            let mut current = l.listen.clone();
            current.seconds = l.heard as u32;
            if current.seconds > 0 && !self.stats.hidden.hides(&current) {
                all.push(current);
            }
        }
        let earliest = all.iter().map(|l| l.at).min();
        let summary = Rc::new(stats::summary_back(&all, key.period, key.back, now, offset));
        self.stats.cache = Some(Cached { key, summary: summary.clone(), earliest, offset });
        (summary, earliest, offset)
    }

    // ---- The page ------------------------------------------------------------------

    pub(crate) fn stats_page(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let scroll = self.stats.scroll.clone();
        // A readable column in the middle, however wide the window.
        let page = |column: gpui::Div| {
            div()
                .id("stats")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .px(px(32.0))
                .pt(px(28.0))
                .pb(px(48.0))
                .flex()
                .flex_col()
                .items_center()
                .child(column)
                .into_any_element()
        };
        let mut column = div().w_full().max_w(px(1100.0)).flex().flex_col().gap_8();
        let Some(listens) = self.stats.listens.as_ref() else {
            column = column.child(Self::heading(t("Stats"), 30.0)).child(div().text_color(rgba(SECONDARY)).child(t("Reading your listening…")));
            return page(column);
        };
        let has_history = !listens.is_empty();
        if !has_history && self.listening.as_ref().is_none_or(|l| l.heard < 1.0) {
            column = column.child(Self::heading(t("Stats"), 30.0)).child(welcome());
            if !self.stats.hidden.is_empty() {
                column = column.child(self.manage_section(None, false, cx));
            }
            return page(column);
        }

        let (summary, earliest, offset) = self.stats_summary();
        for song in &summary.top_songs {
            self.fetch_art(&song_track(song), cx);
        }
        for artist in &summary.top_artists {
            if let Some(url) = &artist.thumbnail {
                self.fetch_art_at(url.clone(), url.clone(), 120, 120, cx);
            }
        }

        column = column.child(self.stats_header(&summary, earliest, offset, cx));
        if summary.seconds == 0 {
            column = column.child(self.empty_period(&summary, cx));
        } else {
            column = column
                .child(self.stats_hero(&summary, cx))
                .child(numbers(&summary))
                .children(self.undo_bar(cx))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_8()
                        .items_start()
                        .child(div().flex_1().min_w(px(360.0)).child(self.top_songs(&summary, cx)))
                        .child(div().flex_1().min_w(px(300.0)).max_w(px(420.0)).child(self.top_artists(&summary, cx))),
                )
                .child(when_you_listen(&summary));
            if summary.period != Period::All && !summary.discoveries.is_empty() {
                column = column.child(self.discoveries(&summary, cx));
            }
            column = column.child(self.wrapped_section(&summary, cx));
        }
        column = column.child(self.manage_section(Some(&summary), has_history, cx));
        page(column)
    }

    /// The title, the kind of period (week, month, year, all time) and which one, with ‹ ›.
    fn stats_header(&self, summary: &Summary, earliest: Option<u64>, offset: i64, cx: &mut Context<Self>) -> gpui::Div {
        let (period, back) = (self.stats.period, self.stats.back);
        let mut kinds = div().flex().gap(px(2.0)).p(px(3.0)).rounded(px(10.0)).bg(rgba(0x0000_0033)).border_1().border_color(rgba(STROKE));
        for (n, (kind, name)) in [(Period::Week, "Week"), (Period::Month, "Month"), (Period::Year, "Year"), (Period::All, "All time")].into_iter().enumerate() {
            let on = kind == period;
            kinds = kinds.child(
                div()
                    .id(("stats-period", n))
                    .h(px(32.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .map(|d| match on {
                        true => d.bg(rgb(accent())).text_color(rgb(on_accent())).font_weight(FontWeight::SEMIBOLD),
                        false => d.text_color(rgba(SECONDARY)).hover(|s| s.bg(rgba(SUBTLE_STRONG)).text_color(rgb(0xffffff))),
                    })
                    .child(t(name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.stats.period = kind;
                        this.stats.back = 0;
                        this.stats.note = None;
                        cx.notify();
                    })),
            );
        }
        // Back as far as there's anything to see; ahead up to now.
        let can_go_back = period != Period::All && earliest.is_some_and(|e| e < period.span(back + 1, unix_now(), offset).end);
        let can_go_ahead = back > 0;
        let arrow = |id: &'static str, glyph: &'static str, enabled: bool| {
            div()
                .id(id)
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(rgba(STROKE))
                .bg(rgba(SUBTLE))
                .font_family(ICONS)
                .text_size(px(11.0))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(rgba(SUBTLE_STRONG))))
                .when(!enabled, |d| d.opacity(0.3))
                .child(glyph)
        };
        let navigator = (period != Period::All).then(|| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(arrow("stats-back", "\u{E76B}", can_go_back).on_click(cx.listener(move |this, _, _, cx| {
                    if can_go_back {
                        this.stats.back += 1;
                        this.stats.note = None;
                        cx.notify();
                    }
                })))
                .child(div().min_w(px(160.0)).text_center().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).child(summary.label.clone()))
                .child(arrow("stats-ahead", "\u{E76C}", can_go_ahead).on_click(cx.listener(move |this, _, _, cx| {
                    this.stats.back = this.stats.back.saturating_sub(1);
                    this.stats.note = None;
                    cx.notify();
                })))
                .when(back > 0, |d| {
                    d.child(
                        div()
                            .id("stats-now")
                            .ml_2()
                            .text_size(px(13.0))
                            .text_color(rgb(accent()))
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child(t("Back to now"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stats.back = 0;
                                this.stats.note = None;
                                cx.notify();
                            })),
                    )
                })
        });
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Self::heading(t("Stats"), 30.0))
                    .child(div().text_size(px(14.0)).text_color(rgba(SECONDARY)).child(t("What you've been listening to, in plain numbers"))),
            )
            .child(div().flex().flex_wrap().items_center().justify_between().gap_4().child(kinds).children(navigator))
    }

    /// The period in a sentence or two, the top song beside it, and a button to play the top songs.
    fn stats_hero(&self, s: &Summary, cx: &mut Context<Self>) -> gpui::Div {
        let top = s.top_songs.first();
        let cover = top.and_then(|x| self.art.get(&x.video_id)).map(|a| a.small.clone());
        let queue: Vec<Track> = s.top_songs.iter().map(song_track).collect();
        let mut facts = tf("That's {} by {}.", &[&i18n::songs(s.plays as usize), &plural(s.artists as usize, "{} artist", "{} artists")]);
        if let Some(c) = comparison(s) {
            facts.push(' ');
            facts.push_str(&c);
        }
        let picture = match cover {
            Some(image) => artwork(Some(image), 136.0, 14.0).shadow_lg(),
            None => div().size(px(136.0)).flex_none().rounded(px(14.0)).bg(rgba(SUBTLE_STRONG)).flex().items_center().justify_center().child(brand_mark(44.0)),
        };
        div()
            .p(px(24.0))
            .flex()
            .items_center()
            .gap_6()
            .rounded(px(16.0))
            .border_1()
            .border_color(rgba(accent_wash(0x4d)))
            .bg(linear_gradient(120.0, linear_color_stop(rgba(accent_wash(0x30)), 0.0), linear_color_stop(rgba(0xffff_ff08), 1.0)))
            .child(picture)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(accent())).child(s.label.to_uppercase()))
                    .child(div().font_family(DISPLAY).text_size(px(30.0)).line_height(px(36.0)).font_weight(FontWeight::BOLD).child(headline(s, self.stats.back)))
                    .child(div().text_size(px(14.0)).text_color(rgba(SECONDARY)).child(facts))
                    .when_some(top, |d, song| {
                        d.child(
                            div()
                                .text_size(px(14.0))
                                .child(format!("{} · {}", tf("Your #1 song is “{}” by {}", &[&song.title, &song.artist]), plural(song.plays as usize, "{} play", "{} plays"))),
                        )
                    })
                    .when(s.plays < 5, |d| d.child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(t("Just getting started: the more you listen, the fuller this gets."))))
                    .when(!queue.is_empty(), |d| {
                        d.child(
                            div().pt_2().flex().child(
                                pill_button("stats-play-top", "\u{E768}", t("Play your top songs"), true)
                                    .on_click(cx.listener(move |this, _, _, cx| this.play_from(queue.clone(), 0, cx))),
                            ),
                        )
                    }),
            )
    }

    /// Nothing heard in the period on screen (but there's history elsewhere).
    fn empty_period(&self, s: &Summary, cx: &mut Context<Self>) -> gpui::Div {
        // (The period's name is right above, by the arrows.)
        div()
            .py(px(48.0))
            .px(px(24.0))
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .rounded(px(16.0))
            .bg(rgba(SUBTLE))
            .border_1()
            .border_color(rgba(STROKE))
            .child(brand_mark(40.0))
            .child(div().font_family(DISPLAY).text_size(px(22.0)).font_weight(FontWeight::SEMIBOLD).child(t("No music here yet")))
            .child(div().text_size(px(14.0)).text_color(rgba(SECONDARY)).text_center().child(t("Nothing was played in this stretch. Pick another one above, or see everything at once.")))
            .when(s.period != Period::All, |d| {
                d.child(div().pt_2().child(pill_button("stats-see-all", "\u{E81C}", t("See all time"), true).on_click(cx.listener(|this, _, _, cx| {
                    this.stats.period = Period::All;
                    this.stats.back = 0;
                    cx.notify();
                }))))
            })
    }

    /// "“Song” is hidden · Undo", after hiding something.
    fn undo_bar(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let undo = self.stats.undo.clone()?;
        let what = match &undo {
            Undo::Song { title, .. } => tf("“{}” is hidden from your stats", &[title]),
            Undo::Artist(name) => tf("{} is hidden from your stats, with their songs", &[name]),
        };
        Some(
            div()
                .px(px(16.0))
                .py(px(10.0))
                .flex()
                .items_center()
                .gap_3()
                .rounded(px(10.0))
                .bg(rgba(SUBTLE_STRONG))
                .border_1()
                .border_color(rgba(STROKE))
                .child(div().font_family(ICONS).text_size(px(14.0)).text_color(rgba(SECONDARY)).child("\u{ED1A}"))
                .child(div().flex_1().min_w_0().text_size(px(13.0)).child(what))
                .child(
                    div()
                        .id("stats-undo")
                        .px(px(12.0))
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(accent()))
                        .hover(|s| s.bg(rgba(SUBTLE_STRONG)))
                        .child(t("Undo"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            match &undo {
                                Undo::Song { id, .. } => this.stats.hidden.show_song(id),
                                Undo::Artist(name) => this.stats.hidden.show_artist(name),
                            };
                            this.stats.undo = None;
                            this.hidden_changed();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("stats-undo-close")
                        .size(px(28.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .font_family(ICONS)
                        .text_size(px(10.0))
                        .text_color(rgba(SECONDARY))
                        .hover(|s| s.bg(rgba(SUBTLE_STRONG)))
                        .child("\u{E711}")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.stats.undo = None;
                            cx.notify();
                        })),
                ),
        )
    }

    /// A section heading, with a line under it and, on the right, an optional control.
    fn section_head(title: &'static str, hint: Option<&'static str>, control: Option<AnyElement>) -> gpui::Div {
        div()
            .flex()
            .items_end()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(Self::heading(t(title), 20.0))
                    .when_some(hint, |d, h| d.child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(t(h)))),
            )
            .children(control)
    }

    /// "Show top 10" / "Show fewer", for both lists.
    fn more_toggle(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(id)
            .text_size(px(13.0))
            .text_color(rgb(accent()))
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(if self.stats.expanded { t("Show fewer") } else { t("Show top 10") })
            .on_click(cx.listener(|this, _, _, cx| {
                this.stats.expanded = !this.stats.expanded;
                cx.notify();
            }))
            .into_any_element()
    }

    fn top_songs(&self, s: &Summary, cx: &mut Context<Self>) -> gpui::Div {
        let count = if self.stats.expanded { stats::TOP } else { SHORT_LIST };
        let tracks: Vec<Track> = s.top_songs.iter().map(song_track).collect();
        let mut list = div().flex().flex_col();
        for (n, (song, track)) in s.top_songs.iter().zip(tracks.iter().cloned()).enumerate().take(count) {
            let queue = tracks.clone();
            let group: SharedString = format!("stats-song-{n}").into();
            let hide = song.clone();
            let menu_track = track.clone();
            list = list.child(
                div()
                    .id(("top-song", n))
                    .group(group.clone())
                    .h(px(58.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(SUBTLE)))
                    .child(rank(n))
                    .child(artwork(self.art.get(&track.video_id).map(|a| a.small.clone()), 44.0, 6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).truncate().child(song.title.clone()))
                            .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).truncate().child(song.artist.clone())),
                    )
                    .child(div().flex_none().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(plural(song.plays as usize, "{} play", "{} plays")))
                    .child(hide_button(("hide-song", n), group).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.hide_song(&hide);
                        cx.notify();
                    })))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.open_track_menu(menu_track.clone(), event.position, None);
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.play_from(queue.clone(), n, cx))),
            );
        }
        let toggle = (s.top_songs.len() > SHORT_LIST).then(|| self.more_toggle("stats-more-songs", cx));
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::section_head("Top songs", Some("Played the most. Click one to play it."), toggle))
            .child(list.mx(px(-8.0)))
    }

    fn top_artists(&self, s: &Summary, cx: &mut Context<Self>) -> gpui::Div {
        let count = if self.stats.expanded { stats::TOP } else { SHORT_LIST };
        let mut list = div().flex().flex_col();
        let most = s.top_artists.first().map_or(1, |a| a.minutes.max(1)) as f32;
        for (n, artist) in s.top_artists.iter().enumerate().take(count) {
            let name = artist.name.clone();
            let hide = artist.name.clone();
            let group: SharedString = format!("stats-artist-{n}").into();
            let picture = artist.thumbnail.as_ref().and_then(|u| self.art.get(u)).map(|a| a.small.clone());
            list = list.child(
                div()
                    .id(("top-artist", n))
                    .group(group.clone())
                    .h(px(58.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(SUBTLE)))
                    .child(rank(n))
                    .child(artwork(picture, 40.0, 20.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().flex_1().min_w_0().truncate().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).child(artist.name.clone()))
                                    .child(div().flex_none().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(tf("{} min", &[&artist.minutes]))),
                            )
                            .child(
                                div()
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(rgba(SUBTLE_STRONG))
                                    .child(div().h_full().rounded_full().w(gpui::relative((artist.minutes as f32 / most).clamp(0.02, 1.0))).bg(rgb(accent()))),
                            ),
                    )
                    .child(hide_button(("hide-artist", n), group).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.hide_artist(&hide);
                        cx.notify();
                    })))
                    // Their songs, through search.
                    .on_click(cx.listener(move |this, _, _, cx| this.run_search(name.clone(), cx))),
            );
        }
        let toggle = (s.top_artists.len() > SHORT_LIST && s.top_songs.len() <= SHORT_LIST).then(|| self.more_toggle("stats-more-artists", cx));
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::section_head("Top artists", Some("By minutes heard. Click one to find their songs."), toggle))
            .child(list.mx(px(-8.0)))
    }

    /// Artists heard for the first time in this stretch; a click finds their songs.
    fn discoveries(&self, s: &Summary, cx: &mut Context<Self>) -> gpui::Div {
        let mut chips = div().flex().flex_wrap().gap_2();
        for (n, a) in s.discoveries.iter().enumerate() {
            let name = a.name.clone();
            chips = chips.child(
                div()
                    .id(("stats-new", n))
                    .h(px(32.0))
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(rgba(SUBTLE))
                    .border_1()
                    .border_color(rgba(STROKE))
                    .hover(|s| s.bg(rgba(SUBTLE_STRONG)))
                    .text_size(px(13.0))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(a.name.clone()))
                    .child(div().text_color(rgba(SECONDARY)).child(tf("{} min", &[&a.minutes])))
                    .on_click(cx.listener(move |this, _, _, cx| this.run_search(name.clone(), cx))),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(Self::heading(t("New to you"), 20.0))
                    .child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(tf("{} artists you hadn't played before", &[&s.new_artists]))),
            )
            .child(chips)
    }

    // ---- Hiding --------------------------------------------------------------------

    fn hide_song(&mut self, song: &SongStat) {
        if self.stats.hidden.hide_song(song.key(), &song.title, &song.artist) {
            self.stats.undo = Some(Undo::Song { id: song.key().to_string(), title: song.title.clone() });
            self.hidden_changed();
        }
    }

    fn hide_artist(&mut self, name: &str) {
        if self.stats.hidden.hide_artist(name) {
            self.stats.undo = Some(Undo::Artist(name.to_string()));
            self.hidden_changed();
        }
    }

    /// What's hidden changed: counted again, and kept.
    fn hidden_changed(&mut self) {
        self.stats.cache = None;
        if let Some(path) = stats::hidden_path() {
            if let Err(e) = self.stats.hidden.save(&path) {
                crate::audio::log(format!("stats: couldn't keep what's hidden: {e:#}"));
                self.stats.data_note = Some(tf("Couldn't save that: {}", &[&format!("{e:#}")]).into());
            }
        }
    }

    // ---- Clearing ------------------------------------------------------------------

    /// Deletes the listening history for good: `span`'s, or all of it.
    fn clear_history(&mut self, span: Option<stats::Span>, label: String, cx: &mut Context<Self>) {
        let Some(path) = stats::default_path() else { return };
        let result = match span {
            Some(span) => stats::clear_span(&path, span).map(|_| ()),
            None => stats::clear_all(&path),
        };
        match result {
            Ok(()) => {
                if let Some(all) = self.stats.listens.as_mut() {
                    match span {
                        Some(span) => all.retain(|l| !span.contains(l.at)),
                        None => all.clear(),
                    }
                }
                // The song playing now starts counting afresh, if it was in what went.
                if let Some(l) = self.listening.as_mut() {
                    if span.is_none_or(|s| s.contains(l.listen.at)) {
                        l.heard = 0.0;
                        l.counted = Instant::now();
                        l.listen.at = unix_now();
                    }
                }
                self.stats.data_note = Some(match span {
                    Some(_) => tf("{} is cleared", &[&label]).into(),
                    None => t("Your listening history is cleared").into(),
                });
                if span.is_none() {
                    self.stats.back = 0;
                }
                self.stats.undo = None;
            }
            Err(e) => {
                crate::audio::log(format!("stats: couldn't clear the history: {e:#}"));
                self.stats.data_note = Some(tf("Couldn't clear it: {}", &[&format!("{e:#}")]).into());
            }
        }
        self.stats.clearing = false;
        self.stats.cache = None;
        cx.notify();
    }

    /// Hidden songs and artists, and clearing the history; then how it's all counted.
    fn manage_section(&self, summary: Option<&Summary>, has_history: bool, cx: &mut Context<Self>) -> gpui::Div {
        let hidden = &self.stats.hidden;
        let row = |glyph: &'static str, title: &'static str, hint: &'static str, control: AnyElement| {
            div()
                .min_h(px(68.0))
                .px(px(16.0))
                .py(px(12.0))
                .flex()
                .items_center()
                .gap_4()
                .child(div().w(px(20.0)).flex_none().font_family(ICONS).text_size(px(16.0)).text_color(rgba(SECONDARY)).child(glyph))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(180.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().text_size(px(14.0)).child(t(title)))
                        .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(t(hint))),
                )
                .child(control)
        };
        let hidden_control = if hidden.is_empty() {
            div().text_size(px(13.0)).text_color(rgba(TERTIARY)).child(t("None")).into_any_element()
        } else {
            pill_button("stats-hidden-toggle", if self.stats.show_hidden { "\u{E70E}" } else { "\u{E70D}" }, if self.stats.show_hidden { t("Close").into() } else { tf("Show all {}", &[&hidden.len()]) }, false)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.stats.show_hidden = !this.stats.show_hidden;
                    cx.notify();
                }))
                .into_any_element()
        };
        let mut card = div().flex().flex_col().rounded(px(12.0)).bg(rgba(SUBTLE)).border_1().border_color(rgba(STROKE)).child(row(
            "\u{ED1A}",
            "Hidden songs and artists",
            "Left out of everything here, minutes and Wrapped too. Hiding an artist hides songs they're on. Hover a song or artist above and press Hide.",
            hidden_control,
        ));
        if self.stats.show_hidden && !hidden.is_empty() {
            let mut list = div().flex().flex_col().px(px(16.0)).pb(px(12.0)).gap(px(2.0));
            let entries = hidden
                .songs
                .iter()
                .map(|s| (true, s.id.clone(), if s.artist.is_empty() { s.title.clone() } else { format!("{} — {}", s.title, s.artist) }))
                .chain(hidden.artists.iter().map(|a| (false, a.clone(), a.clone())));
            for (n, (song, id, name)) in entries.enumerate() {
                list = list.child(
                    div()
                        .h(px(44.0))
                        .pl(px(36.0))
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded(px(8.0))
                        .child(div().font_family(ICONS).text_size(px(13.0)).text_color(rgba(TERTIARY)).child(if song { "\u{E8D6}" } else { "\u{E77B}" }))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(13.0)).child(name))
                        .child(div().flex_none().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(if song { t("Song") } else { t("Artist") }))
                        .child(pill_button(("stats-unhide", n), "\u{E890}", t("Show again"), false).on_click(cx.listener(move |this, _, _, cx| {
                            if song {
                                this.stats.hidden.show_song(&id);
                            } else {
                                this.stats.hidden.show_artist(&id);
                            }
                            this.stats.undo = None;
                            this.hidden_changed();
                            if this.stats.hidden.is_empty() {
                                this.stats.show_hidden = false;
                            }
                            cx.notify();
                        }))),
                );
            }
            card = card.child(list);
        }
        if has_history {
            let armed = self.stats.clearing;
            card = card.child(div().h(px(1.0)).mx(px(16.0)).bg(rgba(STROKE))).child(row(
                "\u{E74D}",
                "Clear listening history",
                "Start your stats and Wrapped over. Your playlists, likes and songs stay as they are.",
                pill_button("stats-clear", "\u{E74D}", if armed { t("Cancel") } else { t("Clear…") }, false)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stats.clearing = !this.stats.clearing;
                        this.stats.data_note = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            ));
            if armed {
                let (period, back) = (self.stats.period, self.stats.back);
                let this_one = summary.filter(|_| period != Period::All).map(|s| (s.span, s.label.clone(), clear_label(period, back, &s.label)));
                card = card.child(
                    div()
                        .mx(px(16.0))
                        .mb(px(16.0))
                        .p(px(16.0))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded(px(10.0))
                        .bg(rgba(0xc42b_1c1f))
                        .border_1()
                        .border_color(rgba(0xc42b_1c66))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(div().font_family(ICONS).text_size(px(18.0)).text_color(rgb(0xff99a4)).child("\u{E7BA}"))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).child(t("Clear your listening history for good?")))
                                        .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(t("This can't be undone. Hidden songs and artists stay hidden."))),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .when_some(this_one, |d, (span, label, button)| {
                                    d.child(danger_button("stats-clear-period", button).on_click(cx.listener(move |this, _, _, cx| this.clear_history(Some(span), label.clone(), cx))))
                                })
                                .child(danger_button("stats-clear-all", t("Clear everything").into()).on_click(cx.listener(|this, _, _, cx| this.clear_history(None, String::new(), cx))))
                                .child(pill_button("stats-clear-cancel", "\u{E711}", t("Cancel"), false).on_click(cx.listener(|this, _, _, cx| {
                                    this.stats.clearing = false;
                                    cx.notify();
                                }))),
                        ),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::heading(t("Manage your stats"), 20.0))
            .child(card)
            .when_some(self.stats.data_note.clone(), |d, note| d.child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(note)))
            .child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(t(
                "How it's counted: a song counts as played once you've heard 30 seconds of it (or half, for a short one). Minutes count everything you heard. It all stays on this PC.",
            )))
    }

    // ---- Wrapped -------------------------------------------------------------------

    /// What a Wrapped card shows, gathered once for whichever style draws it.
    fn card_data(&self, s: &Summary) -> CardData {
        let art = |id: &str| self.art.get(id).map(|a| a.small.clone());
        let top = s.top_songs.first();
        CardData {
            label: s.label.clone(),
            minutes: s.minutes,
            plays: s.plays,
            songs: s.songs,
            artists: s.artists,
            streak: s.longest_streak,
            hour: s.favourite_hour(),
            songs_top: s.top_songs.iter().take(5).cloned().collect(),
            artists_top: s.top_artists.iter().take(5).cloned().collect(),
            covers: s.top_songs.iter().take(5).map(|x| art(&x.video_id)).collect(),
            big: top.and_then(|x| self.art.get(&big_key(&x.video_id))).map(|a| a.small.clone()),
        }
    }

    fn wrapped_section(&mut self, s: &Summary, cx: &mut Context<Self>) -> gpui::Div {
        let style = self.stats.style;
        if style.big_cover() {
            if let Some(top) = s.top_songs.first() {
                if let (Some(url), false) = (top.thumbnail.clone(), local::is_local(&top.video_id)) {
                    self.fetch_art_at(big_key(&top.video_id), url, 544, 400, cx);
                }
            }
        }
        let data = self.card_data(s);
        let cell = self.stats.card.clone();
        let card = draw_card(style, &data)
            .shadow(vec![BoxShadow::new(px(0.0), px(16.0), gpui::hsla(0.0, 0.0, 0.0, 0.4)).blur_radius(px(36.0))])
            // Where it is, for the picture.
            .child(canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0());

        let mut grid = div().w(px(2.0 * 118.0 + 8.0)).flex().flex_wrap().gap(px(8.0));
        for (n, option) in WrappedStyle::ALL.into_iter().enumerate() {
            let on = option == style;
            grid = grid.child(
                div()
                    .id(("wrapped-style", n))
                    .w(px(118.0))
                    .p(px(3.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .rounded(px(11.0))
                    .cursor_pointer()
                    .border_2()
                    .border_color(if on { rgb(accent()) } else { rgba(0x0000_0000) })
                    .hover(|s| s.bg(rgba(SUBTLE)))
                    .child(swatch(option))
                    .child(
                        div()
                            .pb(px(3.0))
                            .text_center()
                            .text_size(px(12.0))
                            .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .text_color(if on { rgb(0xffffff) } else { rgba(SECONDARY) })
                            .child(t(option.name())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.stats.style = option;
                        this.stats.note = None;
                        settings::set("wrapped_style", option.key());
                        cx.notify();
                    })),
            );
        }
        let picker = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).child(t("Card style")))
            .child(grid)
            .child(
                div()
                    .w(px(244.0))
                    .text_size(px(12.0))
                    .text_color(rgba(TERTIARY))
                    .child(t("Something on the card you'd rather not share? Hover it in Top songs or Top artists and press Hide.")),
            );
        let save = div()
            .flex()
            .items_center()
            .gap_3()
            .child(pill_button("save-wrapped", "\u{E74E}", t("Save as picture"), true).on_click(cx.listener(|this, _, window, cx| this.save_wrapped(window, cx))))
            .when_some(self.stats.note.clone(), |d, note| d.child(div().flex_1().min_w_0().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(note)));
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(Self::heading(t("Your Wrapped"), 20.0))
                    .child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(t("Your stretch on one card. Pick a style, then save it as a picture to share."))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap_8()
                    .child(div().flex().flex_col().gap_4().child(card).child(save))
                    .child(picker),
            )
    }

    /// The Wrapped card, as it shows, to a PNG in Pictures. Out of view first is brought
    /// into view (and a side panel over it closed), then taken once it's on screen.
    fn save_wrapped(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bounds) = self.stats.card.get() else { return };
        let scale = window.scale_factor();
        let mut moved = false;
        // The lyrics or queue panel lies over the page's right side: it would be in the picture.
        let viewport = window.viewport_size();
        if self.panel != Panel::None && f32::from(bounds.origin.x + bounds.size.width) > f32::from(viewport.width) - PANEL - 8.0 {
            self.panel = Panel::None;
            moved = true;
        }
        let view = self.stats.scroll.bounds();
        let (top, bottom) = (f32::from(bounds.origin.y), f32::from(bounds.origin.y + bounds.size.height));
        let (view_top, view_bottom) = (f32::from(view.origin.y), f32::from(view.origin.y + view.size.height));
        if bottom - top > view_bottom - view_top {
            self.stats.note = Some(t("Make the window taller so the whole card fits, then save").into());
            cx.notify();
            return;
        }
        let offset = self.stats.scroll.offset();
        let mut y = f32::from(offset.y);
        if top < view_top {
            y += view_top + 16.0 - top;
        } else if bottom > view_bottom {
            y -= bottom - view_bottom + 16.0;
        }
        if y != f32::from(offset.y) {
            let lowest = -f32::from(self.stats.scroll.max_offset().y);
            self.stats.scroll.set_offset(gpui::point(offset.x, px(y.clamp(lowest.min(0.0), 0.0))));
            moved = true;
        }
        if !moved {
            self.capture_wrapped(scale, cx);
            return;
        }
        self.stats.note = Some(t("Saving…").into());
        cx.notify();
        // Once the page has been drawn where it now is.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let _ = this.update(cx, |app, cx| app.capture_wrapped(scale, cx));
        })
        .detach();
    }

    fn capture_wrapped(&mut self, scale: f32, cx: &mut Context<Self>) {
        let Some(bounds) = self.stats.card.get() else { return };
        let view = self.stats.scroll.bounds();
        let (top, bottom) = (f32::from(bounds.origin.y), f32::from(bounds.origin.y + bounds.size.height));
        if top < f32::from(view.origin.y) - 1.0 || bottom > f32::from(view.origin.y + view.size.height) + 1.0 {
            self.stats.note = Some(t("Scroll until the whole card shows, then save").into());
            cx.notify();
            return;
        }
        let stem = format!("NabyTune Wrapped {} - {}", self.stats.period_label(), t(self.stats.style.name()));
        let path = snapshot::unique_name(&snapshot::pictures_dir(), &stem);
        let (x, y) = ((f32::from(bounds.origin.x) * scale).round() as i32, (top * scale).round() as i32);
        let (w, h) = ((f32::from(bounds.size.width) * scale).round() as i32, ((bottom - top) * scale).round() as i32);
        self.stats.note = Some(match snapshot::save_window_region(self.hwnd, x, y, w, h, &path) {
            Ok(()) => tf("Saved to {}", &[&path.display()]).into(),
            Err(e) => tf("Couldn't save it: {}", &[&format!("{e:#}")]).into(),
        });
        cx.notify();
    }
}

impl StatsView {
    fn period_label(&self) -> String {
        stats::Period::label(self.period, self.back, unix_now(), stats::local_offset()).replace(['/', '\\', ':'], "-")
    }
}

// ---- The page's pieces ----------------------------------------------------------------

/// A top song, as a track to play.
fn song_track(s: &SongStat) -> Track {
    Track {
        video_id: s.video_id.clone(),
        title: s.title.clone().into(),
        artist: s.artist.clone().into(),
        detail: if s.album.is_empty() { s.artist.clone().into() } else { format!("{} • {}", s.artist, s.album).into() },
        duration: SharedString::default(),
        thumbnail: s.thumbnail.clone(),
        local: None,
    }
}

/// Where the top song's large cover is kept among the artwork.
fn big_key(video_id: &str) -> String {
    format!("{video_id}#wrapped")
}

/// A pill button: accent-filled for the main action, quiet for the rest.
fn pill_button(id: impl Into<gpui::ElementId>, glyph: &'static str, label: impl Into<SharedString>, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(34.0))
        .px(px(16.0))
        .flex()
        .items_center()
        .gap_2()
        .rounded_full()
        .cursor_pointer()
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .map(|d| {
            if primary {
                d.bg(rgb(accent())).text_color(rgb(on_accent())).hover(|s| s.opacity(0.9))
            } else {
                d.bg(rgba(SUBTLE_STRONG)).border_1().border_color(rgba(STROKE)).hover(|s| s.bg(rgba(0xffff_ff26)))
            }
        })
        .child(div().font_family(ICONS).text_size(px(12.0)).child(glyph))
        .child(label.into())
}

/// A red button that deletes for good.
fn danger_button(id: &'static str, label: SharedString) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(34.0))
        .px(px(16.0))
        .flex()
        .items_center()
        .gap_2()
        .rounded_full()
        .cursor_pointer()
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .bg(rgb(0xc42b1c))
        .text_color(rgb(0xffffff))
        .hover(|s| s.bg(rgb(0xd83b2c)))
        .child(div().font_family(ICONS).text_size(px(12.0)).child("\u{E74D}"))
        .child(label)
}

/// "Hide", shown while the row it's on is hovered.
fn hide_button(id: impl Into<gpui::ElementId>, group: SharedString) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded_full()
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgba(SECONDARY))
        .bg(rgba(SUBTLE_STRONG))
        .opacity(0.0)
        .group_hover(group, |s| s.opacity(1.0))
        .hover(|s| s.bg(rgba(0xffff_ff26)).text_color(rgb(0xffffff)))
        .child(div().font_family(ICONS).text_size(px(11.0)).child("\u{ED1A}"))
        .child(t("Hide"))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// A row's place in a top list.
fn rank(n: usize) -> gpui::Div {
    div()
        .w(px(22.0))
        .flex_none()
        .text_right()
        .text_size(px(14.0))
        .font_weight(FontWeight::BOLD)
        .text_color(if n == 0 { rgb(accent()) } else { rgba(TERTIARY) })
        .child(format!("{}", n + 1))
}

/// No history at all yet.
fn welcome() -> gpui::Div {
    div()
        .py(px(56.0))
        .px(px(24.0))
        .flex()
        .flex_col()
        .items_center()
        .gap_3()
        .rounded(px(16.0))
        .bg(rgba(SUBTLE))
        .border_1()
        .border_color(rgba(STROKE))
        .child(brand_mark(48.0))
        .child(div().font_family(DISPLAY).text_size(px(24.0)).font_weight(FontWeight::SEMIBOLD).child(t("Your stats start here")))
        .child(
            div()
                .max_w(px(520.0))
                .text_center()
                .text_size(px(14.0))
                .text_color(rgba(SECONDARY))
                .child(t("Play some music and NabyTune keeps count: your top songs and artists, when you listen, and a Wrapped card to share.")),
        )
        .child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(t("It all stays on this PC.")))
}

/// A count of minutes as people say it: "45 minutes", "2 hours 15 minutes", "12 hours".
fn friendly_time(minutes: u32) -> String {
    if minutes < 60 {
        return plural(minutes as usize, "{} minute", "{} minutes");
    }
    let (hours, rest) = ((minutes / 60) as usize, (minutes % 60) as usize);
    if hours < 10 && rest > 0 {
        format!("{} {}", plural(hours, "{} hour", "{} hours"), plural(rest, "{} minute", "{} minutes"))
    } else {
        plural(((minutes + 30) / 60) as usize, "{} hour", "{} hours")
    }
}

/// "You listened for 12 hours this month".
fn headline(s: &Summary, back: u32) -> String {
    let time = friendly_time(s.minutes);
    match (s.period, back) {
        (Period::All, _) => tf("You've listened for {} in all", &[&time]),
        (Period::Week, 0) => tf("You listened for {} this week", &[&time]),
        (Period::Week, 1) => tf("You listened for {} last week", &[&time]),
        (Period::Month, 0) => tf("You listened for {} this month", &[&time]),
        (Period::Year, 0) => tf("You listened for {} this year", &[&time]),
        _ => tf("You listened for {} in {}", &[&time, &s.label]),
    }
}

/// How the period compares with the one before, in words.
fn comparison(s: &Summary) -> Option<String> {
    let change = s.change?;
    let pct = format!("{:.0}", change.abs());
    let (same, more, less) = match s.period {
        Period::Week => ("About the same as the week before.", "{}% more than the week before.", "{}% less than the week before."),
        Period::Month => ("About the same as the month before.", "{}% more than the month before.", "{}% less than the month before."),
        Period::Year => ("About the same as the year before.", "{}% more than the year before.", "{}% less than the year before."),
        Period::All => return None,
    };
    Some(if change.abs() < 2.0 {
        t(same).to_string()
    } else if change > 0.0 {
        tf(more, &[&pct])
    } else {
        tf(less, &[&pct])
    })
}

/// "Clear this week", "Clear October 2026".
fn clear_label(period: Period, back: u32, label: &str) -> SharedString {
    match (period, back) {
        (Period::Week, 0) => t("Clear this week").into(),
        (Period::Week, 1) => t("Clear last week").into(),
        (Period::Month, 0) => t("Clear this month").into(),
        (Period::Year, 0) => t("Clear this year").into(),
        _ => tf("Clear {}", &[&label]).into(),
    }
}

/// An hour as a clock shows it, in the UI's language: "22:00", "22.00".
fn clock_hour(hour: usize) -> String {
    if i18n::language() == "id" { format!("{hour:02}.00") } else { format!("{hour:02}:00") }
}

/// A count with its thousands set apart, as the UI's language writes it: "1,234", "1.234".
fn grouped(n: u32) -> String {
    let separator = if i18n::language() == "id" { '.' } else { ',' };
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(separator);
        }
        out.push(c);
    }
    out
}

/// The big numbers: minutes, plays, artists and days in a row, each with a line on it.
fn numbers(s: &Summary) -> gpui::Div {
    let tile = |glyph: &'static str, label: &'static str, value: String, note: Option<String>| {
        div()
            .flex_1()
            .min_w(px(190.0))
            .p(px(18.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .rounded(px(14.0))
            .bg(rgba(SUBTLE))
            .border_1()
            .border_color(rgba(STROKE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().font_family(ICONS).text_size(px(14.0)).text_color(rgb(accent())).child(glyph))
                    .child(div().text_size(px(13.0)).text_color(rgba(SECONDARY)).child(t(label))),
            )
            .child(div().font_family(DISPLAY).text_size(px(32.0)).line_height(px(38.0)).font_weight(FontWeight::BOLD).child(value))
            .when_some(note, |d, n| d.child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(n)))
    };
    let busiest = s.busiest_day.as_ref().map(|b| tf("Most on {} ({} min)", &[&b.date.label(), &b.minutes]));
    div()
        .flex()
        .flex_wrap()
        .gap_4()
        .child(tile("\u{E823}", "Minutes listened", grouped(s.minutes), busiest))
        .child(tile("\u{E8D6}", "Songs played", grouped(s.plays), Some(tf("{} different", &[&s.songs]))))
        .child(tile("\u{E77B}", "Artists", grouped(s.artists), (s.new_artists > 0 && s.period != Period::All).then(|| tf("{} new to you", &[&s.new_artists]))))
        .child(tile("\u{E787}", "Days in a row", s.current_streak.to_string(), Some(plural(s.longest_streak as usize, "Longest: {} day", "Longest: {} days"))))
}

/// When in the day and the week the music plays: a sentence, then bars.
fn when_you_listen(s: &Summary) -> gpui::Div {
    let bars = |values: &[f32], labels: Vec<Option<String>>, height: f32| {
        let most = values.iter().cloned().fold(0.0f32, f32::max).max(1.0);
        let mut row = div().flex().items_end().gap(px(4.0)).h(px(height + 18.0));
        for (v, label) in values.iter().zip(labels) {
            row = row.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_end()
                    .gap(px(4.0))
                    .h_full()
                    .child(div().w_full().h(px((v / most * height).max(2.0))).rounded_t(px(3.0)).bg(if *v >= most { rgb(accent()) } else { rgba(accent_wash(0x66)) }))
                    .child(div().h(px(12.0)).text_size(px(10.0)).text_color(rgba(TERTIARY)).whitespace_nowrap().children(label)),
            );
        }
        row
    };
    let hours: Vec<Option<String>> = (0..24).map(|h| (h % 6 == 0).then(|| clock_hour(h))).collect();
    let days: Vec<Option<String>> = (0..7).map(|d| Some(stats::weekday_name(d).chars().take(3).collect())).collect();
    let mut words = Vec::new();
    if let Some(h) = s.favourite_hour() {
        let who = match h {
            5..=11 => t("You're a morning listener"),
            12..=16 => t("You're an afternoon listener"),
            17..=21 => t("You're an evening listener"),
            _ => t("You're a night owl"),
        };
        words.push(tf("{}: most of your music plays around {}.", &[&who, &clock_hour(h)]));
    }
    if let Some(d) = s.favourite_weekday() {
        words.push(tf("{} is your biggest day of the week.", &[&stats::weekday_name(d)]));
    }
    let panel = |label: &'static str, chart: gpui::Div| {
        div()
            .p(px(18.0))
            .flex()
            .flex_col()
            .gap_3()
            .rounded(px(14.0))
            .bg(rgba(SUBTLE))
            .border_1()
            .border_color(rgba(STROKE))
            .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(t(label)))
            .child(chart)
    };
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(NabyTune::heading(t("When you listen"), 20.0))
        .child(div().text_size(px(14.0)).text_color(rgba(SECONDARY)).child(words.join(" ")))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_4()
                .child(div().flex_1().min_w(px(360.0)).child(panel("By hour", bars(&s.by_hour, hours, 90.0))))
                .child(div().w(px(300.0)).flex_none().child(panel("By day", bars(&s.by_weekday, days, 90.0)))),
        )
}

// ---- The Wrapped cards ----------------------------------------------------------------

/// What a Wrapped card shows.
struct CardData {
    label: String,
    minutes: u32,
    plays: u32,
    songs: u32,
    artists: u32,
    /// The longest run of days in a row.
    streak: u32,
    hour: Option<usize>,
    /// Up to five each.
    songs_top: Vec<SongStat>,
    artists_top: Vec<ArtistStat>,
    /// The top songs' covers, row-sized, in order.
    covers: Vec<Option<Arc<RenderImage>>>,
    /// The top song's cover, large, once fetched.
    big: Option<Arc<RenderImage>>,
}

impl CardData {
    fn hour_text(&self) -> String {
        self.hour.map(clock_hour).unwrap_or_else(|| "—".into())
    }

    fn cover(&self, i: usize) -> Option<Arc<RenderImage>> {
        self.covers.get(i).cloned().flatten()
    }

    /// The top song's cover, as large as there is.
    fn big_cover(&self) -> Option<Arc<RenderImage>> {
        self.big.clone().or_else(|| self.cover(0))
    }

    /// Something to seed the barcodes with, so each card's is its own.
    fn seed(&self) -> u64 {
        (self.minutes as u64) << 20 ^ (self.plays as u64) << 8 ^ self.songs as u64 ^ 0x9e37_79b9
    }
}

fn draw_card(style: WrappedStyle, d: &CardData) -> gpui::Div {
    match style {
        WrappedStyle::Signature => signature(d),
        WrappedStyle::Poster => poster(d),
        WrappedStyle::Minimal => minimal(d),
        WrappedStyle::Receipt => receipt(d),
        WrappedStyle::Neon => neon(d),
        WrappedStyle::Aurora => aurora(d),
        WrappedStyle::Cassette => cassette(d),
        WrappedStyle::Polaroid => polaroid(d),
        WrappedStyle::Numbers => big_numbers(d),
        WrappedStyle::Magazine => magazine(d),
    }
}

/// The card's frame: its size, square corners (clean in the picture), nothing spilling out.
fn base() -> gpui::Div {
    div().relative().w(px(CARD_W)).h(px(CARD_H)).flex_none().overflow_hidden()
}

/// One line of text at `size`, with room for its descenders.
fn text_line(content: impl Into<SharedString>, size: f32) -> gpui::Div {
    div().text_size(px(size)).line_height(px((size * 1.25).round())).child(content.into())
}

/// The font size that fits `text` across `width`, at about `em` of the size per character.
fn fit(text: &str, width: f32, em: f32, max: f32) -> f32 {
    (width / (text.chars().count().max(1) as f32 * em)).min(max)
}

/// A colour (0xRRGGBB) at `alpha`, as a shadow takes it.
fn shadow_colour(colour: u32, alpha: f32) -> gpui::Hsla {
    let channel = |shift: u32| ((colour >> shift) & 0xff) as f32 / 255.0;
    let (r, g, b) = (channel(16), channel(8), channel(0));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let s = if max == min { 0.0 } else { (max - min) / (1.0 - (2.0 * l - 1.0).abs()) };
    gpui::hsla(crate::appearance::hue_of(colour), s, l, alpha)
}

/// A soft, blurred blob of colour.
fn blob(colour: u32, x: f32, y: f32, size: f32, blur: f32) -> gpui::Div {
    div().absolute().left(px(x)).top(px(y)).size(px(size)).rounded_full().bg(rgba(colour)).blur(px(blur))
}

/// Text that glows: a blurred copy of it under it.
fn glow_text(content: impl Into<SharedString>, size: f32, colour: u32, weight: FontWeight) -> gpui::Div {
    let content = content.into();
    let copy = |d: gpui::Div| d.text_size(px(size)).line_height(px((size * 1.15).round())).font_weight(weight).text_color(rgb(colour)).whitespace_nowrap();
    div()
        .relative()
        .child(copy(div().absolute().top_0().left_0()).blur(px((size * 0.2).max(4.0))).child(content.clone()))
        .child(copy(div()).child(content))
}

/// Bars of a made-up barcode, the same for the same seed.
fn barcode(seed: u64, colour: u32, height: f32, bars: usize) -> gpui::Div {
    let mut x = seed | 1;
    let mut row = div().flex().h(px(height));
    for _ in 0..bars {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let width = 1.0 + ((x >> 33) % 3) as f32;
        let gap = 1.0 + ((x >> 41) % 3) as f32;
        row = row.child(div().w(px(width)).h_full().mr(px(gap)).bg(rgb(colour)));
    }
    row
}

/// "NabyTune Wrapped", as the app writes its name.
fn wordmark(size: f32, wrapped: u32) -> gpui::Div {
    div()
        .flex()
        .items_baseline()
        .text_size(px(size))
        .child(div().font_family(DISPLAY).font_weight(FontWeight::SEMIBOLD).child("Naby"))
        .child(div().font_family(SERIF).italic().text_size(px(size + 1.0)).child("Tune"))
        .child(div().pl(px(size * 0.5)).font_family(DISPLAY).font_weight(FontWeight::SEMIBOLD).text_color(rgb(wrapped)).child("Wrapped"))
}

/// 1. Dark, lit by the accent.
fn signature(d: &CardData) -> gpui::Div {
    let colour = accent();
    let caption = |s: &'static str| text_line(t(s), 11.0).font_weight(FontWeight::SEMIBOLD).letter_spacing(px(0.6)).text_color(rgba(0xffff_ff8c));
    let ranked = |items: Vec<String>| {
        let mut list = div().flex().flex_col().gap(px(5.0));
        for (n, item) in items.into_iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .gap_2()
                    .text_size(px(13.0))
                    .child(div().w(px(14.0)).flex_none().font_weight(FontWeight::BOLD).text_color(rgb(colour)).child(format!("{}", n + 1)))
                    .child(div().flex_1().min_w_0().truncate().child(item)),
            );
        }
        list
    };
    let stat = |label: &'static str, value: String| div().flex().flex_col().gap(px(2.0)).child(caption(label)).child(text_line(value, 22.0).font_family(DISPLAY).font_weight(FontWeight::BOLD));
    base()
        .p(px(28.0))
        .flex()
        .flex_col()
        .gap(px(20.0))
        .text_color(rgb(0xffffff))
        .bg(linear_gradient(155.0, linear_color_stop(rgb(0x1a1a1a), 0.0), linear_color_stop(rgb(0x080808), 1.0)))
        .child(blob(accent_wash(0x48), 260.0, -150.0, 340.0, 80.0))
        .child(blob(accent_wash(0x22), -150.0, 430.0, 320.0, 90.0))
        .child(div().flex().items_center().gap(px(10.0)).child(brand_mark(18.0)).child(wordmark(16.0, colour)).child(div().flex_1()).child(text_line(d.label.clone(), 13.0).text_color(rgba(0xffff_ffb3))))
        .child(
            div()
                .flex()
                .items_end()
                .gap_3()
                .child(text_line(grouped(d.minutes), fit(&grouped(d.minutes), 260.0, 0.56, 76.0)).line_height(px(76.0)).font_family(DISPLAY).font_weight(FontWeight::BOLD).text_color(rgb(colour)))
                .child(text_line(t("minutes of music"), 16.0).pb(px(8.0)).text_color(rgba(0xffff_ffcc))),
        )
        .when_some(d.songs_top.first(), |c, s| {
            c.child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(artwork(d.cover(0), 76.0, 10.0).shadow_lg())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(caption("YOUR SONG"))
                            .child(text_line(s.title.clone(), 20.0).font_family(DISPLAY).font_weight(FontWeight::BOLD).truncate())
                            .child(text_line(format!("{} · {}", s.artist, plural(s.plays as usize, "{} play", "{} plays")), 13.0).text_color(rgba(0xffff_ffb3)).truncate()),
                    ),
            )
        })
        .child(
            div()
                .flex()
                .gap_6()
                .child(div().flex_1().min_w_0().flex().flex_col().gap_2().child(caption("TOP SONGS")).child(ranked(d.songs_top.iter().map(|s| s.title.clone()).collect())))
                .child(div().w(px(160.0)).flex().flex_col().gap_2().child(caption("TOP ARTISTS")).child(ranked(d.artists_top.iter().map(|a| a.name.clone()).collect()))),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .justify_between()
                .child(stat("SONGS", grouped(d.songs)))
                .child(stat("ARTISTS", grouped(d.artists)))
                .child(stat("BEST HOUR", d.hour_text()))
                .child(stat("DAY STREAK", d.streak.to_string())),
        )
}

/// 2. A bold poster: one loud colour and huge condensed type.
fn poster(d: &CardData) -> gpui::Div {
    const RED: u32 = 0xff4a1c;
    const INK: u32 = 0x150d0a;
    const CREAM: u32 = 0xfff1de;
    let heavy = |content: String, size: f32, colour: u32| {
        div().font_family(IMPACT).text_size(px(size)).line_height(px((size * 1.04).round())).text_color(rgb(colour)).whitespace_nowrap().child(content)
    };
    let number = grouped(d.minutes);
    let mut artists = div().flex().flex_col();
    for (n, a) in d.artists_top.iter().enumerate() {
        artists = artists.child(
            div()
                .flex()
                .items_baseline()
                .gap_3()
                .child(heavy(format!("{:02}", n + 1), 18.0, CREAM).w(px(28.0)).flex_none())
                .child(heavy(a.name.to_uppercase(), 30.0, INK).flex_1().min_w_0().truncate()),
        );
    }
    base()
        .bg(rgb(RED))
        .p(px(28.0))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .gap_4()
                .child(heavy("NABYTUNE WRAPPED".into(), 18.0, INK).flex_none())
                .child(heavy(d.label.to_uppercase(), 18.0, CREAM).flex_1().min_w_0().text_right().truncate()),
        )
        .child(heavy(number.clone(), fit(&number, 424.0, 0.5, 150.0), INK).mt(px(4.0)))
        .child(heavy(t("minutes of music").to_uppercase(), 30.0, CREAM).truncate())
        .child(div().h(px(8.0)).bg(rgb(INK)).my(px(14.0)))
        .when_some(d.songs_top.first(), |c, s| {
            c.child(heavy("#1".into(), 18.0, CREAM))
                .child(heavy(s.title.to_uppercase(), 40.0, INK).truncate())
                .child(div().text_size(px(16.0)).font_weight(FontWeight::BOLD).text_color(rgb(INK)).truncate().child(s.artist.clone()))
        })
        .child(div().flex_1())
        .child(artists)
}

/// 3. Minimal: black on white, light type, thin rules.
fn minimal(d: &CardData) -> gpui::Div {
    const PAPER: u32 = 0xf5f4f0;
    const INK: u32 = 0x161616;
    const SOFT: u32 = 0x8b8a85;
    const RULE: u32 = 0xdcdad3;
    let caption = |s: &'static str| text_line(t(s), 10.0).font_family(MONO).letter_spacing(px(1.5)).text_color(rgb(SOFT));
    let mut list = div().flex().flex_col();
    for (n, s) in d.songs_top.iter().enumerate() {
        list = list.child(
            div()
                .h(px(32.0))
                .flex()
                .items_center()
                .gap_3()
                .border_b_1()
                .border_color(rgb(RULE))
                .child(text_line(format!("{:02}", n + 1), 12.0).w(px(22.0)).flex_none().font_family(MONO).text_color(rgb(SOFT)))
                .child(text_line(s.title.clone(), 14.0).flex_1().min_w_0().truncate())
                .child(text_line(s.artist.clone(), 12.0).max_w(px(150.0)).truncate().text_color(rgb(SOFT))),
        );
    }
    let stat = |label: &'static str, value: String| div().flex_1().flex().flex_col().gap(px(4.0)).child(caption(label)).child(text_line(value, 20.0).font_family(MONO));
    let number = grouped(d.minutes);
    base()
        .bg(rgb(PAPER))
        .text_color(rgb(INK))
        .font_family(DISPLAY)
        .p(px(40.0))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .justify_between()
                .items_baseline()
                .child(text_line("Wrapped", 14.0).font_weight(FontWeight::SEMIBOLD))
                .child(text_line(d.label.clone(), 13.0).text_color(rgb(SOFT))),
        )
        .child(div().h(px(44.0)))
        .child(text_line(number.clone(), fit(&number, 400.0, 0.52, 96.0)).line_height(px(100.0)).font_weight(FontWeight::LIGHT).letter_spacing(px(-3.0)))
        .child(text_line(t("minutes of music"), 15.0).text_color(rgb(SOFT)))
        .child(div().flex_1())
        .child(caption("TOP SONGS").pb(px(6.0)).border_b_1().border_color(rgb(INK)))
        .child(list)
        .child(div().h(px(24.0)))
        .child(div().flex().gap_4().child(stat("SONGS", grouped(d.songs))).child(stat("ARTISTS", grouped(d.artists))).child(stat("BEST HOUR", d.hour_text())))
}

/// 4. A till receipt, printed in typewriter type, coming out of the printer.
fn receipt(d: &CardData) -> gpui::Div {
    const PAPER: u32 = 0xfbf7ee;
    const INK: u32 = 0x2a2620;
    const FADED: u32 = 0x857e72;
    let typed = |content: String, size: f32| text_line(content, size).font_family(TYPEWRITER).font_weight(FontWeight::BOLD).whitespace_nowrap();
    let rule = || div().w_full().h(px(0.0)).border_t_2().border_dashed().border_color(rgb(FADED)).my(px(8.0));
    let item = |left: String, right: String| {
        div().w_full().flex().gap_2().child(typed(left, 13.0).flex_1().min_w_0().truncate()).child(typed(right, 13.0).flex_none())
    };
    let mut paper = div()
        .w(px(352.0))
        .flex_1()
        .bg(rgb(PAPER))
        .text_color(rgb(INK))
        .px(px(22.0))
        .pt(px(26.0))
        .flex()
        .flex_col()
        .items_center()
        .shadow(vec![BoxShadow::new(px(0.0), px(8.0), gpui::hsla(0.0, 0.0, 0.0, 0.4)).blur_radius(px(24.0))])
        .child(typed("NABYTUNE".into(), 26.0).letter_spacing(px(4.0)))
        .child(typed("* WRAPPED *".into(), 13.0).letter_spacing(px(2.0)))
        .child(typed(d.label.to_uppercase(), 12.0).text_color(rgb(FADED)))
        .child(rule())
        .child(item(t("TOP SONGS").into(), "×".into()));
    for (n, s) in d.songs_top.iter().enumerate() {
        paper = paper.child(item(format!("{} {}", n + 1, s.title.to_uppercase()), s.plays.to_string()));
    }
    paper = paper.child(rule()).child(item(t("TOP ARTISTS").into(), String::new()));
    for (n, a) in d.artists_top.iter().take(3).enumerate() {
        paper = paper.child(item(format!("{} {}", n + 1, a.name.to_uppercase()), tf("{} min", &[&a.minutes])));
    }
    paper = paper
        .child(rule())
        .child(item(t("SONGS").into(), grouped(d.songs)))
        .child(item(t("ARTISTS").into(), grouped(d.artists)))
        .child(item(t("BEST HOUR").into(), d.hour_text()))
        .child(item(t("DAY STREAK").into(), d.streak.to_string()))
        .child(rule())
        .child(div().w_full().flex().justify_between().child(typed(t("TOTAL").into(), 18.0)).child(typed(tf("{} MIN", &[&grouped(d.minutes)]), 18.0)))
        .child(rule())
        .child(typed(t("THANK YOU FOR LISTENING").into(), 12.0).mt(px(2.0)))
        .child(barcode(d.seed(), INK, 40.0, 46).mt(px(12.0)))
        .child(typed("#NabyTuneWrapped".into(), 11.0).text_color(rgb(FADED)).mt(px(4.0)));
    base()
        .bg(linear_gradient(180.0, linear_color_stop(rgb(0x34434b), 0.0), linear_color_stop(rgb(0x161d21), 1.0)))
        .flex()
        .flex_col()
        .items_center()
        .pt(px(18.0))
        .child(paper)
        // The printer's slot it comes out of.
        .child(div().absolute().top(px(10.0)).left(px((CARD_W - 392.0) / 2.0)).w(px(392.0)).h(px(16.0)).rounded_full().bg(rgb(0x0b0f11)))
}

/// 5. Neon signs on a night-purple ground, over a glowing floor.
fn neon(d: &CardData) -> gpui::Div {
    const PINK: u32 = 0xff3ec8;
    const CYAN: u32 = 0x3ff0ff;
    const WHITE: u32 = 0xf3e9ff;
    let mut floor = div().absolute().left_0().right_0().bottom_0().h(px(160.0)).flex().flex_col().justify_end();
    for i in 0..7 {
        floor = floor.child(div().h(px(1.0)).mb(px(4.0 + i as f32 * 3.0)).bg(rgba((PINK << 8) | 0x55)));
    }
    let list = |title: &'static str, items: Vec<String>| {
        let mut col = div().flex_1().min_w_0().flex().flex_col().gap(px(4.0)).child(text_line(t(title), 11.0).letter_spacing(px(2.0)).text_color(rgb(CYAN)));
        for (n, item) in items.into_iter().enumerate() {
            col = col.child(
                div()
                    .flex()
                    .gap_2()
                    .text_size(px(13.0))
                    .child(div().w(px(12.0)).flex_none().text_color(rgb(PINK)).font_weight(FontWeight::BOLD).child(format!("{}", n + 1)))
                    .child(div().flex_1().min_w_0().truncate().child(item)),
            );
        }
        col
    };
    base()
        .bg(linear_gradient(180.0, linear_color_stop(rgb(0x170b30), 0.0), linear_color_stop(rgb(0x05030b), 1.0)))
        .font_family(ROUNDED)
        .text_color(rgb(WHITE))
        .child(blob((PINK << 8) | 0x40, -120.0, -120.0, 320.0, 90.0))
        .child(blob((CYAN << 8) | 0x2a, 260.0, 300.0, 320.0, 90.0))
        .child(floor)
        .child(
            div()
                .absolute()
                .inset_0()
                .px(px(30.0))
                .py(px(28.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.0))
                .child(glow_text("WRAPPED", 46.0, PINK, FontWeight::BOLD).letter_spacing(px(6.0)))
                .child(text_line(d.label.to_uppercase(), 13.0).letter_spacing(px(3.0)).text_color(rgb(CYAN)))
                .child(glow_text(grouped(d.minutes), fit(&grouped(d.minutes), 400.0, 0.56, 84.0), CYAN, FontWeight::BOLD).mt(px(4.0)))
                .child(text_line(t("minutes of music").to_uppercase(), 12.0).letter_spacing(px(4.0)).text_color(rgba((WHITE << 8) | 0xb3)))
                .when_some(d.songs_top.first(), |c, s| {
                    c.child(
                        div()
                            .w(px(380.0))
                            .mt(px(6.0))
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .items_center()
                            .gap(px(14.0))
                            .rounded(px(14.0))
                            .border_2()
                            .border_color(rgb(PINK))
                            .bg(rgba(0x0000_0055))
                            .shadow(vec![BoxShadow::new(px(0.0), px(0.0), shadow_colour(PINK, 0.67)).blur_radius(px(18.0))])
                            .child(artwork(d.cover(0), 54.0, 8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(text_line("#1", 11.0).letter_spacing(px(2.0)).text_color(rgb(PINK)))
                                    .child(text_line(s.title.clone(), 18.0).font_weight(FontWeight::SEMIBOLD).truncate())
                                    .child(text_line(s.artist.clone(), 13.0).text_color(rgb(CYAN)).truncate()),
                            ),
                    )
                })
                .child(
                    div()
                        .w_full()
                        .mt(px(6.0))
                        .flex()
                        .gap_6()
                        .child(list("TOP SONGS", d.songs_top.iter().map(|s| s.title.clone()).collect()))
                        .child(list("TOP ARTISTS", d.artists_top.iter().map(|a| a.name.clone()).collect())),
                ),
        )
}

/// 6. Northern lights behind frosted glass.
fn aurora(d: &CardData) -> gpui::Div {
    let glass = |el: gpui::Div| el.bg(rgba(0xffff_ff14)).border_1().border_color(rgba(0xffff_ff2e)).rounded(px(18.0));
    let caption = |s: &'static str| text_line(t(s), 11.0).font_weight(FontWeight::SEMIBOLD).letter_spacing(px(1.0)).text_color(rgba(0xffff_ffaa));
    let pill = |label: &'static str, value: String| {
        glass(div()).flex_1().py(px(10.0)).flex().flex_col().items_center().child(text_line(value, 22.0).font_weight(FontWeight::BOLD)).child(caption(label))
    };
    let mut names = div().flex().flex_wrap().gap_2();
    for a in d.artists_top.iter().take(4) {
        names = names.child(div().h(px(28.0)).px(px(12.0)).flex().items_center().rounded_full().bg(rgba(0xffff_ff1a)).text_size(px(13.0)).max_w(px(190.0)).child(div().truncate().child(a.name.clone())));
    }
    let number = grouped(d.minutes);
    base()
        .bg(rgb(0x070b1c))
        .font_family(DISPLAY)
        .text_color(rgb(0xffffff))
        .child(blob(0x19d3_b5aa, -90.0, -70.0, 360.0, 90.0))
        .child(blob(0x7a4d_ffaa, 230.0, 10.0, 380.0, 100.0))
        .child(blob(0x5cff_8a66, 40.0, 360.0, 300.0, 90.0))
        .child(blob(0xff5f_a866, 280.0, 400.0, 320.0, 100.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .p(px(30.0))
                .flex()
                .flex_col()
                .items_center()
                .child(text_line("NabyTune Wrapped", 13.0).font_weight(FontWeight::SEMIBOLD).letter_spacing(px(1.0)).text_color(rgba(0xffff_ffcc)))
                .child(text_line(d.label.clone(), 15.0).text_color(rgba(0xffff_ffb3)))
                .child(text_line(number.clone(), fit(&number, 400.0, 0.55, 92.0)).mt(px(26.0)).font_weight(FontWeight::BOLD).letter_spacing(px(-3.0)))
                .child(text_line(t("minutes of music"), 17.0).text_color(rgba(0xffff_ffcc)))
                .child(div().flex_1())
                .child(
                    glass(div())
                        .w_full()
                        .p(px(18.0))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .when_some(d.songs_top.first(), |c, s| {
                            c.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_4()
                                    .child(artwork(d.cover(0), 64.0, 12.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(caption("YOUR SONG"))
                                            .child(text_line(s.title.clone(), 19.0).font_weight(FontWeight::SEMIBOLD).truncate())
                                            .child(text_line(s.artist.clone(), 13.0).text_color(rgba(0xffff_ffb3)).truncate()),
                                    ),
                            )
                            .child(div().h(px(1.0)).bg(rgba(0xffff_ff26)))
                        })
                        .child(caption("TOP ARTISTS"))
                        .child(names),
                )
                .child(div().w_full().mt(px(12.0)).flex().gap_3().child(pill("SONGS", grouped(d.songs))).child(pill("ARTISTS", grouped(d.artists))).child(pill("BEST HOUR", d.hour_text()))),
        )
}

/// 7. A 70s mixtape: the label in handwriting, side A the songs, side B the artists.
fn cassette(d: &CardData) -> gpui::Div {
    const CREAM: u32 = 0xf1e5c9;
    const INK: u32 = 0x2e2015;
    const BODY: u32 = 0x2b2521;
    const LABEL: u32 = 0xf8f1df;
    const RUST: u32 = 0xb84a1c;
    let reel = || {
        div()
            .size(px(52.0))
            .flex_none()
            .rounded_full()
            .bg(rgb(LABEL))
            .border_4()
            .border_color(rgb(0x4a3f38))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(18.0)).rounded_full().bg(rgb(BODY)).border_2().border_color(rgb(0x8a7f74)))
    };
    let window = div()
        .w(px(270.0))
        .h(px(66.0))
        .rounded(px(33.0))
        .bg(rgb(0x1a1512))
        .px(px(8.0))
        .flex()
        .items_center()
        .child(reel())
        .child(div().flex_1().h(px(30.0)).bg(rgb(0x5b3a26)))
        .child(reel());
    let stripe = |colour: u32| div().h(px(6.0)).bg(rgb(colour));
    let label = div()
        .w_full()
        .h(px(178.0))
        .rounded(px(10.0))
        .bg(rgb(LABEL))
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .text_color(rgb(INK))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().size(px(26.0)).flex_none().rounded(px(4.0)).bg(rgb(INK)).text_color(rgb(LABEL)).flex().items_center().justify_center().font_family(MONO).font_weight(FontWeight::BOLD).text_size(px(15.0)).child("A"))
                .child(text_line(d.label.clone(), 22.0).font_family(HAND).font_weight(FontWeight::BOLD).flex_1().min_w_0().truncate())
                .child(text_line(format!("C-{}", d.minutes), 13.0).flex_none().font_family(MONO).font_weight(FontWeight::BOLD)),
        )
        .child(div().flex().flex_col().gap(px(3.0)).child(stripe(0xe8622c)).child(stripe(0xf3a53a)).child(stripe(0xf6d36b)))
        .child(div().flex().justify_center().child(window));
    let screw = |left: Option<f32>, right: Option<f32>, top: Option<f32>, bottom: Option<f32>| {
        let mut s = div().absolute().size(px(8.0)).rounded_full().bg(rgb(0x6b625a));
        if let Some(v) = left {
            s = s.left(px(v));
        }
        if let Some(v) = right {
            s = s.right(px(v));
        }
        if let Some(v) = top {
            s = s.top(px(v));
        }
        if let Some(v) = bottom {
            s = s.bottom(px(v));
        }
        s
    };
    let hole = || div().size(px(12.0)).rounded_full().bg(rgb(0x14100e));
    let body = div()
        .relative()
        .w(px(424.0))
        .h(px(270.0))
        .flex_none()
        .rounded(px(18.0))
        .bg(rgb(BODY))
        .p(px(14.0))
        .shadow(vec![BoxShadow::new(px(0.0), px(10.0), gpui::hsla(0.0, 0.0, 0.0, 0.33)).blur_radius(px(20.0))])
        .child(label)
        .child(div().absolute().bottom_0().left(px(82.0)).w(px(260.0)).h(px(52.0)).rounded_t(px(12.0)).bg(rgb(0x3a332e)).flex().items_center().justify_center().gap(px(80.0)).child(hole()).child(hole()))
        .child(screw(Some(6.0), None, Some(6.0), None))
        .child(screw(None, Some(6.0), Some(6.0), None))
        .child(screw(Some(6.0), None, None, Some(6.0)))
        .child(screw(None, Some(6.0), None, Some(6.0)));
    let side = |title: &'static str, items: Vec<String>| {
        let mut col = div().flex_1().min_w_0().flex().flex_col().gap(px(3.0)).child(text_line(t(title), 11.0).font_family(MONO).font_weight(FontWeight::BOLD).letter_spacing(px(1.0)).text_color(rgb(RUST)));
        for (n, item) in items.into_iter().enumerate() {
            col = col.child(text_line(format!("{}. {}", n + 1, item), 12.0).font_family(MONO).truncate());
        }
        col
    };
    base()
        .bg(rgb(CREAM))
        .text_color(rgb(INK))
        .p(px(28.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(16.0))
        .child(
            div()
                .w_full()
                .flex()
                .justify_between()
                .child(text_line("NABYTUNE WRAPPED", 12.0).font_family(MONO).font_weight(FontWeight::BOLD).letter_spacing(px(2.0)))
                .child(text_line("STEREO", 12.0).font_family(MONO).font_weight(FontWeight::BOLD).letter_spacing(px(2.0)).text_color(rgb(RUST))),
        )
        .child(body)
        .child(
            div()
                .w_full()
                .flex()
                .gap_5()
                .child(side("SIDE A · TOP SONGS", d.songs_top.iter().map(|s| s.title.clone()).collect()))
                .child(side("SIDE B · TOP ARTISTS", d.artists_top.iter().map(|a| a.name.clone()).collect())),
        )
        .child(div().flex_1())
        .child(
            text_line(format!("{} · {} · {}", tf("{} min", &[&grouped(d.minutes)]), i18n::songs(d.songs as usize), plural(d.artists as usize, "{} artist", "{} artists")), 11.0)
                .w_full()
                .font_family(MONO)
                .text_color(rgb(0x7a6a58)),
        )
}

/// 8. The top song's cover as an instant photo, a handwritten caption, the next ones small.
fn polaroid(d: &CardData) -> gpui::Div {
    const FRAME: u32 = 0xfcfbf7;
    const INK: u32 = 0x2b2b2b;
    const PHOTO: f32 = 300.0;
    let shadow = || vec![BoxShadow::new(px(0.0), px(10.0), gpui::hsla(0.0, 0.0, 0.0, 0.3)).blur_radius(px(22.0))];
    let photo = match d.big_cover() {
        Some(image) => div().size(px(PHOTO)).flex_none().child(img(image).object_fit(ObjectFit::Cover).size_full()),
        None => div().size(px(PHOTO)).flex_none().bg(rgb(0x2a2a2a)).flex().items_center().justify_center().child(brand_mark(64.0)),
    };
    let number = grouped(d.minutes);
    let frame = div()
        .relative()
        .w(px(PHOTO + 28.0))
        .p(px(14.0))
        .pb(px(0.0))
        .bg(rgb(FRAME))
        .flex()
        .flex_col()
        .shadow(shadow())
        .child(photo)
        .child(
            div().h(px(66.0)).flex().flex_col().items_center().justify_center().text_color(rgb(INK)).when_some(d.songs_top.first(), |c, s| {
                c.child(text_line(s.title.clone(), 24.0).font_family(HAND).font_weight(FontWeight::BOLD).max_w(px(PHOTO)).truncate())
                    .child(text_line(s.artist.clone(), 15.0).font_family(HAND).text_color(rgb(0x5a5a5a)).max_w(px(PHOTO)).truncate())
            }),
        )
        // A sticker with the minutes on the corner.
        .child(
            div()
                .absolute()
                .top(px(-16.0))
                .right(px(-28.0))
                .size(px(96.0))
                .rounded_full()
                .bg(rgb(0xffd23f))
                .border_2()
                .border_color(rgb(0xf2b705))
                .shadow(vec![BoxShadow::new(px(0.0), px(4.0), gpui::hsla(0.0, 0.0, 0.0, 0.25)).blur_radius(px(10.0))])
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .text_color(rgb(0x231a00))
                .child(text_line(number.clone(), fit(&number, 76.0, 0.55, 24.0)).font_family(DISPLAY).font_weight(FontWeight::BOLD))
                .child(text_line(t("minutes"), 11.0).font_weight(FontWeight::SEMIBOLD)),
        );
    let mut minis = div().flex().gap(px(14.0)).mt(px(12.0));
    for i in 1..d.songs_top.len().min(5) {
        minis = minis.child(
            div()
                .w(px(76.0))
                .p(px(5.0))
                .pb(px(0.0))
                .bg(rgb(FRAME))
                .flex()
                .flex_col()
                .items_center()
                .shadow(shadow())
                .child(artwork(d.cover(i), 66.0, 0.0))
                .child(text_line(format!("#{}", i + 1), 13.0).h(px(20.0)).font_family(HAND).font_weight(FontWeight::BOLD).text_color(rgb(INK))),
        );
    }
    base()
        .bg(linear_gradient(160.0, linear_color_stop(rgb(0xd8ccb6), 0.0), linear_color_stop(rgb(0xb9a688), 1.0)))
        .flex()
        .flex_col()
        .items_center()
        .pt(px(30.0))
        .child(frame)
        .child(text_line(format!("{} · {}", t("My No. 1"), d.label), 20.0).mt(px(14.0)).max_w(px(420.0)).truncate().font_family(HAND).font_weight(FontWeight::BOLD).text_color(rgb(0x3a2f1f)))
        .child(minis)
}

/// 9. Big numbers on colour blocks.
fn big_numbers(d: &CardData) -> gpui::Div {
    let tile = |bg: u32, ink: u32, label: &'static str, value: String, width: f32, size: f32| {
        let size = fit(&value, width, 0.55, size);
        div()
            .rounded(px(18.0))
            .bg(rgb(bg))
            .text_color(rgb(ink))
            .p(px(16.0))
            .flex()
            .flex_col()
            .justify_between()
            .child(text_line(t(label), 12.0).font_weight(FontWeight::SEMIBOLD))
            .child(text_line(value, size).line_height(px(size)).font_family(ROUNDED).font_weight(FontWeight::BOLD).whitespace_nowrap())
    };
    base()
        .bg(rgb(0x0e0e10))
        .text_color(rgb(0xffffff))
        .p(px(20.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(brand_mark(16.0))
                .child(text_line("Wrapped", 15.0).font_family(DISPLAY).font_weight(FontWeight::SEMIBOLD))
                .child(div().flex_1())
                .child(text_line(d.label.clone(), 13.0).text_color(rgba(0xffff_ff99))),
        )
        .child(tile(0xc6f24e, 0x10140a, "Minutes listened", grouped(d.minutes), 400.0, 104.0).h(px(150.0)))
        .child(
            div()
                .h(px(112.0))
                .flex()
                .gap(px(10.0))
                .child(tile(0x4cc2ff, 0x05202e, "Songs played", grouped(d.plays), 180.0, 52.0).flex_1())
                .child(tile(0xff6f59, 0x2b0b05, "Different songs", grouped(d.songs), 180.0, 52.0).flex_1()),
        )
        .child(
            div()
                .h(px(112.0))
                .flex()
                .gap(px(10.0))
                .child(tile(0xa99bff, 0x140f33, "Artists", grouped(d.artists), 180.0, 52.0).flex_1())
                .child(tile(0xffb938, 0x2e1c00, "Days in a row", d.streak.to_string(), 180.0, 52.0).flex_1()),
        )
        .child(
            div()
                .flex_1()
                .rounded(px(18.0))
                .bg(rgb(0x1f1f23))
                .p(px(16.0))
                .flex()
                .items_center()
                .gap_3()
                .child(artwork(d.cover(0), 52.0, 10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(text_line(t("YOUR SONG"), 11.0).font_weight(FontWeight::SEMIBOLD).text_color(rgba(0xffff_ff99)))
                        .when_some(d.songs_top.first(), |c, s| {
                            c.child(text_line(s.title.clone(), 16.0).font_weight(FontWeight::SEMIBOLD).truncate()).child(text_line(s.artist.clone(), 12.0).text_color(rgba(0xffff_ffb3)).truncate())
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .child(text_line(t("BEST HOUR"), 11.0).font_weight(FontWeight::SEMIBOLD).text_color(rgba(0xffff_ff99)))
                        .child(text_line(d.hour_text(), 22.0).font_family(ROUNDED).font_weight(FontWeight::BOLD)),
                ),
        )
}

/// 10. A magazine cover: the top song's cover full-bleed, a serif masthead, cover lines.
fn magazine(d: &CardData) -> gpui::Div {
    const GOLD: u32 = 0xffd53d;
    let caps = |content: String, size: f32| text_line(content, size).font_family(DISPLAY).font_weight(FontWeight::BOLD).letter_spacing(px(1.5)).whitespace_nowrap();
    let also: Vec<String> = d.artists_top.iter().take(4).map(|a| a.name.clone()).collect();
    base()
        .bg(linear_gradient(170.0, linear_color_stop(rgb(0x8c1d2c), 0.0), linear_color_stop(rgb(0x1c070b), 1.0)))
        .text_color(rgb(0xffffff))
        .when_some(d.big_cover(), |c, image| c.child(img(image).absolute().inset_0().size_full().object_fit(ObjectFit::Cover)))
        // Darkened at the top and the bottom, for the words.
        .child(div().absolute().top_0().left_0().right_0().h(px(260.0)).bg(linear_gradient(180.0, linear_color_stop(rgba(0x0000_00b3), 0.0), linear_color_stop(rgba(0x0000_0000), 1.0))))
        .child(div().absolute().bottom_0().left_0().right_0().h(px(400.0)).bg(linear_gradient(180.0, linear_color_stop(rgba(0x0000_0000), 0.0), linear_color_stop(rgba(0x0000_00eb), 1.0))))
        .child(
            div()
                .absolute()
                .inset_0()
                .p(px(26.0))
                .flex()
                .flex_col()
                .child(div().flex().justify_between().gap_4().child(caps("NABYTUNE".into(), 11.0).letter_spacing(px(4.0))).child(caps(d.label.to_uppercase(), 11.0)))
                .child(text_line("WRAPPED", 80.0).line_height(px(84.0)).font_family(SERIF).font_weight(FontWeight::BOLD).letter_spacing(px(-1.0)).whitespace_nowrap())
                .child(div().flex().items_center().gap_2().child(div().flex_1().h(px(2.0)).bg(rgb(0xffffff))).child(caps(t("THE MUSIC ISSUE").into(), 11.0).text_color(rgb(GOLD))))
                .child(div().flex_1())
                .child(text_line(grouped(d.minutes), 56.0).line_height(px(60.0)).font_family(SERIF).font_weight(FontWeight::BOLD).text_color(rgb(GOLD)))
                .child(text_line(t("minutes of music"), 20.0).font_family(SERIF).italic())
                .when_some(d.songs_top.first(), |c, s| {
                    c.child(caps(t("COVER STORY").into(), 11.0).mt(px(18.0)).text_color(rgb(GOLD)))
                        .child(text_line(s.title.clone(), 30.0).font_family(SERIF).italic().truncate())
                        .child(text_line(s.artist.clone(), 15.0).text_color(rgba(0xffff_ffcc)).truncate())
                })
                .when(!also.is_empty(), |c| {
                    c.child(caps(t("ALSO INSIDE").into(), 11.0).mt(px(14.0)).text_color(rgb(GOLD))).child(text_line(also.join(" · "), 15.0).font_family(SERIF).truncate())
                })
                .child(
                    div()
                        .mt(px(16.0))
                        .flex()
                        .items_end()
                        .justify_between()
                        .gap_4()
                        .child(
                            caps(format!("{} · {}", i18n::songs(d.songs as usize), plural(d.artists as usize, "{} artist", "{} artists")).to_uppercase(), 10.0)
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(rgba(0xffff_ffb3)),
                        )
                        .child(div().flex_none().bg(rgb(0xffffff)).px(px(5.0)).py(px(4.0)).child(barcode(d.seed(), 0x000000, 26.0, 22))),
                ),
        )
}

/// A small picture of each style, to pick from.
fn swatch(style: WrappedStyle) -> gpui::Div {
    let tile = div().relative().w(px(108.0)).h(px(72.0)).rounded(px(8.0)).overflow_hidden().flex().items_center().justify_center();
    let dot = |size: f32, colour: u32| div().size(px(size)).rounded_full().bg(rgb(colour));
    match style {
        WrappedStyle::Signature => tile
            .bg(linear_gradient(155.0, linear_color_stop(rgb(0x1d1d1d), 0.0), linear_color_stop(rgb(0x080808), 1.0)))
            .child(blob(accent_wash(0x77), 50.0, -40.0, 90.0, 20.0))
            .child(text_line("1,234", 20.0).font_family(DISPLAY).font_weight(FontWeight::BOLD).text_color(rgb(accent()))),
        WrappedStyle::Poster => tile.bg(rgb(0xff4a1c)).child(text_line("123", 36.0).line_height(px(38.0)).font_family(IMPACT).text_color(rgb(0x150d0a))),
        WrappedStyle::Minimal => tile.bg(rgb(0xf5f4f0)).child(text_line("1234", 26.0).font_family(DISPLAY).font_weight(FontWeight::LIGHT).text_color(rgb(0x161616))),
        WrappedStyle::Receipt => tile.bg(rgb(0x2e3b42)).items_start().child(
            div()
                .w(px(50.0))
                .h(px(64.0))
                .mt(px(8.0))
                .bg(rgb(0xfbf7ee))
                .flex()
                .flex_col()
                .items_center()
                .pt(px(8.0))
                .gap(px(5.0))
                .children((0..6).map(|i| div().w(px(if i == 0 { 26.0 } else { 36.0 })).h(px(2.0)).bg(rgb(0x857e72)))),
        ),
        WrappedStyle::Neon => tile.bg(rgb(0x0d0720)).font_family(ROUNDED).child(glow_text("NEON", 20.0, 0xff3ec8, FontWeight::BOLD).letter_spacing(px(2.0))),
        WrappedStyle::Aurora => tile
            .bg(rgb(0x070b1c))
            .child(blob(0x19d3_b5dd, -20.0, -24.0, 74.0, 16.0))
            .child(blob(0x7a4d_ffdd, 46.0, 4.0, 74.0, 16.0))
            .child(blob(0xff5f_a8aa, 14.0, 40.0, 64.0, 16.0)),
        WrappedStyle::Cassette => tile.bg(rgb(0xf1e5c9)).child(
            div()
                .w(px(76.0))
                .h(px(48.0))
                .rounded(px(6.0))
                .bg(rgb(0x2b2521))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(16.0))
                .child(dot(15.0, 0xf8f1df))
                .child(dot(15.0, 0xf8f1df)),
        ),
        WrappedStyle::Polaroid => tile.bg(rgb(0xc9b99c)).child(
            div()
                .p(px(4.0))
                .pb(px(12.0))
                .bg(rgb(0xfcfbf7))
                .child(div().size(px(40.0)).bg(linear_gradient(150.0, linear_color_stop(rgb(0xff9a5c), 0.0), linear_color_stop(rgb(0x5b3cc4), 1.0)))),
        ),
        WrappedStyle::Numbers => {
            let block = |colour: u32| div().flex_1().rounded(px(4.0)).bg(rgb(colour));
            tile.bg(rgb(0x0e0e10)).p(px(8.0)).flex_col().items_stretch().gap(px(4.0)).child(block(0xc6f24e)).child(
                div().flex_1().flex().gap(px(4.0)).child(block(0x4cc2ff)).child(block(0xff6f59)).child(block(0xa99bff)),
            )
        }
        WrappedStyle::Magazine => tile
            .bg(linear_gradient(170.0, linear_color_stop(rgb(0x8c1d2c), 0.0), linear_color_stop(rgb(0x1c070b), 1.0)))
            .child(text_line("W", 40.0).line_height(px(44.0)).font_family(SERIF).font_weight(FontWeight::BOLD).text_color(rgb(0xffffff))),
    }
}
