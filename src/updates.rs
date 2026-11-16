//! Updates: NabyTune asks its GitHub releases (updater.rs) once a day, quietly, and Settings ›
//! About offers the new version; one click downloads it, checks it, and lets the installer
//! put it in place and start it again. A copy run from a build folder only links to the page.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{div, prelude::*, px, rgb, rgba, Context, FontWeight, SharedString};

use crate::appearance::{accent, on_accent};
use crate::i18n::{t, tf};
use crate::{settings, updater, NabyTune, ICONS, SECONDARY, STROKE, SUBTLE_STRONG};

/// Where NabyTune's releases are published ("owner/name" on GitHub). Settings' `update_repo`
/// takes over when set; empty means updates aren't set up yet.
pub(crate) const REPO: &str = "";

/// How long between quiet checks.
const EVERY: u64 = 24 * 3600;

pub(crate) enum Update {
    Idle,
    Checking,
    Latest,
    Available(updater::Release),
    Downloading { done: u64, total: u64 },
    Failed(String),
}

fn repo() -> Option<String> {
    settings::get("update_repo").filter(|r| r.contains('/')).or_else(|| Some(REPO.to_string()).filter(|r| r.contains('/')))
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl NabyTune {
    /// At start: last time's download is cleared away, and once a day the releases are asked.
    pub(crate) fn check_updates_quietly(&mut self, cx: &mut Context<Self>) {
        updater::clean_downloads();
        let last = settings::get("update_checked").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if repo().is_some() && unix_now().saturating_sub(last) >= EVERY {
            self.check_updates(true, cx);
        }
    }

    /// Asks GitHub for a newer release. `quiet`: a failure says nothing.
    pub(crate) fn check_updates(&mut self, quiet: bool, cx: &mut Context<Self>) {
        let Some(repo) = repo() else {
            self.update = Update::Failed(t("Updates aren't set up for this copy yet").into());
            cx.notify();
            return;
        };
        if !quiet {
            self.update = Update::Checking;
        }
        let http = self.youtube.client();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { updater::check(&http, &repo, env!("CARGO_PKG_VERSION")) }).await;
            let _ = this.update(cx, |app, cx| {
                settings::set("update_checked", &unix_now().to_string());
                app.update = match result {
                    Ok(Some(release)) => Update::Available(release),
                    Ok(None) => if quiet { Update::Idle } else { Update::Latest },
                    Err(e) => {
                        crate::audio::log(format!("update check failed: {e:#}"));
                        if quiet { Update::Idle } else { Update::Failed(format!("{e:#}")) }
                    }
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Downloads the release, checks it, and hands over to its installer, which closes this
    /// copy, puts the new one in place and starts it.
    pub(crate) fn install_update(&mut self, cx: &mut Context<Self>) {
        let Update::Available(release) = &self.update else { return };
        let release = release.clone();
        if !updater::is_installed() {
            let _ = std::process::Command::new("explorer").arg(&release.page).spawn();
            return;
        }
        self.update = Update::Downloading { done: 0, total: release.size };
        cx.notify();
        let http = self.youtube.client();
        let progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let seen = progress.clone();
        // The bar moves while it downloads.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            let going = this
                .update(cx, |app, cx| match &mut app.update {
                    Update::Downloading { done, .. } => {
                        *done = seen.load(std::sync::atomic::Ordering::Relaxed);
                        cx.notify();
                        true
                    }
                    _ => false,
                })
                .unwrap_or(false);
            if !going {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater::download(&http, &release, |done, _| progress.store(done, std::sync::atomic::Ordering::Relaxed)) })
                .await
                .and_then(|path| updater::install_and_restart(&path));
            let _ = this.update(cx, |app, cx| match result {
                Ok(()) => {
                    // The installer takes it from here: it waits for this copy to go.
                    app.save_session();
                    app.listen_flush();
                    cx.quit();
                }
                Err(e) => {
                    app.update = Update::Failed(format!("{e:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Settings › About's row for updates: what's known, and the button for the next step.
    pub(crate) fn update_control(&self, cx: &mut Context<Self>) -> (SharedString, gpui::AnyElement) {
        let button = |id: &'static str, glyph: &'static str, label: SharedString, primary: bool| {
            div()
                .id(id)
                .h(px(32.0))
                .px(px(14.0))
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.0))
                .cursor_pointer()
                .text_size(px(13.0))
                .map(|d| match primary {
                    true => d.bg(rgb(accent())).text_color(rgb(on_accent())).font_weight(FontWeight::SEMIBOLD).hover(|s| s.opacity(0.9)),
                    false => d.border_1().border_color(rgba(STROKE)).bg(rgba(SUBTLE_STRONG)).hover(|s| s.bg(rgba(0xffffff26))),
                })
                .child(div().font_family(ICONS).text_size(px(12.0)).child(glyph))
                .child(label)
        };
        let check = || button("update-check", "\u{E72C}", t("Check for updates").into(), false).on_click(cx.listener(|this, _, _, cx| this.check_updates(false, cx)));
        match &self.update {
            Update::Idle => (t("Updates come from NabyTune's releases on GitHub").into(), check().into_any_element()),
            Update::Checking => (t("Checking…").into(), div().text_size(px(12.0)).text_color(rgba(SECONDARY)).child("…").into_any_element()),
            Update::Latest => (t("You have the newest version").into(), check().into_any_element()),
            Update::Failed(e) => (tf("Couldn't check: {}", &[e]).into(), check().into_any_element()),
            Update::Available(r) => {
                let label: SharedString = if updater::is_installed() { tf("Update to {}", &[&r.version]).into() } else { t("Open the release").into() };
                (
                    format!("{}{}", tf("Version {} is out", &[&r.version]), if r.notes.is_empty() { String::new() } else { format!(": {}", first_line(&r.notes)) }).into(),
                    button("update-install", "\u{E896}", label, true).on_click(cx.listener(|this, _, _, cx| this.install_update(cx))).into_any_element(),
                )
            }
            Update::Downloading { done, total } => {
                let part = if *total > 0 { (*done as f32 / *total as f32).clamp(0.0, 1.0) } else { 0.0 };
                (
                    tf("Downloading… {} of {} MB", &[&format!("{:.1}", *done as f32 / 1_048_576.0), &format!("{:.1}", *total as f32 / 1_048_576.0)]).into(),
                    div().w(px(160.0)).h(px(6.0)).rounded_full().bg(rgba(SUBTLE_STRONG)).child(div().h_full().rounded_full().w(gpui::relative(part)).bg(rgb(accent()))).into_any_element(),
                )
            }
        }
    }
}

fn first_line(notes: &str) -> String {
    let line = notes.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_start_matches(['#', '*', '-', ' ']);
    if line.chars().count() > 90 { format!("{}…", line.chars().take(90).collect::<String>()) } else { line.to_string() }
}
