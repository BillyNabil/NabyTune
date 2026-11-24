//! Lyrics on the desktop: the line being sung, floating over everything else (a game, a
//! browser, work), on a see-through window. Unlocked, it can be dragged, sized and locked;
//! locked, clicks go through it to whatever is underneath, and only NabyTune (Settings, the
//! full-screen player, the tray, Ctrl+D twice) brings the handles back.
//!
//! What sits behind the words is the listener's: nothing at all, a soft shadow, or a plate
//! (see-through or dark), with or without a hairline border. (No Acrylic or Mica: Windows
//! only gives glass to windows that can be active, and this one never takes the focus, so a
//! game or whatever else is in front keeps it.)

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    div, hsla, point, prelude::*, px, rgb, rgba, size, AnyElement, App, BoxShadow, Bounds, Context, Entity, FontWeight, MouseButton, Pixels, Point, SharedString, Size, Subscription, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use crate::floating_window;

use crate::appearance::accent;
use crate::i18n::t;
use crate::{settings, Lyrics, NabyTune, DISPLAY, ICONS, TEXT};

const WIDTH: f32 = 980.0;
/// The words' size: the listener's, between these.
pub(crate) const SMALLEST: f32 = 20.0;
pub(crate) const LARGEST: f32 = 56.0;
const DEFAULT_SIZE: f32 = 32.0;
/// Room round the words for their shadow to fade out in: the shadow reaches three blurs (plus
/// its spread) past the words, and all of that has to be inside the window, or its edge shows
/// as a straight cut.
const SHADOW: f32 = 36.0;
const SHADOW_BLUR: f32 = 11.0;
const SHADOW_SPREAD: f32 = 2.0;

/// Which side of the window the words keep to: centred, or to one side so they can sit in a
/// corner of the screen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    pub const ALL: [Align; 3] = [Align::Left, Align::Center, Align::Right];

    pub fn name(self) -> &'static str {
        match self {
            Align::Left => t("Left"),
            Align::Center => t("Center"),
            Align::Right => t("Right"),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == key)
    }

    fn next(self) -> Self {
        let i = Self::ALL.iter().position(|a| *a == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// Segoe's align-left, -centre and -right glyphs.
    fn glyph(self) -> &'static str {
        match self {
            Align::Left => "\u{E8E4}",
            Align::Center => "\u{E8E3}",
            Align::Right => "\u{E8E2}",
        }
    }
}

