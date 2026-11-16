//! The equalizer: five bands (a low shelf, three peaks, a high shelf) on what plays, set
//! from Settings › Playback while the song goes on. The gains live in shared atomics the
//! audio thread reads; the filters are recomputed when they change.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The bands' centres, in Hz.
pub const BANDS: [f32; 5] = [60.0, 230.0, 910.0, 3600.0, 12000.0];
/// How far a band goes, either way, in dB.
pub const RANGE: f32 = 12.0;

/// Ready-made curves: (name, gains in dB per band).
pub const PRESETS: [(&str, [f32; 5]); 6] = [
    ("Flat", [0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass", [6.0, 3.5, 0.0, 0.0, 0.5]),
    ("Vocal", [-2.0, -0.5, 2.5, 3.0, 1.0]),
    ("Treble", [0.0, 0.0, 0.5, 3.0, 5.5]),
    ("Loud", [4.5, 1.5, -1.0, 1.5, 4.0]),
    ("Soft", [-1.5, 0.0, 0.5, -1.0, -3.5]),
];

/// The settings the audio thread reads: on or off, and each band's gain (in 1/100 dB).
pub struct Params {
    enabled: AtomicBool,
    gains: [AtomicI32; 5],
    /// Counts changes, so filters are rebuilt only when something moved.
    version: AtomicU32,
}

impl Params {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { enabled: AtomicBool::new(false), gains: Default::default(), version: AtomicU32::new(1) })
    }

    pub fn set(&self, enabled: bool, gains: [f32; 5]) {
        self.enabled.store(enabled, Ordering::Relaxed);
        for (slot, g) in self.gains.iter().zip(gains) {
            slot.store((g.clamp(-RANGE, RANGE) * 100.0).round() as i32, Ordering::Relaxed);
        }
        self.version.fetch_add(1, Ordering::Release);
    }

    fn read(&self) -> (bool, [f32; 5]) {
        let mut gains = [0.0; 5];
        for (g, slot) in gains.iter_mut().zip(&self.gains) {
            *g = slot.load(Ordering::Relaxed) as f32 / 100.0;
        }
        (self.enabled.load(Ordering::Relaxed), gains)
    }
}

/// Gains as kept in settings: "g1,g2,g3,g4,g5".
pub fn parse(text: &str) -> Option<[f32; 5]> {
    let values: Vec<f32> = text.split(',').filter_map(|v| v.trim().parse().ok()).collect();
    (values.len() == 5).then(|| [values[0], values[1], values[2], values[3], values[4]])
}

pub fn format(gains: &[f32; 5]) -> String {
    gains.iter().map(|g| format!("{g:.1}")).collect::<Vec<_>>().join(",")
}

/// The preset these gains are, if any.
pub fn preset_of(gains: &[f32; 5]) -> Option<&'static str> {
    PRESETS.iter().find(|(_, p)| p.iter().zip(gains).all(|(a, b)| (a - b).abs() < 0.05)).map(|(n, _)| *n)
}

#[derive(Clone, Copy, Default)]
struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Clone, Copy, Default)]
struct State {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

enum Shape {
    LowShelf,
    Peak,
    HighShelf,
}

/// RBJ cookbook biquads.
fn design(shape: Shape, freq: f32, gain_db: f32, rate: f32) -> Coefficients {
    let freq = freq.min(rate * 0.45);
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / rate;
    let (sin, cos) = w0.sin_cos();
    let (b0, b1, b2, a0, a1, a2) = match shape {
        Shape::Peak => {
            let alpha = sin / (2.0 * 1.0);
            (1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a)
        }
        Shape::LowShelf | Shape::HighShelf => {
            // Shelf slope 1.
            let alpha = sin / 2.0 * 2f32.sqrt();
            let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
            match shape {
                Shape::LowShelf => (
                    a * ((a + 1.0) - (a - 1.0) * cos + two_sqrt_a_alpha),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - two_sqrt_a_alpha),
                    (a + 1.0) + (a - 1.0) * cos + two_sqrt_a_alpha,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - two_sqrt_a_alpha,
                ),
                _ => (
                    a * ((a + 1.0) + (a - 1.0) * cos + two_sqrt_a_alpha),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - two_sqrt_a_alpha),
                    (a + 1.0) - (a - 1.0) * cos + two_sqrt_a_alpha,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - two_sqrt_a_alpha,
                ),
            }
        }
    };
    Coefficients { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
}

const MAX_CHANNELS: usize = 8;

