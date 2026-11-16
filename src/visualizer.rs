//! The full-screen player's visuals that move with the music: a glow in the accent that
//! breathes with the beat, and a spectrum along the bottom. Both read the audio thread's
//! scope (dsp.rs) every frame while they show, and cost nothing while they're off.

use gpui::{div, prelude::*, px, rgba, AnyElement, Window};

use crate::appearance::accent;
use crate::i18n::t;
use crate::NabyTune;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Visual {
    Off,
    /// The accent glow, breathing with the beat.
    Pulse,
    /// The spectrum along the bottom.
    Bars,
    Both,
}

impl Visual {
    pub const ALL: [Visual; 4] = [Visual::Off, Visual::Pulse, Visual::Bars, Visual::Both];

    /// What it's called, in the UI's language.
    pub fn name(self) -> &'static str {
        t(match self {
            Visual::Off => "Off",
            Visual::Pulse => "Pulse",
            Visual::Bars => "Spectrum",
            Visual::Both => "Pulse and spectrum",
        })
    }

    pub fn key(self) -> &'static str {
        match self {
            Visual::Off => "off",
            Visual::Pulse => "pulse",
            Visual::Bars => "bars",
            Visual::Both => "both",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == key)
    }
}

/// How many bars the spectrum has.
const BARS: usize = 48;

impl NabyTune {
    /// The visuals over the full-screen backdrop (under everything else), while music plays.
    pub(crate) fn visualizer(&self, window: &mut Window) -> Option<AnyElement> {
        if self.visual == Visual::Off || !self.playing || self.loading {
            return None;
        }
        // Every frame while it shows: the scope's readings only mean something when they're
        // read often (the beat is a rise between readings).
        window.request_animation_frame();
        let scope = self.audio.scope();
        let level = scope.level();
        let beat = scope.beat();
        let colour = accent();
        let mut layer = div().absolute().inset_0().overflow_hidden();
        if matches!(self.visual, Visual::Pulse | Visual::Both) {
            let viewport = window.viewport_size();
            let side = f32::from(viewport.width).min(f32::from(viewport.height)) * (0.78 + beat * 0.16 + level * 0.06);
            let alpha = ((0.10 + beat * 0.20 + level * 0.12).min(0.42) * 255.0) as u32;
            layer = layer.child(
                div().absolute().inset_0().flex().items_center().justify_center().child(
                    div().size(px(side)).flex_none().rounded_full().bg(rgba((colour << 8) | alpha)).blur(px(side * 0.22)),
                ),
            );
        }
        if matches!(self.visual, Visual::Bars | Visual::Both) {
            let bands = scope.bands(BARS);
            let mut row = div().absolute().left_0().right_0().bottom_0().h(px(170.0)).px(px(48.0)).flex().items_end().gap(px(4.0));
            for (i, level) in bands.iter().enumerate() {
                // The middle bars a little brighter, the edges fading out.
                let edge = 1.0 - ((i as f32 / (BARS - 1) as f32) - 0.5).abs() * 0.9;
                let alpha = ((0.18 + level * 0.32) * edge * 255.0) as u32;
                row = row.child(div().flex_1().h(px(3.0 + level * 150.0)).rounded_t(px(3.0)).bg(rgba((colour << 8) | alpha.min(255))));
            }
            layer = layer.child(row);
        }
        Some(layer.into_any_element())
    }
}
