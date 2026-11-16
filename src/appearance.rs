//! How NabyTune looks, from Settings › Appearance: the accent colour every highlight takes,
//! and the window's material (what shows through behind the page).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// NabyTune's own lime, the accent until another is picked.
pub const LIME: u32 = 0xc6f24e;

/// No colour at all: a soft, slightly warm white, quiet on any cover.
pub const NEUTRAL: u32 = 0xe9e6e0;

/// A neutral grey (a touch warm, as NEUTRAL) at lightness `l`, 0 to 1.
pub fn neutral(l: f32) -> u32 {
    let byte = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32).min(255);
    (byte(l) << 16) | (byte(l - 0.006) << 8) | byte(l - 0.02)
}

/// Whether a colour is a grey (or near one): the brightness strip then runs through greys.
pub fn is_neutral(rgb: u32) -> bool {
    let channel = |shift: u32| ((rgb >> shift) & 0xff) as i32;
    let (r, g, b) = (channel(16), channel(8), channel(0));
    r.max(g).max(b) - r.min(g).min(b) < 16
}

/// The accents to pick from, as in the web player: (name, colour).
pub const PRESETS: [(&str, u32); 8] = [
    ("Neutral", NEUTRAL),
    ("Lime", LIME),
    ("Sky", 0x4cc2ff),
    ("Mint", 0x3ddc97),
    ("Amber", 0xffb938),
    ("Coral", 0xff6f59),
    ("Rose", 0xff5c93),
    ("Lavender", 0xa99bff),
];

/// The accent until another is picked: neutral, so nothing clashes with any cover.
pub const DEFAULT: u32 = NEUTRAL;

/// The accent as drawn: lifted if it was too dark to read on the dark page.
static ACCENT: AtomicU32 = AtomicU32::new(DEFAULT);
/// The accent as picked (what shows when the cover isn't followed, or has no colour yet).
static PICKED: AtomicU32 = AtomicU32::new(DEFAULT);
/// The accent follows the playing song's cover.
static FOLLOW_COVER: AtomicBool = AtomicBool::new(false);
/// The playing song's cover colour, for "cover" (0 until a song has one).
static COVER: AtomicU32 = AtomicU32::new(0);

/// The accent, for highlights, fills and lit text.
pub fn accent() -> u32 {
    ACCENT.load(Ordering::Relaxed)
}

/// Text and icons on an accent fill: near-black on a light accent, white on a dark one.
pub fn on_accent() -> u32 {
    if luminance(accent()) > 0.4 { 0x111408 } else { 0xffffff }
}

/// The accent at `alpha` (0–255), for washes behind accent text.
pub fn accent_wash(alpha: u8) -> u32 {
    (accent() << 8) | alpha as u32
}

/// Takes `choice` ("system", or a colour as "#rrggbb") as the accent picked. While the
/// accent follows the cover, it's what shows until a song's cover gives a colour.
pub fn set_accent(choice: &str) {
    // Windows' accent is lifted until it reads on the dark page; a colour picked here is
    // taken as it is, dark or light, as the listener chose it.
    let colour = match choice {
        "system" => readable(system_accent().unwrap_or(DEFAULT)),
        hex => u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(DEFAULT),
    };
    PICKED.store(colour, Ordering::Relaxed);
    refresh();
}

/// Whether the accent takes the playing song's cover colour.
pub fn set_follow_cover(on: bool) {
    FOLLOW_COVER.store(on, Ordering::Relaxed);
    refresh();
}

pub fn follows_cover() -> bool {
    FOLLOW_COVER.load(Ordering::Relaxed)
}

/// The accent as it should be now: the cover's colour when it's followed and known, else the pick.
fn refresh() {
    let cover = COVER.load(Ordering::Relaxed);
    let colour = if FOLLOW_COVER.load(Ordering::Relaxed) && cover != 0 { cover } else { PICKED.load(Ordering::Relaxed) };
    ACCENT.store(colour, Ordering::Relaxed);
}

/// The playing song's cover colour (None: a cover with none to speak of). Taken as the
/// accent at once when the accent follows the cover. Whether the accent changed.
pub fn set_cover_colour(colour: Option<u32>) -> bool {
    let Some(colour) = colour else { return false };
    COVER.store(colour, Ordering::Relaxed);
    if FOLLOW_COVER.load(Ordering::Relaxed) && ACCENT.load(Ordering::Relaxed) != colour {
        ACCENT.store(colour, Ordering::Relaxed);
        return true;
    }
    false
}

/// The colour "cover" would give now, if a song has one.
pub fn cover_colour() -> Option<u32> {
    Some(COVER.load(Ordering::Relaxed)).filter(|c| *c != 0)
}

/// Windows' accent colour (Settings › Personalisation › Colours), as 0xRRGGBB.
pub fn system_accent() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\DWM\0".encode_utf16().collect();
    let name: Vec<u16> = "AccentColor\0".encode_utf16().collect();
    let mut value = 0u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size)
    };
    // Stored as 0xAABBGGRR.
    (status == 0).then(|| ((value & 0xff) << 16) | (value & 0xff00) | ((value >> 16) & 0xff))
}

