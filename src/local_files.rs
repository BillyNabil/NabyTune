//! The listener's own music: folders they pick, walked for audio files that play here,
//! with each file's tags, length and cover read by lofty. A scan keeps what it read in
//! %LOCALAPPDATA%\NabyTune\local-index.json, so the next one only opens files that are new
//! or changed since; a library of thousands then comes back in a moment.
//!
//! Local songs live alongside YouTube Music's under the id "file:" + the absolute path.
//! Their bytes go to decoder.rs like any other song's; it tells the format from the first
//! bytes, so nothing else needs passing for the formats listed in `EXTENSIONS`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use anyhow::Context;
use image::RgbaImage;
use lofty::config::ParseOptions;
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::Tag;
use serde_json::{json, Value};

/// File extensions (lower case) of the formats decoder.rs plays: MP3, FLAC, AAC or ALAC
/// in MP4, raw AAC (ADTS), Ogg Vorbis and WAV. Opus, WMA and AIFF are left out.
pub const EXTENSIONS: [&str; 8] = ["mp3", "flac", "m4a", "mp4", "aac", "ogg", "oga", "wav"];

/// What a song is called when neither its tags nor its file name say who made it.
pub const UNKNOWN_ARTIST: &str = "Unknown artist";

/// The index's layout; an index written by another version is read as empty.
const INDEX_VERSION: u64 = 1;

/// Covers wider or taller than this are scaled down on reading: the app never shows
/// them bigger, and a 3000-pixel scan of a sleeve is 36 MB once decoded.
const COVER_MAX: u32 = 1200;

/// How deep a scan goes below a picked folder.
const MAX_DEPTH: usize = 64;

/// A song on disk, as read from its tags (or, failing them, its file name).
#[derive(Clone, Debug, PartialEq)]
pub struct LocalTrack {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    /// Empty when the tags don't name one.
    pub album: String,
    /// The album's artist, or the song's artist when the tags don't say.
    pub album_artist: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    /// 0 when the file doesn't state its length.
    pub duration_secs: f64,
    /// Whether the file carries a picture of its own (see `cover` for the folder's).
    pub has_cover: bool,
    /// ReplayGain's track gain in dB, when the file was measured.
    pub gain_db: Option<f32>,
    /// Last written, in milliseconds since 1970.
    pub modified: u64,
    /// In bytes.
    pub size: u64,
}

impl LocalTrack {
    /// The id the app plays and keeps this song by: "file:" and the absolute path.
    pub fn id(&self) -> String {
        format!("file:{}", self.path.display())
    }
}

/// The file behind a local song's id; None for YouTube's and anything else.
pub fn path_of(id: &str) -> Option<PathBuf> {
    id.strip_prefix("file:").filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// Whether a file's extension is one of the formats that play here.
pub fn is_supported(path: &Path) -> bool {
    extension_hint(path).is_some_and(|ext| EXTENSIONS.contains(&ext.as_str()))
}

/// The file's extension in lower case, for symphonia's `Hint` should the decoder ever
/// take one. Today's probe goes by the first bytes and needs none; the one format it
/// misses that way is raw AAC from MPEG-2 encoders (ADTS starting FF F9), rare in .aac files.
pub fn extension_hint(path: &Path) -> Option<String> {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)
}

/// What a scan found and how much of it was read afresh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Audio files found in the folders.
    pub found: usize,
    /// Files whose tags were read this time (new or changed).
    pub read: usize,
    /// Files taken from the index unchanged.
    pub reused: usize,
    /// Files that couldn't be read as audio and are left out.
    pub failed: usize,
}

/// %LOCALAPPDATA%\NabyTune\local-index.json, where scans keep what they read.
pub fn default_index() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(base).join("NabyTune").join("local-index.json"))
}

/// Every song under `folders` (and their subfolders), ordered by album artist, album,
/// disc and track. Files unchanged since the index at `index` was written are taken from
/// it; the rest are read, and the index is written back. Blocks for as long as the disk
/// takes, so call it off the UI thread (or use `scan_in_background`).
pub fn scan(folders: &[PathBuf], index: &Path) -> Vec<LocalTrack> {
    scan_with_stats(folders, index).0
}

/// `scan`, on a thread of its own. The handle says when it's done (`is_finished`); an
/// error only when the system won't start another thread.
#[allow(dead_code)] // examples/local_files.rs; the app scans with `scan` on its own executor
pub fn scan_in_background(folders: Vec<PathBuf>, index: PathBuf) -> std::io::Result<std::thread::JoinHandle<Vec<LocalTrack>>> {
    std::thread::Builder::new().name("local-scan".into()).spawn(move || scan(&folders, &index))
}

