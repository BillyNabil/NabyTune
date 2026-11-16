//! Updates, from the project's GitHub releases.
//!
//! `check` asks GitHub for the latest release and answers with it when it is newer than this
//! build and carries a NabyTune-Setup.exe. `download` fetches that setup into
//! %TEMP%\NabyTune-update, checking its size and SHA-256 on the way. `install_and_restart`
//! starts it silently: the setup waits for this process to quit, installs over it and opens
//! the new version. Everything here blocks, so call it from a worker thread.
//!
//! The expected hash comes from the asset's `digest` (GitHub adds "sha256:<hex>" to every
//! asset it stores), or else from a NabyTune-Setup.exe.sha256 file published beside the setup,
//! as package.ps1 writes one.

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

/// Who is asking, for GitHub (which turns away requests without a User-Agent).
const AGENT: &str = "NabyTune-Updater";
/// The release asset that installs NabyTune.
const SETUP: &str = "NabyTune-Setup.exe";
/// Its checksum, when the release carries one: the hex SHA-256, optionally followed by a name.
const CHECKSUM: &str = "NabyTune-Setup.exe.sha256";

/// A release newer than this build, ready to download.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// The version, without the tag's "v": "0.2.0".
    pub version: String,
    /// The release notes, as written on GitHub (Markdown).
    pub notes: String,
    /// Where NabyTune-Setup.exe downloads from.
    pub url: String,
    /// The setup's size in bytes, as GitHub reports it (0 if it did not say).
    pub size: u64,
    /// The setup's SHA-256, lowercase hex, when the release says what it should be.
    pub sha256: Option<String>,
    /// The release's page on GitHub, for "What's new".
    pub page: String,
}

/// What the releases JSON gave: the release, and where to read its checksum from when the
/// asset itself carries none.
#[derive(Debug, PartialEq)]
pub struct Found {
    pub release: Release,
    pub checksum_url: Option<String>,
}

/// The latest release of `repo` ("owner/name"), if it is newer than `current` and has a setup
/// to install. None also when the repository has no published release yet.
pub fn check(http: &reqwest::blocking::Client, repo: &str, current: &str) -> Result<Option<Release>> {
    let valid = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    match repo.split_once('/') {
        Some((owner, name)) if valid(owner) && valid(name) => {}
        _ => bail!("not a GitHub repository: {repo:?} (expected owner/name)"),
    }
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let response = http
        .get(&url)
        .header("user-agent", AGENT)
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .timeout(Duration::from_secs(20))
        .send()
        .context("asking GitHub for the latest release")?;
    match response.status().as_u16() {
        // No release published yet (drafts and pre-releases do not count).
        404 => return Ok(None),
        403 | 429 => bail!("GitHub is limiting requests from this address; try again later"),
        _ => {}
    }
    let json: Value = response.error_for_status()?.json().context("reading GitHub's answer")?;
    let Some(found) = parse_release(&json, current)? else {
        return Ok(None);
    };
    let mut release = found.release;
    if release.sha256.is_none() {
        if let Some(url) = found.checksum_url {
            release.sha256 = Some(fetch_checksum(http, &url)?);
        }
    }
    Ok(Some(release))
}

/// Reads one release from GitHub's JSON: Some when it is a published release, newer than
/// `current`, with a NabyTune-Setup.exe among its assets.
pub fn parse_release(json: &Value, current: &str) -> Result<Option<Found>> {
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let tag = json["tag_name"].as_str().ok_or_else(|| anyhow!("the release has no tag"))?.trim();
    let version = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    if !newer(version, current) {
        return Ok(None);
    }
    let assets = json["assets"].as_array().map(Vec::as_slice).unwrap_or_default();
    let named = |name: &str| assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.trim().eq_ignore_ascii_case(name)));
    let Some(setup) = named(SETUP) else {
        return Ok(None);
    };
    let url = setup["browser_download_url"].as_str().filter(|u| u.starts_with("https://")).ok_or_else(|| anyhow!("{SETUP} has no download link"))?;
    let sha256 = setup["digest"].as_str().and_then(|d| d.trim().strip_prefix("sha256:")).and_then(sha256_hex);
    let checksum_url = named(CHECKSUM).and_then(|a| a["browser_download_url"].as_str()).filter(|u| u.starts_with("https://")).map(String::from);
    Ok(Some(Found {
        release: Release {
            version: version.to_string(),
            notes: json["body"].as_str().unwrap_or_default().trim().to_string(),
            url: url.to_string(),
            size: setup["size"].as_u64().unwrap_or(0),
            sha256,
            page: json["html_url"].as_str().unwrap_or_default().to_string(),
        },
        checksum_url,
    }))
}