/// The lightness Settings' brightness strip runs between: all but black, to all but white.
pub const DARKEST: f32 = 0.04;
pub const LIGHTEST: f32 = 0.86;

/// A colour by hue (0 to 1) and lightness (0 to 1), vivid: what Settings' strips pick.
pub fn hsl(hue: f32, l: f32) -> u32 {
    let s = 0.85f32;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = (hue.rem_euclid(1.0)) * 6.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let byte = |v: f32| (((v + m) * 255.0).round().max(0.0) as u32).min(255);
    (byte(r) << 16) | (byte(g) << 8) | byte(b)
}

/// A colour's HSL lightness, 0 to 1.
pub fn lightness_of(rgb: u32) -> f32 {
    let channel = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.0;
    let (r, g, b) = (channel(16), channel(8), channel(0));
    (r.max(g).max(b) + r.min(g).min(b)) / 2.0
}

/// A colour's hue, 0 to 1 (0 for greys).
pub fn hue_of(rgb: u32) -> f32 {
    let (r, g, b) = (((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d == 0.0 {
        return 0.0;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h / 6.0
}

/// Relative luminance (WCAG), 0 for black to 1 for white.
fn luminance(rgb: u32) -> f32 {
    let channel = |shift: u32| {
        let c = ((rgb >> shift) & 0xff) as f32 / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// A colour lifted towards white until it reads on NabyTune's dark page (a deep blue
/// Windows accent would vanish there).
fn readable(rgb: u32) -> u32 {
    let mut colour = rgb;
    let mut step = 0.0f32;
    while luminance(colour) < 0.28 && step < 1.0 {
        step += 0.05;
        let mix = |shift: u32| {
            let c = ((rgb >> shift) & 0xff) as f32;
            ((c + (255.0 - c) * step).round() as u32).min(255)
        };
        colour = (mix(16) << 16) | (mix(8) << 8) | mix(0);
    }
    colour
}

/// What shows behind the page.
#[derive(Clone, Copy, PartialEq)]
pub enum Material {
    /// The desktop wallpaper, softly tinted (Windows 11).
    Mica,
    /// Mica with more colour, as tabbed windows have.
    MicaAlt,
    /// Frosted glass over whatever is behind the window.
    Acrylic,
    /// No see-through: a plain dark backdrop, the cheapest to draw.
    Solid,
}

impl Material {
    pub const ALL: [Material; 4] = [Material::Mica, Material::MicaAlt, Material::Acrylic, Material::Solid];

    pub fn name(self) -> &'static str {
        match self {
            Material::Mica => "Mica",
            Material::MicaAlt => "Mica Alt",
            Material::Acrylic => "Acrylic",
            Material::Solid => crate::i18n::t("Solid"),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Material::Mica => "mica",
            Material::MicaAlt => "mica-alt",
            Material::Acrylic => "acrylic",
            Material::Solid => "solid",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.key() == key)
    }

    /// Asks Windows (DWM) for this material behind the window. GPUI's own switch only knows
    /// Mica and leaves it on under the others, so the system backdrop is set here directly.
    pub fn apply(self, window: &Window) {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        // DWMWA_SYSTEMBACKDROP_TYPE: 1 none, 2 Mica, 3 Acrylic, 4 Mica Alt.
        const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
        let kind: i32 = match self {
            Material::Mica => 2,
            Material::MicaAlt => 4,
            Material::Acrylic => 3,
            Material::Solid => 1,
        };
        let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
        unsafe {
            DwmSetWindowAttribute(handle.hwnd.get() as _, DWMWA_SYSTEMBACKDROP_TYPE, (&kind as *const i32).cast(), 4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The accent is one for the whole app: tests that set it take turns.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn lifts_dark_accents_and_picks_text() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        // A deep Windows blue is lifted until it reads; lime is left as it is.
        assert!(luminance(readable(0x003e92)) >= 0.28);
        assert_eq!(readable(LIME), LIME);
        set_accent("#c6f24e");
        assert_eq!(on_accent(), 0x111408);
        set_accent("#a99bff");
        assert_eq!(accent(), 0xa99bff);
    }

    #[test]
    fn picks_colours_round_the_wheel() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        // Red, green and blue sit at 0, a third and two thirds of the strip.
        let red = hsl(0.0, 0.64);
        assert!((red >> 16) & 0xff > 0xe0 && (red >> 8) & 0xff < 0x80);
        assert!((hue_of(hsl(1.0 / 3.0, 0.64)) - 1.0 / 3.0).abs() < 0.01);
        assert!((hue_of(hsl(0.62, 0.4)) - 0.62).abs() < 0.01);
        // Brightness is the listener's: dark stays dark, light stays light.
        assert!((lightness_of(hsl(0.6, 0.3)) - 0.3).abs() < 0.01);
        assert!((lightness_of(hsl(0.6, 0.86)) - 0.86).abs() < 0.01);
        set_accent("#1a3d80");
        assert_eq!(accent(), 0x1a3d80);
    }
}
