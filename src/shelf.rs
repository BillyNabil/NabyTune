//! Rows of cards that scroll sideways (Home's shelves, an artist's albums and singles).
//!
//! The mouse wheel scrolls the page, always: a row under the mouse doesn't take it over. A row
//! scrolls sideways by its ‹ › buttons, by dragging it with the mouse, by Shift + wheel, a
//! tilting wheel or a touchpad swiped sideways, and it glides to where it's going rather than
//! jumping there.

use std::time::Instant;

use gpui::{
    canvas, div, prelude::*, px, rgba, AnyElement, Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollHandle,
    ScrollWheelEvent, SharedString, Window,
};

use crate::{NabyTune, ICONS, SECONDARY, TERTIARY};

/// How fast a row closes on where it's going (per second; higher is snappier).
const GLIDE: f32 = 14.0;
/// One notch of Shift + wheel: about one card.
const NOTCH: f32 = 3.0;
/// A press that moves less than this is a click on a card, not a drag of the row.
const DRAG_SLOP: f32 = 6.0;

/// One row's scrolling.
#[derive(Default)]
pub(crate) struct Shelf {
    handle: ScrollHandle,
    /// Where it's gliding to (the offset: 0 at the start, more negative further along).
    target: Option<f32>,
    /// When the glide last stepped.
    stepped: Option<Instant>,
    /// Being dragged: where the mouse went down, and the offset then.
    drag: Option<(f32, f32)>,
    /// The drag's speed (px/s), for the row to coast on when it's let go.
    speed: f32,
    /// The drag's last mouse x and when.
    last: Option<(f32, Instant)>,
}

impl Shelf {
    fn offset(&self) -> f32 {
        f32::from(self.handle.offset().x)
    }

    /// How far it can go (≥ 0).
    fn reach(&self) -> f32 {
        f32::from(self.handle.max_offset().x).max(0.0)
    }

    fn glide_to(&mut self, to: f32) {
        self.target = Some(to.clamp(-self.reach(), 0.0));
        self.stepped = None;
    }

    fn set(&self, at: f32) {
        let at = at.clamp(-self.reach(), 0.0);
        self.handle.set_offset(gpui::point(px(at), px(0.0)));
    }

    /// One frame of the glide; true while there's further to go.
    fn step(&mut self) -> bool {
        let Some(target) = self.target else { return false };
        let now = Instant::now();
        let dt = self.stepped.map(|t| now.duration_since(t).as_secs_f32()).unwrap_or(1.0 / 60.0).min(0.05);
        self.stepped = Some(now);
        let at = self.offset();
        let next = at + (target - at) * (1.0 - (-GLIDE * dt).exp());
        if (target - next).abs() < 0.5 {
            self.set(target);
            self.target = None;
            return false;
        }
        self.set(next);
        true
    }
}

