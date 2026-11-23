//! Decoding ahead of the speakers.
//!
//! rodio pulls sound out of a source inside the output device's callback, on cpal's WASAPI
//! thread. With the decoder in that chain, every AAC packet was decoded (and its buffer
//! allocated) in the few milliseconds Windows gives the callback each period. On a busy PC
//! (a window dragged across the screen, a heavy app opening on a laptop, the CPU slowed while
//! NabyTune sits minimised) a packet that came late was a hole in the sound: the stutter and
//! the crackle.
//!
//! A `Prefetched` source decodes on a thread of its own, a couple of seconds ahead, into a
//! ring the callback only copies out of. A hiccup on the decoding side now has seconds to
//! catch up instead of milliseconds. The callback's own thread is also given the priority
//! audio needs (see `boost_output_thread`): cpal 0.15 means to, but hands Windows a thread
//! id where a handle belongs, so its thread ran at normal priority.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// What the decoding thread reads from: a decoder, packet by packet.
pub trait Decode: Send + 'static {
    /// Appends the next stretch of interleaved samples to `out`. False at the end.
    fn read_into(&mut self, out: &mut Vec<f32>) -> bool;
    /// Goes to `to` in the track.
    fn seek_to(&mut self, to: Duration) -> Result<(), String>;
}

/// How far ahead the decoder runs.
const AHEAD: Duration = Duration::from_secs(2);
/// What's decoded before a track (or a seek) starts sounding, so it starts on music, not on
/// a gap.
const PRIME: Duration = Duration::from_millis(150);
/// The longest a start or a seek waits for that.
const PRIME_WAIT: Duration = Duration::from_millis(1500);
/// How many samples the callback copies out of the ring at a time.
const CHUNK: usize = 1024;

/// A seek the callback asked for: which one, and where to.
struct SeekRequest {
    id: u64,
    to: Duration,
}

/// A seek the decoding thread carried out: which one, where the ring stood when it did (all
/// before is from the old place), and how it went.
struct SeekDone {
    id: u64,
    mark: usize,
    result: Result<(), String>,
}

struct Shared {
    /// Interleaved samples as f32 bits; its length is a power of two.
    ring: Box<[AtomicU32]>,
    mask: usize,
    /// Samples written by the decoding thread, and read by the callback, so far (wrapping).
    written: AtomicUsize,
    read: AtomicUsize,
    /// The decoder reached the end of the track: nothing comes after `written`.
    finished: AtomicBool,
    /// The source is gone: the decoding thread stops.
    stop: AtomicBool,
    seek: Mutex<Option<SeekRequest>>,
    done: Mutex<Option<SeekDone>>,
    done_signal: Condvar,
}

impl Shared {
    fn buffered(&self) -> usize {
        self.written.load(Ordering::Acquire).wrapping_sub(self.read.load(Ordering::Acquire))
    }
}

/// A track decoded on its own thread, ahead of what plays.
pub struct Prefetched {
    shared: Arc<Shared>,
    decoder_thread: Thread,
    channels: u16,
    rate: u32,
    duration: Option<Duration>,
    /// Samples copied out of the ring, being handed over one at a time.
    chunk: Vec<f32>,
    at: usize,
    /// Silence played while the ring ran dry. As many samples are dropped once it fills
    /// again, so the position rodio counts stays the music's (and the lyrics with it).
    owed: usize,
    /// Never drop more than this to catch up: past it, the music just goes on late.
    owed_limit: usize,
    seeks: u64,
}

