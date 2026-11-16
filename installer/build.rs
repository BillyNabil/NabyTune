//! Packs the payload and gives the setup exe its resources, with std and miniz_oxide only.
//!
//! - `NABY_PAYLOAD_DIR` (required): a folder whose files are installed side by side. It must hold
//!   `NabyTune.exe`; package.ps1 stages `naby-login.exe` and `nabytune.ico` beside it.
//! - `NABY_VERSION` (optional): the version shown and registered; otherwise gpui-proto's
//!   Cargo.toml `version`, otherwise this crate's.
//! - `NABY_SETUP_ICON` (optional): the setup exe's own icon; otherwise `nabytune.ico` from the
//!   payload, otherwise `../assets/nabytune.ico`. Without one the exe gets no icon.
//!
//! Resources (icon, comctl32 v6 manifest, version info) are compiled by the Windows SDK's
//! rc.exe (found under Windows Kits, or `RC` / PATH) and handed to link.exe as a .res. Without
//! rc.exe the setup still builds, falls back to plain message boxes, and has no icon.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(naby_icon)");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=NABY_PAYLOAD_DIR");
    println!("cargo::rerun-if-env-changed=NABY_VERSION");
    println!("cargo::rerun-if-env-changed=NABY_SETUP_ICON");
    println!("cargo::rerun-if-env-changed=RC");

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    // --- payload ---------------------------------------------------------------------------
    let payload_dir = match env::var_os("NABY_PAYLOAD_DIR") {
        Some(d) => PathBuf::from(d),
        None => panic!(
            "NABY_PAYLOAD_DIR is not set: point it at a folder holding NabyTune.exe (and \
             naby-login.exe, nabytune.ico). package.ps1 stages one in target\\package\\payload."
        ),
    };
    println!("cargo::rerun-if-changed={}", payload_dir.display());
    let mut files: Vec<(String, PathBuf)> = fs::read_dir(&payload_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", payload_dir.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    files.sort_by_key(|(n, _)| n.to_lowercase());
    // Windows names are case-insensitive; install the app as exactly "NabyTune.exe".
    for (name, _) in files.iter_mut() {
        if name.eq_ignore_ascii_case("NabyTune.exe") {
            *name = "NabyTune.exe".into();
        }
    }
    if !files.iter().any(|(n, _)| n == "NabyTune.exe") {
        panic!("{} has no NabyTune.exe", payload_dir.display());
    }
    if files.iter().any(|(n, _)| n.eq_ignore_ascii_case("uninstall.exe")) {
        panic!("the payload must not contain uninstall.exe (setup writes its own)");
    }

    // Archive: u32 count, then per file u16 name length, UTF-8 name, u64 length, bytes.
    let mut blob = Vec::new();
    blob.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (name, path) in &files {
        println!("cargo::rerun-if-changed={}", path.display());
        let data = fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
        blob.extend_from_slice(name.as_bytes());
        blob.extend_from_slice(&(data.len() as u64).to_le_bytes());
        blob.extend_from_slice(&data);
    }
    let raw_len = blob.len();
    let packed = miniz_oxide::deflate::compress_to_vec(&blob, 9);
    fs::write(out.join("payload.deflate"), &packed).unwrap();
    println!(
        "cargo::warning=payload: {} files, {:.1} MB -> {:.1} MB deflated",
        files.len(),
        raw_len as f64 / 1e6,
        packed.len() as f64 / 1e6
    );

    // --- version ---------------------------------------------------------------------------
    let version = env::var("NABY_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            let toml = manifest_dir.join("..").join("Cargo.toml");
            println!("cargo::rerun-if-changed={}", toml.display());
            package_version(&fs::read_to_string(toml).ok()?)
        })
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap());

    let names: Vec<String> = files.iter().map(|(n, _)| format!("{n:?}")).collect();
    let generated = format!(
        "pub const VERSION: &str = {version:?};\n\
         pub const FILES: &[&str] = &[{}];\n\
         pub const RAW_LEN: usize = {raw_len};\n\
         pub static PACKED: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/payload.deflate\"));\n",
        names.join(", ")
    );
    fs::write(out.join("payload.rs"), generated).unwrap();

    // --- resources -------------------------------------------------------------------------
    let icon = env::var_os("NABY_SETUP_ICON")
        .map(PathBuf::from)
        .or_else(|| {
            files
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("nabytune.ico"))
                .map(|(_, p)| p.clone())
        })
        .or_else(|| Some(manifest_dir.join("..").join("assets").join("nabytune.ico")))
        .filter(|p| p.is_file());
    if let Some(icon) = &icon {
        println!("cargo::rerun-if-changed={}", icon.display());
    }

    let Some(rc) = find_rc() else {
        println!(
            "cargo::warning=rc.exe not found (Windows SDK): building without an icon, manifest \
             or version info; dialogs fall back to message boxes"
        );
        return;
    };

    let manifest = out.join("setup.manifest");
    fs::write(&manifest, MANIFEST).unwrap();
    let nums: Vec<u16> = version
        .split(|c: char| c == '.' || c == '-' || c == '+')
        .take(3)
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0))
        .collect();
    let (a, b, c) = (
        nums.first().copied().unwrap_or(0),
        nums.get(1).copied().unwrap_or(0),
        nums.get(2).copied().unwrap_or(0),
    );
    let mut rc_src = String::new();
    if let Some(icon) = &icon {
        rc_src += &format!("1 ICON \"{}\"\n", rc_path(icon));
    }
    // 24 = RT_MANIFEST, 1 = CREATEPROCESS_MANIFEST_RESOURCE_ID.
    rc_src += &format!("1 24 \"{}\"\n", rc_path(&manifest));
    rc_src += &format!(
        r#"1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEFLAGSMASK 0x3F
FILEFLAGS 0
FILEOS 0x40004
FILETYPE 1
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "NabyTune"
      VALUE "FileDescription", "NabyTune Setup"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "NabyTune-Setup"
      VALUE "LegalCopyright", "GPL-3.0-only"
      VALUE "OriginalFilename", "NabyTune-Setup.exe"
      VALUE "ProductName", "NabyTune"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let rc_file = out.join("setup.rc");
    let res_file = out.join("setup.res");
    fs::write(&rc_file, rc_src).unwrap();
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .status()
        .unwrap_or_else(|e| panic!("running {}: {e}", rc.display()));
    if !status.success() {
        panic!("{} failed on {}", rc.display(), rc_file.display());
    }
    // link.exe takes a compiled .res as an ordinary input.
    println!("cargo::rustc-link-arg-bins={}", res_file.display());
    if icon.is_some() {
        println!("cargo::rustc-cfg=naby_icon");
    } else {
        println!("cargo::warning=no icon found (NABY_SETUP_ICON, payload nabytune.ico, ../assets/nabytune.ico): setup has no icon");
    }
}

