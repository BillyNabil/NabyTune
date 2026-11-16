//! Lyrics on the desktop: the line being sung, floating over everything else (a game, a
//! browser, work), on a see-through window. Unlocked, it can be dragged, sized and locked;
//! locked, clicks go through it to whatever is underneath, and only NabyTune (Settings, the
//! full-screen player, the tray, Ctrl+D twice) brings the handles back.
//!
//! What sits behind the words is the listener's: nothing at all, a soft shadow, or a plate
//! (see-through or dark), with or without a hairline border. (No Acrylic or Mica: Windows
//! only gives glass to windows that can be active, and this one never takes the focus, so a
//! game or whatever else is in front keeps it.)

use std::time::Duration;

use gpui::{
    div, prelude::*, px, rgb, rgba, size, App, Bounds, Context, Entity, FontWeight, Point, SharedString, Subscription, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowHandle, WindowKind, WindowOptions,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::appearance::accent;
use crate::i18n::t;
use crate::{settings, Lyrics, NabyTune, DISPLAY, ICONS, TEXT};

const WIDTH: f32 = 980.0;
/// The words' size: the listener's, between these.
pub(crate) const SMALLEST: f32 = 20.0;
pub(crate) const LARGEST: f32 = 56.0;
const DEFAULT_SIZE: f32 = 32.0;

/// What sits behind the words.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Backdrop {
    /// The words alone, straight on whatever is underneath.
    None,
    /// A soft dark glow behind the words, with no edges: reads on most wallpapers.
    Shadow,
    /// A light see-through plate.
    Glass,
    /// A dark plate, for busy backgrounds.
    Dark,
}

impl Backdrop {
    pub const ALL: [Backdrop; 4] = [Backdrop::None, Backdrop::Shadow, Backdrop::Glass, Backdrop::Dark];