impl Prefetched {
    /// Starts decoding `decoder` (`channels` × `rate`, `duration` long when known) on a
    /// thread of its own, and waits for its first moments.
    pub fn new(decoder: impl Decode, channels: u16, rate: u32, duration: Option<Duration>) -> Self {
        let channels = channels.max(1);
        let rate = rate.max(1);
        let per_second = rate as usize * channels as usize;
        let size = ((per_second as f64 * AHEAD.as_secs_f64()) as usize).max(16 * CHUNK).next_power_of_two();
        let shared = Arc::new(Shared {
            ring: (0..size).map(|_| AtomicU32::new(0)).collect(),
            mask: size - 1,
            written: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            finished: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            seek: Mutex::new(None),
            done: Mutex::new(None),
            done_signal: Condvar::new(),
        });
        let feed = shared.clone();
        let handle = std::thread::Builder::new()
            .name("nabytune-decode".into())
            .spawn(move || decode(feed, decoder))
            .expect("decoding thread");
        let source = Self {
            shared,
            decoder_thread: handle.thread().clone(),
            channels,
            rate,
            duration,
            chunk: Vec::with_capacity(CHUNK),
            at: 0,
            owed: 0,
            owed_limit: per_second / 2,
            seeks: 0,
        };
        source.prime();
        source
    }

    /// Waits (a little) until the first moments are decoded, or the track ended before that.
    fn prime(&self) {
        let want = ((self.rate as f64 * PRIME.as_secs_f64()) as usize * self.channels as usize).min(self.shared.ring.len() / 2);
        let deadline = Instant::now() + PRIME_WAIT;
        while self.shared.buffered() < want && !self.shared.finished.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Copies what's in the ring (up to a chunk) into `chunk`. Whether anything came.
    fn refill(&mut self) -> bool {
        let shared = &self.shared;
        let written = shared.written.load(Ordering::Acquire);
        let mut read = shared.read.load(Ordering::Relaxed);
        let mut available = written.wrapping_sub(read);
        if self.owed > 0 && available > 0 {
            let skip = self.owed.min(available);
            read = read.wrapping_add(skip);
            available -= skip;
            self.owed -= skip;
        }
        let n = available.min(CHUNK);
        self.chunk.clear();
        for i in 0..n {
            self.chunk.push(f32::from_bits(shared.ring[read.wrapping_add(i) & shared.mask].load(Ordering::Relaxed)));
        }
        shared.read.store(read.wrapping_add(n), Ordering::Release);
        if n < CHUNK / 2 {
            // Running low: the decoding thread may be napping on a full ring.
            self.decoder_thread.unpark();
        }
        self.at = 0;
        n > 0
    }
}

impl Drop for Prefetched {
    fn drop(&mut self) {
        // Never waits: the decoding thread sees this and lets go of the decoder itself.
        self.shared.stop.store(true, Ordering::Release);
        self.decoder_thread.unpark();
    }
}

impl Iterator for Prefetched {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        boost_output_thread();
        if self.at >= self.chunk.len() && !self.refill() {
            if self.shared.finished.load(Ordering::Acquire) && self.shared.buffered() == 0 {
                return None;
            }
            // The decoder is behind: silence for now, made up for once it catches up.
            if self.owed < self.owed_limit {
                self.owed += 1;
            }
            return Some(0.0);
        }
        let sample = self.chunk[self.at];
        self.at += 1;
        Some(sample)
    }
}

