//! The YouTube Music account: signing in through naby-login.exe (Google's own page), and
//! the session it hands back, kept encrypted for this Windows user (DPAPI) in
//! %APPDATA%\NabyTune\account.bin. Requests are signed as music.youtube.com signs its own:
//! the cookies, and `Authorization: SAPISIDHASH <time>_<sha1(time SAPISID origin)>`.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::process::{Command, Stdio};

use anyhow::{bail, Context as _, Result};

const ORIGIN: &str = "https://music.youtube.com";

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("NabyTune").join("account.bin"))
}

/// naby-login.exe, beside NabyTune's own exe.
fn helper() -> Result<PathBuf> {
    let me = std::env::current_exe()?;
    let helper = me.with_file_name("naby-login.exe");
    if !helper.exists() {
        bail!("naby-login.exe is missing beside {}", me.display());
    }
    Ok(helper)
}

#[derive(Debug)]
pub enum SignInIssue {
    Timeout,
    WebView,
    MissingSession,
}

impl SignInIssue {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Timeout => "Sign-in timed out. Please try again.",
            Self::WebView => "Couldn't open the sign-in window. Check that Microsoft Edge WebView2 is installed, then try again.",
            Self::MissingSession => "YouTube Music did not return a session. Please try again.",
        }
    }
}

impl std::fmt::Display for SignInIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for SignInIssue {}

/// Opens the sign-in window and waits for it: the session's cookies, or None if the
/// window was closed first.
pub fn sign_in(cancel: &AtomicBool) -> Result<Option<String>> {
    let mut child = Command::new(helper()?).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("starting the sign-in window")?;
    let mut stdout = child.stdout.take().context("opening the sign-in response")?;
    let reader = std::thread::spawn(move || {
        let mut response = String::new();
        stdout.read_to_string(&mut response)?;
        Ok::<_, std::io::Error>(response)
    });
    let started = Instant::now();
    let status = loop {
        let cancelled = cancel.load(Ordering::Relaxed);
        if cancelled || started.elapsed() >= Duration::from_secs(300) {
            let _ = child.kill();
            let _ = child.wait();
            return if cancelled {
                Ok(None)
            } else {
                Err(SignInIssue::Timeout.into())
            };
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    anyhow::ensure!(status.success(), SignInIssue::WebView);
    let response = reader.join().map_err(|_| anyhow::anyhow!("reading the sign-in response"))??;
    let cookie = response.trim();
    if cookie.is_empty() || cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    anyhow::ensure!(authorization(cookie, 0).is_some(), SignInIssue::MissingSession);
    Ok(Some(cookie.to_owned()))
}

/// Removes the saved session before the app returns to guest mode.
pub fn sign_out() -> Result<()> {
    let path = path().context("no %APPDATA%")?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("removing the saved YouTube Music session"),
    }
}

pub fn clear_legacy_browser_data() {
    if let Ok(helper) = helper() {
        let _ = Command::new(helper).arg("--sign-out").status();
    }
}

pub fn save(cookie: &str) -> Result<()> {
    let path = path().context("no %APPDATA%")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, protect(cookie.as_bytes(), true)?)?;
    Ok(())
}

pub fn load() -> Option<String> {
    let sealed = std::fs::read(path()?).ok()?;
    String::from_utf8(protect(&sealed, false).ok()?).ok()
}

/// DPAPI: seals bytes so only this Windows user can open them (`seal` false opens them).
fn protect(data: &[u8], seal: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        if seal {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut output)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut output)
        }
    };
    if ok == 0 {
        bail!("Windows couldn't {} the account data", if seal { "protect" } else { "read" });
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe { LocalFree(output.pbData as _) };
    Ok(bytes)
}

/// The value of cookie `name` in a `Cookie` header.
fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie.split(';').filter_map(|pair| pair.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

/// The `Authorization` header for a request at unix time `now`, signed with the session's
/// SAPISID (or its secure twins).
pub fn authorization(cookie: &str, now: u64) -> Option<String> {
    let secret = ["SAPISID", "__Secure-3PAPISID", "__Secure-1PAPISID"].iter().find_map(|name| cookie_value(cookie, name).filter(|value| !value.is_empty()))?;
    let digest = sha1_smol::Sha1::from(format!("{now} {secret} {ORIGIN}")).digest().to_string();
    Some(format!("SAPISIDHASH {now}_{digest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_with_sapisid() {
        let cookie = "PREF=x; SAPISID=abc123/def; __Secure-3PAPISID=zzz";
        // sha1("1700000000 abc123/def https://music.youtube.com")
        let expected = sha1_smol::Sha1::from("1700000000 abc123/def https://music.youtube.com").digest().to_string();
        assert_eq!(authorization(cookie, 1_700_000_000), Some(format!("SAPISIDHASH 1700000000_{expected}")));
        assert_eq!(authorization("PREF=x", 1), None);
        assert_eq!(cookie_value("a=1; b=2", "b"), Some("2"));
    }

    #[test]
    fn seals_and_opens() {
        let sealed = protect(b"SAPISID=secret", true).unwrap();
        assert_ne!(sealed.as_slice(), b"SAPISID=secret");
        assert_eq!(protect(&sealed, false).unwrap(), b"SAPISID=secret");
    }
}
