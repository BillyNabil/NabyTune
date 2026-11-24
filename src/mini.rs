//! The mini player: a small window that stays on top of others, with the cover, the song,
//! the line being sung and the buttons. The main window steps aside while it's up, and
//! comes back from its arrow.
//!
//! The sung line can show the original, Latin letters, or a companion reading or translation.
//! Each new line rises in as the last one leaves.

use std::time::Duration;

use gpui::{
    div, ease_out_quint, img, prelude::*, px, rgb, rgba, size, Animation, AnimationExt as _, App, Context, Entity, FontWeight, MouseButton, ObjectFit,
    SharedString, Subscription, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions,
};

use crate::appearance::{accent, accent_wash, on_accent};
use crate::floating_window;
use crate::i18n::t;
use crate::lyrics_view::LyricLanguage;
use crate::{settings, track_menu, Lyrics, NabyTune, DISPLAY, ICONS, SECONDARY, SUBTLE_STRONG, TEXT};

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 152.0;
const ART: f32 = 120.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Preset {
    #[default]
    Classic,
    Vinyl,
    Cassette,
    Spotlight,
}

struct MiniStyle {
    from: u32,
    to: u32,
    ink: u32,
    secondary: u32,
    button_ink: u32,
    art_size: f32,
    radius: f32,
    title_font: &'static str,
}

impl Preset {
    pub(crate) const ALL: [Self; 4] = [Self::Classic, Self::Vinyl, Self::Cassette, Self::Spotlight];

    fn key(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Vinyl => "vinyl",
            Self::Cassette => "cassette",
            Self::Spotlight => "spotlight",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        t(match self {
            Self::Classic => "Classic",
            Self::Vinyl => "Vinyl",
            Self::Cassette => "Cassette",
            Self::Spotlight => "Spotlight",
        })
    }

    pub(crate) fn selected() -> Self {
        settings::get("mini_preset").and_then(|key| Self::ALL.into_iter().find(|p| p.key() == key)).unwrap_or_default()
    }

    fn next(self) -> Self {
        match self {
            Self::Classic => Self::Vinyl,
            Self::Vinyl => Self::Cassette,
            Self::Cassette => Self::Spotlight,
            Self::Spotlight => Self::Classic,
        }
    }

    fn style(self) -> MiniStyle {
        let (from, to, ink, secondary, button_ink, art_size, radius, title_font) = match self {
            Self::Classic => (0x141414, 0x141414, accent(), SECONDARY, on_accent(), ART, 12.0, DISPLAY),
            Self::Vinyl => (0x362620, 0x171313, 0xffbb89, 0xf6d5becc, 0x29160f, 112.0, 56.0, DISPLAY),
            Self::Cassette => (0x333127, 0x24231e, 0xe8d49c, 0xece2c9cc, 0x272117, 84.0, 5.0, "Bahnschrift"),
            Self::Spotlight => (0x142a36, 0x10171f, 0x9ee2eb, 0xcfedf4cc, 0x10232b, 0.0, 9.0, DISPLAY),
        };
        MiniStyle { from, to, ink, secondary, button_ink, art_size, radius, title_font }
    }
}

impl NabyTune {
    pub(crate) fn set_mini_preset(&mut self, preset: Preset, cx: &mut Context<Self>) {
        settings::set("mini_preset", preset.key());
        cx.notify();
    }
}

pub(crate) struct MiniPlayer {
    app: Entity<NabyTune>,
    _observe: Subscription,
    /// The line shown now and the one before it (it leaves as the new one rises in).
    line: Option<usize>,
    leaving: Option<usize>,
}

impl MiniPlayer {
    fn new(app: Entity<NabyTune>, cx: &mut Context<Self>) -> Self {
        // Whatever repaints the app repaints this too: the song, the line, the buttons.
        let observe = cx.observe(&app, |_, _, cx| cx.notify());
        Self { app, _observe: observe, line: None, leaving: None }
    }
}

