//! The mini player: a small window that stays on top of others, with the cover, the song,
//! the line being sung and the buttons. The main window steps aside while it's up, and
//! comes back from its arrow.
//!
//! The sung line can show as written, with its romaji under it (as the full-screen player
//! does), or as romaji alone; each new line rises in as the last one leaves.

use std::time::Duration;

use gpui::{
    div, ease_out_quint, img, prelude::*, px, rgb, rgba, size, Animation, AnimationExt as _, App, Bounds, Context, Entity, FontWeight, ObjectFit, Point,
    SharedString, Subscription, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowHandle, WindowKind,
    WindowOptions,
};

use crate::appearance::{accent, accent_wash, on_accent};
use crate::i18n::t;
use crate::{settings, track_menu, Lyrics, NabyTune, DISPLAY, ICONS, SECONDARY, SUBTLE_STRONG, TEXT};

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 152.0;
const ART: f32 = 120.0;

/// How the mini player writes the sung line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum MiniRomaji {
    /// As written.
    Original,
    /// As written, its romaji under it.
    Both,
    /// In Latin letters only (as written when it has none).
    Romaji,
}

impl MiniRomaji {
    pub fn key(self) -> &'static str {
        match self {
            MiniRomaji::Original => "original",
            MiniRomaji::Both => "both",
            MiniRomaji::Romaji => "romaji",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [MiniRomaji::Original, MiniRomaji::Both, MiniRomaji::Romaji].into_iter().find(|m| m.key() == key)
    }

    fn next(self) -> Self {
        match self {
            MiniRomaji::Original => MiniRomaji::Both,
            MiniRomaji::Both => MiniRomaji::Romaji,
            MiniRomaji::Romaji => MiniRomaji::Original,
        }
    }

    /// What the button says: the mode it's in.
    fn label(self) -> &'static str {
        match self {
            MiniRomaji::Original => "あ",
            MiniRomaji::Both => "あ·A",
            MiniRomaji::Romaji => "A",
        }
    }

    /// Whether this mode needs the romaji worked out.
    pub fn wants_romaji(self) -> bool {
        self != MiniRomaji::Original
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
        // Where it was last, else the bottom-right corner of the screen.
        let origin = settings::get("mini_at").and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some(Point::new(px(x.parse().ok()?), px(y.parse().ok()?)))
        });
        let bounds = match origin {
            Some(at) => Bounds::new(at, size(px(WIDTH), px(HEIGHT))),
            None => {
                let screen = cx.primary_display().map(|d| d.bounds()).unwrap_or(Bounds::new(Point::default(), size(px(1920.0), px(1080.0))));
                let at = Point::new(screen.origin.x + screen.size.width - px(WIDTH + 24.0), screen.origin.y + screen.size.height - px(HEIGHT + 72.0));
                Bounds::new(at, size(px(WIDTH), px(HEIGHT)))
            }
        };
        // Opened once this update is over: the mini player reads the app as it draws.
        cx.defer(move |cx: &mut App| {
            let entity = app.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions { title: Some(t("NabyTune mini player").into()), appears_transparent: true, ..Default::default() }),
                    kind: WindowKind::PopUp,
                    is_resizable: false,
                    window_background: WindowBackgroundAppearance::MicaBackdrop,
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| MiniPlayer::new(entity, cx)),
            );
            if let Ok(handle) = opened {
                app.update(cx, |app, cx| {
                    app.mini = Some(handle);
                    crate::integrations::minimize_window(app.hwnd);
                    // Its romaji, if it shows some and the lyrics view hasn't asked yet.
                    app.fetch_extras(cx);
                    cx.notify();
                });
            }
        });
    }

    /// The mini player's next way of writing the line; romaji is worked out when it's wanted.
    fn cycle_mini_romaji(&mut self, cx: &mut Context<Self>) {
        self.mini_romaji = self.mini_romaji.next();
        settings::set("mini_romaji", self.mini_romaji.key());
        self.fetch_extras(cx);
        cx.notify();
    }
}

impl MiniPlayer {
    /// Closes the mini player (remembering where it was); the main window comes back.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = window.bounds().origin;
        settings::set("mini_at", &format!("{},{}", f32::from(at.x).round(), f32::from(at.y).round()));
        window.remove_window();
        self.app.update(cx, |app, cx| {
            app.mini = None;
            crate::integrations::show_window(app.hwnd);
            cx.notify();
        });
    }
}

pub(crate) type MiniHandle = WindowHandle<MiniPlayer>;

/// The line as the mini player writes it: (the main text, a smaller one under it).
fn written(mode: MiniRomaji, text: &str, romaji: Option<&str>) -> (SharedString, Option<SharedString>) {
    let romaji = romaji.map(str::trim).filter(|r| !r.is_empty() && *r != text.trim());
    match (mode, romaji) {
        (MiniRomaji::Both, Some(r)) => (text.to_string().into(), Some(r.to_string().into())),
        (MiniRomaji::Romaji, Some(r)) => (r.to_string().into(), None),
        _ => (text.to_string().into(), None),
    }
}