/// A source with the equalizer on it.
pub struct Equalized<S> {
    inner: S,
    params: Arc<Params>,
    version: u32,
    on: bool,
    rate: u32,
    channels: u16,
    channel: usize,
    /// Turns the whole down by the biggest boost, so boosts don't clip.
    preamp: f32,
    filters: [Coefficients; 5],
    states: [[State; 5]; MAX_CHANNELS],
}

impl<S: rodio::Source<Item = f32>> Equalized<S> {
    pub fn new(inner: S, params: Arc<Params>) -> Self {
        let mut eq = Self {
            rate: inner.sample_rate(),
            channels: inner.channels(),
            inner,
            params,
            version: 0,
            on: false,
            channel: 0,
            preamp: 1.0,
            filters: [Coefficients::default(); 5],
            states: [[State::default(); 5]; MAX_CHANNELS],
        };
        eq.refresh();
        eq
    }

    fn refresh(&mut self) {
        let version = self.params.version.load(Ordering::Acquire);
        let (rate, channels) = (self.inner.sample_rate(), self.inner.channels());
        if version == self.version && rate == self.rate && channels == self.channels {
            return;
        }
        let (on, gains) = self.params.read();
        self.version = version;
        self.rate = rate.max(1);
        self.channels = channels.max(1);
        self.on = on && gains.iter().any(|g| g.abs() > 0.05);
        let rate = self.rate as f32;
        for (i, (freq, gain)) in BANDS.iter().zip(gains).enumerate() {
            // The shelves' corners sit past their band, so the band named gets the whole gain.
            let (shape, corner) = match i {
                0 => (Shape::LowShelf, freq * 1.8),
                4 => (Shape::HighShelf, freq / 1.8),
                _ => (Shape::Peak, *freq),
            };
            self.filters[i] = design(shape, corner, gain, rate);
        }
        let boost = gains.iter().cloned().fold(0.0f32, f32::max);
        self.preamp = 10f32.powf(-boost * 0.7 / 20.0);
    }
}

impl<S: rodio::Source<Item = f32>> Iterator for Equalized<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        if self.channel == 0 {
            self.refresh();
        }
        let channel = self.channel.min(MAX_CHANNELS - 1);
        self.channel = (self.channel + 1) % self.channels as usize;
        if !self.on {
            return Some(sample);
        }
        let mut x = sample * self.preamp;
        for (f, s) in self.filters.iter().zip(self.states[channel].iter_mut()) {
            let y = f.b0 * x + f.b1 * s.x1 + f.b2 * s.x2 - f.a1 * s.y1 - f.a2 * s.y2;
            s.x2 = s.x1;
            s.x1 = x;
            s.y2 = s.y1;
            s.y1 = y;
            x = y;
        }
        // A soft knee past full scale rather than a hard clip.
        Some(if x.abs() > 0.95 { x.signum() * (0.95 + (x.abs() - 0.95).tanh() * 0.05) } else { x })
    }
}

impl<S: rodio::Source<Item = f32>> rodio::Source for Equalized<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)?;
        self.states = [[State::default(); 5]; MAX_CHANNELS];
        self.channel = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine's loudness through the equalizer, relative to without.
    fn gain_at(freq: f32, gains: [f32; 5]) -> f32 {
        let rate = 48_000u32;
        let samples: Vec<f32> = (0..rate as usize).map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / rate as f32).sin() * 0.25).collect();
        let params = Params::new();
        params.set(true, gains);
        let source = rodio::buffer::SamplesBuffer::new(1, rate, samples);
        let out: Vec<f32> = Equalized::new(source, params).skip(rate as usize / 2).collect();
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        let reference = 0.25 / 2f32.sqrt();
        20.0 * (rms / reference).log10()
    }

    #[test]
    fn boosts_and_cuts_the_right_band() {
        // Bass +6: 60 Hz louder than 3.6 kHz, by about the boost.
        let bass = [6.0, 0.0, 0.0, 0.0, 0.0];
        let low = gain_at(60.0, bass);
        let high = gain_at(3600.0, bass);
        assert!(low - high > 4.0, "low {low:.1} high {high:.1}");
        // A middle cut: 910 Hz quieter than 60 Hz.
        let cut = [0.0, 0.0, -9.0, 0.0, 0.0];
        assert!(gain_at(910.0, cut) < gain_at(60.0, cut) - 6.0);
        // Flat is untouched.
        assert!(gain_at(1000.0, [0.0; 5]).abs() < 0.2);
    }

    #[test]
    fn keeps_presets_and_text() {
        let g = PRESETS[1].1;
        assert_eq!(parse(&format(&g)), Some(g));
        assert_eq!(preset_of(&g), Some("Bass"));
        assert_eq!(preset_of(&[1.0, 2.0, 3.0, 4.0, 5.0]), None);
    }
}