impl NabyTune {
    /// Opens the mini player (or brings it back), and steps the main window aside.
    pub(crate) fn open_mini_player(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.mini {
            if handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
                return;
            }
        }
        let app = cx.entity();
        let (bounds, display_id) = floating_window::initial_bounds(cx, "mini_at", size(px(WIDTH), px(HEIGHT)), |screen, dimensions| {
            gpui::point(
                screen.origin.x + screen.size.width - dimensions.width - px(24.0),
                screen.origin.y + screen.size.height - dimensions.height - px(24.0),
            )
        });
        // Opened once this update is over: the mini player reads the app as it draws.
        cx.defer(move |cx: &mut App| {
            let entity = app.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    display_id,
                    titlebar: Some(TitlebarOptions { title: Some(t("NabyTune mini player").into()), appears_transparent: true, ..Default::default() }),
                    kind: WindowKind::PopUp,
                    is_resizable: false,
                    is_movable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::MicaBackdrop,
                    ..Default::default()
                },
                |window, cx| {
                    let app = entity.clone();
                    window.on_window_should_close(cx, move |window, cx| {
                        app.update(cx, |app, cx| app.restore_from_mini(window, cx));
                        true
                    });
                    cx.new(|cx| MiniPlayer::new(entity, cx))
                },
            );
            if let Ok(handle) = opened {
                app.update(cx, |app, cx| {
                    app.mini = Some(handle);
                    crate::integrations::minimize_window(app.hwnd);
                    // Work out the Latin reading if the mini player needs it.
                    app.fetch_extras(cx);
                    cx.notify();
                });
            }
        });
    }

    fn cycle_mini_lyric_language(&mut self, cx: &mut Context<Self>) {
        self.mini_lyric_language = self.mini_lyric_language.next();
        settings::set("mini_romaji", self.mini_lyric_language.key());
        self.fetch_extras(cx);
        cx.notify();
    }
}

impl NabyTune {
    fn restore_from_mini(&mut self, window: &Window, cx: &mut Context<Self>) {
        let at = window.bounds().origin;
        settings::set("mini_at", &format!("{},{}", f32::from(at.x).round(), f32::from(at.y).round()));
        self.mini = None;
        crate::integrations::show_window(self.hwnd);
        cx.notify();
    }
}

impl MiniPlayer {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| app.restore_from_mini(window, cx));
        window.remove_window();
    }
}

pub(crate) type MiniHandle = WindowHandle<MiniPlayer>;