    pub fn name(self) -> &'static str {
        match self {
            Backdrop::None => t("None"),
            Backdrop::Shadow => t("Shadow"),
            Backdrop::Glass => t("See-through"),
            Backdrop::Dark => t("Dark"),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Backdrop::None => "none",
            Backdrop::Shadow => "shadow",
            Backdrop::Glass => "glass",
            Backdrop::Dark => "dark",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.key() == key)
    }

    fn next(self) -> Self {
        let i = Self::ALL.iter().position(|b| *b == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// What's behind the words, as kept (a soft shadow until another is picked).
pub(crate) fn backdrop() -> Backdrop {
    settings::get("desk_lyrics_backdrop").and_then(|k| Backdrop::from_key(&k)).unwrap_or(Backdrop::Shadow)
}

/// A hairline round the plate, as kept (off until switched on).
pub(crate) fn bordered() -> bool {
    settings::get("desk_lyrics_border").is_some_and(|v| v == "1")
}

pub(crate) struct DesktopLyrics {
    app: Entity<NabyTune>,
    _observe: Subscription,
    hwnd: isize,
    /// The lock as last applied to the window (clicks through or not).
    applied: Option<bool>,
}

pub(crate) type DesktopLyricsHandle = WindowHandle<DesktopLyrics>;

/// The words' size, as kept.
pub(crate) fn text_size() -> f32 {
    settings::get("desk_lyrics_size").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_SIZE).clamp(SMALLEST, LARGEST)
}

/// Room for the buttons above the words (unlocked).
const CONTROLS: f32 = 36.0;

/// The window's height for words of `text` px: the buttons' row, the sung line (two rows
/// when it's long) and a smaller line under it.
fn height_for(text: f32) -> f32 {
    (CONTROLS + text * 1.3 * 2.0 + text * 0.62 * 1.3 + 32.0).round()
}

impl NabyTune {
    /// Lyric line `i` in Latin letters, when romaji is on and has been worked out.
    pub(crate) fn line_extra(&self, i: usize) -> Option<SharedString> {
        self.extras.romaji.as_ref().filter(|_| self.show_romaji)?.get(i).cloned().flatten().map(SharedString::from)
    }

    /// Opens the floating lyrics (or closes them, when they're up).
    pub(crate) fn toggle_desktop_lyrics(&mut self, cx: &mut Context<Self>) {
        if self.desk_lyrics.is_some() {
            self.close_desktop_lyrics(cx);
        } else {
            self.open_desktop_lyrics(cx);
        }
    }

    pub(crate) fn open_desktop_lyrics(&mut self, cx: &mut Context<Self>) {
        if self.desk_lyrics.is_some() {
            return;
        }
        settings::set("desk_lyrics", "1");
        let app = cx.entity();
        let height = height_for(text_size());
        let origin = settings::get("desk_lyrics_at").and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some(Point::new(px(x.parse().ok()?), px(y.parse().ok()?)))
        });
        // Where it was last, else low and centred, above the taskbar.
        let bounds = match origin {
            Some(at) => Bounds::new(at, size(px(WIDTH), px(height))),
            None => {
                let screen = cx.primary_display().map(|d| d.bounds()).unwrap_or(Bounds::new(Point::default(), size(px(1920.0), px(1080.0))));
                let at = Point::new(
                    screen.origin.x + (screen.size.width - px(WIDTH)) / 2.0,
                    screen.origin.y + screen.size.height - px(height + 110.0),
                );
                Bounds::new(at, size(px(WIDTH), px(height)))
            }
        };
        // Opened once this update is over: the window reads the app as it draws.
        cx.defer(move |cx: &mut App| {
            let entity = app.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions { title: Some(t("NabyTune lyrics").into()), appears_transparent: true, ..Default::default() }),
                    kind: WindowKind::PopUp,
                    is_resizable: false,
                    focus: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    let hwnd = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
                        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
                        _ => 0,
                    };
                    cx.new(|cx| DesktopLyrics { _observe: cx.observe(&entity, |_, _, cx| cx.notify()), app: entity, hwnd, applied: None })
                },
            );
            if let Ok(handle) = opened {
                app.update(cx, |app, cx| {
                    app.desk_lyrics = Some(handle);
                    cx.notify();
                });
            }
        });
    }

    pub(crate) fn close_desktop_lyrics(&mut self, cx: &mut Context<Self>) {
        settings::set("desk_lyrics", "0");
        if let Some(handle) = self.desk_lyrics.take() {
            // Removed once this update is over (the window reads the app while it goes).
            cx.defer(move |cx: &mut App| {
                let _ = handle.update(cx, |this, window, _| this.remember(window));
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        cx.notify();
    }

    /// Locks the floating lyrics in place (clicks go through) or frees them to move.
    pub(crate) fn set_desktop_lyrics_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        self.desk_locked = locked;
        settings::set("desk_lyrics_locked", if locked { "1" } else { "0" });
        cx.notify();
    }

    /// What sits behind the floating words.
    pub(crate) fn set_desktop_lyrics_backdrop(&mut self, backdrop: Backdrop, cx: &mut Context<Self>) {
        settings::set("desk_lyrics_backdrop", backdrop.key());
        cx.notify();
    }

    /// A hairline round them, or none.
    pub(crate) fn set_desktop_lyrics_border(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set("desk_lyrics_border", if on { "1" } else { "0" });
        cx.notify();
    }

    /// The words a size up or down.
    pub(crate) fn resize_desktop_lyrics(&mut self, step: f32, cx: &mut Context<Self>) {
        let text = (text_size() + step).clamp(SMALLEST, LARGEST);
        settings::set("desk_lyrics_size", &text.to_string());
        if let Some(handle) = self.desk_lyrics {
            cx.defer(move |cx: &mut App| {
                let _ = handle.update(cx, |_, window, _| window.resize(size(px(WIDTH), px(height_for(text)))));
            });
        }
        cx.notify();
    }
}

/// Windows' own "accent" behind a window: off, so the window is only what's drawn in it.
/// (GPUI's see-through windows ask for a "transparent gradient", which Windows 11 paints as a
/// dim box over the whole window.)
fn clear_accent(hwnd: isize) {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        colour: u32,
        animation: u32,
    }
    #[repr(C)]
    struct Data {
        attribute: u32,
        data: *mut core::ffi::c_void,
        size: usize,
    }
    type SetWindowCompositionAttribute = unsafe extern "system" fn(isize, *mut Data) -> i32;
    unsafe {
        let user32 = GetModuleHandleA(c"user32.dll".as_ptr().cast());
        if user32.is_null() {
            return;
        }
        let Some(set) = GetProcAddress(user32, c"SetWindowCompositionAttribute".as_ptr().cast()) else { return };
        let set: SetWindowCompositionAttribute = std::mem::transmute(set);
        let mut accent = AccentPolicy { state: 0, flags: 0, colour: 0, animation: 0 };
        // WCA_ACCENT_POLICY.
        let mut data = Data { attribute: 0x13, data: (&mut accent as *mut AccentPolicy).cast(), size: std::mem::size_of::<AccentPolicy>() };
        set(hwnd, &mut data);
    }
}