/// Which side the words keep to, as kept (centred until another is picked).
pub(crate) fn align() -> Align {
    settings::get("desk_lyrics_align").and_then(|k| Align::from_key(&k)).unwrap_or(Align::Center)
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Preset {
    #[default]
    Custom,
    Cinema,
    Neon,
    Editorial,
}

struct LyricStyle {
    main: u32,
    secondary: u32,
    dim: u32,
    highlight: u32,
    plate: u32,
    stroke: u32,
    radius: f32,
    font: &'static str,
    weight: FontWeight,
}

impl Preset {
    pub(crate) const ALL: [Self; 4] = [Self::Custom, Self::Cinema, Self::Neon, Self::Editorial];

    fn key(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Cinema => "cinema",
            Self::Neon => "neon",
            Self::Editorial => "editorial",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        t(match self {
            Self::Custom => "Custom",
            Self::Cinema => "Cinema",
            Self::Neon => "Neon",
            Self::Editorial => "Editorial",
        })
    }

    pub(crate) fn selected() -> Self {
        settings::get("desk_lyrics_preset").and_then(|key| Self::ALL.into_iter().find(|p| p.key() == key)).unwrap_or_default()
    }

    fn next(self) -> Self {
        match self {
            Self::Custom => Self::Cinema,
            Self::Cinema => Self::Neon,
            Self::Neon => Self::Editorial,
            Self::Editorial => Self::Custom,
        }
    }

    fn setting_key(self, key: &str) -> String {
        if self == Self::Custom { key.to_owned() } else { format!("{key}.{}", self.key()) }
    }

    fn default_backdrop(self) -> Backdrop {
        match self {
            Self::Custom => Backdrop::Shadow,
            Self::Cinema | Self::Editorial => Backdrop::Dark,
            Self::Neon => Backdrop::Glass,
        }
    }

    fn style(self) -> LyricStyle {
        let (main, secondary, dim, highlight, plate, stroke, radius, font, weight) = match self {
            Self::Custom => (0xffffff, 0xffffffb8, 0xffffff70, accent(), 0x121212, 0xffffff2e, 18.0, DISPLAY, FontWeight::BOLD),
            Self::Cinema => (0xfff9e8, 0xffedcbd9, 0xffefcda6, 0xffd47c, 0x0b0b0c, 0xf5ddaa55, 5.0, DISPLAY, FontWeight::SEMIBOLD),
            Self::Neon => (0xe3fcff, 0xb4deffc9, 0xd9f6ff90, 0x6df0e0, 0x0c2341, 0x67ddf299, 13.0, DISPLAY, FontWeight::BOLD),
            Self::Editorial => (0xffeedb, 0xf3cbb2df, 0xfbe0c59e, 0xffa98b, 0x271b25, 0xffad8490, 3.0, "Georgia", FontWeight::NORMAL),
        };
        LyricStyle { main, secondary, dim, highlight, plate, stroke, radius, font, weight }
    }
}

/// What's behind the words, as kept (a soft shadow until another is picked).
pub(crate) fn backdrop() -> Backdrop {
    let preset = Preset::selected();
    settings::get(&preset.setting_key("desk_lyrics_backdrop")).and_then(|k| Backdrop::from_key(&k)).unwrap_or_else(|| preset.default_backdrop())
}

/// A hairline round the plate, as kept (off until switched on).
pub(crate) fn bordered() -> bool {
    let preset = Preset::selected();
    settings::get(&preset.setting_key("desk_lyrics_border")).map_or(matches!(preset, Preset::Neon | Preset::Editorial), |v| v == "1")
}

pub(crate) struct DesktopLyrics {
    app: Entity<NabyTune>,
    _observe: Subscription,
    hwnd: isize,
    /// The window's scale (physical pixels per logical one), for where the mouse is.
    scale: f32,
    /// Where the mouse is, as last looked: over the window, over its Unlock button.
    hover: Hover,
    controls_above: bool,
    dragging: bool,
    /// Offsets are relative to the unshifted bounds measured by GPUI.
    word_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    wanted_offset: Point<Pixels>,
    drawn_offset: Point<Pixels>,
    viewport: Size<Pixels>,
    /// As last applied to the window: (locked, its Unlock button taking clicks).
    applied: Option<(bool, bool)>,
}

/// The mouse over the floating lyrics: anywhere on them, and on the Unlock button in
/// particular (locked, that's the one part that takes clicks).
#[derive(Clone, Copy, PartialEq, Default)]
struct Hover {
    over: bool,
    on_unlock: bool,
}

/// The Unlock button's width.
const UNLOCK_WIDTH: f32 = 112.0;

pub(crate) type DesktopLyricsHandle = WindowHandle<DesktopLyrics>;

/// How solid the words (and their plate or shadow) are, in percent: the steps offered. The
/// window stays on top of everything, so fainter words get in the way less.
pub(crate) const OPACITIES: [u8; 5] = [100, 80, 60, 40, 25];

/// The words' opacity, in percent, as kept.
pub(crate) fn opacity() -> u8 {
    settings::get("desk_lyrics_opacity").and_then(|v| v.parse().ok()).unwrap_or(100).clamp(10, 100)
}

/// The next step down, and from the faintest back to solid.
fn next_opacity(percent: u8) -> u8 {
    OPACITIES.iter().copied().find(|&step| step < percent).unwrap_or(100)
}

/// The words' size, as kept.
pub(crate) fn text_size() -> f32 {
    settings::get("desk_lyrics_size").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_SIZE).clamp(SMALLEST, LARGEST)
}

/// Room for the buttons under the words.
const CONTROLS: f32 = 36.0;

/// The window's height for words of `text` px: the sung line (two rows when it's long), a
/// smaller line under it, room for the shadow round them, and the buttons' row below.
fn height_for(text: f32) -> f32 {
    (SHADOW * 2.0 + text * 1.3 * 2.0 + text * 0.62 * 1.3 + 32.0 + CONTROLS * 2.0).round()
}