/// A NabyTune-Setup.exe.sha256 file: the hash first, like sha256sum writes it.
fn fetch_checksum(http: &reqwest::blocking::Client, url: &str) -> Result<String> {
    let text = http
        .get(url)
        .header("user-agent", AGENT)
        .timeout(Duration::from_secs(20))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .context("reading the setup's checksum")?;
    text.split_whitespace().next().and_then(sha256_hex).ok_or_else(|| anyhow!("{CHECKSUM} does not hold a SHA-256"))
}

/// A SHA-256 written as 64 hex digits, lowercased; None for anything else.
fn sha256_hex(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())).then(|| text.to_ascii_lowercase())
}

// --- versions ----------------------------------------------------------------------------------

/// A version as releases are tagged: dotted numbers, maybe a pre-release suffix after "-"
/// ("0.2.0-beta.1"), maybe build metadata after "+" (which does not count).
struct Version {
    numbers: Vec<u64>,
    pre: Vec<String>,
}

fn version(text: &str) -> Option<Version> {
    let text = text.trim();
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let text = text.split('+').next().unwrap_or(text);
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, pre.split('.').map(String::from).collect()),
        None => (text, Vec::new()),
    };
    let numbers = core.split('.').map(|n| n.parse().ok()).collect::<Option<Vec<u64>>>()?;
    Some(Version { numbers, pre })
}

fn compare(a: &Version, b: &Version) -> Ordering {
    let len = a.numbers.len().max(b.numbers.len());
    for i in 0..len {
        // A missing part counts as 0: "1.2" is "1.2.0".
        let (x, y) = (a.numbers.get(i).copied().unwrap_or(0), b.numbers.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    match (a.pre.is_empty(), b.pre.is_empty()) {
        (true, true) => Ordering::Equal,
        // A pre-release comes before its release.
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for (x, y) in a.pre.iter().zip(&b.pre) {
                // Numbers compare as numbers and before words, as semver has it.
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
            a.pre.len().cmp(&b.pre.len())
        }
    }
}

/// Whether `candidate` is a later version than `current`. Either one unreadable: no.
pub fn newer(candidate: &str, current: &str) -> bool {
    match (version(candidate), version(current)) {
        (Some(c), Some(now)) => compare(&c, &now) == Ordering::Greater,
        _ => false,
    }
}

// --- download ----------------------------------------------------------------------------------

/// %TEMP%\NabyTune-update, where setups are downloaded to.
fn update_dir() -> PathBuf {
    std::env::temp_dir().join("NabyTune-update")
}

/// Downloads the release's setup to %TEMP%\NabyTune-update\NabyTune-Setup-<version>.exe,
/// telling `progress` (bytes so far, bytes in all, 0 when unknown) as it goes. The file is
/// checked against the size and SHA-256 the release gave; if it does not match, it is deleted
/// and this fails.
pub fn download(http: &reqwest::blocking::Client, release: &Release, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    let dir = update_dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let safe: String = release.version.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
    let name = format!("NabyTune-Setup-{safe}.exe");
    // Earlier downloads are of no more use.
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
    let part = dir.join(format!("{name}.part"));
    if let Err(e) = fetch_to(http, release, &part, &mut progress) {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    let path = dir.join(&name);
    let _ = fs::remove_file(&path);
    if let Err(e) = fs::rename(&part, &path) {
        let _ = fs::remove_file(&part);
        return Err(e).with_context(|| format!("saving {}", path.display()));
    }
    Ok(path)
}

fn fetch_to(http: &reqwest::blocking::Client, release: &Release, to: &Path, progress: &mut impl FnMut(u64, u64)) -> Result<()> {
    if !release.url.starts_with("https://") {
        bail!("refusing to download the update over {}", release.url);
    }
    // No overall timeout: the client's own applies to each read, so a stalled download fails
    // while a slow one finishes.
    let mut response = http.get(&release.url).header("user-agent", AGENT).send().and_then(|r| r.error_for_status()).context("downloading the update")?;
    let total = if release.size > 0 { release.size } else { response.content_length().unwrap_or(0) };
    let mut file = File::create(to).with_context(|| format!("creating {}", to.display()))?;
    let mut hash = Sha256::new()?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut head = [0u8; 2];
    progress(0, total);
    loop {
        let n = match response.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).context("downloading the update"),
        };
        if done < 2 {
            for (i, b) in buf[..n].iter().take(2 - done as usize).enumerate() {
                head[done as usize + i] = *b;
            }
        }
        file.write_all(&buf[..n]).with_context(|| format!("writing {}", to.display()))?;
        hash.update(&buf[..n])?;
        done += n as u64;
        if release.size > 0 && done > release.size {
            bail!("the update is larger than GitHub said ({} bytes)", release.size);
        }
        progress(done, total);
    }
    file.sync_all().with_context(|| format!("writing {}", to.display()))?;
    drop(file);
    if release.size > 0 && done != release.size {
        bail!("the update was cut short ({done} of {} bytes)", release.size);
    }
    if &head != b"MZ" {
        bail!("the download is not a Windows program");
    }
    if let Some(expected) = &release.sha256 {
        let got = hash.finish()?;
        if !got.eq_ignore_ascii_case(expected) {
            bail!("the update is damaged: its SHA-256 is {got}, not {expected}");
        }
    }
    Ok(())
}

/// Deletes whatever downloads are left in %TEMP%\NabyTune-update (best effort: a setup that is
/// still running stays until next time). Worth calling once at startup.
pub fn clean_downloads() {
    let _ = fs::remove_dir_all(update_dir());
}

/// SHA-256 through Windows' CNG, fed as the download comes in.
struct Sha256 {
    alg: windows_sys::Win32::Security::Cryptography::BCRYPT_ALG_HANDLE,
    hash: windows_sys::Win32::Security::Cryptography::BCRYPT_HASH_HANDLE,
}

impl Sha256 {
    fn new() -> Result<Self> {
        use windows_sys::Win32::Security::Cryptography::{BCryptCloseAlgorithmProvider, BCryptCreateHash, BCryptOpenAlgorithmProvider, BCRYPT_SHA256_ALGORITHM};
        unsafe {
            let mut alg = std::ptr::null_mut();
            let status = BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_SHA256_ALGORITHM, std::ptr::null(), 0);
            if status < 0 {
                bail!("SHA-256 is not available (0x{:08X})", status as u32);
            }
            let mut hash = std::ptr::null_mut();
            let status = BCryptCreateHash(alg, &mut hash, std::ptr::null_mut(), 0, std::ptr::null(), 0, 0);
            if status < 0 {
                BCryptCloseAlgorithmProvider(alg, 0);
                bail!("SHA-256 is not available (0x{:08X})", status as u32);
            }
            Ok(Self { alg, hash })
        }
    }

    fn update(&mut self, data: &[u8]) -> Result<()> {
        use windows_sys::Win32::Security::Cryptography::BCryptHashData;
        for chunk in data.chunks(u32::MAX as usize) {
            let status = unsafe { BCryptHashData(self.hash, chunk.as_ptr(), chunk.len() as u32, 0) };
            if status < 0 {
                bail!("hashing the update failed (0x{:08X})", status as u32);
            }
        }
        Ok(())
    }

    /// The digest as lowercase hex.
    fn finish(self) -> Result<String> {
        use windows_sys::Win32::Security::Cryptography::BCryptFinishHash;
        let mut out = [0u8; 32];
        let status = unsafe { BCryptFinishHash(self.hash, out.as_mut_ptr(), out.len() as u32, 0) };
        if status < 0 {
            bail!("hashing the update failed (0x{:08X})", status as u32);
        }
        Ok(out.iter().map(|b| format!("{b:02x}")).collect())
    }
}