impl DesktopLyrics {
    fn remember(&self, window: &Window) {
        let at = window.bounds().origin;
        settings::set("desk_lyrics_at", &format!("{},{}", f32::from(at.x).round(), f32::from(at.y).round()));
    }

    /// Sets the window up: no frame, nothing behind the words but what's drawn, and whether
    /// clicks go through it (locked: layered and transparent to the mouse, as far as Windows
    /// is concerned).
    fn apply(&mut self, locked: bool) {
        if self.applied == Some(locked) || self.hwnd == 0 {
            return;
        }
        let first = self.applied.is_none();
        self.applied = Some(locked);
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            SWP_NOZORDER, WS_CAPTION, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
        };
        unsafe {
            let hwnd = self.hwnd as _;
            if first {
                // A top-level window with no style is "overlapped", so Windows gives it a
                // caption and draws that frame behind the client area: through a see-through
                // window it shows as a dim box with a dark top edge. A popup has no frame.
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                SetWindowLongPtrW(hwnd, GWL_STYLE, ((style & !(WS_CAPTION | WS_THICKFRAME | WS_SYSMENU)) | WS_POPUP) as isize);
                SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
                clear_accent(self.hwnd);
                // Square corners and no border: only the words (and their plate) show.
                const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
                const DWMWA_BORDER_COLOR: u32 = 34;
                let (square, no_colour): (i32, u32) = (1, 0xFFFF_FFFE);
                DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, (&square as *const i32).cast(), 4);
                DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&no_colour as *const u32).cast(), 4);
            }
            // Layered and transparent lets clicks through. No layered attributes are set: with
            // them DWM shades the whole window, and the content (DirectComposition) shows
            // without them anyway. Unlocked, it's a plain window again, so it takes clicks.
            let mut style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 | WS_EX_NOACTIVATE;
            if locked {
                style |= WS_EX_LAYERED | WS_EX_TRANSPARENT;
            } else {
                style &= !(WS_EX_LAYERED | WS_EX_TRANSPARENT);
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize);
        }
    }
}

impl Render for DesktopLyrics {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let locked = app.desk_locked;
        let text = text_size();
        let now = app.lyric_now() + Duration::from_millis(150);
        let playing = app.current().is_some();
        let backdrop = backdrop();
        let border = bordered();
        // The line being sung, and what comes under it: its romaji when that's on, else the
        // next line.
        let (line, under): (Option<crate::lyrics::Line>, Option<SharedString>) = match (&app.lyrics, app.sung_line()) {
            (Lyrics::Ready(lines), Some(i)) => {
                let line = lines.get(i).cloned().filter(|l| !l.text.trim().is_empty());
                let extra = app.line_extra(i);
                let next = lines.iter().skip(i + 1).find(|l| !l.text.trim().is_empty()).map(|l| SharedString::from(l.text.clone()));
                (line, extra.or(next))
            }
            (Lyrics::Ready(lines), None) if app.lyrics_synced => {
                (None, lines.iter().find(|l| !l.text.trim().is_empty()).map(|l| SharedString::from(l.text.clone())))
            }
            _ => (None, None),
        };
        // Nothing to sing: the song's name, quietly, so the window isn't blank.
        let fallback: Option<SharedString> =
            if line.is_none() { app.current().map(|t| format!("♪  {} — {}", t.title, t.artist).into()) } else { None };
        let _ = window;
        self.apply(locked);
        let controls = (!locked).then(|| self.controls(backdrop, cx));