/// `version = "..."` from a Cargo.toml's [package] table.
fn package_version(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package {
            if let Some(rest) = line.strip_prefix("version") {
                let rest = rest.trim_start().strip_prefix('=')?.trim();
                return Some(rest.trim_matches('"').to_string());
            }
        }
    }
    None
}

fn rc_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}

fn find_rc() -> Option<PathBuf> {
    if let Some(rc) = env::var_os("RC") {
        return Some(PathBuf::from(rc));
    }
    let host = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let kits = [
        env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        env::var_os("ProgramFiles").map(PathBuf::from),
    ];
    for base in kits.into_iter().flatten() {
        let bin = base.join("Windows Kits").join("10").join("bin");
        let Ok(entries) = fs::read_dir(&bin) else { continue };
        let mut versions: Vec<(Vec<u32>, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let v: Vec<u32> = name.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
                let rc = e.path().join(host).join("rc.exe");
                rc.is_file().then_some((v, rc))
            })
            .collect();
        versions.sort();
        if let Some((_, rc)) = versions.pop() {
            return Some(rc);
        }
        let rc = bin.join(host).join("rc.exe");
        if rc.is_file() {
            return Some(rc);
        }
    }
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path).map(|d| d.join("rc.exe")).find(|p| p.is_file())
    })
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="NabyTune.Setup" version="1.0.0.0" processorArchitecture="*"/>
  <description>NabyTune Setup</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#;
