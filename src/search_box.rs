//! The title bar's search field: one line of text typed through the platform's input
//! handler (so IME composition works), with what a text box on Windows is expected to do:
//! selection by Shift+arrows, mouse drag and double-click; Ctrl+A, Ctrl+C/X/V; Ctrl+arrows
//! and Ctrl+Backspace/Delete by word; Home/End. Enter searches, Escape clears (or leaves an
//! empty box), and × clears with the mouse. Long text scrolls to keep the caret in view.

use std::ops::Range;
use std::time::Duration;

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, rgb, rgba, size, App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, ShapedLine, SharedString, Task, TextRun, UTF16Selection, Window,
};

use crate::appearance::accent;

actions!(
    search_box,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Submit,
        Clear
    ]
);

const CONTEXT: &str = "SearchBox";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("shift-backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, Some(CONTEXT)),
        KeyBinding::new("ctrl-delete", DeleteWordRight, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("ctrl-left", WordLeft, Some(CONTEXT)),
        KeyBinding::new("ctrl-right", WordRight, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectHome, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
        KeyBinding::new("ctrl-x", Cut, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        KeyBinding::new("shift-delete", Cut, Some(CONTEXT)),
        KeyBinding::new("shift-insert", Paste, Some(CONTEXT)),
        KeyBinding::new("ctrl-insert", Copy, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
        KeyBinding::new("escape", Clear, Some(CONTEXT)),
    ]);
}

/// Enter pressed with this text.
pub struct Submitted(pub String);

/// Escape on an empty box: the listener moves focus back to the app.
pub struct Dismissed;

pub struct SearchBox {
    text: String,
    /// Byte offsets: the selection, caret at `end` unless `reversed` (caret at `start`).
    selected: Range<usize>,
    reversed: bool,
    /// IME composition in progress.
    marked: Option<Range<usize>>,
    focus: FocusHandle,
    caret_on: bool,
    _blink: Task<()>,
    /// Where the text was last drawn, for mouse hits.
    last_line: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// How far the text is scrolled left to keep the caret in view.
    scroll: Pixels,
    dragging: bool,
    /// Grey text while empty, the field's width, and whether "Ctrl F" shows at its end.
    placeholder: &'static str,
    width: f32,
    shortcut: bool,
}

impl EventEmitter<Submitted> for SearchBox {}
impl EventEmitter<Dismissed> for SearchBox {}

impl Focusable for SearchBox {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Whether a character belongs to a word, for Ctrl+arrows and double-click.
fn wordy(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// The start of the word before byte `from` (spaces and marks just before it skipped).
fn word_start(text: &str, from: usize) -> usize {
    let chars: Vec<(usize, char)> = text[..from].char_indices().collect();
    let mut i = chars.len();
    while i > 0 && !wordy(chars[i - 1].1) {
        i -= 1;
    }
    while i > 0 && wordy(chars[i - 1].1) {
        i -= 1;
    }
    chars.get(i).map_or(from, |c| c.0)
}

/// The end of the word after byte `from`.
fn word_end(text: &str, from: usize) -> usize {
    let rest = &text[from..];
    let skip = rest.char_indices().find(|(_, c)| wordy(*c)).map_or(rest.len(), |(i, _)| i);
    let word = rest[skip..].char_indices().find(|(_, c)| !wordy(*c)).map_or(rest.len() - skip, |(i, _)| i);
    from + skip + word
}

impl SearchBox {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            text: String::new(),
            selected: 0..0,
            reversed: false,
            marked: None,
            focus: cx.focus_handle(),
            caret_on: false,
            _blink: Task::ready(()),
            last_line: None,
            last_bounds: None,
            scroll: px(0.0),
            dragging: false,
            placeholder: "Search songs, artists, albums",
            width: 420.0,
            shortcut: true,
        }
    }

    /// A plain text field: `placeholder` while empty, `width` wide, no shortcut hint.
    pub fn field(placeholder: &'static str, width: f32, cx: &mut Context<Self>) -> Self {
        Self { placeholder, width, shortcut: false, ..Self::new(cx) }
    }

    /// What's typed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Puts `text` in, as if typed, without taking the keyboard.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.marked = None;
        self.selected = self.text.len()..self.text.len();
        self.reversed = false;
    }

    /// Empties the field.
    pub fn clear_text(&mut self, cx: &mut Context<Self>) {
        self.text.clear();
        self.marked = None;
        self.move_to(0, cx);
    }

    /// Puts `text` in, all of it selected (ready to type over), and takes the keyboard.
    pub fn edit(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.text = text.to_string();
        self.selected = 0..self.text.len();
        self.reversed = false;
        self.focus.focus(window, cx);
        self.blink(cx);
    }

    /// The caret's byte offset.
    fn caret(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn blink(&mut self, cx: &mut Context<Self>) {
        self.caret_on = true;
        self._blink = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(530)).await;
            if this
                .update(cx, |b, cx| {
                    b.caret_on = !b.caret_on;
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        });
        cx.notify();
    }

    fn move_to(&mut self, at: usize, cx: &mut Context<Self>) {
        self.selected = at..at;
        self.reversed = false;
        self.blink(cx);
    }

    /// Moves the caret to `at`, keeping the other end of the selection where it was.
    fn select_to(&mut self, at: usize, cx: &mut Context<Self>) {
        let anchor = if self.reversed { self.selected.end } else { self.selected.start };
        if at < anchor {
            self.selected = at..anchor;
            self.reversed = true;
        } else {
            self.selected = anchor..at;
            self.reversed = false;
        }
        self.blink(cx);
    }

    fn prev(&self, from: usize) -> usize {
        self.text[..from].char_indices().last().map_or(0, |(i, _)| i)
    }
    fn next(&self, from: usize) -> usize {
        self.text[from..].chars().next().map_or(from, |c| from + c.len_utf8())
    }
    fn word_start(&self, from: usize) -> usize {
        word_start(&self.text, from)
    }
    fn word_end(&self, from: usize) -> usize {
        word_end(&self.text, from)
    }

    /// Replaces the selection (or the range given) with `new`, caret after it.
    fn replace(&mut self, range: Range<usize>, new: &str, cx: &mut Context<Self>) {
        // One line only.
        let new = new.replace(['\n', '\r'], " ");
        self.text.replace_range(range.clone(), &new);
        self.move_to(range.start + new.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let start = self.prev(self.caret());
            self.replace(start..self.caret(), "", cx);
        } else {
            self.replace(self.selected.clone(), "", cx);
        }
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let end = self.next(self.caret());
            self.replace(self.caret()..end, "", cx);
        } else {
            self.replace(self.selected.clone(), "", cx);
        }
    }
    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() { self.word_start(self.caret())..self.caret() } else { self.selected.clone() };
        self.replace(range, "", cx);
    }
    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() { self.caret()..self.word_end(self.caret()) } else { self.selected.clone() };
        self.replace(range, "", cx);
    }
    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        // A selection collapses to its left end, as in any Windows text box.
        let at = if self.selected.is_empty() { self.prev(self.caret()) } else { self.selected.start };
        self.move_to(at, cx);
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selected.is_empty() { self.next(self.caret()) } else { self.selected.end };
        self.move_to(at, cx);
    }
    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.word_start(self.caret()), cx);
    }
    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.word_end(self.caret()), cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.prev(self.caret()), cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next(self.caret()), cx);
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.word_start(self.caret()), cx);
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.word_end(self.caret()), cx);
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.text.len(), cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.text.len();
        self.reversed = false;
        self.blink(cx);
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.selected.clone()].to_string()));
        }
    }
    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.selected.clone()].to_string()));
            self.replace(self.selected.clone(), "", cx);
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace(self.selected.clone(), text.trim(), cx);
        }
    }
    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        let query = self.text.trim().to_string();
        if !query.is_empty() {
            cx.emit(Submitted(query));
        }
    }
    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        if self.text.is_empty() {
            cx.emit(Dismissed);
        }
        self.text.clear();
        self.marked = None;
        self.move_to(0, cx);
    }

    /// The byte offset under a point in the window.
    fn index_at(&self, position: gpui::Point<Pixels>) -> usize {
        match (&self.last_line, self.last_bounds) {
            (Some(line), Some(bounds)) if !self.text.is_empty() => line.closest_index_for_x(position.x - bounds.left() + self.scroll).min(self.text.len()),
            _ => 0,
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let at = self.index_at(event.position);
        match event.click_count {
            // Double click: the word under the mouse; triple: everything.
            2 => {
                let start = self.word_start(self.next(at).min(self.text.len()));
                let end = self.word_end(start);
                self.selected = start..end.max(start);
                self.reversed = false;
                self.blink(cx);
            }
            n if n >= 3 => {
                self.selected = 0..self.text.len();
                self.reversed = false;
                self.blink(cx);
            }
            _ if event.modifiers.shift => self.select_to(at, cx),
            _ => {
                self.dragging = true;
                self.move_to(at, cx);
            }
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.dragging && event.pressed_button == Some(MouseButton::Left) {
            let at = self.index_at(event.position);
            self.select_to(at, cx);
        } else {
            self.dragging = false;
        }
    }
}

fn to_utf16(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().map(char::len_utf16).sum()
}
fn from_utf16(s: &str, units: usize) -> usize {
    let mut count = 0;
    for (i, c) in s.char_indices() {
        if count >= units {
            return i;
        }
        count += c.len_utf16();
    }
    s.len()
}

impl EntityInputHandler for SearchBox {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let r = from_utf16(&self.text, range.start)..from_utf16(&self.text, range.end);
        actual.replace(to_utf16(&self.text, r.start)..to_utf16(&self.text, r.end));
        Some(self.text[r].to_string())
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: to_utf16(&self.text, self.selected.start)..to_utf16(&self.text, self.selected.end), reversed: self.reversed })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| to_utf16(&self.text, r.start)..to_utf16(&self.text, r.end))
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, new: &str, _: &mut Window, cx: &mut Context<Self>) {
        let r = range
            .map(|r| from_utf16(&self.text, r.start)..from_utf16(&self.text, r.end))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        self.marked = None;
        self.replace(r, new, cx);
    }
    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, new: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let r = range
            .map(|r| from_utf16(&self.text, r.start)..from_utf16(&self.text, r.end))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let new = new.replace(['\n', '\r'], " ");
        self.text.replace_range(r.clone(), &new);
        self.marked = (!new.is_empty()).then(|| r.start..r.start + new.len());
        self.move_to(r.start + new.len(), cx);
    }
    fn bounds_for_range(&mut self, range: Range<usize>, bounds: Bounds<Pixels>, window: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        // Where the IME's candidate window goes: under the text being composed.
        let line = self.last_line.as_ref()?;
        let start = from_utf16(&self.text, range.start);
        let x = bounds.left() + line.x_for_index(start) - self.scroll;
        Some(Bounds::new(point(x, bounds.top()), size(px(2.0), window.line_height())))
    }
    fn character_index_for_point(&mut self, point: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(to_utf16(&self.text, self.index_at(point)))
    }
}

