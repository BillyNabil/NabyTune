//! Saving part of a window as a picture: the Wrapped card, as it is on screen, to a PNG
//! the listener can share.
//!
//! NabyTune's window is drawn by DirectX through DirectComposition, so copying from the
//! window's own device context comes out black. The screen's does not: what is copied is
//! what the compositor shows, so the window has to be visible and on top while it's taken.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

/// Saves a rectangle of a window's client area, as it appears on screen, as a PNG at `path`.
/// `x`, `y`, `width` and `height` are physical pixels from the client area's top-left; the
/// rectangle is cut to the client area. Something else covering the window is captured too.
pub fn save_window_region(hwnd: isize, x: i32, y: i32, width: i32, height: i32, path: &Path) -> anyhow::Result<()> {
    let image = capture(hwnd, x, y, width, height)?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    image.save_with_format(path, image::ImageFormat::Png).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The pixels of a rectangle of a window's client area, as on screen.
pub fn capture(hwnd: isize, x: i32, y: i32, width: i32, height: i32) -> anyhow::Result<image::RgbaImage> {
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

    let window = hwnd as HWND;
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if unsafe { GetClientRect(window, &mut client) } == 0 {
        bail!("no such window");
    }
    // Cut to the client area.
    let (left, top) = (x.max(0), y.max(0));
    let right = x.saturating_add(width).min(client.right);
    let bottom = y.saturating_add(height).min(client.bottom);
    let (w, h) = (right - left, bottom - top);
    if w <= 0 || h <= 0 {
        bail!("the rectangle is outside the window");
    }
    let mut origin = POINT { x: left, y: top };
    if unsafe { ClientToScreen(window, &mut origin) } == 0 {
        bail!("couldn't place the window on screen");
    }

    let mut pixels = vec![0u8; w as usize * h as usize * 4];
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            bail!("no screen to copy from");
        }
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let copied = if memory.is_null() || bitmap.is_null() {
            false
        } else {
            let old = SelectObject(memory, bitmap);
            // CAPTUREBLT takes in layered windows (menus, tooltips, translucent ones) too.
            let ok = BitBlt(memory, 0, 0, w, h, screen, origin.x, origin.y, SRCCOPY | CAPTUREBLT) != 0;
            SelectObject(memory, old);
            ok
        };
        let mut read = 0;
        if copied {
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative: rows top to bottom.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            // The bitmap must be out of any device context for this.
            read = GetDIBits(memory, bitmap, 0, h as u32, pixels.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        if !memory.is_null() {
            DeleteDC(memory);
        }
        ReleaseDC(std::ptr::null_mut(), screen);
        if !copied || read != h {
            bail!("couldn't copy from the screen");
        }
    }
    // BGRA to RGBA; the screen has no alpha of its own.
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    image::RgbaImage::from_raw(w as u32, h as u32, pixels).context("pixel buffer of the wrong size")
}

/// The listener's Pictures folder (wherever Windows keeps it, OneDrive included), else
/// %USERPROFILE%\Pictures, else the temp folder.
pub fn pictures_dir() -> PathBuf {
    use windows_sys::Win32::UI::Shell::{SHGetFolderPathW, CSIDL_MYPICTURES};
    let mut buf = [0u16; 260];
    let found = unsafe { SHGetFolderPathW(std::ptr::null_mut(), CSIDL_MYPICTURES as i32, std::ptr::null_mut(), 0, buf.as_mut_ptr()) };
    if found == 0 {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let dir = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
        if dir.is_dir() {
            return dir;
        }
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let dir = PathBuf::from(profile).join("Pictures");
        if dir.is_dir() {
            return dir;
        }
    }
    std::env::temp_dir()
}

/// A PNG path in `dir` named after `stem` ("NabyTune Wrapped 2026-11.png") that isn't
/// taken yet, adding " (2)", " (3)" and so on if it is. Characters Windows doesn't allow
/// in file names become "-".
pub fn unique_name(dir: &Path, stem: &str) -> PathBuf {
    let stem: String = stem.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '-' } else { c }).collect();
    let stem = stem.trim().trim_end_matches('.');
    let stem = if stem.is_empty() { "NabyTune" } else { stem };
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).png")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}
