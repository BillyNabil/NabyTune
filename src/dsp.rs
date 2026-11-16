//! What the visuals hear.
//!
//! A `Tap` sits in a song's source and copies a mono mix of what plays into a `Scope`, a
//! ring of samples the audio thread writes without ever waiting on the UI. The UI reads the
//! scope up to sixty times a second: the spectrum as log-spaced bands, the loudness, and a
//! beat pulse for a background that breathes with the music. When nothing new arrives
//! (paused, stopped) the readings fall to rest on their own.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Samples the spectrum is taken over (about 43 ms at 48 kHz).
const FFT: usize = 2048;
/// The ring's length in mono samples: twice the window, so a read never meets the writer.
const RING: usize = 8192;
/// How many mono samples a tap gathers before it writes them out.
const CHUNK: usize = 128;
/// The spectrum's range for `bands`, in Hz.
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;
/// A band at this level or below reads 0; at 0 dB it reads 1.
const FLOOR_DB: f32 = -60.0;
/// Music falls by about 3 dB an octave; bands are tilted up by as much (around 1 kHz) so the
/// highs show.
const TILT_DB_PER_OCTAVE: f32 = 3.0;
/// The loudness `level` maps to 0.
const LEVEL_FLOOR_DB: f32 = -48.0;
/// The bass range the beat listens to, in Hz.
const BEAT_LOW_HZ: f32 = 40.0;
const BEAT_HIGH_HZ: f32 = 200.0;
/// A kick: the bass at least this many times its average over the last second or so…
const ONSET_RATIO: f32 = 2.5;
/// …and this many times (9 dB) what it was a moment before (within `RISE_NS`). Asking for a
/// sudden rise as well keeps steady noise, whose bass wobbles a lot in a 43 ms window, from
/// pulsing.
const ONSET_RISE: f32 = 8.0;
const RISE_NS: u64 = 70_000_000;
/// No second pulse sooner than this after one (nanoseconds).
const REFRACTORY_NS: u64 = 150_000_000;
/// No new samples for this long and the scope reads as silence (nanoseconds).
const STALE_NS: u64 = 150_000_000;
/// The analysis is rerun at most this often; calls in between share it (nanoseconds).
const REANALYSE_NS: u64 = 4_000_000;

/// What the taps have played lately, for the visuals. Shared as `Arc<Scope>` between the
/// audio thread (taps write) and the UI (reads).
pub struct Scope {
    /// Mono samples as f32 bits.
    ring: Box<[AtomicU32]>,
    /// Samples written so far; the newest sits just before this, modulo the ring.
    written: AtomicUsize,
    /// Samples before this count were cleared and read as silence.
    floor: AtomicUsize,
    /// The sample rate of what was written last.
    rate: AtomicU32,
    /// When the last chunk came in, in nanoseconds of the scope's clock; 0 before any.
    last_write: AtomicU64,
    /// The tap that may write now: the newest one playing (0 when none).
    owner: AtomicU64,
    next_id: AtomicU64,
    /// Set by `clear`, so the next analysis forgets its averages too.
    cleared: AtomicBool,
    epoch: Instant,
    /// A clock the tests drive; `u64::MAX` means the wall clock.
    manual_clock: AtomicU64,
    /// What the UI keeps between reads; only the UI locks it.
    analysis: Mutex<Analysis>,
}