impl Render for SearchBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let has_text = !self.text.is_empty();
        div()
            .id("search")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::clear))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .w(px(self.width))
            .h(px(32.0))
            .pl_3()
            .pr(px(6.0))
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(8.0))
            .bg(if focused { rgba(0xffffff1f) } else { rgba(0xffffff12) })
            .border_1()
            .border_color(if focused { rgba((accent() << 8) | 0x99) } else { rgba(0xffffff14) })
            .text_size(px(13.0))
            .line_height(px(18.0))
            .when(self.shortcut, |d| d.child(div().font_family(crate::ICONS).text_size(px(13.0)).text_color(rgba(0xffffff99)).child("\u{E721}")))
            .child(div().flex_1().min_w_0().h(px(18.0)).child(Field { box_: cx.entity() }))
            .map(|d| {
                if has_text {
                    // × clears the box, as search fields on Windows do.
                    d.child(
                        div()
                            .id("search-clear")
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .font_family(crate::ICONS)
                            .text_size(px(10.0))
                            .text_color(rgba(0xffffff99))
                            .hover(|s| s.bg(rgba(0xffffff1a)).text_color(rgb(0xffffff)))
                            .child("\u{E711}")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.text.clear();
                                this.marked = None;
                                this.focus.focus(window, cx);
                                this.move_to(0, cx);
                            })),
                    )
                } else if self.shortcut {
                    d.child(
                        div()
                            .px_1p5()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgba(0xffffff1f))
                            .text_size(px(11.0))
                            .text_color(rgba(0xffffff80))
                            .child("Ctrl F"),
                    )
                } else {
                    d
                }
            })
    }
}