impl rodio::Source for Prefetched {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    /// Asks the decoding thread to go to `pos` and waits for it, then for the first moments
    /// from there. It runs in the output's callback, as the decoder's own seek did before;
    /// only now the callback waits on the decoding thread rather than doing the work.
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.seeks += 1;
        let id = self.seeks;
        *self.shared.seek.lock().unwrap_or_else(|e| e.into_inner()) = Some(SeekRequest { id, to: pos });
        self.decoder_thread.unpark();
        let deadline = Instant::now() + PRIME_WAIT * 2;
        let mut done = self.shared.done.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(d) = done.as_ref().filter(|d| d.id == id) {
                let (mark, result) = (d.mark, d.result.clone());
                drop(done);
                // What was decoded from the old place is skipped.
                self.shared.read.store(mark, Ordering::Release);
                self.chunk.clear();
                self.at = 0;
                self.owed = 0;
                self.decoder_thread.unpark();
                self.prime();
                return result.map_err(|e| rodio::source::SeekError::Other(Box::new(std::io::Error::other(e))));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(rodio::source::SeekError::Other(Box::new(std::io::Error::other("the decoder didn't answer the seek"))));
            }
            done = self.shared.done_signal.wait_timeout(done, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// The decoding thread: keeps the ring full, and seeks when asked.
fn decode(shared: Arc<Shared>, mut decoder: impl Decode) {
    boost_decoder_thread();
    let size = shared.ring.len();
    // Decoded, not yet in the ring.
    let mut pending: Vec<f32> = Vec::with_capacity(8192);
    let mut pending_at = 0usize;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let request = shared.seek.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(SeekRequest { id, to }) = request {
            let result = decoder.seek_to(to);
            pending.clear();
            pending_at = 0;
            shared.finished.store(false, Ordering::Release);
            let mark = shared.written.load(Ordering::Acquire);
            *shared.done.lock().unwrap_or_else(|e| e.into_inner()) = Some(SeekDone { id, mark, result });
            shared.done_signal.notify_all();
            continue;
        }
        if pending_at >= pending.len() {
            pending.clear();
            pending_at = 0;
            if shared.finished.load(Ordering::Acquire) || !decoder.read_into(&mut pending) {
                shared.finished.store(true, Ordering::Release);
                // Nothing more to decode: wait for a seek back, or the end.
                std::thread::park_timeout(Duration::from_millis(200));
                continue;
            }
        }
        let written = shared.written.load(Ordering::Relaxed);
        let free = size - written.wrapping_sub(shared.read.load(Ordering::Acquire)).min(size);
        if free < CHUNK.min(pending.len() - pending_at) {
            // Full: a nap, cut short when the callback runs low or a seek comes.
            std::thread::park_timeout(Duration::from_millis(40));
            continue;
        }
        let n = free.min(pending.len() - pending_at);
        for (i, sample) in pending[pending_at..pending_at + n].iter().enumerate() {
            shared.ring[written.wrapping_add(i) & shared.mask].store(sample.to_bits(), Ordering::Relaxed);
        }
        pending_at += n;
        shared.written.store(written.wrapping_add(n), Ordering::Release);
    }
}

/// To the audio engine's log (%TEMP%\nabytune-proto.log); kept here so the examples that
/// decode without the engine still build.
#[cfg(windows)]
fn log(line: String) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("nabytune-proto.log")) {
        let _ = writeln!(f, "{:?} {line}", std::time::SystemTime::now());
    }
}

thread_local! {
    static BOOSTED: Cell<bool> = const { Cell::new(false) };
}

/// Once per thread: the output callback's thread gets real-time priority from Windows'
/// multimedia scheduler (MMCSS, "Pro Audio"), as audio apps' are, and time-critical priority.
fn boost_output_thread() {
    if BOOSTED.with(|b| b.replace(true)) {
        return;
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{AvSetMmThreadCharacteristicsW, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
        let task: Vec<u16> = "Pro Audio".encode_utf16().chain(Some(0)).collect();
        let mut index = 0u32;
        let mmcss = AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index);
        let raised = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        log(format!("output thread: mmcss {}, time-critical {}", !mmcss.is_null(), raised != 0));
    }
}

/// The decoding thread runs above the apps around it, so a busy PC doesn't starve it.
fn boost_decoder_thread() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
}

