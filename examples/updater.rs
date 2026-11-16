//! Tries the updater without installing anything: reads a canned release, asks GitHub for a
//! repository's latest release, and with --download fetches that release's smallest .exe
//! through `updater::download`, checked against GitHub's own SHA-256 of it, then deletes it.
//! `cargo run --example updater -- [owner/name] [current version] [--download]`

#[path = "../src/updater.rs"]
#[allow(dead_code)]
mod updater;

use std::time::Duration;

const SAMPLE: &str = r#"{
    "html_url": "https://github.com/owner/nabytune/releases/tag/v0.2.0",
    "tag_name": "v0.2.0",
    "draft": false,
    "prerelease": false,
    "body": "Faster search.",
    "assets": [
        { "name": "NabyTune-Setup.exe", "size": 26214400,
          "digest": "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
          "browser_download_url": "https://github.com/owner/nabytune/releases/download/v0.2.0/NabyTune-Setup.exe" }
    ]
}"#;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let fetch = args.iter().any(|a| a == "--download");
    args.retain(|a| a != "--download");
    let repo = args.first().cloned().unwrap_or_else(|| "notepad-plus-plus/notepad-plus-plus".into());
    let current = args.get(1).cloned().unwrap_or_else(|| "0.1.0".into());

    // 1. The parser, on a release that has a setup.
    let sample: serde_json::Value = serde_json::from_str(SAMPLE)?;
    println!("canned, from 0.1.0: {:#?}", updater::parse_release(&sample, "0.1.0")?);
    println!("canned, from 0.2.0: {:?}", updater::parse_release(&sample, "0.2.0")?);

    // 2. The live API path. Another project's release has no NabyTune-Setup.exe: expect None.
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(20)).build()?;
    println!("check({repo}, {current}) = {:?}", updater::check(&http, &repo, &current)?);
    println!("is_installed() = {}", updater::is_installed());

    // 3. The download path, on a real asset of that release.
    if fetch {
        let json: serde_json::Value = http
            .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
            .header("user-agent", "NabyTune-Updater")
            .send()?
            .error_for_status()?
            .json()?;
        let asset = json["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["name"].as_str().is_some_and(|n| n.to_lowercase().ends_with(".exe")))
            .min_by_key(|a| a["size"].as_u64().unwrap_or(u64::MAX))
            .ok_or_else(|| anyhow::anyhow!("{repo}'s latest release has no .exe"))?;
        let mut release = updater::Release {
            version: json["tag_name"].as_str().unwrap_or("test").trim_start_matches('v').to_string(),
            notes: String::new(),
            url: asset["browser_download_url"].as_str().unwrap_or_default().to_string(),
            size: asset["size"].as_u64().unwrap_or(0),
            sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(String::from),
            page: String::new(),
        };
        println!("downloading {} ({} bytes, sha256 {:?})", release.url, release.size, release.sha256);
        let mut last = 0;
        let path = updater::download(&http, &release, |done, total| {
            let pct = if total > 0 { done * 100 / total } else { 0 };
            if pct >= last + 25 || done == total {
                println!("  {done} / {total} ({pct}%)");
                last = pct;
            }
        })?;
        println!("verified and saved: {} ({} bytes)", path.display(), std::fs::metadata(&path)?.len());

        // A wrong hash must fail and leave nothing behind.
        release.sha256 = Some("0".repeat(64));
        match updater::download(&http, &release, |_, _| {}) {
            Ok(p) => anyhow::bail!("a wrong SHA-256 was accepted: {}", p.display()),
            Err(e) => println!("wrong SHA-256 refused: {e:#}"),
        }
        let left: Vec<_> = std::fs::read_dir(std::env::temp_dir().join("NabyTune-update"))?.flatten().map(|e| e.file_name()).collect();
        println!("left in NabyTune-update: {left:?}");
        updater::clean_downloads();
    }
    Ok(())
}
