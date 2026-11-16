//! Radio: an endless queue from a song or an artist, topped up as it runs low, and steered
//! with YouTube Music's own chips (Popular, Discover, Deep cuts, Chill…). Songs turned down
//! never come back on it; picking another list of songs ends it.

use gpui::{div, prelude::*, px, rgb, rgba, Context, SharedString};

use crate::appearance::{accent, accent_wash};
use crate::i18n::{t, tf};
use crate::youtube::RadioChip;
use crate::{NabyTune, Track, ICONS, SECONDARY, STROKE, SUBTLE, SUBTLE_STRONG};

/// Below this many songs left after the one playing, the radio fetches more.
const LOW: usize = 5;
/// How many it asks for each time.
const BATCH: usize = 20;

pub(crate) struct RadioState {
    /// What it started from, as the queue's heading says it ("Blinding Lights", "The Weeknd").
    pub seed: SharedString,
    pub playlist_id: String,
    pub continuation: Option<String>,
    pub chips: Vec<RadioChip>,
    /// A fetch is under way.
    pub loading: bool,
}

impl RadioState {
    fn new(seed: SharedString, playlist_id: String) -> Self {
        Self { seed, playlist_id, continuation: None, chips: Vec::new(), loading: true }
    }

    fn selected(&self) -> Option<String> {
        self.chips.iter().find(|c| c.selected).map(|c| c.label.clone()).filter(|l| l != "All")
    }
}

impl NabyTune {
    /// Starts a radio from `track`: it plays at once, songs like it follow.
    pub(crate) fn start_radio(&mut self, track: Track, cx: &mut Context<Self>) {
        let video_id = track.video_id.clone();
        let seed = track.title.clone();
        self.play_from(vec![track], 0, cx);
        self.radio = Some(RadioState::new(seed, format!("RDAMVM{video_id}")));
        let youtube = self.youtube.clone();
        cx.spawn(async move |this, cx| {
            let playlist = format!("RDAMVM{video_id}");
            let page = cx.background_executor().spawn(async move { youtube.radio_start(Some(&video_id), &playlist, None) }).await;
            let _ = this.update(cx, |app, cx| app.radio_arrived(page, false, cx));
        })
        .detach();
        cx.notify();
    }

