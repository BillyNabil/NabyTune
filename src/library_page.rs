//! Library: the listener's own playlists and what they played lately. A playlist opens
//! in place, with Play, Shuffle, Rename and Delete; songs come in through + on any row.

use gpui::{div, prelude::*, px, rgb, rgba, AnyElement, Context, FontWeight, SharedString, Window};
use gpui_ce_motion::MotionState;

use crate::appearance::{accent, on_accent};
use crate::i18n::{songs, t};
use crate::{artwork, brand_mark, NabyTune, Source, Track, ICONS, SECONDARY, STROKE, SUBTLE, SUBTLE_STRONG, TERTIARY};

/// A pill button: accent-filled for the main action, quiet for the rest.
pub(crate) fn button(id: &'static str, glyph: &'static str, label: impl Into<SharedString>, primary: bool) -> gpui::Stateful<gpui::Div> {
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
        .map(|d| {
            if primary {
                d.bg(rgb(accent())).text_color(rgb(on_accent())).hover(|s| s.opacity(0.9))
            } else {
                d.bg(rgba(SUBTLE_STRONG)).border_1().border_color(rgba(STROKE)).hover(|s| s.bg(rgba(0xffffff26)))
            }
        })
        .child(div().font_family(ICONS).text_size(px(12.0)).child(glyph))
        .child(label.into())
}