impl NabyTune {
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
        let (bounds, display_id) = floating_window::initial_bounds(cx, "desk_lyrics_at", size(px(WIDTH), px(height_for(text_size()))), |screen, dimensions| {
            point(
                screen.origin.x + (screen.size.width - dimensions.width) / 2.0,
                screen.origin.y + screen.size.height - dimensions.height - px(28.0),
            )
        });
        // Opened once this update is over: the window reads the app as it draws.
        cx.defer(move |cx: &mut App| {
            let entity = app.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    display_id,
                    titlebar: Some(TitlebarOptions { title: Some(t("NabyTune lyrics").into()), appears_transparent: true, ..Default::default() }),
                    kind: WindowKind::PopUp,
                    is_resizable: false,
                    is_movable: false,
                    is_minimizable: false,
                    focus: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    let hwnd = floating_window::hwnd(window);
                    let scale = window.scale_factor();
                    cx.new(|cx| {
                        // Where the mouse is, a few times a second: locked, the window can't
                        // see it (clicks pass through), so it's looked up. Only a change repaints.
                        cx.spawn(async move |this, cx| loop {
                            cx.background_executor().timer(Duration::from_millis(120)).await;
                            if this.read_with(cx, |view: &DesktopLyrics, _| !view.dragging).unwrap_or(false) {
                                if let Some(rect) = floating_window::rect(hwnd) {
                                    if let Some(work) = floating_window::work_area(floating_window::center(rect)) {
                                        floating_window::place(hwnd, rect, rect.left, rect.top, work);
                                    }
                                }
                            }
                            if this.update(cx, |this: &mut DesktopLyrics, cx| this.refresh_placement(cx)).is_err() {
                                break;
                            }
                        })
                        .detach();
                        let wanted_offset = settings::get("desk_lyrics_offset")
                            .and_then(|saved| {
                                let (x, y) = saved.split_once(',')?;
                                let (x, y) = (x.parse::<f32>().ok()?, y.parse::<f32>().ok()?);
                                (x.is_finite() && y.is_finite()).then(|| point(px(x), px(y)))
                            })
                            .unwrap_or_default();
                        DesktopLyrics {
                            _observe: cx.observe(&entity, |_, _, cx| cx.notify()),
                            app: entity,
                            hwnd,
                            scale,
                            hover: Hover::default(),
                            controls_above: false,
                            dragging: false,
                            word_bounds: Rc::new(Cell::new(None)),
                            wanted_offset,
                            drawn_offset: Point::default(),
                            viewport: window.viewport_size(),
                            applied: None,
                        }
                    })
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
                let _ = handle.update(cx, |this, _, _| this.remember());
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

    pub(crate) fn set_desktop_lyrics_preset(&mut self, preset: Preset, cx: &mut Context<Self>) {
        settings::set("desk_lyrics_preset", preset.key());
        cx.notify();
    }

    /// What sits behind the floating words.
    pub(crate) fn set_desktop_lyrics_backdrop(&mut self, backdrop: Backdrop, cx: &mut Context<Self>) {
        settings::set(&Preset::selected().setting_key("desk_lyrics_backdrop"), backdrop.key());
        cx.notify();
    }

    /// Which side of the window the words keep to.
    pub(crate) fn set_desktop_lyrics_align(&mut self, align: Align, cx: &mut Context<Self>) {
        settings::set("desk_lyrics_align", align.key());
        cx.notify();
    }

    /// How solid the words are, in percent.
    pub(crate) fn set_desktop_lyrics_opacity(&mut self, percent: u8, cx: &mut Context<Self>) {
        settings::set("desk_lyrics_opacity", &percent.clamp(10, 100).to_string());
        cx.notify();
    }

    /// A hairline round them, or none.
    pub(crate) fn set_desktop_lyrics_border(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set(&Preset::selected().setting_key("desk_lyrics_border"), if on { "1" } else { "0" });
        cx.notify();
    }

    /// The words a size up or down.
    pub(crate) fn resize_desktop_lyrics(&mut self, step: f32, cx: &mut Context<Self>) {
        let text = (text_size() + step).clamp(SMALLEST, LARGEST);
        settings::set("desk_lyrics_size", &text.to_string());
        if let Some(handle) = self.desk_lyrics {
            cx.defer(move |cx: &mut App| {
                let _ = handle.update(cx, |_, window, _| window.resize(size(window.viewport_size().width, px(height_for(text)))));
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
    fn remember(&self) {
        if let Some(rect) = floating_window::rect(self.hwnd) {
            let at = point(px(rect.left as f32 / self.scale), px(rect.top as f32 / self.scale));
            let store_point = |key, value: Point<Pixels>| {
                settings::set(key, &format!("{},{}", f32::from(value.x).round(), f32::from(value.y).round()));
            };
            store_point("desk_lyrics_at", at);
            store_point("desk_lyrics_offset", self.wanted_offset);
        }
    }

    fn refresh_placement(&mut self, cx: &mut Context<Self>) {
        let above = floating_window::rect(self.hwnd).is_some_and(|rect| {
            let center = floating_window::center(rect);
            floating_window::work_area(center).is_some_and(|work| center.y > floating_window::center(work).y)
        });
        let now = self.hover_now(above);
        if now != self.hover || above != self.controls_above || self.word_offset() != self.drawn_offset {
            self.hover = now;
            self.controls_above = above;
            cx.notify();
        }
    }

    fn word_offset(&self) -> Point<Pixels> {
        let Some(words) = self.word_bounds.get() else { return Point::default() };
        let toolbar = px(CONTROLS);
        let (top, bottom) = if self.controls_above { (toolbar, self.viewport.height) } else { (px(0.0), self.viewport.height - toolbar) };
        point(
            keep_inside(self.wanted_offset.x, words.left(), words.right(), px(0.0), self.viewport.width),
            keep_inside(self.wanted_offset.y, words.top(), words.bottom(), top, bottom),
        )
    }

    /// Move the window first, then let the words continue through its empty space at an edge.
    fn start_drag(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.dragging {
            return;
        }
        let original = self.drawn_offset;
        let hwnd = self.hwnd;
        let view = cx.weak_entity();
        self.dragging = floating_window::track_drag(window, cx, move |position, cx| {
            floating_window::place(hwnd, position.rect, position.wanted.x, position.wanted.y, position.work);
            let Some(rect) = floating_window::rect(hwnd) else { return };
            let _ = view.update(cx, |view, cx| {
                view.wanted_offset = point(
                    original.x + px((position.wanted.x - rect.left) as f32 / view.scale),
                    original.y + px((position.wanted.y - rect.top) as f32 / view.scale),
                );
                view.refresh_placement(cx);
            });
        }, |view, cx| {
            view.dragging = false;
            view.wanted_offset = view.word_offset();
            view.remember();
            cx.notify();
        });
    }

    fn hover_now(&self, controls_above: bool) -> Hover {
        let (Some(at), Some(r)) = (floating_window::cursor(), floating_window::rect(self.hwnd)) else {
            return Hover::default();
        };
        let over = at.x >= r.left && at.x < r.right && at.y >= r.top && at.y < r.bottom;
        let px = |v: f32| (v * self.scale).round() as i32;
        let top = if controls_above { r.top } else { r.bottom - px(CONTROLS) };
        let on_unlock = over && at.x >= r.right - px(UNLOCK_WIDTH + 8.0) && at.y >= top && at.y < top + px(CONTROLS);
        Hover { over, on_unlock }
    }

    /// Sets the window up: no frame, nothing behind the words but what's drawn, and whether
    /// clicks go through it (locked: layered and transparent to the mouse, as far as Windows
    /// is concerned; but for the Unlock button, while the mouse is on it).
    fn apply(&mut self, locked: bool, unlock_takes_clicks: bool) {
        let want = (locked, locked && unlock_takes_clicks);
        if self.applied == Some(want) || self.hwnd == 0 {
            return;
        }
        let first = self.applied.is_none();
        self.applied = Some(want);
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
            match want {
                (true, false) => style |= WS_EX_LAYERED | WS_EX_TRANSPARENT,
                // Locked, the mouse on Unlock: the window takes that click, then goes back.
                (true, true) => style = (style | WS_EX_LAYERED) & !WS_EX_TRANSPARENT,
                (false, _) => style &= !(WS_EX_LAYERED | WS_EX_TRANSPARENT),
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
        let preset = Preset::selected();
        let style = preset.style();
        let backdrop = backdrop();
        let border = bordered();
        let align = align();
        // The sung line, with the selected companion text or the next line below it.
        let language = app.lyric_language;
        let (line, shown, under) = match (&app.lyrics, app.sung_line()) {
            (Lyrics::Ready(lines), Some(i)) => {
                let line = lines.get(i).filter(|line| !line.text.trim().is_empty());
                let (shown, extra) = line.map_or(("", None), |line| app.displayed_lyric(language, &line.text, i));
                let under = extra.or_else(|| {
                    lines.iter().enumerate().skip(i + 1).find(|(_, line)| !line.text.trim().is_empty())
                        .map(|(index, line)| app.displayed_lyric(language, &line.text, index).0)
                });
                (line.cloned(), shown.to_owned(), under.map(SharedString::from))
            }
            _ => (None, String::new(), None),
        };
        // Nothing to sing: the song's name, quietly, so the window isn't blank.
        let fallback: Option<SharedString> =
            if line.is_none() { app.current().map(|t| format!("♪  {} — {}", t.title, t.artist).into()) } else { None };
        let hint = app.lyric_language_note().unwrap_or_else(|| t("Drag the words to move them · Lock lets clicks pass through").into());
        self.scale = window.scale_factor();
        self.viewport = window.viewport_size();
        let offset = self.word_offset();
        self.drawn_offset = offset;
        let hover = self.hover;
        self.apply(locked, hover.on_unlock);
        // Unlocked: the buttons, named. Locked: only Unlock, and only while the mouse is over
        // the lyrics (they stay out of the way otherwise).
        let controls: Option<AnyElement> = if locked {
            hover.over.then(|| self.unlock_button(hover.on_unlock, cx).into_any_element())
        } else {
            Some(self.controls(preset, backdrop, language, hint, cx).into_any_element())
        };

        let lit = rgb(style.main);
        let dim = rgba(style.dim);
        // The sung line: word by word when the source timed its words.
        let main = match &line {
            Some(l) if shown == l.text && !l.words.is_empty() => {
                let mut row = div().flex().flex_wrap().map(|d| match align {
                    Align::Left => d.justify_start(),
                    Align::Center => d.justify_center(),
                    Align::Right => d.justify_end(),
                });
                for word in &l.words {
                    let done = word.end <= now;
                    let on = word.at <= now;
                    let colour = if done || on { lit } else { dim };
                    row = row.child(div().text_color(colour).when(on && !done, |d| d.text_color(rgb(style.highlight))).child(if word.gap {
                        format!("{} ", word.text)
                    } else {
                        word.text.clone()
                    }));
                }
                row.into_any_element()
            }
            Some(_) => div().text_color(lit).child(shown).into_any_element(),
            None => div().text_color(rgba(style.secondary)).text_size(px(text * 0.62)).children(fallback).into_any_element(),
        };
        // Lines (and the whole block) to the side the listener picked.
        let side = |d: gpui::Div| match align {
            Align::Left => d.items_start(),
            Align::Center => d.items_center(),
            Align::Right => d.items_end(),
        };
        let text_side = |d: gpui::Div| match align {
            Align::Left => d.text_left(),
            Align::Center => d.text_center(),
            Align::Right => d.text_right(),
        };
        let words = div()
            .relative()
            .flex()
            .flex_col()
            .map(side)
            .gap(px(4.0))
            .px(px(28.0))
            .py(px(10.0))
            .rounded(px(style.radius))
            .map(|d| match backdrop {
                Backdrop::Glass => {
                    let fill = match preset {
                        Preset::Custom => 0x1010_1052,
                        Preset::Neon => (style.plate << 8) | 0xbb,
                        _ => (style.plate << 8) | 0x96,
                    };
                    d.bg(rgba(fill))
                }
                Backdrop::Dark => {
                    let alpha = if preset == Preset::Custom { 0xdc } else { 0xe6 };
                    d.bg(rgba((style.plate << 8) | alpha))
                }
                _ => d,
            })
            .when(border && matches!(backdrop, Backdrop::Glass | Backdrop::Dark), |d| {
                d.map(|d| if preset == Preset::Editorial { d.border_b_2() } else { d.border_1() }).border_color(rgba(style.stroke))
            })
            // A dark glow with no edge: a soft shadow of the words' own rounded box, which
            // fades out within the room left round the words (SHADOW), so the window's edge
            // never cuts it.
            .when(backdrop == Backdrop::Shadow, |d| {
                d.shadow(vec![BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.55),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(SHADOW_BLUR),
                    spread_radius: px(SHADOW_SPREAD),
                    inset: false,
                }])
            });
        let words = words
            .child(
                div()
                    .font_family(style.font)
                    .font_weight(style.weight)
                    .text_size(px(text))
                    .line_height(px(text * 1.3))
                    .map(text_side)
                    .child(main),
            )
            .when_some(under.filter(|_| line.is_some()), |d, u| {
                d.child(
                    div()
                        .font_family(DISPLAY)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(text * 0.62))
                        .line_height(px(text * 0.62 * 1.3))
                        .text_color(rgba(style.secondary))
                        .map(text_side)
                        .truncate()
                        .child(u),
                )
            });
        let faded = opacity();
        let drag = |view: &mut Self, _: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| {
            cx.stop_propagation();
            view.start_drag(window, cx);
        };
        let words = words
            .left(offset.x)
            .top(offset.y)
            .when(faded < 100, |d| d.opacity(faded as f32 / 100.0))
            .id("desk-text")
            .when(!locked, |d| d.cursor_move().on_mouse_down(MouseButton::Left, cx.listener(drag)));
        let word_bounds = self.word_bounds.clone();
        let drag_area = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .pt(px(SHADOW))
            .px(px(SHADOW))
            .flex()
            .flex_col()
            .map(side)
            .justify_start()
            .on_children_prepainted(move |children, _, _| {
                word_bounds.set(children.first().map(|bounds| Bounds::new(bounds.origin - offset, bounds.size)));
            })
            .id("desk-words")
            .when(playing, |d| d.child(words))
            .when(!locked, |d| d.cursor_move().on_mouse_down(MouseButton::Left, cx.listener(drag)));
        let row = div().h(px(CONTROLS)).flex_none().w_full().flex().justify_end().items_center().pr(px(8.0)).children(controls);
        let space = div().h(px(CONTROLS)).flex_none();
        let (top, bottom) = if self.controls_above { (row, space) } else { (space, row) };
        div()
            .id("desk-lyrics")
            .size_full()
            .font_family(TEXT)
            .flex()
            .flex_col()
            .items_center()
            .child(top)
            .child(drag_area)
            .child(bottom)

    }
}

impl DesktopLyrics {
    /// Locked, with the mouse over the lyrics: the way back to moving them.
    fn unlock_button(&self, hot: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id("desk-unlock")
            .w(px(UNLOCK_WIDTH))
            .h(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .rounded_full()
            .cursor_pointer()
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(0xffffff))
            .bg(rgba(if hot { 0x3a3a_3af0 } else { 0x1414_14d0 }))
            .border_1()
            .border_color(rgba(0xffffff26))
            .child(div().font_family(ICONS).text_size(px(12.0)).child("\u{E785}"))
            .child(t("Unlock"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.app.update(cx, |app, cx| app.set_desktop_lyrics_locked(false, cx));
            }))
    }

    fn controls(&self, preset: Preset, backdrop: Backdrop, language: crate::lyrics_view::LyricLanguage, hint: SharedString, cx: &mut Context<Self>) -> gpui::Div {
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
            .w_full()
            .flex()
            .items_center()
            .gap(px(4.0))
            .pl(px(8.0))
            // What the plate is for, said once, where the eye goes after the words.
            .child(
                div().flex_1().min_w_0().pl(px(8.0)).child(
                    div()
                        .h(px(28.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .rounded(px(7.0))
                        .bg(rgba(0x1414_14d0))
                        .text_size(px(11.0))
                        .text_color(rgba(0xffffffcc))
                        .truncate()
                        .child(hint),
                ),
            )
            .child(
                div()
                    .id("desk-preset")
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .bg(rgba(0x1414_14ed))
                    .hover(|s| s.bg(rgba(0x3a3a_3af0)))
                    .text_size(px(11.0))
                    .text_color(rgb(0xffffff))
                    .child(div().font_family(ICONS).text_size(px(12.0)).child("\u{E790}"))
                    .child(preset.name())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.update(cx, |app, cx| app.set_desktop_lyrics_preset(preset.next(), cx));
                    })),
            )
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
                    .child(div().font_family(ICONS).text_size(px(12.0)).child("\u{E91B}"))
                    .child(backdrop.name())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.update(cx, |app, cx| app.set_desktop_lyrics_backdrop(backdrop.next(), cx));
                    })),
            )
            // Which side the words keep to: left, centre, right, round again.
            .child({
                let align = align();
                div()
                    .id("desk-align")
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
                    .child(div().font_family(ICONS).text_size(px(12.0)).child(align.glyph()))
                    .child(align.name())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.update(cx, |app, cx| app.set_desktop_lyrics_align(align.next(), cx));
                    }))
            })
            // How solid the words are: a step fainter each click, then solid again.
            .child({
                let percent = opacity();
                div()
                    .id("desk-opacity")
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
                    .child(div().font_family(ICONS).text_size(px(12.0)).child("\u{E706}"))
                    .child(format!("{percent}%"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.update(cx, |app, cx| app.set_desktop_lyrics_opacity(next_opacity(percent), cx));
                    }))
            })
            .child(control("desk-language", language.short_label()).font_family(TEXT).on_click(cx.listener(|this, _, _, cx| {
                this.app.update(cx, |app, cx| app.cycle_lyric_language(cx));
            })))
            .child(control("desk-smaller", "\u{E71F}").on_click(cx.listener(|this, _, _, cx| {
                this.app.update(cx, |app, cx| app.resize_desktop_lyrics(-4.0, cx));
            })))
            .child(control("desk-bigger", "\u{E8A3}").on_click(cx.listener(|this, _, _, cx| {
                this.app.update(cx, |app, cx| app.resize_desktop_lyrics(4.0, cx));
            })))
            .child(
                div()
                    .id("desk-lock")
                    .h(px(28.0))
                    .pl(px(10.0))
                    .pr(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .bg(rgb(accent()))
                    .text_color(rgb(crate::appearance::on_accent()))
                    .hover(|s| s.opacity(0.9))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().font_family(ICONS).text_size(px(12.0)).child("\u{E72E}"))
                    .child(t("Lock"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remember();
                        this.app.update(cx, |app, cx| app.set_desktop_lyrics_locked(true, cx));
                    })),
            )
            .child(control("desk-close", "\u{E8BB}").on_click(cx.listener(|this, _, _, cx| {
                this.remember();
                this.app.update(cx, |app, cx| app.close_desktop_lyrics(cx));
            })))
    }
}