/// Asks Windows not to slow NabyTune down when it's minimised or behind other windows
/// (EcoQoS, "efficiency mode"): on a laptop, a throttled player was a crackling one. Also
/// keeps its timers as precise as it asks while hidden.
pub fn keep_full_speed() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
        };
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
            // Both controlled, neither on: full speed, and timers kept.
            StateMask: 0,
        };
        let ok = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const PROCESS_POWER_THROTTLING_STATE as *const core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
        log(format!("power throttling off: {}", ok != 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source as _;

    /// A track that counts: sample n is n (as f32), `len` of them, in packets of `packet`.
    struct Counting {
        at: usize,
        len: usize,
        packet: usize,
        /// Each packet takes this long to decode.
        slow: Duration,
    }

    impl Decode for Counting {
        fn read_into(&mut self, out: &mut Vec<f32>) -> bool {
            if self.at >= self.len {
                return false;
            }
            std::thread::sleep(self.slow);
            let end = (self.at + self.packet).min(self.len);
            out.extend((self.at..end).map(|n| n as f32));
            self.at = end;
            true
        }
        fn seek_to(&mut self, to: Duration) -> Result<(), String> {
            // 1000 samples a second, one channel.
            self.at = (to.as_millis() as usize).min(self.len);
            Ok(())
        }
    }

    fn counting(len: usize) -> Counting {
        Counting { at: 0, len, packet: 1024, slow: Duration::ZERO }
    }

    /// The next sample, pulled no faster than the decoder gives them (a test runs far ahead
    /// of real time, where the ring would run dry).
    fn pull(source: &mut Prefetched) -> Option<f32> {
        if source.at >= source.chunk.len() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while source.shared.buffered() == 0 && !source.shared.finished.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_micros(200));
            }
        }
        source.next()
    }

    fn rest(source: &mut Prefetched) -> Vec<f32> {
        std::iter::from_fn(|| pull(source)).collect()
    }

    #[test]
    fn plays_every_sample_in_order_then_ends() {
        let mut source = Prefetched::new(counting(100_000), 1, 1000, None);
        let out = rest(&mut source);
        assert_eq!(out.len(), 100_000);
        assert!(out.iter().enumerate().all(|(i, s)| *s == i as f32));
    }

    #[test]
    fn starts_on_music_not_silence() {
        let mut source = Prefetched::new(Counting { slow: Duration::from_millis(2), ..counting(50_000) }, 1, 1000, None);
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.next(), Some(1.0));
        assert_eq!(source.owed, 0);
    }

    #[test]
    fn seeks_forward_and_back() {
        let mut source = Prefetched::new(counting(100_000), 1, 1000, None);
        for _ in 0..10 {
            pull(&mut source);
        }
        source.try_seek(Duration::from_millis(40_000)).unwrap();
        assert_eq!(pull(&mut source), Some(40_000.0));
        assert_eq!(pull(&mut source), Some(40_001.0));
        source.try_seek(Duration::from_millis(5)).unwrap();
        assert_eq!(pull(&mut source), Some(5.0));
        // And on to the end from there, every sample.
        let rest = rest(&mut source);
        assert_eq!(rest, (6..100_000).map(|n| n as f32).collect::<Vec<_>>());
    }

    #[test]
    fn seeks_after_the_decoder_finished() {
        let mut source = Prefetched::new(counting(3000), 1, 1000, None);
        std::thread::sleep(Duration::from_millis(50));
        assert!(source.shared.finished.load(Ordering::Acquire));
        source.try_seek(Duration::from_millis(2990)).unwrap();
        assert_eq!(rest(&mut source), (2990..3000).map(|n| n as f32).collect::<Vec<_>>());
    }

    #[test]
    fn a_dry_ring_pays_its_silence_back() {
        // A decoder far slower than the music: the ring runs dry.
        let mut source = Prefetched::new(Counting { packet: 64, slow: Duration::from_millis(30), ..counting(10_000) }, 1, 1000, None);
        // Pulled far faster than the music would be: every gap is paid back, however long.
        source.owed_limit = usize::MAX;
        let mut out = Vec::new();
        let started = Instant::now();
        while out.len() < 6000 && started.elapsed() < Duration::from_secs(20) {
            match source.next() {
                Some(s) => out.push(s),
                None => break,
            }
        }
        // Whatever is music sits where it belongs: sample i is i, or silence in its place.
        for (i, s) in out.iter().enumerate() {
            assert!(*s == 0.0 || *s == i as f32, "sample {i} is {s}");
        }
        assert!(out.iter().skip(1).any(|s| *s != 0.0), "some music came through");
    }

    #[test]
    fn the_decoding_thread_stops_with_the_source() {
        let source = Prefetched::new(counting(10_000_000), 2, 48_000, None);
        let shared = source.shared.clone();
        drop(source);
        let deadline = Instant::now() + Duration::from_secs(2);
        // The thread holds the other reference until it returns.
        while Arc::strong_count(&shared) > 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(Arc::strong_count(&shared), 1);
    }
}