impl Scope {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ring: (0..RING).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicUsize::new(0),
            floor: AtomicUsize::new(0),
            rate: AtomicU32::new(48_000),
            last_write: AtomicU64::new(0),
            owner: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
            cleared: AtomicBool::new(false),
            epoch: Instant::now(),
            manual_clock: AtomicU64::new(u64::MAX),
            analysis: Mutex::new(Analysis::new()),
        })
    }

    /// The spectrum as `n` bands spaced evenly in pitch from about 40 Hz to 16 kHz, each 0..1
    /// on a dB scale (−60 dB → 0, 0 dB → 1), rising fast and falling slowly between calls.
    pub fn bands(&self, n: usize) -> Vec<f32> {
        let now = self.clock();
        let mut a = self.analysis.lock().unwrap_or_else(|e| e.into_inner());
        a.update(self, now);
        a.bands(n, now)
    }

    /// The loudness, 0..1 (−48 dB RMS → 0, full scale → 1), smoothed.
    pub fn level(&self) -> f32 {
        let now = self.clock();
        let mut a = self.analysis.lock().unwrap_or_else(|e| e.into_inner());
        a.update(self, now);
        a.level
    }

    /// A pulse, 0..1: it jumps when the bass hits well above its recent average (a kick) and
    /// dies away over about a quarter of a second. It needs reading every frame (20 times a
    /// second at the least), since a kick is seen as a sudden rise between readings.
    pub fn beat(&self) -> f32 {
        let now = self.clock();
        let mut a = self.analysis.lock().unwrap_or_else(|e| e.into_inner());
        a.update(self, now);
        a.beat
    }

    /// Forgets what was played, so the readings fall to rest (on pause, or a new song). It
    /// never blocks and may be called from any thread.
    pub fn clear(&self) {
        self.floor.store(self.written.load(Ordering::Acquire), Ordering::Release);
        self.cleared.store(true, Ordering::Release);
    }

    fn clock(&self) -> u64 {
        match self.manual_clock.load(Ordering::Relaxed) {
            u64::MAX => self.epoch.elapsed().as_nanos() as u64 + 1,
            t => t,
        }
    }

    /// Takes the write for tap `id` if it is the newest, or the one writing has gone quiet.
    fn claim(&self, id: u64, now: u64) -> bool {
        let owner = self.owner.load(Ordering::Acquire);
        if owner == id {
            return true;
        }
        let quiet = now.saturating_sub(self.last_write.load(Ordering::Acquire)) > STALE_NS / 2;
        if owner == 0 || id > owner || quiet {
            return self.owner.compare_exchange(owner, id, Ordering::AcqRel, Ordering::Acquire).is_ok();
        }
        false
    }

    fn push(&self, id: u64, samples: &[f32], rate: u32) {
        let now = self.clock();
        if samples.is_empty() || !self.claim(id, now) {
            return;
        }
        let start = self.written.load(Ordering::Acquire);
        for (i, s) in samples.iter().enumerate() {
            self.ring[(start.wrapping_add(i)) % RING].store(s.to_bits(), Ordering::Relaxed);
        }
        self.rate.store(rate, Ordering::Relaxed);
        self.written.store(start.wrapping_add(samples.len()), Ordering::Release);
        self.last_write.store(now, Ordering::Release);
    }

    fn release(&self, id: u64) {
        let _ = self.owner.compare_exchange(id, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
}

/// The UI's side of the scope: FFT tables and buffers, and the smoothed readings.
struct Analysis {
    window: Vec<f32>,
    /// Hann-windowed full-scale sine → 1.0 in `power`.
    norm: f32,
    cos: Vec<f32>,
    sin: Vec<f32>,
    bitrev: Vec<u32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Power per bin, FFT / 2 + 1 of them, relative to a full-scale sine.
    power: Vec<f32>,
    rate: f32,
    analysed_at: Option<u64>,
    level: f32,
    beat: f32,
    bass_avg: f32,
    /// Recent bass readings and when they were taken, newest last.
    bass_recent: [(u64, f32); 8],
    last_onset: Option<u64>,
    bands: Vec<f32>,
    bands_at: Option<u64>,
}

impl Analysis {
    fn new() -> Self {
        let window: Vec<f32> =
            (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT as f32).cos()).collect();
        let sum: f32 = window.iter().sum();
        let bits = FFT.trailing_zeros();
        Self {
            norm: (2.0 / sum) * (2.0 / sum),
            window,
            cos: (0..FFT / 2).map(|k| (2.0 * std::f64::consts::PI * k as f64 / FFT as f64).cos() as f32).collect(),
            sin: (0..FFT / 2).map(|k| (2.0 * std::f64::consts::PI * k as f64 / FFT as f64).sin() as f32).collect(),
            bitrev: (0..FFT as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect(),
            re: vec![0.0; FFT],
            im: vec![0.0; FFT],
            power: vec![0.0; FFT / 2 + 1],
            rate: 48_000.0,
            analysed_at: None,
            level: 0.0,
            beat: 0.0,
            bass_avg: 0.0,
            bass_recent: [(0, 0.0); 8],
            last_onset: None,
            bands: Vec::new(),
            bands_at: None,
        }
    }

    /// Reads the latest window from the ring and works out the spectrum, the loudness and
    /// the beat, unless that was done a moment ago.
    fn update(&mut self, scope: &Scope, now: u64) {
        if let Some(t) = self.analysed_at {
            if now.saturating_sub(t) < REANALYSE_NS {
                return;
            }
        }
        let dt = self.analysed_at.map_or(0.0, |t| now.saturating_sub(t) as f32 * 1e-9).min(0.5);
        self.analysed_at = Some(now);
        if scope.cleared.swap(false, Ordering::AcqRel) {
            self.bass_avg = 0.0;
            self.bass_recent = [(0, 0.0); 8];
            self.last_onset = None;
        }
        let rate = scope.rate.load(Ordering::Relaxed);
        self.rate = if rate == 0 { 48_000.0 } else { rate as f32 };
        let written = scope.written.load(Ordering::Acquire);
        let floor = scope.floor.load(Ordering::Acquire);
        let last = scope.last_write.load(Ordering::Acquire);
        let live = last != 0 && now.saturating_sub(last) < STALE_NS && written > floor;

        let mut sum_sq = 0.0f32;
        if live {
            let start = written as i64 - FFT as i64;
            for i in 0..FFT {
                let at = start + i as i64;
                let x = if at < floor as i64 || at < 0 {
                    0.0
                } else {
                    f32::from_bits(scope.ring[at as usize % RING].load(Ordering::Relaxed))
                };
                let x = if x.is_finite() { x } else { 0.0 };
                if i >= FFT / 2 {
                    sum_sq += x * x;
                }
                self.re[i] = x * self.window[i];
                self.im[i] = 0.0;
            }
            fft(&mut self.re, &mut self.im, &self.cos, &self.sin, &self.bitrev);
            for (k, p) in self.power.iter_mut().enumerate() {
                *p = (self.re[k] * self.re[k] + self.im[k] * self.im[k]) * self.norm;
            }
        } else {
            self.power.iter_mut().for_each(|p| *p = 0.0);
        }

        // Loudness: RMS of the newest half window.
        let rms = (sum_sq / (FFT / 2) as f32).sqrt();
        let db = 20.0 * (rms + 1e-9).log10();
        let target = ((db - LEVEL_FLOOR_DB) / -LEVEL_FLOOR_DB).clamp(0.0, 1.0);
        self.level = follow(self.level, target, dt, 0.04, 0.3);

        // The beat: bass energy against its average over the last second or so.
        self.beat *= (-dt / 0.09).exp();
        let bin = self.rate / FFT as f32;
        let (lo, hi) = (((BEAT_LOW_HZ / bin).ceil() as usize).max(1), ((BEAT_HIGH_HZ / bin) as usize).min(FFT / 2));
        let bass: f32 = self.power[lo..=hi.max(lo)].iter().sum();
        if live && bass > 1e-6 {
            let ratio = bass / self.bass_avg.max(1e-6);
            let before = self
                .bass_recent
                .iter()
                .filter(|(t, _)| *t != 0 && now.saturating_sub(*t) <= RISE_NS)
                .map(|(_, b)| *b)
                .fold(f32::INFINITY, f32::min);
            // Nothing within the window (the first reading, or the UI was away): no rise to see.
            let rise = if before.is_finite() { bass / before.max(1e-7) } else { 0.0 };
            let ready = self.last_onset.map_or(true, |t| now.saturating_sub(t) > REFRACTORY_NS);
            if ratio > ONSET_RATIO && rise > ONSET_RISE && ready {
                let strength = ((ratio - ONSET_RATIO) / 4.0).clamp(0.0, 1.0);
                self.beat = self.beat.max(0.6 + 0.4 * strength);
                self.last_onset = Some(now);
            }
        }
        self.bass_avg += (bass - self.bass_avg) * (1.0 - (-dt / 1.0).exp());
        self.bass_recent.rotate_left(1);
        self.bass_recent[7] = (now, if live { bass } else { 0.0 });
    }

    fn bands(&mut self, n: usize, now: u64) -> Vec<f32> {
        if self.bands.len() != n {
            self.bands = vec![0.0; n];
            self.bands_at = None;
        }
        let dt = self.bands_at.map_or(1.0, |t| now.saturating_sub(t) as f32 * 1e-9).min(1.0);
        self.bands_at = Some(now);
        let bin = self.rate / FFT as f32;
        let last = FFT / 2;
        let span = (HIGH_HZ.min(self.rate * 0.48) / LOW_HZ).max(1.0);
        for i in 0..n {
            let lo = LOW_HZ * span.powf(i as f32 / n as f32);
            let hi = LOW_HZ * span.powf((i + 1) as f32 / n as f32);
            let centre = (lo * hi).sqrt();
            let (first, end) = ((lo / bin).ceil() as usize, ((hi / bin).ceil() as usize).min(last + 1));
            let power = if first < end {
                self.power[first..end].iter().cloned().fold(0.0, f32::max)
            } else {
                // Narrower than a bin (the lows): read between the two nearest.
                let x = (centre / bin).min(last as f32);
                let k = (x as usize).min(last - 1);
                let f = x - k as f32;
                self.power[k] * (1.0 - f) + self.power[k + 1] * f
            };
            let db = 10.0 * (power + 1e-12).log10() + TILT_DB_PER_OCTAVE * (centre / 1000.0).log2();
            let target = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
            self.bands[i] = follow(self.bands[i], target, dt, 0.025, 0.2);
        }
        self.bands.clone()
    }
}

/// One step of a follower that rises with time constant `attack` and falls with `release`
/// (seconds), `dt` seconds after the last.
fn follow(current: f32, target: f32, dt: f32, attack: f32, release: f32) -> f32 {
    let tau = if target > current { attack } else { release };
    let next = current + (target - current) * (1.0 - (-dt / tau).exp());
    if next < 1e-4 {
        0.0
    } else {
        next
    }
}

/// An in-place radix-2 FFT; `cos`/`sin` hold the twiddles for the full length.
fn fft(re: &mut [f32], im: &mut [f32], cos: &[f32], sin: &[f32], bitrev: &[u32]) {
    let n = re.len();
    for i in 0..n {
        let j = bitrev[i] as usize;
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= n {
        let half = size / 2;
        let step = n / size;
        for start in (0..n).step_by(size) {
            for k in 0..half {
                let (c, s) = (cos[k * step], sin[k * step]);
                let (a, b) = (start + k, start + k + half);
                let tr = re[b] * c + im[b] * s;
                let ti = im[b] * c - re[b] * s;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        size *= 2;
    }
}

/// A source that plays as it is and feeds a mono mix of itself to a scope.
pub struct Tap<S> {
    inner: S,
    scope: Arc<Scope>,
    id: u64,
    channels: u16,
    channel: u16,
    sum: f32,
    chunk: [f32; CHUNK],
    len: usize,
}

impl<S: rodio::Source<Item = f32>> Tap<S> {
    pub fn new(inner: S, scope: Arc<Scope>) -> Self {
        let id = scope.next_id.fetch_add(1, Ordering::Relaxed);
        Self { channels: inner.channels().max(1), inner, scope, id, channel: 0, sum: 0.0, chunk: [0.0; CHUNK], len: 0 }
    }

    fn flush(&mut self) {
        self.scope.push(self.id, &self.chunk[..self.len], self.inner.sample_rate());
        self.len = 0;
    }
}

impl<S> Drop for Tap<S> {
    fn drop(&mut self) {
        self.scope.release(self.id);
    }
}

impl<S: rodio::Source<Item = f32>> Iterator for Tap<S> {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        if self.channel == 0 {
            self.channels = self.inner.channels().max(1);
        }
        self.sum += sample;
        self.channel += 1;
        if self.channel >= self.channels {
            self.chunk[self.len] = self.sum / self.channels as f32;
            self.len += 1;
            self.sum = 0.0;
            self.channel = 0;
            if self.len == CHUNK {
                self.flush();
            }
        }
        Some(sample)
    }
}

impl<S: rodio::Source<Item = f32>> rodio::Source for Tap<S> {
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
        self.channel = 0;
        self.sum = 0.0;
        self.len = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;

    const RATE: u32 = 48_000;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin() * amp).collect()
    }

    /// A small xorshift, for noise.
    struct Noise(u32);
    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
        }
    }

    /// Plays `mono` (as identical stereo) through a tap in 60 fps steps on the scope's test
    /// clock, calling `each` after every step with the time in seconds.
    fn play(scope: &Arc<Scope>, mono: &[f32], start_ns: u64, mut each: impl FnMut(&Scope, f32)) -> u64 {
        let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
        scope.manual_clock.store(start_ns, Ordering::Relaxed);
        let mut tap = Tap::new(SamplesBuffer::new(2, RATE, stereo), scope.clone());
        let step = RATE as usize / 60;
        let mut t = start_ns;
        let mut done = 0;
        while done < mono.len() {
            let n = step.min(mono.len() - done);
            for _ in 0..n * 2 {
                tap.next();
            }
            done += n;
            t += n as u64 * 1_000_000_000 / RATE as u64;
            scope.manual_clock.store(t, Ordering::Relaxed);
            each(scope, t as f32 * 1e-9);
        }
        t
    }

    fn band_of(freq: f32, n: usize) -> usize {
        ((freq / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln() * n as f32) as usize
    }

    #[test]
    fn sines_light_their_bands() {
        let n = 48;
        for freq in [100.0f32, 5000.0] {
            let scope = Scope::new();
            let mut bands = Vec::new();
            play(&scope, &sine(freq, 0.5, RATE as usize / 2), 1, |s, _| bands = s.bands(n));
            let at = band_of(freq, n);
            let peak = bands.iter().cloned().fold(0.0, f32::max);
            // Two octaves away, either side (or the lowest band, for 100 Hz).
            let (far_lo, far_hi) = (band_of((freq / 4.0).max(LOW_HZ), n), band_of(freq * 4.0, n).min(n - 1));
            println!("{freq} Hz: band {at} = {:.2}, peak {:.2}, two octaves off {:.2}/{:.2}", bands[at], peak, bands[far_lo], bands[far_hi]);
            assert!(bands[at] > 0.5 && bands[at] >= peak - 0.05, "{bands:?}");
            assert!(bands[far_lo] < 0.2 && bands[far_hi] < 0.2, "{bands:?}");
            // Nothing lit three octaves and more away.
            let lit = |i: usize| bands[i] > 0.05;
            assert!((0..n).filter(|i| lit(*i)).all(|i| (i as i32 - at as i32).abs() <= (n as i32 * 3 / 9)), "{bands:?}");
        }
    }

    #[test]
    fn silence_and_pause_fall_to_rest() {
        let scope = Scope::new();
        let t = play(&scope, &sine(440.0, 0.5, RATE as usize / 2), 1, |s, _| {
            s.level();
        });
        let level = scope.level();
        assert!(level > 0.8, "level {level}");
        // Silence played: everything falls.
        let mut last = (Vec::new(), 0.0);
        let t = play(&scope, &vec![0.0; RATE as usize * 2], t, |s, _| last = (s.bands(32), s.level()));
        assert!(last.0.iter().all(|b| *b < 0.01) && last.1 < 0.01, "{last:?}");
        // Music then nothing (paused): falls on the clock alone.
        let t = play(&scope, &sine(440.0, 0.5, RATE as usize / 2), t, |s, _| {
            s.bands(32);
        });
        assert!(scope.bands(32).iter().any(|b| *b > 0.5));
        for i in 1..=120 {
            scope.manual_clock.store(t + i * 16_666_667, Ordering::Relaxed);
            scope.bands(32);
        }
        assert!(scope.bands(32).iter().all(|b| *b < 0.01) && scope.level() < 0.01);
        // Clear: falls too, without waiting for staleness.
        let t = play(&scope, &sine(440.0, 0.5, RATE as usize / 2), t + 3_000_000_000, |s, _| {
            s.bands(32);
        });
        assert!(scope.bands(32).iter().any(|b| *b > 0.5));
        scope.clear();
        for i in 1..=60 {
            scope.manual_clock.store(t + i * 16_666_667, Ordering::Relaxed);
            scope.bands(32);
        }
        assert!(scope.bands(32).iter().all(|b| *b < 0.01));
    }

    /// Plays `signal` and reads `beat` each frame: when it fired (rose past 0.5), the mean
    /// pulse and the share of frames over 0.5 after the first half second.
    fn pulses(signal: &[f32]) -> (Vec<f32>, f32, f32) {
        let scope = Scope::new();
        let (mut onsets, mut was_low, mut sum, mut high, mut count) = (Vec::new(), true, 0.0, 0, 0);
        play(&scope, signal, 1, |s, t| {
            let b = s.beat();
            if b > 0.5 && was_low {
                onsets.push((t * 1000.0).round() / 1000.0);
            }
            was_low = b < 0.3;
            if t > 0.5 {
                sum += b;
                count += 1;
                high += (b > 0.5) as usize;
            }
        });
        (onsets, sum / count as f32, high as f32 / count as f32)
    }

    /// A 60 Hz kick (decaying, 120 ms) every 500 ms.
    fn kick(i: usize) -> f32 {
        let t = (i % (RATE as usize / 2)) as f32 / RATE as f32;
        if t < 0.12 {
            (2.0 * std::f32::consts::PI * 60.0 * t).sin() * 0.8 * (-t / 0.05).exp()
        } else {
            0.0
        }
    }

    #[test]
    fn beat_follows_kicks_not_noise() {
        let seconds = 6;
        // Kicks over a quiet hiss.
        let mut noise = Noise(12345);
        let clean: Vec<f32> = (0..RATE as usize * seconds).map(|i| kick(i) + noise.next() * 0.02).collect();
        let (onsets, mean, high) = pulses(&clean);
        println!("kicks: onsets at {onsets:?}, mean {mean:.2}, frames over 0.5 {:.0}%", high * 100.0);
        assert!(onsets.len() >= seconds * 2 - 1, "{onsets:?}");
        // Each pulse within 50 ms of its kick, and none in between.
        assert!(onsets.iter().all(|t| (t % 0.5) < 0.05), "{onsets:?}");

        // Kicks over a busy mix: a steady 80 Hz bass line and loud hiss.
        let mut noise = Noise(4242);
        let busy: Vec<f32> = (0..RATE as usize * seconds)
            .map(|i| kick(i) + (2.0 * std::f32::consts::PI * 80.0 * i as f32 / RATE as f32).sin() * 0.2 + noise.next() * 0.15)
            .collect();
        let (onsets, mean, high) = pulses(&busy);
        println!("busy kicks: onsets at {onsets:?}, mean {mean:.2}, frames over 0.5 {:.0}%", high * 100.0);
        assert!(onsets.len() >= seconds * 2 - 2, "{onsets:?}");
        assert!(onsets.iter().filter(|t| (*t % 0.5) >= 0.05).count() <= 1, "{onsets:?}");

        // Steady noise, loud and full band: the pulse stays low.
        let mut noise = Noise(777);
        let hiss: Vec<f32> = (0..RATE as usize * seconds).map(|_| noise.next() * 0.3).collect();
        let (onsets, mean, high) = pulses(&hiss);
        println!("steady noise: {} pulses in {seconds} s, mean {mean:.3}, frames over 0.5 {:.1}%", onsets.len(), high * 100.0);
        assert!(mean < 0.05 && high < 0.02, "mean {mean} high {high}");
    }
}