impl Render for MiniPlayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let track = app.current().cloned();
        let cover = app.cover.clone();
        let sharp = app.cover_sharp;
        let playing = app.playing;
        let liked = track.as_ref().is_some_and(|t| app.is_liked(&t.video_id));
        let mode = app.mini_lyric_language;
        let preset = Preset::selected();
        let style = preset.style();
        let ink = rgb(style.ink);
        let secondary = rgba(style.secondary);
        let large_lyrics = preset == Preset::Spotlight;
        let tape = preset == Preset::Cassette;
        let button_radius = if tape { 5.0 } else { 17.0 };
        let has_lyrics = matches!(app.lyrics, Lyrics::Ready(_));
        let pending = mode.wants_latin() && app.extras.romaji_pending;
        // The line being sung (blank ones count as none), as this mode writes it.
        let at = |i: usize| -> Option<(SharedString, Option<SharedString>)> {
            let Lyrics::Ready(lines) = &app.lyrics else { return None };
            let line = lines.get(i).filter(|l| !l.text.trim().is_empty())?;
            let (shown, secondary) = app.displayed_lyric(mode, &line.text, i);
            Some((shown.to_owned().into(), secondary.map(|text| text.to_owned().into())))
        };
        let sung = app.sung_line().filter(|i| at(*i).is_some());
        if sung != self.line {
            self.leaving = self.line;
            self.line = sung;
        }
        let current = self.line.and_then(at).map(|l| (self.line.unwrap_or(0), l));
        let leaving = self.leaving.and_then(at).map(|l| (self.leaving.unwrap_or(0), l));
        let progress = if app.length.is_zero() { 0.0 } else { (app.now().as_secs_f32() / app.length.as_secs_f32()).clamp(0.0, 1.0) };

        let button = |id: &'static str, glyph: &'static str, size_px: f32| {
            div()
                .id(id)
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(button_radius))
                .cursor_pointer()
                .font_family(ICONS)
                .text_size(px(size_px))
                .hover(|s| s.bg(rgba(SUBTLE_STRONG)))
                .child(glyph)
        };
        let corner = |id: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .size(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .font_family(ICONS)
                .text_size(px(10.0))
                .text_color(secondary)
                .hover(|s| s.bg(rgba(SUBTLE_STRONG)).text_color(rgb(0xffffff)))
                .child(glyph)
        };
        let drag = |_: &mut Self, _: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| {
            cx.stop_propagation();
            floating_window::drag(window, cx);
        };
        // The cover and the words drag the window; the buttons stay buttons.
        let art = div()
            .id("mini-art")
            .cursor_move().on_mouse_down(MouseButton::Left, cx.listener(drag))
            .relative()
            .size(px(style.art_size))
            .flex_none()
            .rounded(px(style.radius))
            .overflow_hidden()
            .bg(rgba(SUBTLE_STRONG))
            .shadow_md()
            .when(tape, |d| d.p(px(5.0)).border_1().border_color(rgba(0xe8d49c65)))
            .when_some(cover, |d, image| d.child(img(image).object_fit(ObjectFit::Cover).size_full().when(!sharp, |i| i.blur(px(3.0)))))
            .when(preset == Preset::Vinyl, |d| d.child(
                div().absolute().inset_0().rounded_full().border_4().border_color(rgba(0x111010ce))
                    .flex().items_center().justify_center()
                    .child(div().size(px(26.0)).rounded_full().bg(ink).border_2().border_color(rgba(0x211914c0))
                        .flex().items_center().justify_center().child(div().size(px(5.0)).rounded_full().bg(rgb(style.button_ink)))),
            ));
        let (title, artist): (SharedString, SharedString) = match &track {
            Some(t) => (t.title.clone(), t.artist.clone()),
            None => (t("Nothing playing").into(), t("Pick a song in NabyTune").into()),
        };

        let lyric_size = if large_lyrics { 17.0 } else { 13.0 };
        let lyric_height = if large_lyrics { 23.0 } else { 18.0 };
        // A line and its optional companion text.
        let line_block = |(main, under): (SharedString, Option<SharedString>)| {
            div()
                .flex()
                .flex_col()
                .child(div().font_family(DISPLAY).text_size(px(lyric_size)).line_height(px(lyric_height)).font_weight(FontWeight::SEMIBOLD).truncate().child(main))
                .when_some(under, |d, u| d.child(div().text_size(px(11.0)).line_height(px(15.0)).text_color(secondary).truncate().child(u)))
        };
        // The new line rises in; the one before rises on and fades as it goes.
        let mut words = div().id("mini-line").cursor_move().on_mouse_down(MouseButton::Left, cx.listener(drag)).relative().h(px(lyric_height + 16.0)).overflow_hidden();
        if let Some((i, l)) = leaving {
            words = words.child(
                div().absolute().top_0().left_0().right_0().child(
                    line_block(l)
                        .text_color(rgba(0xffffffd9))
                        .with_animation(("mini-out", i), Animation::new(Duration::from_millis(260)).with_easing(ease_out_quint()), |d, k| {
                            d.opacity(1.0 - k).mt(px(-12.0 * k))
                        }),
                ),
            );
        }
        if let Some((i, l)) = current {
            words = words.child(
                div().absolute().top_0().left_0().right_0().child(
                    line_block(l)
                        .text_color(rgb(0xffffff))
                        .with_animation(("mini-in", i), Animation::new(Duration::from_millis(420)).with_easing(ease_out_quint()), |d, k| {
                            d.opacity(k).mt(px(14.0 * (1.0 - k)))
                        }),
                ),
            );
        }

        // Cycle original, Latin below or on its own, and English below.
        let selected_fill = if preset == Preset::Classic { accent_wash(0x2e) } else { (style.ink << 8) | 0x2e };
        let romaji_button = has_lyrics.then(|| {
            div()
                .id("mini-romaji")
                .h(px(26.0))
                .min_w(px(30.0))
                .px(px(8.0))
                .ml(px(2.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .font_family(DISPLAY)
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .map(|d| match mode {
                    LyricLanguage::Original => d.text_color(secondary).hover(|s| s.bg(rgba(SUBTLE_STRONG)).text_color(rgb(0xffffff))),
                    _ => d.bg(rgba(selected_fill)).text_color(ink).hover(|s| s.opacity(0.85)),
                })
                .when(pending, |d| d.opacity(0.55))
                .child(mode.short_label())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.update(cx, |app, cx| app.cycle_mini_lyric_language(cx));
                }))
        });

        div()
            .id("mini")
            .size_full()
            .relative()
            .font_family(TEXT)
            .text_color(rgb(0xffffff))
            .map(|d| if preset == Preset::Classic {
                d.bg(rgba(0x1414_14d9))
            } else {
                d.bg(gpui::linear_gradient(120.0, gpui::linear_color_stop(rgb(style.from), 0.0), gpui::linear_color_stop(rgb(style.to), 1.0)))
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .p(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .when(!large_lyrics, |d| d.child(art))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id("mini-words")
                                    .cursor_move().on_mouse_down(MouseButton::Left, cx.listener(drag))
                                    .mr(px(123.0))
                                    .flex()
                                    .flex_col()
                                    .child(div().font_family(style.title_font).text_size(px(15.0)).font_weight(FontWeight::BOLD).truncate().child(title))
                                    .child(div().text_size(px(12.0)).text_color(secondary).truncate().child(artist)),
                            )
                            .child(if large_lyrics {
                                div().border_l_2().border_color(ink).pl(px(10.0)).child(words)
                            } else {
                                div().child(words)
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .ml(px(-6.0))
                                    .child(button("mini-prev", "\u{E892}", 13.0).on_click(cx.listener(|this, _, _, cx| {
                                        this.app.update(cx, |app, cx| app.skip(-1, cx));
                                    })))
                                    .child(
                                        div()
                                            .id("mini-play")
                                            .size(px(34.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(button_radius))
                                            .cursor_pointer()
                                            .bg(ink)
                                            .text_color(rgb(style.button_ink))
                                            .font_family(ICONS)
                                            .text_size(px(13.0))
                                            .hover(|s| s.opacity(0.9))
                                            .child(if playing { "\u{E769}" } else { "\u{E768}" })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.app.update(cx, |app, cx| app.toggle(cx));
                                            })),
                                    )
                                    .child(button("mini-next", "\u{E893}", 13.0).on_click(cx.listener(|this, _, _, cx| {
                                        this.app.update(cx, |app, cx| app.skip(1, cx));
                                    })))
                                    .child(
                                        button("mini-heart", if liked { track_menu::HEART_FILLED } else { track_menu::HEART }, 13.0)
                                            .when(liked, |d| d.text_color(ink))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.app.update(cx, |app, cx| app.toggle_like_current(cx));
                                            })),
                                    )
                                    .children(romaji_button),
                            ),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .right(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        div()
                            .id("mini-preset")
                            .h(px(24.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .bg(rgba(SUBTLE_STRONG))
                            .text_size(px(11.0))
                            .text_color(secondary)
                            .hover(|s| s.text_color(rgb(0xffffff)).bg(rgba(0xffffff28)))
                            .child(preset.name())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.app.update(cx, |app, cx| app.set_mini_preset(preset.next(), cx));
                            })),
                    )
                    .child(corner("mini-back", "\u{E73F}").on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))
                    .child(corner("mini-close", "\u{E8BB}").on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))),
            )
            // How far into the song, along the bottom edge.
            .child(
                div().absolute().left_0().right_0().bottom_0().h(px(3.0)).bg(rgba(0xffffff14))
                    .when(preset != Preset::Classic, |d| d.left(px(16.0)).right(px(16.0)).bottom(px(6.0)).rounded_full().overflow_hidden())
                    .child(div().h_full().w(gpui::relative(progress)).bg(ink)),
            )
    }
}