/// The text, selection and caret, painted directly, with the window's input handler attached.
struct Field {
    box_: Entity<SearchBox>,
}

impl IntoElement for Field {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct Painted {
    line: ShapedLine,
    selection: Option<PaintQuad>,
    caret: Option<PaintQuad>,
    scroll: Pixels,
}

impl Element for Field {
    type RequestLayoutState = ();
    type PrepaintState = Painted;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&gpui::GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Painted {
        let b = self.box_.read(cx);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let empty = b.text.is_empty();
        let (text, color): (SharedString, _) =
            if empty { (crate::i18n::t(b.placeholder).into(), gpui::hsla(0., 0., 1., 0.45)) } else { (b.text.clone().into(), style.color) };
        let run = |len: usize, underline: bool| TextRun {
            len,
            font: style.font(),
            color,
            background_color: None,
            underline: underline.then(|| gpui::UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
            strikethrough: None,
            letter_spacing: None,
        };
        // Text being composed (IME) is underlined.
        let runs = match (&b.marked, empty) {
            (Some(m), false) if m.end <= text.len() => [run(m.start, false), run(m.end - m.start, true), run(text.len() - m.end, false)]
                .into_iter()
                .filter(|r| r.len > 0)
                .collect(),
            _ => vec![run(text.len(), false)],
        };
        let line = window.text_system().shape_line(text, font_size, &runs, None);
        let focused = b.focus.is_focused(window);
        let caret_x = if empty { px(0.) } else { line.x_for_index(b.caret()) };
        // Keep the caret inside the field.
        let width = bounds.size.width;
        let mut scroll = b.scroll;
        if caret_x - scroll > width - px(2.0) {
            scroll = caret_x - width + px(2.0);
        } else if caret_x < scroll {
            scroll = caret_x;
        }
        if line.width <= width {
            scroll = px(0.0);
        }
        let selection = (focused && !b.selected.is_empty() && !empty).then(|| {
            let (a, z) = (line.x_for_index(b.selected.start), line.x_for_index(b.selected.end));
            fill(
                Bounds::new(point(bounds.left() + a - scroll, bounds.top()), size(z - a, window.line_height())),
                rgba((accent() << 8) | 0x55),
            )
        });
        let caret = (focused && b.caret_on && b.selected.is_empty())
            .then(|| fill(Bounds::new(point(bounds.left() + caret_x - scroll, bounds.top()), size(px(1.5), window.line_height())), rgb(accent())));
        let keep = line.clone();
        self.box_.update(cx, |b, _| {
            b.last_line = (!empty).then_some(keep);
            b.last_bounds = Some(bounds);
            b.scroll = scroll;
        });
        Painted { line, selection, caret, scroll }
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        painted: &mut Painted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.box_.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.box_.clone()), cx);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = painted.selection.take() {
                window.paint_quad(selection);
            }
            let origin = point(bounds.left() - painted.scroll, bounds.top());
            let _ = painted.line.paint(origin, window.line_height(), gpui::TextAlign::Left, None, window, cx);
            if let Some(caret) = painted.caret.take() {
                window.paint_quad(caret);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_word_edges() {
        let text = "lemon  kenshi yonezu";
        assert_eq!(word_start(text, 20), 14);
        assert_eq!(word_start(text, 14), 7);
        assert_eq!(word_start(text, 7), 0);
        assert_eq!(word_start(text, 0), 0);
        assert_eq!(word_end(text, 0), 5);
        assert_eq!(word_end(text, 5), 13);
        assert_eq!(word_end(text, 20), 20);
        // Japanese counts as word characters too.
        assert_eq!(word_end("夢 ならば", 0), "夢".len());
    }
}
