//! NabyTune's settings, kept in %APPDATA%\NabyTune\gpui-proto.ini as `key=value` lines.
//!
//! The file is read once and held in memory: some settings are read every frame (the
//! desktop lyrics' look, for one), and reading the file each time cost a disk read per value.
//! Writes go to memory and straight on to the file, only when a value actually changes.

use std::path::PathBuf;
use std::sync::Mutex;

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("NabyTune").join("gpui-proto.ini"))
}

/// A line's key and value, past a byte-order mark an editor may have put at the file's start.
fn parse(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim_start_matches('\u{feff}').trim();
    (!key.is_empty()).then(|| (key.to_string(), value.trim().to_string()))
}

/// The file as last read or written: where it is, and its lines in order.
struct Loaded {
    path: PathBuf,
    lines: Vec<(String, String)>,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

/// Runs `f` on the settings, read from the file first if they aren't in memory yet (or the
/// file has moved, as it does in tests).
fn with<T>(f: impl FnOnce(&mut Loaded) -> T) -> Option<T> {
    let path = path()?;
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if loaded.as_ref().is_none_or(|l| l.path != path) {
        let lines = std::fs::read_to_string(&path).unwrap_or_default().lines().filter_map(parse).collect();
        *loaded = Some(Loaded { path, lines });
    }
    loaded.as_mut().map(f)
}

fn save(loaded: &Loaded) {
    if let Some(dir) = loaded.path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = loaded.lines.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    let _ = std::fs::write(&loaded.path, text);
}

pub fn get(key: &str) -> Option<String> {
    with(|l| l.lines.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())).flatten()
}

pub fn set(key: &str, value: &str) {
    with(|l| {
        if l.lines.iter().any(|(k, v)| k == key && v == value) {
            return;
        }
        l.lines.retain(|(k, _)| k != key);
        l.lines.push((key.to_string(), value.to_string()));
        save(l);
    });
}

/// How many keys start with `prefix`.
pub fn count_prefixed(prefix: &str) -> usize {
    with(|l| l.lines.iter().filter(|(k, _)| k.starts_with(prefix)).count()).unwrap_or(0)
}

/// Drops every key that starts with `prefix`.
pub fn remove_prefixed(prefix: &str) {
    with(|l| {
        let before = l.lines.len();
        l.lines.retain(|(k, _)| !k.starts_with(prefix));
        if l.lines.len() != before {
            save(l);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_keys_past_a_byte_order_mark() {
        assert_eq!(parse("\u{feff}volume=0.5"), Some(("volume".into(), "0.5".into())));
        assert_eq!(parse(" key = value "), Some(("key".into(), "value".into())));
        assert_eq!(parse("no equals sign"), None);
        assert_eq!(parse("=orphan"), None);
    }
}
