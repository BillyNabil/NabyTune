//! Quick setup, the first time NabyTune opens: five short steps (welcome, look, account,
//! lyrics, support) over the app, each a choice with a sensible default already made, so
//! Next is always enough. Skippable at any point; everything in it lives in Settings too.

use gpui::{div, prelude::*, px, rgb, rgba, AnyElement, Context, FontWeight, SharedString, Window};
use gpui_ce_motion::{Motion, Target, Transition};

use crate::appearance::{self, accent, on_accent, Material, PRESETS};
use crate::i18n::{t, tf};
use crate::settings_page::{support_buttons, switch};
use crate::{brand_mark, settings, Look, NabyTune, DISPLAY, ICONS, SECONDARY, STROKE, SUBTLE_STRONG, TERTIARY};

const STEPS: usize = 5;

impl NabyTune {
    /// Whether quick setup still has to be shown (it hasn't been finished or skipped).
    pub(crate) fn setup_pending() -> bool {
        settings::get("setup_done").is_none()
    }

    fn finish_setup(&mut self) {
        settings::set("setup_done", "1");
        self.setup_step = None;
    }

    fn go_to_step(&mut self, step: usize) {
        self.setup_step = Some(step);
        self.setup_motion = Default::default();
    }

    pub(crate) fn setup_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let step = self.setup_step?;
        let rise = self.setup_motion.frame(
            Motion::new(Target::default().y(0.0).opacity(1.0)).initial(Target::default().y(16.0).opacity(0.0)).transition(Transition::Spring(self.spring)),
            window,
            cx,
        );
        let (title, body): (&str, AnyElement) = match step {
            0 => (t("Welcome to NabyTune"), self.setup_welcome()),
            1 => (t("Make it yours"), self.setup_look(cx)),
            2 => (t("Your YouTube Music"), self.setup_account(cx)),
            3 => (t("Lyrics, your way"), self.setup_lyrics(window, cx)),
            _ => (t("One last thing"), self.setup_support(cx)),
        };
        let last = step + 1 == STEPS;
        let dots = (0..STEPS).fold(div().flex().gap(px(6.0)), |d, i| {
            d.child(div().h(px(6.0)).w(px(if i == step { 22.0 } else { 6.0 })).rounded_full().bg(if i <= step { rgb(accent()) } else { rgba(0xffffff33) }))
        });
        let pill = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(38.0))
                .px(px(20.0))
                .flex()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .map(|d| {
                    if primary {
                        d.bg(rgb(accent())).text_color(rgb(on_accent())).hover(|s| s.opacity(0.9))
                    } else {
                        d.bg(rgba(SUBTLE_STRONG)).border_1().border_color(rgba(STROKE)).hover(|s| s.bg(rgba(0xffffff26)))
                    }
                })
                .child(label)
        };
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("setup-skip")
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .text_color(rgba(TERTIARY))
                    .hover(|s| s.text_color(rgb(0xffffff)))
                    .when(!last, |d| d.child(t("Skip setup")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.finish_setup();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .when(step > 0, |d| {
                        d.child(pill("setup-back", t("Back"), false).on_click(cx.listener(move |this, _, _, cx| {
                            this.go_to_step(step - 1);
                            cx.notify();
                        })))
                    })
                    .child(pill("setup-next", if last { t("Start listening") } else if step == 0 { t("Get started") } else { t("Next") }, true).on_click(cx.listener(
                        move |this, _, _, cx| {
                            if last {
                                this.finish_setup();
                            } else {
                                this.go_to_step(step + 1);
                            }
                            cx.notify();
                        },
                    ))),
            );
        let card = div()
            .id("setup-card")
            .w(px(600.0))
            .p(px(32.0))
            .flex()
            .flex_col()
            .gap_6()
            .rounded(px(20.0))
            .bg(rgba(0x1c1c_1cf2))
            .border_1()
            .border_color(rgba(STROKE))
            .shadow_lg()
            .child(dots)
            .child(div().font_family(DISPLAY).text_size(px(28.0)).font_weight(FontWeight::BOLD).child(title))
            .child(body)
            .child(footer);
        Some(
            div()
                .id("setup")
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::linear_gradient(
                    160.0,
                    gpui::linear_color_stop(rgba(appearance::accent_wash(0x33)), 0.0),
                    gpui::linear_color_stop(rgba(0x0a0a_0af0), 0.55),
                ))
                .child(rise.apply(card))
                .into_any_element(),
        )
    }

    fn setup_welcome(&self) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().py_2().child(brand_mark(44.0)))
            .child(
                div()
                    .text_size(px(14.0))
                    .line_height(px(22.0))
                    .text_color(rgba(SECONDARY))
                    .child(t("YouTube Music on Windows, made light and quiet: synced lyrics with romaji, a full-screen player, your library, and lossless where you have it. A minute here sets it up the way you like.")),
            )
            .into_any_element()
    }

    fn setup_look(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut choices: Vec<(String, u32, &str)> = vec![("cover".into(), appearance::cover_colour().unwrap_or(appearance::DEFAULT), "Cover")];
        choices.extend(PRESETS.iter().map(|(name, c)| (format!("#{c:06x}"), *c, *name)));
        let mut swatches = div().flex().flex_wrap().gap(px(10.0));
        for (n, (key, colour, name)) in choices.into_iter().enumerate() {
            let on = if key == "cover" { appearance::follows_cover() } else { !appearance::follows_cover() && key == self.accent_choice };
            swatches = swatches.child(
                div()
                    .id(("setup-accent", n))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(36.0))
                            .rounded_full()
                            .bg(rgb(colour))
                            .border_2()
                            .border_color(if on { rgb(0xffffff) } else { rgba(0x0000_0000) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(key == "cover", |d| d.child(div().font_family(ICONS).text_size(px(13.0)).text_color(rgba(0x000000aa)).child("\u{EB9F}"))),
                    )
                    .child(div().text_size(px(11.0)).text_color(if on { rgb(0xffffff) } else { rgba(TERTIARY) }).child(t(name)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pick_accent(&key);
                        cx.notify();
                    })),
            );
        }
        let materials = Material::ALL.iter().map(|m| (*m, SharedString::from(m.name()))).collect();
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(label(t("Accent colour"), t("Cover follows each song's artwork; any colour at all is in Settings")))
            .child(swatches)
            .child(label(t("Window"), t("What shows behind the app: your wallpaper's colours (Mica), frosted glass, or plain")))
            .child(div().flex().child(self.segmented(
                "setup-material",
                materials,
                self.material,
                |this, material, window, _| {
                    this.material = material;
                    material.apply(window);
                    settings::set("material", material.key());
                },
                cx,
            )))
            .into_any_element()
    }

    fn setup_account(&self, cx: &mut Context<Self>) -> AnyElement {
        let status: AnyElement = match (&self.account, self.signing_in) {
            (Some(account), _) => div()
                .flex()
                .items_center()
                .gap_3()
                .child(div().font_family(ICONS).text_size(px(18.0)).text_color(rgb(accent())).child("\u{E73E}"))
                .child(div().text_size(px(14.0)).child(tf("Signed in as {}", &[&account.name])))
                .into_any_element(),
            (None, true) => div().text_size(px(13.0)).text_color(rgba(SECONDARY)).child(t("Finish signing in in the window that opened…")).into_any_element(),
            (None, false) => div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .id("setup-sign-in")
                        .h(px(38.0))
                        .px(px(20.0))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(rgb(0xffffff))
                        .text_color(rgb(0x111111))
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .hover(|s| s.opacity(0.9))
                        .child(t("Sign in with Google"))
                        .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                )
                .when_some(self.account_error.clone(), |d, e| d.child(div().text_size(px(12.0)).text_color(rgba(0xff8a80ff)).child(e)))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(14.0))
                    .line_height(px(22.0))
                    .text_color(rgba(SECONDARY))
                    .child(t("Optional. Signed in, Home is made for you, and your playlists and liked songs come along. Google's own page opens: your password goes to Google, never through NabyTune.")),
            )
            .child(status)
            .into_any_element()
    }

    fn setup_lyrics(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let looks = Look::ALL.iter().map(|l| (*l, SharedString::from(l.name()))).collect();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().mx(px(-16.0)).child(self.lyric_preview(window)))
            .child(div().flex().child(self.segmented(
                "setup-look",
                looks,
                self.look,
                |this, look, _, _| {
                    this.look = look;
                    settings::set("lyric_look", look.key());
                },
                cx,
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label(t("Romaji"), t("Japanese, Chinese, Korean and more, in Latin letters under each line")))
                    .child(switch("setup-romaji", self.show_romaji).on_click(cx.listener(|this, _, _, cx| {
                        this.show_romaji = !this.show_romaji;
                        settings::set("lyrics_romaji", if this.show_romaji { "1" } else { "0" });
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }

    fn setup_support(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(14.0))
                    .line_height(px(22.0))
                    .text_color(rgba(SECONDARY))
                    .child(t("NabyTune is free, with no ads, and made by one developer. If it becomes part of your day, you can support it — it's what keeps it going. (It's in Settings too, any time.)")),
            )
            .child(support_buttons(cx))
            .into_any_element()
    }
}

/// A setting's name and a line about it.
fn label(title: &'static str, hint: &'static str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_size(px(12.0)).text_color(rgba(TERTIARY)).child(hint))
}
