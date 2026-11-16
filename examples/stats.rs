//! Tries the listening statistics and the PNG snapshot outside the app.
//! `cargo run --example stats [-- path\to\listens.jsonl]` prints every period's summary
//! (of a made-up log in the temp folder, unless a log is given) and saves a piece of the
//! screen as a PNG. `cargo test --example stats` runs the statistics' tests.

#[path = "../src/stats.rs"]
#[allow(dead_code)]
mod stats;
#[path = "../src/snapshot.rs"]
#[allow(dead_code)]
mod snapshot;
#[path = "../src/i18n.rs"]
#[allow(dead_code)]
mod i18n;

use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> anyhow::Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let offset = stats::local_offset();
    println!("local offset: {:+} h", offset as f64 / 3600.0);

    let path = match std::env::args_os().nth(1) {
        Some(p) => std::path::PathBuf::from(p),
        None => demo_log(now)?,
    };
    let t = std::time::Instant::now();
    let listens = stats::load(&path);
    println!("{} listens from {} in {:?}", listens.len(), path.display(), t.elapsed());

    for period in stats::Period::EVERY {
        let t = std::time::Instant::now();
        let s = stats::summary(&listens, period, now, offset);
        let took = t.elapsed();
        println!("\n== {} ({took:?})", s.label);
        let change = s.change.map(|c| format!("{c:+.0}% vs before")).unwrap_or_else(|| "nothing before".into());
        println!("{} min, {} plays, {} songs, {} artists, {} days ({change})", s.minutes, s.plays, s.songs, s.artists, s.days);
        for (i, song) in s.top_songs.iter().take(5).enumerate() {
            println!("  {}. {} - {}: {} plays, {} min", i + 1, song.title, song.artist, song.plays, song.minutes);
        }
        for (i, a) in s.top_artists.iter().take(5).enumerate() {
            println!("  #{} {}: {} plays, {} min", i + 1, a.name, a.plays, a.minutes);
        }
        if let Some(h) = s.favourite_hour() {
            println!("  favourite hour {h}:00, favourite day {}", stats::weekday_name(s.favourite_weekday().unwrap_or(0)));
        }
        if let Some(b) = s.busiest_day {
            println!("  busiest day {} ({} min)", b.date.label(), b.minutes);
        }
        let found: Vec<&str> = s.discoveries.iter().map(|a| a.name.as_str()).collect();
        println!("  streak {} (longest {}), {} new artists: {found:?}", s.current_streak, s.longest_streak, s.new_artists);
    }

    snapshot_check()
}

/// A year of made-up listening, written to a log in the temp folder.
fn demo_log(now: u64) -> anyhow::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("nabytune-stats-demo");
    let path = dir.join("listens.jsonl");
    let _ = std::fs::remove_file(&path);
    let artists = ["Tulus", "Hindia", "Nadin Amizah", "Nabila Maharani & Tri Suaka", "Bernadya", "Sheila On 7", "Earth, Wind & Fire"];
    let mut seed = 7u64;
    let mut next = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        seed >> 33
    };
    for i in 0..3000u64 {
        let song = next() % 60;
        let length = 180 + (song as u32 % 5) * 20;
        let heard = if next() % 6 == 0 { (next() % 25) as u32 } else { length };
        stats::record(
            &path,
            &stats::Listen {
                video_id: format!("demo{song:04}"),
                title: format!("Lagu {song}"),
                artist: artists[(song % artists.len() as u64) as usize].into(),
                album: String::new(),
                thumbnail: None,
                at: now - (3000 - i) * 10_500 - next() % 3000,
                seconds: heard,
                length,
            },
        )?;
    }
    Ok(path)
}

/// Saves a piece of the desktop as a PNG and checks it isn't all black.
fn snapshot_check() -> anyhow::Result<()> {
    use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetDesktopWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let desktop = unsafe { GetDesktopWindow() } as isize;
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let path = snapshot::unique_name(&std::env::temp_dir(), "NabyTune snapshot test");
    // The bottom-left corner of the main screen: the taskbar and some of what is above it.
    snapshot::save_window_region(desktop, 0, h - 300, 600, 300, &path)?;
    let saved = image::open(&path)?.to_rgba8();
    let lit = saved.pixels().filter(|p| p.0[0] > 16 || p.0[1] > 16 || p.0[2] > 16).count();
    let share = lit as f64 * 100.0 / (saved.width() * saved.height()) as f64;
    println!("\nsnapshot {}x{} of a {w}x{h} screen -> {} ({share:.0}% not black)", saved.width(), saved.height(), path.display());
    anyhow::ensure!(lit > 0, "the snapshot is all black");
    println!("pictures folder: {}", snapshot::pictures_dir().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::snapshot::unique_name;

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("nabytune-snapshot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = unique_name(&dir, "NabyTune Wrapped 2026-11");
        assert_eq!(first.file_name().unwrap(), "NabyTune Wrapped 2026-11.png");
        std::fs::write(&first, b"x").unwrap();
        let second = unique_name(&dir, "NabyTune Wrapped 2026-11");
        assert_eq!(second.file_name().unwrap(), "NabyTune Wrapped 2026-11 (2).png");
        std::fs::write(&second, b"x").unwrap();
        assert_eq!(unique_name(&dir, "NabyTune Wrapped 2026-11").file_name().unwrap(), "NabyTune Wrapped 2026-11 (3).png");
        assert_eq!(unique_name(&dir, "Week: 9/11?").file_name().unwrap(), "Week- 9-11-.png");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