impl Render for MiniPlayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let track = app.current().cloned();
        let cover = app.cover.clone();
        let sharp = app.cover_sharp;
        let playing = app.playing;
        let liked = track.as_ref().is_some_and(|t| app.is_liked(&t.video_id));
        let mode = app.mini_romaji;
        let has_lyrics = matches!(app.lyrics, Lyrics::Ready(_));
        let pending = mode.wants_romaji() && app.extras.romaji_pending;
        // The line being sung (blank ones count as none), as this mode writes it.
        let at = |i: usize| -> Option<(SharedString, Option<SharedString>)> {
            let Lyrics::Ready(lines) = &app.lyrics else { return None };
            let line = lines.get(i).filter(|l| !l.text.trim().is_empty())?;
            let romaji = app.extras.romaji.as_ref().and_then(|r| r.get(i).cloned().flatten());
            Some(written(mode, &line.text, romaji.as_deref()))
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
                .rounded_full()
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
                .text_color(rgba(SECONDARY))
                .hover(|s| s.bg(rgba(SUBTLE_STRONG)).text_color(rgb(0xffffff)))
                .child(glyph)
        };
        // The cover and the words drag the window; the buttons stay buttons.
        let art = div()
            .id("mini-art")
            .window_control_area(WindowControlArea::Drag)
            .size(px(ART))
            .flex_none()
            .rounded(px(12.0))
            .overflow_hidden()
            .bg(rgba(SUBTLE_STRONG))
            .shadow_md()
            .when_some(cover, |d, image| d.child(img(image).object_fit(ObjectFit::Cover).size_full().when(!sharp, |i| i.blur(px(3.0)))));
        let (title, artist): (SharedString, SharedString) = match &track {
            Some(t) => (t.title.clone(), t.artist.clone()),
            None => (t("Nothing playing").into(), t("Pick a song in NabyTune").into()),
        };

        // A line: its text, and its romaji under it when both show.
        let line_block = |(main, under): (SharedString, Option<SharedString>)| {
            div()
                .flex()
                .flex_col()
                .child(div().font_family(DISPLAY).text_size(px(13.0)).line_height(px(18.0)).font_weight(FontWeight::SEMIBOLD).truncate().child(main))
                .when_some(under, |d, u| d.child(div().text_size(px(11.0)).line_height(px(15.0)).text_color(rgba(0xffffff99)).truncate().child(u)))
        };
        // The new line rises in; the one before rises on and fades as it goes.
        let mut words = div().id("mini-line").window_control_area(WindowControlArea::Drag).relative().h(px(34.0)).overflow_hidden();
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

        // Romaji's three ways, one click each: as written, both, Latin letters only.
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
                    MiniRomaji::Original => d.text_color(rgba(SECONDARY)).hover(|s| s.bg(rgba(SUBTLE_STRONG)).text_color(rgb(0xffffff))),
                    _ => d.bg(rgba(accent_wash(0x2e))).text_color(rgb(accent())).hover(|s| s.bg(rgba(accent_wash(0x45)))),
                })
                .when(pending, |d| d.opacity(0.55))
                .child(mode.label())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.update(cx, |app, cx| app.cycle_mini_romaji(cx));
                }))
        });

        div()
            .id("mini")
            .size_full()
            .relative()
            .font_family(TEXT)
            .text_color(rgb(0xffffff))
            .bg(rgba(0x1414_14d9))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .p(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .child(art)
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
                                    .window_control_area(WindowControlArea::Drag)
                                    .mr(px(54.0))
                                    .flex()
                                    .flex_col()
                                    .child(div().font_family(DISPLAY).text_size(px(15.0)).font_weight(FontWeight::BOLD).truncate().child(title))
                                    .child(div().text_size(px(12.0)).text_color(rgba(SECONDARY)).truncate().child(artist)),
                            )
                            .child(words)
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
                                            .rounded_full()
                                            .cursor_pointer()
                                            .bg(rgb(accent()))
                                            .text_color(rgb(on_accent()))
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
                                            .when(liked, |d| d.text_color(rgb(accent())))
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
                    .gap(px(2.0))
                    .child(corner("mini-back", "\u{E73F}").on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))
                    .child(corner("mini-close", "\u{E8BB}").on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))),
            )
            // How far into the song, along the bottom edge.
            .child(div().absolute().bottom_0().left_0().right_0().h(px(3.0)).bg(rgba(0xffffff14)).child(div().h_full().w(gpui::relative(progress)).bg(rgb(accent()))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_line_each_way() {
        let (main, under) = written(MiniRomaji::Both, "君の名は", Some("Kimi no na wa"));
        assert_eq!((main.as_ref(), under.as_deref()), ("君の名は", Some("Kimi no na wa")));
        let (main, under) = written(MiniRomaji::Romaji, "君の名は", Some("Kimi no na wa"));
        assert_eq!((main.as_ref(), under), ("Kimi no na wa", None));
        // No romaji (or the same as written): the line as it is, once.
        let (main, under) = written(MiniRomaji::Both, "Hello", Some("Hello"));
        assert_eq!((main.as_ref(), under), ("Hello", None));
        let (main, under) = written(MiniRomaji::Romaji, "Hello", None);
        assert_eq!((main.as_ref(), under), ("Hello", None));
        assert_eq!(MiniRomaji::from_key(MiniRomaji::Both.next().key()), Some(MiniRomaji::Romaji));
    }
}
