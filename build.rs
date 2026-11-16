//! Gives NabyTune.exe its icon (resource 1: GPUI takes it for the window and the taskbar too)
//! and version info, compiled by the Windows SDK's rc.exe. Without rc.exe it builds as before,
//! with no icon.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=assets/nabytune.ico");
    println!("cargo::rerun-if-env-changed=RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("assets").join("nabytune.ico");
    let Some(rc) = find_rc() else {
        println!("cargo::warning=rc.exe not found (Windows SDK): NabyTune.exe builds without an icon");
        return;
    };
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let nums: Vec<u16> = version.split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    let (a, b, c) = (nums.first().copied().unwrap_or(0), nums.get(1).copied().unwrap_or(0), nums.get(2).copied().unwrap_or(0));
    let mut src = String::new();
    if icon.is_file() {
        src += &format!("1 ICON \"{}\"\n", rc_path(&icon));
    }
    src += &format!(
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
      VALUE "FileDescription", "NabyTune"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "NabyTune"
      VALUE "LegalCopyright", "GPL-3.0-only"
      VALUE "OriginalFilename", "NabyTune.exe"
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
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let (rc_file, res_file) = (out.join("nabytune.rc"), out.join("nabytune.res"));
    fs::write(&rc_file, src).unwrap();
    let ok = Command::new(&rc).arg("/nologo").arg("/fo").arg(&res_file).arg(&rc_file).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        println!("cargo::warning={} failed: NabyTune.exe builds without an icon", rc.display());
        return;
    }
    // The app only: the sign-in helper and the examples keep none.
    println!("cargo::rustc-link-arg-bin=nabytune-gpui-proto={}", res_file.display());
}

fn rc_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}

/// The Windows SDK's rc.exe: `RC`, else the newest under Windows Kits, else PATH.
fn find_rc() -> Option<PathBuf> {
    if let Some(rc) = env::var_os("RC") {
        return Some(PathBuf::from(rc));
    }
    let host = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    for base in [env::var_os("ProgramFiles(x86)"), env::var_os("ProgramFiles")].into_iter().flatten().map(PathBuf::from) {
        let bin = base.join("Windows Kits").join("10").join("bin");
        let Ok(entries) = fs::read_dir(&bin) else { continue };
        let mut found: Vec<(Vec<u32>, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let v: Vec<u32> = e.file_name().to_string_lossy().split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
                let rc = e.path().join(host).join("rc.exe");
                rc.is_file().then_some((v, rc))
            })
            .collect();
        found.sort();
        if let Some((_, rc)) = found.pop() {
            return Some(rc);
        }
    }
    env::var_os("PATH").and_then(|path| env::split_paths(&path).map(|d| d.join("rc.exe")).find(|p| p.is_file()))
}
