//! The YouTube Music account: signing in through naby-login.exe (Google's own page), and
//! the session it hands back, kept encrypted for this Windows user (DPAPI) in
//! %APPDATA%\NabyTune\account.bin. Requests are signed as music.youtube.com signs its own:
//! the cookies, and `Authorization: SAPISIDHASH <time>_<sha1(time SAPISID origin)>`.

use std::path::PathBuf;
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

/// Opens the sign-in window and waits for it: the session's cookies, or None if the
/// window was closed first.
pub fn sign_in() -> Result<Option<String>> {
    let output = Command::new(helper()?).stdout(Stdio::piped()).stderr(Stdio::null()).output().context("starting the sign-in window")?;
    let cookie = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!cookie.is_empty()).then_some(cookie))
}

/// Forgets the session: the kept cookies, and the sign-in window's own browser data.
pub fn sign_out() {
    if let Some(path) = path() {
        let _ = std::fs::remove_file(path);
    }
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
    let secret = ["SAPISID", "__Secure-3PAPISID", "__Secure-1PAPISID"].iter().find_map(|name| cookie_value(cookie, name))?;
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