/// `scan`, also saying how many files were read and how many came from the index.
pub fn scan_with_stats(folders: &[PathBuf], index: &Path) -> (Vec<LocalTrack>, ScanStats) {
    let files = walk(folders);
    let known = load_index(index);
    let mut stats = ScanStats { found: files.len(), ..Default::default() };

    // Each file either comes from the index as it was, or is read.
    let mut entries: Vec<Option<Entry>> = Vec::with_capacity(files.len());
    let mut todo = Vec::new();
    for (i, file) in files.iter().enumerate() {
        match known.get(&file.path) {
            Some(entry) if entry.size == file.size && entry.modified == file.modified => {
                stats.reused += 1;
                entries.push(Some(entry.clone()));
            }
            _ => {
                todo.push(i);
                entries.push(None);
            }
        }
    }
    stats.read = todo.len();
    for (i, entry) in read_all(&files, &todo) {
        entries[i] = Some(entry);
    }

    let entries: Vec<Entry> = entries.into_iter().flatten().collect();
    save_index(index, &entries);
    let mut tracks: Vec<LocalTrack> = entries.into_iter().filter_map(|e| e.track).collect();
    stats.failed = stats.found - tracks.len();
    sort(&mut tracks);
    (tracks, stats)
}

/// A file found on disk, before its tags are read.
struct Found {
    path: PathBuf,
    size: u64,
    modified: u64,
}

/// A file as the index keeps it: its song, or None when it couldn't be read (kept too,
/// so a broken file isn't opened again at every scan).
#[derive(Clone)]
struct Entry {
    path: PathBuf,
    size: u64,
    modified: u64,
    track: Option<LocalTrack>,
}

