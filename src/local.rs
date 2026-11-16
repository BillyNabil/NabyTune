//! The listener's own music files, beside YouTube Music: folders picked in Library, scanned
//! in the background (local_files.rs reads the tags), shown under "On this PC", found by
//! search, and played like any song. Their ids are "file:" and the path.

use std::path::PathBuf;

use gpui::{div, prelude::*, px, rgba, Context, FontWeight, PathPromptOptions, SharedString};

use crate::i18n::{songs, t, tf};
use crate::local_files::{self, LocalTrack};
use crate::{ambient, settings, Art, NabyTune, Source, Track, ICONS, SECONDARY, STROKE, SUBTLE, SUBTLE_STRONG, TERTIARY};

/// Rows shown before "Show all".
const SHOWN: usize = 30;

pub(crate) fn is_local(video_id: &str) -> bool {
    video_id.starts_with("file:")
}

/// A file as a song in the app.
pub(crate) fn track_of(t: &LocalTrack) -> Track {
    let detail = if t.album.is_empty() { t.artist.clone() } else { format!("{} • {}", t.artist, t.album) };
    Track {
        video_id: t.id(),
        title: t.title.clone().into(),
        artist: t.artist.clone().into(),
        detail: detail.into(),
        duration: local_files::duration_text(t.duration_secs).into(),
        thumbnail: None,
        local: None,
    }
}

/// The folders picked, as kept ("|" between them).
pub(crate) fn saved_folders() -> Vec<PathBuf> {
    settings::get("local_folders").map(|v| v.split('|').filter(|p| !p.is_empty()).map(PathBuf::from).collect()).unwrap_or_default()
}

impl NabyTune {
    /// Reads the folders again (in the background): new files show, gone ones go.
    pub(crate) fn scan_local(&mut self, cx: &mut Context<Self>) {
        let folders = self.local_folders.clone();
        if folders.is_empty() {
            self.local_tracks.clear();
            cx.notify();
            return;
        }
        let Some(index) = local_files::default_index() else { return };
        self.local_scanning = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { local_files::scan(&folders, &index) }).await;
            let _ = this.update(cx, |app, cx| {
                app.local_scanning = false;
                app.local_tracks = found;
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks for folders to add (Windows' own picker), then scans.
    pub(crate) fn add_local_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some(t("Add to NabyTune").into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let _ = this.update(cx, |app, cx| {
                for p in paths {
                    if !app.local_folders.contains(&p) {
                        app.local_folders.push(p);
                    }
                }
                app.keep_local_folders();
                app.scan_local(cx);
            });
        })
        .detach();
    }

    pub(crate) fn remove_local_folder(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.local_folders.len() {
            self.local_folders.remove(i);
            self.keep_local_folders();
            self.scan_local(cx);
        }
    }

    fn keep_local_folders(&self) {
        let joined: Vec<String> = self.local_folders.iter().map(|p| p.display().to_string()).collect();
        settings::set("local_folders", &joined.join("|"));
    }

    /// A file's cover, for its row (read from the file, in the background).
    pub(crate) fn fetch_local_art(&mut self, track: &Track, cx: &mut Context<Self>) {
        let key = track.video_id.clone();
        if self.art.contains_key(&key) || !self.art_pending.insert(key.clone()) {
            return;
        }
        let Some(path) = local_files::path_of(&key) else { return };
        cx.spawn(async move |this, cx| {
            let art = cx
                .background_executor()
                .spawn(async move { local_files::cover(&path).map(|img| Art { small: ambient::cover(&img, 120) }) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.art_pending.remove(&key);
                if let Some(art) = art {
                    app.keep_art(key, art);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The files whose title, artist or album has every word of `query`, as songs.
    pub(crate) fn local_matches(&self, query: &str, most: usize) -> Vec<Track> {
        local_files::search(&self.local_tracks, query).into_iter().take(most).map(track_of).collect()
    }

    /// Library's "On this PC": the folders, and the songs in them.
    pub(crate) fn local_section(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let count = self.local_tracks.len();
        let shown = if self.local_show_all { count } else { count.min(SHOWN) };
        let tracks: Vec<Track> = self.local_tracks.iter().take(shown).map(track_of).collect();
        for t in &tracks {
            self.fetch_local_art(t, cx);
        }
        let pill = |id: (&'static str, usize), glyph: &'static str, label: SharedString| {
            div()
                .id(id)
                .h(px(30.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap_2()
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.0))
                .bg(rgba(SUBTLE_STRONG))
                .border_1()
                .border_color(rgba(STROKE))
                .hover(|s| s.bg(rgba(0xffffff26)))
                .child(div().font_family(ICONS).text_size(px(11.0)).child(glyph))
                .child(label)
        };
        let mut folders = div().flex().flex_wrap().gap_2();
        for (i, folder) in self.local_folders.iter().enumerate() {
            let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| folder.display().to_string());
            folders = folders.child(
                pill(("local-folder", i), "\u{E8B7}", name.into())
                    .child(div().font_family(ICONS).text_size(px(9.0)).text_color(rgba(TERTIARY)).child("\u{E8BB}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_local_folder(i, cx))),
            );
        }
        folders = folders.child(pill(("local-add", 0), "\u{E710}", t("Add a folder").into()).on_click(cx.listener(|this, _, _, cx| this.add_local_folder(cx))));
        if !self.local_folders.is_empty() {
            folders = folders.child(pill(("local-rescan", 0), "\u{E72C}", t("Scan again").into()).on_click(cx.listener(|this, _, _, cx| this.scan_local(cx))));
        }
        let status: SharedString = match (self.local_scanning, count, self.local_folders.is_empty()) {
            (true, _, _) => t("Reading your music…").into(),
            (false, _, true) => t("Add a folder with music (MP3, FLAC, M4A, OGG, WAV) and it plays here, beside YouTube Music").into(),
            (false, 0, false) => t("No music found in these folders").into(),
            (false, n, false) => songs(n).into(),
        };
        let mut list = div().flex().flex_col().mx(px(-12.0));
        for (i, track) in tracks.iter().enumerate() {
            list = list.child(self.row(Source::Local, i, track, true, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(Self::heading(t("On this PC"), 20.0))
                    .child(div().pb(px(3.0)).text_size(px(12.0)).text_color(rgba(SECONDARY)).child(status)),
            )
            .child(folders)
            .when(!tracks.is_empty(), |d| d.child(list))
            .when(count > SHOWN, |d| {
                let all = self.local_show_all;
                d.child(
                    div()
                        .id("local-more")
                        .w_full()
                        .h(px(34.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.0))
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .bg(rgba(SUBTLE))
                        .hover(|s| s.bg(rgba(SUBTLE_STRONG)))
                        .child(if all { t("Show fewer").to_string() } else { tf("Show all {}", &[&count]) })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.local_show_all = !this.local_show_all;
                            cx.notify();
                        })),
                )
            })
    }
}
