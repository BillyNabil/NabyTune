//! Lyrics on screen: finding them, their romaji, which line is sung, and the views that
//! show them (the scrolling column, the single line, plain text) with the look menu.

use super::*;

impl NabyTune {
    /// The lyrics, once the track's length is known (the exact matches need it): every source
    /// at once. The one picked for this song before is shown, else the best.
    pub(crate) fn fetch_lyrics(&mut self, generation: u64, video_id: String, title: String, artist: String, length: Duration, cx: &mut Context<Self>) {
        let youtube = self.youtube.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    // Found before: from disk, at once. Otherwise asked for, and kept.
                    if let Some(found) = lyrics::cached(&video_id) {
                        return found;
                    }
                    let found = youtube.lyrics(&video_id, &title, &artist, length);
                    lyrics::keep(&video_id, &found);
                    found
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.generation != generation {
                    return;
                }
                app.lyric_options = found.into_iter().filter(|f| !f.lines.is_empty()).collect();
                // The source picked for this song, else the preferred one (Settings), else the best.
                let key = app.current().map(|t| format!("lyrics_source.{}", t.video_id)).unwrap_or_default();
                let position = |name: &str| app.lyric_options.iter().position(|f| f.source == name);
                let chosen = settings::get(&key).and_then(|name| position(&name)).or_else(|| position(&app.preferred_source)).unwrap_or(0);
                app.show_lyrics(chosen, cx);
            });
        })
        .detach();
    }

    /// Shows version `i` of the song's lyrics (from `lyric_options`), from where the song is.
    pub(crate) fn show_lyrics(&mut self, i: usize, cx: &mut Context<Self>) {
        self.extras = LyricExtras::default();
        self.lyrics = match self.lyric_options.get(i) {
            Some(found) => {
                self.lyric_motion = LyricMotion::new(found.lines.len());
                self.lyrics_source = found.source.into();
                self.lyrics_synced = found.synced;
                Lyrics::Ready(found.lines.clone())
            }
            None => Lyrics::Missing,
        };
        self.fetch_extras(cx);
        cx.notify();
    }

    /// The listener picked another source for this song's lyrics: show it, and keep to it
    /// next time the song plays.
    pub(crate) fn pick_lyrics(&mut self, i: usize, cx: &mut Context<Self>) {
        if let (Some(track), Some(found)) = (self.current(), self.lyric_options.get(i)) {
            settings::set(&format!("lyrics_source.{}", track.video_id), found.source);
        }
        self.show_lyrics(i, cx);
    }

    /// Romanisation of the lyrics, worked out once per song and source when switched on, and
    /// kept: playing the song again redoes nothing.
    pub(crate) fn fetch_extras(&mut self, cx: &mut Context<Self>) {
        let Lyrics::Ready(lines) = &self.lyrics else { return };
        let texts: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
        let generation = self.generation;
        let source = self.lyrics_source.clone();
        let key = self.current().map(|t| format!("{}:{}", t.video_id, source)).unwrap_or_default();
        if self.extras.romaji.is_none() {
            self.extras.romaji = self.extras_cache.get(&key).cloned();
        }
        // Worked out for the lyrics view, or for the mini player when it shows romaji.
        let wanted = self.show_romaji || (self.mini.is_some() && self.mini_romaji.wants_romaji());
        if wanted && self.extras.romaji.is_none() && !self.extras.romaji_pending {
            self.extras.romaji_pending = true;
            self.extras.note = None;
            cx.spawn(async move |this, cx| {
                let result = cx.background_executor().spawn(async move { romaji::romanize(&texts) }).await;
                let _ = this.update(cx, |app, cx| {
                    // Another song, or another source's lines, by now: keep it for later only.
                    if app.generation != generation || app.lyrics_source != source {
                        if let Ok(romaji) = result {
                            app.extras_cache.insert(key, romaji);
                        }
                        return;
                    }
                    app.extras.romaji_pending = false;
                    match result {
                        Ok(romaji) => {
                            app.extras_cache.insert(key, romaji.clone());
                            app.extras.romaji = Some(romaji);
                        }
                        Err(e) => app.extras.note = Some(short_error("Romaji", &e)),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// What shows under each lyric line: its romaji, when that's switched on.
    pub(crate) fn under_lines(&self) -> Option<Vec<Option<String>>> {
        self.extras.romaji.clone().filter(|_| self.show_romaji)
    }

    /// The lyric line being sung, a touch early as karaoke does: it lights as it begins.
    pub(crate) fn sung_line(&self) -> Option<usize> {
        let Lyrics::Ready(lines) = &self.lyrics else { return None };
        // Text without timings has no line being sung.
        if !self.lyrics_synced {
            return None;
        }
        let now = self.lyric_now() + Duration::from_millis(150);
        lines.iter().rposition(|l| l.at <= now)
    }

    /// Lyrics show in the full-screen player, or in the side panel when that's open instead.
    pub(crate) fn lyrics_on_screen(&self) -> bool {
        self.full_screen || self.panel == Panel::Lyrics
    }

    // ---- Side panel -------------------------------------------------------------------

    /// Synced lyrics in the chosen look. The multi-line looks glide on a spring to keep the
    /// sung line at `style.anchor` of the view; a line seeks there when clicked.
    pub(crate) fn lyric_column(&mut self, style: &LyricStyle, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let message = |text: SharedString| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px_6()
                .text_center()
                .text_size(px(14.0))
                .text_color(rgba(SECONDARY))
                .child(text)
                .into_any_element()
        };
        let lines = match &self.lyrics {
            Lyrics::Ready(lines) => lines.clone(),
            Lyrics::Loading => return message(t("Finding lyrics…").into()),
            Lyrics::Missing => return message(t("No lyrics found for this song").into()),
            Lyrics::Idle => return message(t("Play a song to see its lyrics").into()),
        };
        let romaji = self.under_lines();
        if !self.lyrics_synced {
            return self.plain_lyrics(style, &lines, romaji);
        }
        if self.look == Look::Single {
            return self.single_line(style, &lines, window, cx);
        }

        let look = self.look;
        let blur_on = self.blur_lyrics && look.blurs();
        let now = self.lyric_now();
        let active = self.sung_line();
        // Per look: how dim the other lines are, their weight, a size factor for all lines,
        // and how much bigger the sung one grows.
        let (dim, weight, size, grow) = match look {
            Look::Focus => (0.3, FontWeight::BOLD, 1.0, 1.0),
            Look::Karaoke => (0.35, FontWeight::BOLD, 1.0, 1.0),
            Look::Center => (0.32, FontWeight::BOLD, 0.92, 1.14),
            Look::Minimal => (0.26, FontWeight::MEDIUM, 0.78, 1.0),
            Look::Neon => (0.28, FontWeight::BOLD, 1.0, 1.06),
            Look::Single => unreachable!(),
        };
        let motion = &mut self.lyric_motion;
        let viewport = motion.viewport.get();
        let target = {
            let layout = motion.layout.borrow();
            let offset = match active.and_then(|i| layout.get(i)) {
                Some((top, height)) => viewport * style.anchor - (top + height / 2.0),
                None => viewport * style.anchor - layout.first().map_or(0.0, |(_, h)| h / 2.0),
            };
            // Whole pixels: a target that wobbles by fractions would keep the spring awake.
            offset.round()
        };
        // At rest within a pixel: the default threshold is so fine the spring's tail would
        // outlive the gap between lines, and every frame of the glide redraws the blurred lines
        // at the display's rate (120 a second on many screens).
        let glide = Spring::visual(Duration::from_millis(480), 0.0).and_then(|s| s.rest(1.0, 8.0)).expect("valid spring");
        let scroll = motion.scroll.frame(Motion::new(Target::default().y(target)).transition(Transition::Spring(glide)), window, cx);

        // Lines fade out towards the view's top and bottom edges rather than being cut there:
        // where each line sits now (the scroll as drawn this frame, plus its measured place).
        let scrolled = scroll.target().y_px();
        let geometry: Vec<(f32, f32)> = motion.layout.borrow().clone();
        let fade_zone = (viewport * 0.16).clamp(48.0, 130.0);
        let edge_fade = |i: usize| -> f32 {
            if viewport <= 0.0 {
                return 1.0;
            }
            geometry.get(i).map_or(1.0, |(top, height)| {
                let centre = scrolled + top + height / 2.0;
                (centre.min(viewport - centre) / fade_zone).clamp(0.0, 1.0)
            })
        };

        let fade = Transition::Tween(Duration::from_millis(260), Easing::EaseOut);
        let mut column = div().flex().flex_col().gap(px(style.gap * size)).px(px(style.pad));
        let mut easing = false;
        for (i, line) in lines.iter().enumerate() {
            let lit = Some(i) == active;
            let opacity = motion.lines.get_mut(i).map(|state| {
                state.frame(Motion::new(Target::default().opacity(if lit { 1.0 } else { dim })).transition(fade), window, cx)
            });
            // Out of focus by distance from the sung line: the next line is barely soft,
            // the far ones properly blurred. Before the first line, all of them are.
            let distance = active.map_or(i + 1, |a| i.abs_diff(a));
            let blur = motion.blur.get_mut(i).map(|tween| {
                // Switched off, every line eases back into focus rather than snapping.
                tween.aim(if blur_on { (distance as f32 * style.blur_step).min(style.blur_max) } else { 0.0 });
                easing |= !tween.settled();
                tween.value()
            });
            let scale = motion.scale.get_mut(i).map_or(1.0, |tween| {
                tween.aim(if lit { grow } else { 1.0 });
                easing |= !tween.settled();
                tween.value()
            });
            let text_size = style.text * size * scale;
            let at = line.at;
            let text = if line.text.is_empty() { "♪".to_string() } else { line.text.clone() };
            let row = div()
                // The filter only where it shows: an unblurred line, or one far off screen,
                // skips the offscreen pass a blur costs.
                .when_some(blur.filter(|b| *b > 0.1 && distance <= BLUR_REACH), |d, b| d.blur(px(b)))
                .id(("lyric", i))
                .flex()
                .flex_col()
                .when(look == Look::Center, |d| d.items_center())
                .cursor_pointer()
                .font_family(DISPLAY)
                .text_size(px(text_size))
                .line_height(px(style.line_height * size * scale))
                .font_weight(weight)
                .when(look == Look::Center, |d| d.text_center())
                .when(look == Look::Neon && lit, |d| d.text_color(rgb(accent())))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.length > Duration::ZERO {
                        this.seek(at.as_secs_f32() / this.length.as_secs_f32());
                    }
                    cx.notify();
                }));
            let row = if look == Look::Karaoke && lit && !line.words.is_empty() {
                // Each word lights as it's sung, by the source's own word timings.
                let mut spans = div().flex().flex_wrap();
                for word in &line.words {
                    let span = word.end.saturating_sub(word.at).max(Duration::from_millis(60));
                    let lit_part = (now.saturating_sub(word.at).as_secs_f32() / span.as_secs_f32()).clamp(0.0, 1.0);
                    let lit_part = if now < word.at { 0.0 } else { lit_part };
                    spans = spans.child(
                        div()
                            .when(word.gap, |d| d.mr(px(text_size * 0.28)))
                            .text_color(gpui::hsla(0.0, 0.0, 1.0, 0.35 + 0.65 * lit_part))
                            .child(word.text.clone()),
                    );
                }
                row.child(spans)
            } else if look == Look::Karaoke && lit && !line.text.is_empty() {
                // Whole-line timings only: each word lights across its share of the line's time,
                // by length. Japanese and Chinese, without spaces, go character by character.
                let end = lines.get(i + 1).map_or(at + Duration::from_secs(5), |next| next.at);
                let span = end.saturating_sub(at).max(Duration::from_millis(800)).mul_f32(0.9);
                let progress = (now.saturating_sub(at).as_secs_f32() / span.as_secs_f32()).clamp(0.0, 1.0);
                let mut units: Vec<(String, bool)> = Vec::new();
                for word in line.text.split_whitespace() {
                    if word.chars().any(|c| (c as u32) >= 0x2E80) {
                        let chars: Vec<char> = word.chars().collect();
                        units.extend(chars.iter().enumerate().map(|(n, c)| (c.to_string(), n + 1 == chars.len())));
                    } else {
                        units.push((word.to_string(), true));
                    }
                }
                let total: usize = units.iter().map(|(w, _)| w.chars().count()).sum::<usize>().max(1);
                let mut start = 0usize;
                let mut spans = div().flex().flex_wrap();
                for (word, gap) in units {
                    let len = word.chars().count();
                    let from = start as f32 / total as f32;
                    let share = len as f32 / total as f32;
                    start += len;
                    let lit_part = ((progress - from) / share.max(0.0001)).clamp(0.0, 1.0);
                    spans = spans.child(
                        div()
                            .when(gap, |d| d.mr(px(text_size * 0.28)))
                            .text_color(gpui::hsla(0.0, 0.0, 1.0, 0.35 + 0.65 * lit_part))
                            .child(word),
                    );
                }
                row.child(spans)
            } else {
                row.child(text)
            };
            let row = row.children(extras_under(&romaji, i, text_size, look == Look::Center));
            let row = match opacity {
                Some(f) => f.apply(row).into_any_element(),
                None => row.into_any_element(),
            };
            // The edge fade wraps the line, so it multiplies with the line's own dimming.
            column = column.child(div().opacity(edge_fade(i)).child(row));
        }
        if easing {
            window.request_animation_frame();
        }
        let layout = motion.layout.clone();
        let column = column.on_children_prepainted(move |bounds, _, _| {
            let first = bounds.first().map_or(0.0, |b| f32::from(b.origin.y));
            *layout.borrow_mut() = bounds.iter().map(|b| (f32::from(b.origin.y) - first, f32::from(b.size.height))).collect();
        });
        let viewport_cell = motion.viewport.clone();
        div()
            .size_full()
            .overflow_hidden()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(b) = bounds.first() {
                    viewport_cell.set(f32::from(b.size.height));
                }
            })
            .child(div().size_full().overflow_hidden().child(scroll.apply(column)))
            .into_any_element()
    }

    /// The sung line on its own, for the layouts that show lyrics as a subtitle.
    pub(crate) fn subtitle_line(&mut self, style: &LyricStyle, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let note = |text: &'static str| {
            div().size_full().flex().items_center().justify_center().text_size(px(13.0)).text_color(rgba(TERTIARY)).child(text).into_any_element()
        };
        match &self.lyrics {
            Lyrics::Ready(lines) => {
                let lines = lines.clone();
                self.single_line(style, &lines, window, cx)
            }
            Lyrics::Loading => note(t("Finding lyrics…")),
            _ => note(""),
        }
    }

    /// Lyrics without timings (YouTube Music's own text): every line plain, scrolled by hand.
    pub(crate) fn plain_lyrics(&self, style: &LyricStyle, lines: &[lyrics::Line], romaji: Option<Vec<Option<String>>>) -> AnyElement {
        let text = style.text * 0.8;
        let mut column = div().flex().flex_col().gap(px(style.gap * 0.7)).px(px(style.pad)).pb(px(40.0));
        column = column.child(
            div()
                .text_size(px(12.0))
                .text_color(rgba(TERTIARY))
                .child(tf("Lyrics from {} · not time-synced", &[&self.lyrics_source])),
        );
        for (i, line) in lines.iter().enumerate() {
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .font_family(DISPLAY)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(text))
                    .line_height(px(text * 1.3))
                    .text_color(rgba(0xffffffd9))
                    .child(line.text.clone())
                    .children(extras_under(&romaji, i, text, false)),
            );
        }
        div().id("plain-lyrics").size_full().overflow_y_scroll().child(column).into_any_element()
    }

    /// The single-line look: the sung line large in the middle, rising in as it changes,
    /// with the next one waiting small underneath.
    pub(crate) fn single_line(&mut self, style: &LyricStyle, lines: &[lyrics::Line], window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let romaji = self.under_lines();
        let active = self.sung_line();
        let spring = self.spring;
        let motion = &mut self.lyric_motion;
        if motion.single_line != active {
            motion.single_line = active;
            motion.single = MotionState::default();
        }
        let frame = motion.single.frame(
            Motion::new(Target::default().y(0.0).opacity(1.0))
                .initial(Target::default().y(30.0).opacity(0.0))
                .transition(Transition::Spring(spring)),
            window,
            cx,
        );
        let current = active.map(|i| lines[i].text.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "♪".to_string());
        let upcoming = match active {
            Some(i) => lines.get(i + 1),
            None => lines.first(),
        }
        .map(|l| l.text.clone())
        .filter(|t| !t.is_empty());
        let big = style.text * 1.35;
        let at = active.map(|i| lines[i].at);
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(big * 0.7))
            .px(px(32.0))
            .text_center()
            .child(
                frame.apply(
                    div()
                        .id("single-line")
                        .cursor_pointer()
                        .font_family(DISPLAY)
                        .font_weight(FontWeight::BOLD)
                        .flex()
                        .flex_col()
                        .items_center()
                        .text_size(px(big))
                        .line_height(px(big * 1.25))
                        .child(current)
                        .children(active.and_then(|i| extras_under(&romaji, i, big, true)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(at) = at.filter(|_| this.length > Duration::ZERO) {
                                this.seek(at.as_secs_f32() / this.length.as_secs_f32());
                            }
                            cx.notify();
                        })),
                ),
            )
            .when_some(upcoming, |d, next| {
                d.child(
                    div()
                        .font_family(DISPLAY)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(style.text * 0.62))
                        .text_color(rgba(TERTIARY))
                        .child(next),
                )
            })
            .into_any_element()
    }

    /// "Aa" in the side panel's header: opens the look menu below it.
    pub(crate) fn look_button(&self, anchor: LookAnchor, cx: &mut Context<Self>) -> AnyElement {
        let open = self.look_menu == Some(anchor);
        let id = match anchor {
            LookAnchor::Stage => "look-stage",
            LookAnchor::Panel => "look-panel",
        };
        div()
            .relative()
            .child(
                div()
                    .id(id)
                    .size(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .font_family(DISPLAY)
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if open { rgb(0xffffff) } else { rgba(SECONDARY) })
                    .when(open, |d| d.bg(rgba(SUBTLE_STRONG)))
                    .hover(|s| s.bg(rgba(SUBTLE_STRONG)).text_color(rgb(0xffffff)))
                    .child("Aa")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // The mouse-down that closed the menu is this click's own: leave it closed.
                        if this.look_menu_closed.elapsed() > Duration::from_millis(300) {
                            this.look_menu = if this.look_menu == Some(anchor) { None } else { Some(anchor) };
                        }
                        cx.notify();
                    })),
            )
            .when(open, |d| d.child(deferred(div().absolute().top(px(36.0)).right_0().child(self.look_menu_card(cx))).with_priority(1)))
            .into_any_element()
    }

    /// The look menu: six looks with a line each, then a small switch for blur.
    pub(crate) fn look_menu_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut card = div()
            .id("look-menu")
            .w(px(240.0))
            .p(px(6.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .rounded(px(12.0))
            .bg(rgba(GLASS))
            .backdrop_blur(px(24.0))
            .border_1()
            .border_color(rgba(STROKE))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.look_menu = None;
                this.look_menu_closed = Instant::now();
                cx.notify();
            }))
            .child(
                div()
                    .px(px(10.0))
                    .pt(px(6.0))
                    .pb(px(4.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgba(TERTIARY))
                    .child(t("LYRIC STYLE")),
            );
        for (n, look) in Look::ALL.into_iter().enumerate() {
            let selected = look == self.look;
            card = card.child(
                div()
                    .id(("look", n))
                    .h(px(42.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(SUBTLE)))
                    .when(selected, |d| d.bg(rgba(SUBTLE)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if selected { rgb(0xffffff) } else { rgba(0xffffffcc) })
                                    .child(look.name()),
                            )
                            .child(div().text_size(px(11.0)).text_color(rgba(TERTIARY)).truncate().child(look.hint())),
                    )
                    .child(div().size(px(6.0)).rounded_full().when(selected, |d| d.bg(rgb(accent()))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.look = look;
                        settings::set("lyric_look", look.key());
                        this.look_menu = None;
                        this.look_menu_closed = Instant::now();
                        cx.notify();
                    })),
            );
        }
        // Small switches: the knob slides right and the track lights when on.
        let switch_row = |id: &'static str, label: &'static str, on: bool, dimmed: bool| {
            div()
                .id(id)
                .h(px(34.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(|s| s.bg(rgba(SUBTLE)))
                .when(dimmed, |d| d.opacity(0.4))
                .child(div().text_size(px(13.0)).text_color(rgba(0xffffffcc)).child(label))
                .child(
                    div()
                        .w(px(30.0))
                        .h(px(18.0))
                        .p(px(2.0))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .map(|d| if on { d.justify_end().bg(rgb(accent())) } else { d.justify_start().bg(rgba(0xffffff33)) })
                        .child(div().size(px(14.0)).rounded_full().bg(if on { rgb(on_accent()) } else { rgb(0xffffff) })),
                )
        };
        // Romaji has its own button on the full-screen player's rail (stage_rail); the side
        // panel, too narrow for it, keeps it here.
        let in_panel = self.look_menu == Some(LookAnchor::Panel);
        let footnote = self.extras.note.clone();
        // Every version of the lyrics found, to pick another when the shown one is off.
        let mut sources = div().flex().flex_col().gap(px(1.0));
        for (n, found) in self.lyric_options.iter().enumerate() {
            let selected = found.source == self.lyrics_source.as_ref();
            let kind = if found.lines.iter().any(|l| !l.words.is_empty()) {
                t("Word by word")
            } else if found.synced {
                t("Synced")
            } else {
                t("Not synced")
            };
            sources = sources.child(
                div()
                    .id(("lyric-source", n))
                    .h(px(34.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .text_color(if selected { rgb(0xffffff) } else { rgba(0xffffffcc) })
                    .when(selected, |d| d.bg(rgba(SUBTLE)).font_weight(FontWeight::SEMIBOLD))
                    .hover(|s| s.bg(rgba(SUBTLE)))
                    .child(div().flex_1().child(found.source))
                    .child(div().text_size(px(11.0)).font_weight(FontWeight::NORMAL).text_color(rgba(TERTIARY)).child(kind))
                    .child(div().size(px(6.0)).rounded_full().when(selected, |d| d.bg(rgb(accent()))))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_lyrics(n, cx))),
            );
        }
        let has_sources = !self.lyric_options.is_empty();
        card.child(div().my(px(4.0)).mx(px(6.0)).h(px(1.0)).bg(rgba(STROKE)))
            .child(switch_row("look-blur", t("Blur other lines"), self.blur_lyrics, !self.look.blurs()).on_click(cx.listener(|this, _, _, cx| {
                this.blur_lyrics = !this.blur_lyrics;
                settings::set("blur_lyrics", if this.blur_lyrics { "1" } else { "0" });
                cx.notify();
            })))
            .when(in_panel, |d| {
                d.child(switch_row("look-romaji", t("Romaji"), self.show_romaji, false).on_click(cx.listener(|this, _, _, cx| {
                    this.show_romaji = !this.show_romaji;
                    settings::set("lyrics_romaji", if this.show_romaji { "1" } else { "0" });
                    this.fetch_extras(cx);
                    cx.notify();
                })))
            })
            .when(has_sources, |d| {
                d.child(div().my(px(4.0)).mx(px(6.0)).h(px(1.0)).bg(rgba(STROKE)))
                    .child(
                        div()
                            .px(px(10.0))
                            .pt(px(6.0))
                            .pb(px(4.0))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgba(TERTIARY))
                            .child(t("LYRICS SOURCE")),
                    )
                    .child(sources)
            })
            // Timing: when the lyrics run ahead or behind, nudge them (kept for this song).
            .when(self.lyrics_synced && matches!(self.lyrics, Lyrics::Ready(_)), |d| {
                let nudge = |id: &'static str, glyph: &'static str, by: i64| {
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
                        .hover(|s| s.bg(rgba(SUBTLE)))
                        .child(glyph)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.nudge_lyrics(by);
                            cx.notify();
                        }))
                };
                d.child(div().my(px(4.0)).mx(px(6.0)).h(px(1.0)).bg(rgba(STROKE))).child(
                    div()
                        .h(px(34.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(13.0))
                        .child(div().flex_1().child(t("Timing")))
                        .child(nudge("timing-minus", "\u{E738}", -250))
                        .child(div().w(px(54.0)).text_center().text_size(px(12.0)).text_color(rgba(SECONDARY)).child(format!("{:+.2} s", self.lyric_offset as f32 / 1000.0)))
                        .child(nudge("timing-plus", "\u{E710}", 250)),
                )
            })
            .when_some(footnote, |d, note| d.child(div().px(px(10.0)).pt(px(4.0)).pb(px(2.0)).text_size(px(11.0)).text_color(rgba(TERTIARY)).child(note)))
    }
}

/// Under lyric line `i`: its romanisation, when asked for and there.
pub(crate) fn extras_under(romaji: &Option<Vec<Option<String>>>, i: usize, size: f32, centred: bool) -> Option<gpui::Div> {
    let r = romaji.as_ref()?.get(i).cloned().flatten()?;
    Some(
        div()
            .text_size(px(size * 0.56))
            .line_height(px(size * 0.56 * 1.3))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgba(0xffffffcc))
            .when(centred, |d| d.text_center())
            .child(r),
    )
}
