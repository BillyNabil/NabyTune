//! Sharing a song: its YouTube Music link, copied for pasting into a chat or anywhere else.
//! A short note in the dock says it's done; the same note carries other small news
//! (a listen-along invite copied, a Blend made) and goes away on its own.

use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, rgb, rgba, ClipboardItem, Context, SharedString};

use crate::i18n::{t, tf};
use crate::{NabyTune, Track, ICONS, SECONDARY, SUBTLE_STRONG};

/// How long a note stays in the dock.
const NOTE: Duration = Duration::from_secs(4);

/// The song's page on YouTube Music: it opens there, or in the YouTube Music app on a phone.
pub(crate) fn song_link(video_id: &str) -> String {
    format!("https://music.youtube.com/watch?v={video_id}")
}

/// The song in a link found in pasted text: a music.youtube.com or youtube.com watch link,
/// a youtu.be one, or a bare id.
pub(crate) fn video_id_in(text: &str) -> Option<String> {
    let text = text.trim();
    let valid = |id: &str| id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid(text) {
        return Some(text.to_string());
    }
    let id = if let Some(rest) = text.split("youtu.be/").nth(1) {
        rest.split(['?', '&', '#', '/']).next()
    } else {
        text.split(['?', '&']).find_map(|part| part.strip_prefix("v="))
    }?;
    let id = id.split(['&', '#']).next()?;
    valid(id).then(|| id.to_string())
}

impl NabyTune {
    /// A note in the dock for a few seconds.
    pub(crate) fn note(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let shown = Instant::now();
        self.note = Some((text.into(), shown));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTE).await;
            let _ = this.update(cx, |app, cx| {
                if app.note.as_ref().is_some_and(|(_, at)| *at == shown) {
                    app.note = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The note showing, if one is.
    pub(crate) fn current_note(&self) -> Option<SharedString> {
        self.note.as_ref().filter(|(_, at)| at.elapsed() < NOTE).map(|(text, _)| text.clone())
    }

    /// Copies `track`'s link, to paste and share.
    pub(crate) fn share_link(&mut self, track: &Track, cx: &mut Context<Self>) {
        if crate::local::is_local(&track.video_id) {
            self.note(t("Songs from this PC have no link to share"), cx);
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(song_link(&track.video_id)));
        self.note(tf("Link to “{}” copied: paste it to share", &[&track.title]), cx);
    }

    /// The playing song's share button, beside its heart.
    pub(crate) fn share_button(&self, id: &'static str, size_px: f32, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex_none()
            .size(px(size_px + 12.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .font_family(ICONS)
            .text_size(px(size_px))
            .text_color(rgba(SECONDARY))
            .hover(|s| s.bg(rgba(SUBTLE_STRONG)).text_color(rgb(0xffffff)))
            .child(SHARE)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if let Some(track) = this.current().cloned() {
                    this.share_link(&track, cx);
                }
            }))
    }
}

/// Segoe Fluent Icons' Share.
pub(crate) const SHARE: &str = "\u{E72D}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_song_in_a_link() {
        assert_eq!(song_link("dQw4w9WgXcQ"), "https://music.youtube.com/watch?v=dQw4w9WgXcQ");
        for link in [
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDAMVM",
            "https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
            " dQw4w9WgXcQ ",
        ] {
            assert_eq!(video_id_in(link).as_deref(), Some("dQw4w9WgXcQ"), "{link}");
        }
        assert_eq!(video_id_in("https://music.youtube.com/"), None);
        assert_eq!(video_id_in("hello"), None);
    }
}