/// The audio files under `folders`. Hidden and system folders are skipped, as are
/// folders already walked (by their real path), so links that loop back end there.
fn walk(folders: &[PathBuf]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
    let mut seen_files: HashSet<PathBuf> = HashSet::new();
    for root in folders {
        let Ok(root) = std::path::absolute(root) else { continue };
        let mut stack = vec![(root, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(real) = fs::canonicalize(&dir) else { continue };
            if !seen_dirs.insert(real) {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            let mut subdirs = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if hidden_name(&entry.file_name()) {
                    continue;
                }
                // Follows links, so a link to a folder is walked (once) like the folder.
                let Ok(meta) = fs::metadata(&path) else { continue };
                if hidden_attributes(&meta) {
                    continue;
                }
                if meta.is_dir() {
                    if depth < MAX_DEPTH {
                        subdirs.push(path);
                    }
                } else if meta.is_file() && meta.len() > 0 && is_supported(&path) && seen_files.insert(path.clone()) {
                    found.push(Found { size: meta.len(), modified: modified_ms(&meta), path });
                }
            }
            // Popped in name order.
            subdirs.sort();
            stack.extend(subdirs.into_iter().rev().map(|d| (d, depth + 1)));
        }
    }
    found
}

/// Names Windows and other systems keep for themselves, and dot-names (hidden elsewhere).
fn hidden_name(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.') || name.starts_with('$') || name.eq_ignore_ascii_case("System Volume Information")
}

#[cfg(windows)]
fn hidden_attributes(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    meta.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[cfg(not(windows))]
fn hidden_attributes(_meta: &fs::Metadata) -> bool {
    false
}

fn modified_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// Reads the files at `todo` on a few threads; the disk, not the parsing, sets the pace,
/// but on an SSD several readers at once go noticeably faster.
fn read_all(files: &[Found], todo: &[usize]) -> Vec<(usize, Entry)> {
    if todo.is_empty() {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8).min(todo.len());
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&i) = todo.get(k) else { break };
                        let file = &files[i];
                        let track = read_track(&file.path, file.size, file.modified);
                        done.push((i, Entry { path: file.path.clone(), size: file.size, modified: file.modified, track }));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

/// One file's song: its tags where it has them, its file name where it doesn't. None
/// when lofty can't make it out as audio at all.
fn read_track(path: &Path, size: u64, modified: u64) -> Option<LocalTrack> {
    // A malformed file must not take the scan down with it.
    let tagged = catch_unwind(AssertUnwindSafe(|| {
        Probe::open(path).ok()?.guess_file_type().ok()?.options(ParseOptions::new()).read().ok()
    }))
    .ok()
    .flatten()?;

    let mut tags: Vec<&Tag> = Vec::new();
    if let Some(primary) = tagged.primary_tag() {
        tags.push(primary);
    }
    tags.extend(tagged.tags().iter().filter(|t| Some(t.tag_type()) != tagged.primary_tag().map(|p| p.tag_type())));

    let text = |key: ItemKey| -> Option<String> {
        tags.iter().filter_map(|t| t.get_string(key)).map(str::trim).find(|s| !s.is_empty()).map(str::to_owned)
    };
    let (name_artist, name_title, name_track) = from_file_name(path);

    let title = text(ItemKey::TrackTitle).unwrap_or(name_title);
    let artist = text(ItemKey::TrackArtist).or(name_artist).unwrap_or_else(|| UNKNOWN_ARTIST.to_owned());
    let album = text(ItemKey::AlbumTitle).unwrap_or_default();
    let album_artist = text(ItemKey::AlbumArtist).unwrap_or_else(|| artist.clone());
    let track_no = text(ItemKey::TrackNumber).and_then(|s| leading_number(&s)).or(name_track);
    let disc_no = text(ItemKey::DiscNumber).and_then(|s| leading_number(&s));
    let year = [ItemKey::RecordingDate, ItemKey::Year, ItemKey::OriginalReleaseDate]
        .into_iter()
        .find_map(|key| text(key).and_then(|s| year_in(&s)));
    let gain_db = text(ItemKey::ReplayGainTrackGain).and_then(|s| parse_gain(&s));
    let has_cover = tags.iter().any(|t| !t.pictures().is_empty());
    let duration_secs = tagged.properties().duration().as_secs_f64();

    Some(LocalTrack {
        path: path.to_path_buf(),
        title,
        artist,
        album,
        album_artist,
        track_no: track_no.filter(|&n| n > 0),
        disc_no: disc_no.filter(|&n| n > 0),
        year,
        duration_secs,
        has_cover,
        gain_db,
        modified,
        size,
    })
}

/// (artist, title, track number) from a file name: "Artist - Title", "07 - Title",
/// "07. Artist - Title" and plain "Title" are understood.
fn from_file_name(path: &Path) -> (Option<String>, String, Option<u32>) {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = stem.replace('_', " ");
    let mut rest = stem.trim();
    let mut track = None;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let after = &rest[digits..];
        let sep = after.len() - after.trim_start_matches(['.', '-', ' ', ')']).len();
        // "07 Title", "7. Title", "07 - Title", but not "2 Become 1" or "1999".
        let marked = after.starts_with('.') || after.starts_with(" - ") || after.starts_with(')');
        if sep > 0 && sep < after.len() && (marked || digits >= 2) {
            track = rest[..digits].parse().ok();
            rest = after[sep..].trim();
        }
    }
    match rest.split_once(" - ") {
        Some((artist, title)) if !artist.trim().is_empty() && !title.trim().is_empty() => {
            (Some(artist.trim().to_owned()), title.trim().to_owned(), track)
        }
        _ => (None, if rest.is_empty() { stem.trim().to_owned() } else { rest.to_owned() }, track),
    }
}

/// "3", "3/12", " 03 " → 3.
fn leading_number(s: &str) -> Option<u32> {
    let s = s.trim();
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    s[..digits].parse().ok()
}

/// The first four-digit run in a date ("2004", "2004-05-17", "17/05/2004").
fn year_in(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let run = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        if run == 4 {
            return s[i..i + 4].parse().ok().filter(|&y| y > 0);
        }
        i += run.max(1);
    }
    None
}

/// "-6.54 dB", "+1.2dB", "-6,54 dB" → dB.
fn parse_gain(s: &str) -> Option<f32> {
    let number: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | ',')).collect();
    number.replace(',', ".").parse::<f32>().ok().filter(|g| g.is_finite() && g.abs() < 60.0)
}

/// Album artist, album, disc, track, then title, ignoring case.
fn sort(tracks: &mut [LocalTrack]) {
    tracks.sort_by_cached_key(|t| {
        (
            t.album_artist.to_lowercase(),
            t.album.to_lowercase(),
            t.disc_no.unwrap_or(0),
            t.track_no.unwrap_or(0),
            t.title.to_lowercase(),
            t.path.clone(),
        )
    });
}

fn load_index(index: &Path) -> HashMap<PathBuf, Entry> {
    let Ok(text) = fs::read_to_string(index) else { return HashMap::new() };
    let Ok(root) = serde_json::from_str::<Value>(&text) else { return HashMap::new() };
    if root["version"].as_u64() != Some(INDEX_VERSION) {
        return HashMap::new();
    }
    let Some(files) = root["files"].as_array() else { return HashMap::new() };
    files.iter().filter_map(entry_from_json).map(|e| (e.path.clone(), e)).collect()
}