fn keep_inside(wanted: Pixels, start: Pixels, end: Pixels, low: Pixels, high: Pixels) -> Pixels {
    let minimum = low - start;
    wanted.clamp(minimum, (high - end).max(minimum))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_can_reach_both_edges_inside_the_window() {
        assert_eq!(keep_inside(px(-900.0), px(410.0), px(570.0), px(0.0), px(980.0)), px(-410.0));
        assert_eq!(keep_inside(px(900.0), px(410.0), px(570.0), px(0.0), px(980.0)), px(410.0));
    }

    #[test]
    fn text_reaches_the_bottom_when_controls_are_above() {
        assert_eq!(keep_inside(px(900.0), px(72.0), px(124.0), px(36.0), px(260.0)), px(136.0));
        assert_eq!(keep_inside(px(-900.0), px(72.0), px(124.0), px(0.0), px(224.0)), px(-72.0));
    }

    #[test]
    fn backdrops_round_trip_and_cycle() {
        for b in Backdrop::ALL {
            assert_eq!(Backdrop::from_key(b.key()), Some(b));
        }
        assert_eq!(Backdrop::Dark.next(), Backdrop::None);
        for a in Align::ALL {
            assert_eq!(Align::from_key(a.key()), Some(a));
        }
        assert_eq!(Align::Right.next(), Align::Left);
    }

    #[test]
    fn opacity_steps_down_then_back_to_solid() {
        assert_eq!(next_opacity(100), 80);
        assert_eq!(next_opacity(40), 25);
        assert_eq!(next_opacity(25), 100);
        // A value between steps (kept from a hand-edited setting) goes to the next one under it.
        assert_eq!(next_opacity(70), 60);
    }
}