impl NabyTune {
    pub(crate) fn library_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Artwork for what's on screen (fetched once each).
        let mut wanted: Vec<Track> = self.library.playlists.iter().filter_map(|p| p.tracks.first()).map(Track::from).collect();
        match self.open_playlist {
            Some(p) => wanted.extend(self.library.playlists.get(p).map(|l| l.tracks.iter().map(Track::from).collect::<Vec<_>>()).unwrap_or_default()),
            None => wanted.extend(self.library.history.iter().take(30).map(Track::from)),
        }
        for track in &wanted {
            self.fetch_art(track, cx);
        }
        let sections = match self.open_playlist.filter(|p| *p < self.library.playlists.len()) {
            Some(p) => vec![self.playlist_view(p, cx)],
            None if !self.account_playlists.is_empty() => vec![self.playlists_grid(cx), self.account_grid(cx), self.local_section(cx), self.recent_list(cx)],
            None => vec![self.playlists_grid(cx), self.local_section(cx), self.recent_list(cx)],
        };
        if self.library_motion.len() < sections.len() {
            self.library_motion.resize_with(sections.len(), MotionState::default);
        }
        let mut page = div()
            .id("library")
            .size_full()
            .overflow_y_scroll()
            .px(px(32.0))
            .pt(px(28.0))
            .pb(px(40.0))
            .flex()
            .flex_col()
            .gap_8()
            .when(self.open_playlist.is_none(), |d| d.child(Self::heading(t("Library"), 30.0)));
        for (i, section) in sections.into_iter().enumerate() {
            let rise = self.rise(i);
            let frame = self.library_motion[i].frame(rise, window, cx);
            page = page.child(frame.apply(section));
        }
        page.into_any_element()
    }

    /// A playlist's cover: its first song's artwork, or NabyTune's mark on an empty one.
    fn playlist_cover(&self, p: usize, side: f32) -> gpui::Div {
        match self.library.playlists[p].tracks.first().and_then(|s| self.art.get(&s.video_id)) {
            Some(art) => artwork(Some(art.small.clone()), side, 10.0),
            None => div().size(px(side)).flex_none().rounded(px(10.0)).bg(rgba(SUBTLE_STRONG)).flex().items_center().justify_center().child(brand_mark(side * 0.3)),
        }
    }

    fn playlists_grid(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let mut grid = div().flex().flex_wrap().gap_5();
        // The songs given a heart, first.
        let liked = self.library.liked.len();
        grid = grid.child(
            div()
                .id("liked-card")
                .w(px(168.0))
                .flex()
                .flex_col()
                .gap_2()
                .cursor_pointer()
                .child(
                    div()
                        .size(px(168.0))
                        .rounded(px(10.0))
                        .shadow_lg()
                        .bg(gpui::linear_gradient(
                            135.0,
                            gpui::linear_color_stop(rgb(accent()), 0.0),
                            gpui::linear_color_stop(rgba(crate::appearance::accent_wash(0x40)), 1.0),
                        ))
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(ICONS)
                        .text_size(px(44.0))
                        .text_color(rgb(on_accent()))
                        .child(crate::track_menu::HEART_FILLED),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).child(t("Liked songs")))
                        .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(match liked {
                            0 => t("Press ♡ on a song").to_string(),
                            n => songs(n),
                        })),
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_liked(cx))),
        );
        for (p, playlist) in self.library.playlists.iter().enumerate() {
            let count = songs(playlist.tracks.len());
            grid = grid.child(
                div()
                    .id(("playlist-card", p))
                    .w(px(168.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .cursor_pointer()
                    .child(self.playlist_cover(p, 168.0).shadow_lg())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).truncate().child(playlist.name.clone()))
                            .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(count)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_playlist = Some(p);
                        this.renaming = false;
                        this.library_motion.clear();
                        cx.notify();
                    })),
            );
        }
        // A tile to start a new one.
        grid = grid.child(
            div()
                .id("playlist-create")
                .w(px(168.0))
                .flex()
                .flex_col()
                .gap_2()
                .cursor_pointer()
                .child(
                    div()
                        .size(px(168.0))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgba(0xffffff33))
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(ICONS)
                        .text_size(px(28.0))
                        .text_color(rgba(SECONDARY))
                        .hover(|s| s.bg(rgba(SUBTLE)).text_color(rgb(0xffffff)))
                        .child("\u{E710}"),
                )
                .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).child(t("New playlist")))
                .on_click(cx.listener(|this, _, window, cx| {
                    let p = this.library.new_playlist();
                    this.library.save();
                    this.open_playlist = Some(p);
                    this.library_motion.clear();
                    // Straight into naming it.
                    this.renaming = true;
                    let name = this.library.playlists[p].name.clone();
                    this.name_field.update(cx, |f, cx| f.edit(&name, window, cx));
                    cx.notify();
                })),
        );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::heading(t("Playlists"), 20.0))
            .when(self.library.playlists.is_empty(), |d| {
                d.child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).mt(px(-8.0)).child(t("Make one here, or press + on any song")))
            })
            .child(grid)
    }

    /// The signed-in account's playlists on YouTube Music, "Liked music" first; a click plays one.
    fn account_grid(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let mut grid = div().flex().flex_wrap().gap_5();
        for (n, card) in self.account_playlists.clone().into_iter().enumerate() {
            let art = self.art.get(&crate::card_key(&card)).map(|a| a.small.clone());
            let opening = self.opening.as_ref().is_some_and(|t| t.as_ref() == card.title);
            let tile = if card.browse_id.as_deref() == Some("VLLM") {
                // Liked music: a heart on the accent, as YouTube Music shows it.
                div()
                    .size(px(168.0))
                    .rounded(px(10.0))
                    .bg(gpui::linear_gradient(135.0, gpui::linear_color_stop(rgb(accent()), 0.0), gpui::linear_color_stop(rgba(appearance_wash()), 1.0)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(ICONS)
                    .text_size(px(44.0))
                    .text_color(rgb(on_accent()))
                    .child("\u{EB52}")
            } else {
                artwork(art, 168.0, 10.0)
            };
            grid = grid.child(
                div()
                    .id(("account-playlist", n))
                    .w(px(168.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .cursor_pointer()
                    .when(opening, |d| d.opacity(0.6))
                    .child(tile.shadow_lg())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).truncate().child(card.title.clone()))
                            .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).truncate().child(card.subtitle.clone())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open_card(card.clone(), cx))),
            );
        }
        div().flex().flex_col().gap_3().child(Self::heading(t("On YouTube Music"), 20.0)).child(grid)
    }

    fn recent_list(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let recent: Vec<Track> = self.library.history.iter().take(30).map(Track::from).collect();
        let mut column = div().flex().flex_wrap().mx(px(-12.0));
        for (i, track) in recent.iter().enumerate() {
            column = column.child(div().w(relative_half()).pr_2().child(self.row(Source::Recent, i, track, false, cx)));
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::heading(t("Recently played"), 20.0))
            .map(|d| if recent.is_empty() { d.child(div().text_size(px(13.0)).text_color(rgba(TERTIARY)).child(t("Songs you play show up here"))) } else { d.child(column) })
    }

    fn playlist_view(&mut self, p: usize, cx: &mut Context<Self>) -> gpui::Div {
        let playlist = self.library.playlists[p].clone();
        let tracks: Vec<Track> = playlist.tracks.iter().map(Track::from).collect();
        let count = songs(tracks.len());
        let armed = self.delete_armed == Some(p);
        let title = if self.renaming {
            div().child(self.name_field.clone()).into_any_element()
        } else {
            div().font_family(crate::DISPLAY).text_size(px(34.0)).font_weight(FontWeight::BOLD).child(playlist.name.clone()).into_any_element()
        };
        let empty = tracks.is_empty();
        let header = div()
            .flex()
            .items_end()
            .gap_6()
            .child(self.playlist_cover(p, 188.0).shadow_lg())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgba(SECONDARY)).child(t("PLAYLIST")))
                    .child(title)
                    .child(div().text_size(px(13.0)).text_color(rgba(SECONDARY)).child(count))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .flex_wrap()
                            .pt_2()
                            .child(button("playlist-play", "\u{E768}", t("Play"), true).when(empty, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, _, cx| {
                                let tracks = this.tracks_of(Source::Playlist(p));
                                if !tracks.is_empty() {
                                    this.shuffle = false;
                                    this.play_from(tracks, 0, cx);
                                }
                            })))
                            .child(button("playlist-shuffle", "\u{E8B1}", t("Shuffle"), false).when(empty, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, _, cx| {
                                let tracks = this.tracks_of(Source::Playlist(p));
                                if !tracks.is_empty() {
                                    this.shuffle = true;
                                    let start = (std::time::Instant::now().elapsed().as_nanos() as usize ^ this.generation as usize * 2_654_435_761) % tracks.len();
                                    this.play_from(tracks, start, cx);
                                }
                            })))
                            .when(!empty, |d| d.child(button("playlist-share", crate::share::SHARE, t("Share playlist"), false).on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(playlist) = this.library.playlists.get(p) {
                                    let title = playlist.name.clone();
                                    let tracks = this.tracks_of(Source::Playlist(p));
                                    this.share_collection(&title, None, &tracks, cx);
                                }
                            }))))
                            .child(button("playlist-rename", "\u{E8AC}", t("Rename"), false).on_click(cx.listener(move |this, _, window, cx| {
                                this.renaming = true;
                                let name = this.library.playlists[p].name.clone();
                                this.name_field.update(cx, |f, cx| f.edit(&name, window, cx));
                                cx.notify();
                            })))
                            // Deleting asks twice: the first click arms it.
                            .child(
                                button("playlist-delete", "\u{E74D}", if armed { t("Click again to delete") } else { t("Delete") }, false)
                                    .when(armed, |d| d.bg(rgba(0xc42b1c66)).border_color(rgba(0xc42b1caa)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.delete_armed == Some(p) {
                                            this.library.playlists.remove(p);
                                            this.library.save();
                                            this.delete_armed = None;
                                            this.open_playlist = None;
                                            this.library_motion.clear();
                                        } else {
                                            this.delete_armed = Some(p);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        let mut rows = div().flex().flex_col().mx(px(-12.0));
        for (i, track) in tracks.iter().enumerate() {
            rows = rows.child(self.row(Source::Playlist(p), i, track, true, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(
                div()
                    .id("playlist-back")
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .text_color(rgba(SECONDARY))
                    .hover(|s| s.text_color(rgb(0xffffff)))
                    .child(div().font_family(ICONS).text_size(px(11.0)).child("\u{E72B}"))
                    .child(t("Library"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_playlist = None;
                        this.renaming = false;
                        this.delete_armed = None;
                        this.library_motion.clear();
                        cx.notify();
                    })),
            )
            .child(header)
            .map(|d| {
                if empty {
                    d.child(div().text_size(px(13.0)).text_color(rgba(TERTIARY)).child(t("No songs yet. Press + on any song to add it here.")))
                } else {
                    d.child(rows)
                }
            })
    }
}

/// The accent, darkened and see-through: the far end of the Liked music tile's gradient.
fn appearance_wash() -> u32 {
    crate::appearance::accent_wash(0x55)
}

fn relative_half() -> gpui::DefiniteLength {
    gpui::relative(0.5)
}