impl NabyTune {
    /// A heading over a row of `cards` that scrolls sideways. `key` names the row (its scroll
    /// is kept by it), so it should change when the row holds something else.
    pub(crate) fn shelf(&mut self, key: SharedString, heading: impl IntoElement, cards: Vec<AnyElement>, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let shelf = self.shelves.entry(key.clone()).or_default();
        if shelf.step() {
            window.request_animation_frame();
        }
        let handle = shelf.handle.clone();
        let reach = shelf.reach();
        let at = shelf.offset();
        let dragging = shelf.drag.is_some();

        // ‹ ›, once the row is wider than the page; dim at the end it's at.
        let arrow = |id: &'static str, glyph: &'static str, live: bool, by: f32, cx: &mut Context<Self>| {
            let key = key.clone();
            div()
                .id(SharedString::from(format!("{key}-{id}")))
                .size(px(30.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .font_family(ICONS)
                .text_size(px(12.0))
                .bg(rgba(0xffff_ff0f))
                .text_color(rgba(if live { SECONDARY } else { TERTIARY }))
                .when(!live, |d| d.opacity(0.4))
                .when(live, |d| d.cursor_pointer().hover(|s| s.bg(rgba(0xffff_ff24)).text_color(rgba(0xffff_ffff))))
                .child(glyph)
                .when(live, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(shelf) = this.shelves.get_mut(&key) {
                            let page = f32::from(shelf.handle.bounds().size.width) * 0.8;
                            let from = shelf.target.unwrap_or(shelf.offset());
                            shelf.glide_to(from + by * page);
                            cx.notify();
                        }
                    }))
                })
        };
        let arrows = (reach > 1.0).then(|| {
            div()
                .flex()
                .gap(px(6.0))
                .child(arrow("back", "\u{E76B}", at < -0.5, 1.0, cx))
                .child(arrow("ahead", "\u{E76C}", at > -reach + 0.5, -1.0, cx))
        });

        let wheel_key = key.clone();
        let down_key = key.clone();
        let mut row = div()
            .id(SharedString::from(format!("{key}-row")))
            .flex()
            .gap_5()
            .pb_2()
            .overflow_hidden()
            .track_scroll(&handle)
            // Sideways wheels (Shift + wheel, a tilting wheel, a touchpad): the row. An up and
            // down wheel is left to the page.
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
                if dx == 0.0 || dx.abs() < dy.abs() {
                    return;
                }
                let Some(shelf) = this.shelves.get_mut(&wheel_key) else { return };
                if event.delta.precise() {
                    // A touchpad already moves smoothly: follow the fingers.
                    shelf.target = None;
                    shelf.set(shelf.offset() + dx);
                } else {
                    let from = shelf.target.unwrap_or(shelf.offset());
                    shelf.glide_to(from + dx * NOTCH);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.shelf_dragged = false;
                    if let Some(shelf) = this.shelves.get_mut(&down_key) {
                        let x = f32::from(event.position.x);
                        shelf.target = None;
                        shelf.drag = Some((x, shelf.offset()));
                        shelf.speed = 0.0;
                        shelf.last = Some((x, Instant::now()));
                    }
                }),
            )
            .children(cards);
        // While it's dragged, the mouse is followed over the whole window (it rarely stays
        // inside the row), and let go anywhere ends the drag.
        if dragging {
            let entity = cx.entity();
            let key = key.clone();
            row = row.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let (moved, up) = (entity.clone(), entity.clone());
                        let (move_key, up_key) = (key.clone(), key.clone());
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble {
                                return;
                            }
                            moved.update(cx, |this, cx| this.drag_shelf(&move_key, f32::from(event.position.x), event.pressed_button == Some(MouseButton::Left), cx));
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble {
                                return;
                            }
                            up.update(cx, |this, cx| this.drag_shelf(&up_key, 0.0, false, cx));
                        });
                    },
                )
                .absolute()
                .size_0(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().flex().items_center().justify_between().gap_4().child(heading).children(arrows))
            .child(row)
    }

    /// The mouse, while a row is held: the row follows it; let go, it coasts to a stop.
    fn drag_shelf(&mut self, key: &SharedString, x: f32, held: bool, cx: &mut Context<Self>) {
        let Some(shelf) = self.shelves.get_mut(key) else { return };
        let Some((from_x, from_offset)) = shelf.drag else { return };
        if !held {
            shelf.drag = None;
            // Held still before letting go: no flick.
            let moving = shelf.last.is_some_and(|(_, then)| then.elapsed().as_secs_f32() < 0.08);
            shelf.last = None;
            if self.shelf_dragged && moving && shelf.speed.abs() > 50.0 {
                // A flick: carry on a little way in that direction.
                let to = shelf.offset() + shelf.speed * 0.22;
                shelf.glide_to(to);
            }
            cx.notify();
            return;
        }
        let moved = x - from_x;
        if !self.shelf_dragged && moved.abs() < DRAG_SLOP {
            return;
        }
        // Past the slop it's a drag, and the press won't open the card it began on.
        self.shelf_dragged = true;
        let now = Instant::now();
        if let Some((last_x, then)) = shelf.last {
            let dt = now.duration_since(then).as_secs_f32();
            if dt > 0.0 {
                let speed = (x - last_x) / dt;
                shelf.speed = shelf.speed * 0.6 + speed * 0.4;
            }
        }
        shelf.last = Some((x, now));
        shelf.set(from_offset + moved);
        cx.notify();
    }
}