    /// Starts an artist's radio (their "Mix"): it plays from its first song.
    pub(crate) fn start_artist_radio(&mut self, playlist_id: String, name: SharedString, cx: &mut Context<Self>) {
        self.radio = Some(RadioState::new(name, playlist_id.clone()));
        let youtube = self.youtube.clone();
        cx.spawn(async move |this, cx| {
            let page = cx.background_executor().spawn(async move { youtube.radio_start(None, &playlist_id, None) }).await;
            let _ = this.update(cx, |app, cx| {
                let Ok(page) = page else {
                    app.radio = None;
                    app.error = Some(t("The artist's radio didn't load").into());
                    cx.notify();
                    return;
                };
                let tracks: Vec<Track> = page.tracks.iter().filter(|s| !app.library.is_disliked(&s.video_id)).cloned().map(Track::from).collect();
                if tracks.is_empty() {
                    app.radio = None;
                    return;
                }
                for t in &tracks {
                    app.fetch_art(t, cx);
                }
                let radio = app.radio.take();
                app.shuffle = false;
                app.play_from(tracks, 0, cx);
                app.radio = radio;
                if let Some(radio) = app.radio.as_mut() {
                    radio.loading = false;
                    radio.playlist_id = page.playlist_id.clone();
                    radio.continuation = page.continuation.clone();
                    radio.chips = page.chips.clone();
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A radio's songs arrived: the new ones go at the end of the queue. With `steer`, they
    /// replace what was waiting after the playing song.
    fn radio_arrived(&mut self, page: anyhow::Result<crate::youtube::RadioPage>, steer: bool, cx: &mut Context<Self>) {
        let Some(radio) = self.radio.as_mut() else { return };
        radio.loading = false;
        let Ok(page) = page else { return };
        radio.playlist_id = page.playlist_id.clone();
        radio.continuation = page.continuation.clone();
        if !page.chips.is_empty() {
            radio.chips = page.chips.clone();
        }
        if steer {
            if let Some(i) = self.index {
                self.queue.truncate(i + 1);
            }
        }
        let mut added = Vec::new();
        for item in page.tracks {
            if !self.queue.iter().any(|t| t.video_id == item.video_id) && !self.library.is_disliked(&item.video_id) {
                added.push(Track::from(item));
            }
        }
        for t in &added {
            self.fetch_art(t, cx);
        }
        self.queue.extend(added);
        self.queue_changed(cx);
    }

    /// More songs, when the radio's queue runs low.
    pub(crate) fn radio_top_up(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.index else { return };
        let left = self.queue.len().saturating_sub(i + 1);
        let Some(radio) = self.radio.as_mut().filter(|r| !r.loading && left < LOW) else { return };
        radio.loading = true;
        let (playlist, continuation, chip) = (radio.playlist_id.clone(), radio.continuation.clone(), radio.selected());
        let seen: Vec<String> = self.queue.iter().map(|t| t.video_id.clone()).collect();
        let youtube = self.youtube.clone();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { youtube.radio_refill(&playlist, continuation.as_deref(), chip.as_deref(), &seen, BATCH) })
                .await;
            let _ = this.update(cx, |app, cx| app.radio_arrived(page, false, cx));
        })
        .detach();
    }

    /// Steers the radio with chip `n`: what's waiting is replaced by its songs.
    pub(crate) fn steer_radio(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(radio) = self.radio.as_mut() else { return };
        let Some(chip) = radio.chips.get(n).cloned() else { return };
        for (i, c) in radio.chips.iter_mut().enumerate() {
            c.selected = i == n;
        }
        radio.loading = true;
        let youtube = self.youtube.clone();
        cx.spawn(async move |this, cx| {
            let page = cx.background_executor().spawn(async move { youtube.radio_start(None, &chip.playlist_id, chip.params.as_deref()) }).await;
            let _ = this.update(cx, |app, cx| app.radio_arrived(page, true, cx));
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn stop_radio(&mut self, cx: &mut Context<Self>) {
        self.radio = None;
        cx.notify();
    }

    /// Up Next's heading while a radio runs: what it's from, and its chips to steer it.
    pub(crate) fn radio_header(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let radio = self.radio.as_ref()?;
        let mut chips = div().flex().flex_wrap().gap(px(6.0));
        for (n, chip) in radio.chips.iter().enumerate() {
            let on = chip.selected;
            chips = chips.child(
                div()
                    .id(("radio-chip", n))
                    .h(px(26.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .border_1()
                    .map(|d| match on {
                        true => d.border_color(rgb(accent())).bg(rgba(accent_wash(0x29))).text_color(rgb(accent())),
                        false => d.border_color(rgba(STROKE)).bg(rgba(SUBTLE)).hover(|s| s.bg(rgba(SUBTLE_STRONG))),
                    })
                    .child(chip.label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.steer_radio(n, cx))),
            );
        }
        Some(
            div()
                .px_4()
                .pb_2()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().font_family(ICONS).text_size(px(12.0)).text_color(rgb(accent())).child("\u{EC05}"))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.0))
                                .text_color(rgba(SECONDARY))
                                .child(if radio.loading { tf("Radio from {} · finding songs…", &[&radio.seed]) } else { tf("Radio from {}", &[&radio.seed]) }),
                        )
                        .child(
                            div()
                                .id("radio-stop")
                                .px(px(8.0))
                                .h(px(22.0))
                                .flex()
                                .items_center()
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(rgba(SECONDARY))
                                .hover(|s| s.bg(rgba(SUBTLE_STRONG)).text_color(rgb(0xffffff)))
                                .child(t("Stop radio"))
                                .on_click(cx.listener(|this, _, _, cx| this.stop_radio(cx))),
                        ),
                )
                .when(!radio.chips.is_empty(), |d| d.child(chips)),
        )
    }
}