        let lit = rgb(0xffffff);
        let dim = rgba(0xffffff70);
        // The sung line: word by word when the source timed its words.
        let main = match &line {
            Some(l) if !l.words.is_empty() => {
                let mut row = div().flex().flex_wrap().justify_center();
                for word in &l.words {
                    let done = word.end <= now;
                    let on = word.at <= now;
                    let colour = if done || on { lit } else { dim };
                    row = row.child(div().text_color(colour).when(on && !done, |d| d.text_color(rgb(accent()))).child(if word.gap {
                        format!("{} ", word.text)
                    } else {
                        word.text.clone()
                    }));
                }
                row.into_any_element()
            }
            Some(l) => div().text_color(lit).child(l.text.clone()).into_any_element(),
            None => div().text_color(rgba(0xffffffb0)).text_size(px(text * 0.62)).children(fallback).into_any_element(),
        };
        let mut words = div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .px(px(28.0))
            .py(px(10.0))
            .rounded(px(18.0))
            .map(|d| match backdrop {
                Backdrop::Glass => d.bg(rgba(0x1010_1052)),
                Backdrop::Dark => d.bg(rgba(0x1212_12dc)),
                _ => d,
            })
            .when(border && matches!(backdrop, Backdrop::Glass | Backdrop::Dark), |d| d.border_1().border_color(rgba(0xffffff2e)));
        if backdrop == Backdrop::Shadow {
            // A dark glow with no edge, a little wider than the words.
            words = words.child(div().absolute().top(px(-4.0)).bottom(px(-4.0)).left(px(-12.0)).right(px(-12.0)).rounded_full().bg(rgba(0x0000_0080)).blur(px(26.0)));
        }
        let words = words
            .child(
                div()
                    .font_family(DISPLAY)
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(text))
                    .line_height(px(text * 1.3))
                    .text_center()
                    .child(main),
            )
            .when_some(under.filter(|_| line.is_some()), |d, u| {
                d.child(
                    div()
                        .font_family(DISPLAY)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(text * 0.62))
                        .line_height(px(text * 0.62 * 1.3))
                        .text_color(rgba(0xffffffb8))
                        .text_center()
                        .truncate()
                        .child(u),
                )
            });
        // Unlocked, the words move the window (they're where the eye is); the buttons sit in
        // their own row above them, out of the drag area, so they stay buttons.
        let words = words.id("desk-words").when(!locked, |d| d.window_control_area(WindowControlArea::Drag));
        div()
            .id("desk-lyrics")
            .size_full()
            .font_family(TEXT)
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .h(px(CONTROLS))
                    .w_full()
                    .flex()
                    .justify_end()
                    .items_center()
                    .pr(px(6.0))
                    .children(controls),
            )
            .child(div().flex_1().w_full().flex().flex_col().items_center().justify_center().when(playing, |d| d.child(words)))
    }
}

impl DesktopLyrics {
    fn controls(&self, backdrop: Backdrop, cx: &mut Context<Self>) -> gpui::Div {
        let control = |id: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .font_family(ICONS)
                .text_size(px(12.0))
                .text_color(rgba(0xffffffcc))
                .bg(rgba(0x1414_14d0))
                .hover(|s| s.bg(rgba(0x3a3a_3af0)).text_color(rgb(0xffffff)))
                .child(glyph)
        };
        div()
            .flex()
            .gap(px(4.0))
            // What's behind the words: named, and the next one a click away.
            .child(
                div()
                    .id("desk-backdrop")
                    .h(px(28.0))
                    .pl(px(8.0))
                    .pr(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .bg(rgba(0x1414_14d0))
                    .hover(|s| s.bg(rgba(0x3a3a_3af0)))
                    .text_size(px(11.0))
                    .text_color(rgba(0xffffffcc))
                    .child(div().font_family(ICONS).text_size(px(12.0)).child("\u{E790}"))
                    .child(backdrop.name())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.update(cx, |app, cx| app.set_desktop_lyrics_backdrop(backdrop.next(), cx));
                    })),
            )
            .child(control("desk-smaller", "\u{E71F}").on_click(cx.listener(|this, _, _, cx| {
                this.app.update(cx, |app, cx| app.resize_desktop_lyrics(-4.0, cx));
            })))
            .child(control("desk-bigger", "\u{E8A3}").on_click(cx.listener(|this, _, _, cx| {
                this.app.update(cx, |app, cx| app.resize_desktop_lyrics(4.0, cx));
            })))
            .child(control("desk-lock", "\u{E72E}").on_click(cx.listener(|this, _, window, cx| {
                this.remember(window);
                this.app.update(cx, |app, cx| app.set_desktop_lyrics_locked(true, cx));
            })))
            .child(control("desk-close", "\u{E8BB}").on_click(cx.listener(|this, _, window, cx| {
                this.remember(window);
                this.app.update(cx, |app, cx| app.close_desktop_lyrics(cx));
            })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrops_round_trip_and_cycle() {
        for b in Backdrop::ALL {
            assert_eq!(Backdrop::from_key(b.key()), Some(b));
        }
        assert_eq!(Backdrop::Dark.next(), Backdrop::None);
    }
}