fn entry_from_json(v: &Value) -> Option<Entry> {
    let path = PathBuf::from(v["path"].as_str()?);
    let size = v["size"].as_u64()?;
    let modified = v["modified"].as_u64()?;
    let track = if v["failed"].as_bool() == Some(true) {
        None
    } else {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
        let n = |k: &str| v[k].as_u64().and_then(|n| u32::try_from(n).ok());
        Some(LocalTrack {
            path: path.clone(),
            title: s("title"),
            artist: s("artist"),
            album: s("album"),
            album_artist: s("album_artist"),
            track_no: n("track"),
            disc_no: n("disc"),
            year: n("year"),
            duration_secs: v["duration"].as_f64().unwrap_or(0.0),
            has_cover: v["cover"].as_bool().unwrap_or(false),
            gain_db: v["gain"].as_f64().map(|g| g as f32),
            modified,
            size,
        })
    };
    Some(Entry { path, size, modified, track })
}

fn entry_to_json(e: &Entry) -> Option<Value> {
    let path = e.path.to_str()?;
    Some(match &e.track {
        None => json!({ "path": path, "size": e.size, "modified": e.modified, "failed": true }),
        Some(t) => json!({
            "path": path,
            "size": e.size,
            "modified": e.modified,
            "title": t.title,
            "artist": t.artist,
            "album": t.album,
            "album_artist": t.album_artist,
            "track": t.track_no,
            "disc": t.disc_no,
            "year": t.year,
            "duration": t.duration_secs,
            "cover": t.has_cover,
            "gain": t.gain_db,
        }),
    })
}

/// Writes the index beside itself first and then puts it in place, so a crash halfway
/// leaves the old one rather than half a file. Failing to write only costs the next scan time.
fn save_index(index: &Path, entries: &[Entry]) {
    let files: Vec<Value> = entries.iter().filter_map(entry_to_json).collect();
    let root = json!({ "version": INDEX_VERSION, "files": files });
    let Ok(text) = serde_json::to_string(&root) else { return };
    if let Some(dir) = index.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut temp = index.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    if fs::write(&temp, text).is_ok() && fs::rename(&temp, index).is_err() {
        let _ = fs::remove_file(&temp);
    }
}

/// The song's cover: the picture in the file (the front cover when there are several),
/// else a cover.jpg, folder.jpg or front.jpg (any case; .jpeg, .png and .webp too) in its
/// folder. Large pictures come back scaled down to 1200 pixels.
pub fn cover(path: &Path) -> Option<RgbaImage> {
    embedded_cover(path).or_else(|| folder_cover(path)).map(limit_size)
}

fn embedded_cover(path: &Path) -> Option<RgbaImage> {
    catch_unwind(AssertUnwindSafe(|| {
        let tagged = Probe::open(path).ok()?.guess_file_type().ok()?.options(ParseOptions::new().read_properties(false)).read().ok()?;
        let pictures: Vec<_> = tagged.tags().iter().flat_map(|t| t.pictures()).collect();
        let front = pictures.iter().find(|p| p.pic_type() == PictureType::CoverFront);
        front
            .into_iter()
            .chain(pictures.iter())
            .find_map(|p| image::load_from_memory(p.data()).ok())
            .map(|img| img.to_rgba8())
    }))
    .ok()
    .flatten()
}

fn folder_cover(path: &Path) -> Option<RgbaImage> {
    let dir = path.parent()?;
    let names: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.path()))
        .collect();
    for stem in ["cover", "folder", "front"] {
        for ext in ["jpg", "jpeg", "png", "webp"] {
            let wanted = format!("{stem}.{ext}");
            if let Some((_, file)) = names.iter().find(|(name, _)| *name == wanted) {
                if let Some(img) = fs::read(file).ok().and_then(|b| image::load_from_memory(&b).ok()) {
                    return Some(img.to_rgba8());
                }
            }
        }
    }
    None
}

fn limit_size(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= COVER_MAX && h <= COVER_MAX {
        return img;
    }
    let scale = COVER_MAX as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

/// The whole file, for `decoder::AudioSource::new`.
pub fn read(path: &Path) -> anyhow::Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// "3:07", or "1:02:03" past the hour.
pub fn duration_text(secs: f64) -> String {
    let total = if secs.is_finite() && secs > 0.0 { secs.round() as u64 } else { 0 };
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The songs whose title, artist or album (or album artist) hold every word of `query`,
/// ignoring case, in the order given. An empty query matches nothing.
pub fn search<'a>(tracks: &'a [LocalTrack], query: &str) -> Vec<&'a LocalTrack> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    tracks
        .iter()
        .filter(|t| {
            let hay = format!("{}\n{}\n{}\n{}", t.title, t.artist, t.album, t.album_artist).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}