impl Drop for Sha256 {
    fn drop(&mut self) {
        use windows_sys::Win32::Security::Cryptography::{BCryptCloseAlgorithmProvider, BCryptDestroyHash};
        unsafe {
            BCryptDestroyHash(self.hash);
            BCryptCloseAlgorithmProvider(self.alg, 0);
        }
    }
}

// --- install -----------------------------------------------------------------------------------

/// Whether this copy of NabyTune was put there by the setup (its uninstall.exe sits beside
/// it), so an update would replace it. A build run from target\ is not.
pub fn is_installed() -> bool {
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").is_file())).unwrap_or(false)
}

/// Starts the downloaded setup on its own, silently: it waits for this process to exit,
/// installs over it and launches the new version. Quit the app right after this returns Ok;
/// the setup closes it anyway if it is still running some 30 seconds later.
pub fn install_and_restart(setup: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    if !setup.is_file() {
        bail!("{} is gone", setup.display());
    }
    let pid = std::process::id().to_string();
    let spawn = |flags: u32| {
        Command::new(setup)
            .args(["--silent", "--launch", "--wait-pid", &pid])
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    // Out of any job this process is in, so the setup outlives it; not every job allows that.
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    spawn(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(base)).with_context(|| format!("starting {}", setup.display()))?;
    Ok(())
}

// --- tests -------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("v0.2.0", "0.1.9"));
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0", "0.99.99"));
        assert!(newer("0.1.1", "0.1"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
        assert!(!newer("V0.1.0", "v0.1.0"));
        // A pre-release sorts before its release, and after the one before.
        assert!(!newer("0.2.0-beta.1", "0.2.0"));
        assert!(newer("0.2.0", "0.2.0-beta.1"));
        assert!(newer("0.2.0-beta.1", "0.1.0"));
        assert!(newer("0.2.0-beta.2", "0.2.0-beta.1"));
        assert!(newer("0.2.0-beta.10", "0.2.0-beta.9"));
        assert!(newer("0.2.0-beta", "0.2.0-alpha"));
        assert!(newer("0.2.0-alpha.1", "0.2.0-alpha"));
        assert!(newer("0.2.0-alpha", "0.2.0-1"));
        // Build metadata does not count.
        assert!(!newer("0.1.0+abc", "0.1.0"));
        // Unreadable: never an update.
        assert!(!newer("latest", "0.1.0"));
        assert!(!newer("0.2.0", "dev"));
        assert!(!newer("", "0.1.0"));
        assert!(!newer("0..2", "0.1.0"));
    }

    const SAMPLE: &str = r#"{
        "html_url": "https://github.com/owner/nabytune/releases/tag/v0.2.0",
        "tag_name": "v0.2.0",
        "name": "NabyTune 0.2.0",
        "draft": false,
        "prerelease": false,
        "body": "Faster search.\r\nFixes.\n",
        "assets": [
            { "name": "NabyTune-Setup.exe.sha256", "size": 64, "digest": null,
              "browser_download_url": "https://github.com/owner/nabytune/releases/download/v0.2.0/NabyTune-Setup.exe.sha256" },
            { "name": "nabytune-setup.EXE", "size": 26214400,
              "digest": "sha256:ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
              "browser_download_url": "https://github.com/owner/nabytune/releases/download/v0.2.0/NabyTune-Setup.exe" }
        ]
    }"#;

    fn sample() -> Value {
        serde_json::from_str(SAMPLE).unwrap()
    }

    #[test]
    fn parses_a_release() {
        let found = parse_release(&sample(), "0.1.0").unwrap().unwrap();
        assert_eq!(
            found.release,
            Release {
                version: "0.2.0".into(),
                notes: "Faster search.\r\nFixes.".into(),
                url: "https://github.com/owner/nabytune/releases/download/v0.2.0/NabyTune-Setup.exe".into(),
                size: 26214400,
                sha256: Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".into()),
                page: "https://github.com/owner/nabytune/releases/tag/v0.2.0".into(),
            }
        );
        assert_eq!(found.checksum_url.as_deref(), Some("https://github.com/owner/nabytune/releases/download/v0.2.0/NabyTune-Setup.exe.sha256"));
    }

    #[test]
    fn skips_what_is_not_an_update() {
        assert_eq!(parse_release(&sample(), "0.2.0").unwrap(), None);
        assert_eq!(parse_release(&sample(), "0.3.0").unwrap(), None);
        for (key, value) in [("draft", true), ("prerelease", true)] {
            let mut json = sample();
            json[key] = Value::Bool(value);
            assert_eq!(parse_release(&json, "0.1.0").unwrap(), None);
        }
        let mut json = sample();
        json["assets"].as_array_mut().unwrap().remove(1);
        assert_eq!(parse_release(&json, "0.1.0").unwrap(), None);
        json["assets"] = Value::Null;
        assert_eq!(parse_release(&json, "0.1.0").unwrap(), None);
        assert!(parse_release(&serde_json::json!({ "message": "Not Found" }), "0.1.0").is_err());
    }

    #[test]
    fn checksum_without_digest() {
        let mut json = sample();
        json["assets"][1]["digest"] = Value::Null;
        let found = parse_release(&json, "0.1.0").unwrap().unwrap();
        assert_eq!(found.release.sha256, None);
        assert!(found.checksum_url.is_some());
        // A digest that is not SHA-256 is no help either.
        json["assets"][1]["digest"] = Value::String("md5:0123".into());
        assert_eq!(parse_release(&json, "0.1.0").unwrap().unwrap().release.sha256, None);
    }

    #[test]
    fn sha256_text() {
        let hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_hex(&hex.to_uppercase()).as_deref(), Some(hex));
        assert_eq!(sha256_hex(&format!(" {hex}\n")).as_deref(), Some(hex));
        assert_eq!(sha256_hex(&hex[1..]), None);
        assert_eq!(sha256_hex(&hex.replace('a', "g")), None);
    }

    #[test]
    fn sha256_of_abc() {
        let mut hash = Sha256::new().unwrap();
        hash.update(b"a").unwrap();
        hash.update(b"bc").unwrap();
        assert_eq!(hash.finish().unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
