//! The audio engine: one thread owning the output device (WASAPI through
//! cpal) and a rodio sink fed by symphonia's AAC decoder. The UI talks to it
//! with commands and reads a shared status; it never blocks on audio.
//!
//! Output goes to Windows' default playback device and follows it: when the listener picks
//! other speakers or headphones (or the device in use goes away), the track carries on there,
//! from where it was.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rodio::cpal::traits::HostTrait;
use rodio::{Device, DeviceTrait, OutputStream, OutputStreamHandle, Sink, Source};

use crate::decoder::{AudioSource, Bytes};
use crate::{dsp, eq};

enum Command {
    /// Replace whatever plays with these encoded bytes, at this volume.
    /// With `fade`, what played fades out over it while this fades in (crossfade).
    Load { generation: u64, bytes: Vec<u8>, volume: f32, play: bool, fade: Duration },
    Play,
    Pause,
    Seek(Duration),
    Mute(bool),
    /// The playing track's volume, as normalisation is switched on or off.
    Volume(f32),
}

#[derive(Clone, Default)]
pub struct Status {
    /// Which load this status is about.
    pub generation: u64,
    pub position: Duration,
    pub duration: Option<Duration>,
    /// The loaded track played to its end.
    pub ended: bool,
    pub error: Option<String>,
}

/// Diagnostics to %TEMP%\nabytune-proto.log.
pub fn log(line: impl AsRef<str>) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("nabytune-proto.log")) {
        let _ = writeln!(f, "{:?} {}", std::time::SystemTime::now(), line.as_ref());
    }
}

pub struct Audio {
    commands: Sender<Command>,
    status: Arc<Mutex<Status>>,
    eq: Arc<eq::Params>,
    /// What plays, as the visualizer reads it.
    scope: Arc<dsp::Scope>,
}

/// A song fading out under the next: its sink, when the fade began, how long it takes, and
/// the volume it began at.
struct Fading {
    sink: Sink,
    from: Instant,
    length: Duration,
    start: f32,
}

/// The open output: the device it plays on, and its stream (closed when this is dropped).
struct Output {
    device: Device,
    _stream: OutputStream,
    handle: OutputStreamHandle,
}

/// How often Windows' default playback device is looked at.
const WATCH: Duration = Duration::from_secs(1);
/// A track that should be playing but hasn't moved for this long has lost its device (one
/// that was unplugged, or whose format was changed in Windows' sound settings): the output is
/// opened again.
const STALL: Duration = Duration::from_secs(2);

/// Windows' default playback device, opened. None when there's none, or it won't open.
fn open_output() -> Option<Output> {
    let device = rodio::cpal::default_host().default_output_device()?;
    let name = device.name().unwrap_or_default();
    match OutputStream::try_from_device(&device) {
        Ok((stream, handle)) => {
            log(format!("output device open: {name}"));
            Some(Output { device, _stream: stream, handle })
        }
        Err(e) => {
            log(format!("output device {name} won't open: {e}"));
            None
        }
    }
}

/// Whether two handles are the same device (by its Windows endpoint id).
fn same_device(a: &Device, b: &Device) -> bool {
    #[cfg(windows)]
    {
        use rodio::cpal::platform::DeviceInner;
        #[allow(unreachable_patterns)]
        match (a.as_inner(), b.as_inner()) {
            (DeviceInner::Wasapi(a), DeviceInner::Wasapi(b)) => return a == b,
            _ => {}
        }
    }
    a.name().ok() == b.name().ok()
}

/// A sink on `output` playing `bytes` from `at`, through the equalizer and the visualizer's
/// tap, at `volume`, paused or not. Also the track's length, when it states one.
///
/// The decoder is taken to `at` before it's handed over: a seek through the sink waits for
/// the device to ask for sound, and a device that doesn't would hang this thread. The sink
/// then counts from 0, so the track's position is `at` plus what the sink says.
fn start(
    output: &Output,
    bytes: &Bytes,
    eq_params: &Arc<eq::Params>,
    scope: &Arc<dsp::Scope>,
    volume: f32,
    paused: bool,
    at: Duration,
) -> Result<(Sink, Option<Duration>), String> {
    let mut decoder = AudioSource::new(bytes.clone())?;
    let duration = decoder.total_duration();
    log(format!("decoder: {} Hz, {} ch, duration {duration:?}", decoder.sample_rate(), decoder.channels()));
    if at > Duration::ZERO {
        if let Err(e) = decoder.try_seek(at) {
            log(format!("seek on start failed: {e}"));
        }
    }
    let s = Sink::try_new(&output.handle).map_err(|e| e.to_string())?;
    s.set_volume(volume);
    if paused {
        s.pause();
    }
    // The equalizer, then the visualizer hears what's heard.
    s.append(dsp::Tap::new(eq::Equalized::new(decoder, eq_params.clone()), scope.clone()));
    Ok((s, duration))
}

impl Audio {
    pub fn new() -> Self {
        let (commands, inbox) = channel::<Command>();
        let status = Arc::new(Mutex::new(Status::default()));
        let shared = status.clone();
        let eq = eq::Params::new();
        let eq_params = eq.clone();
        let scope = dsp::Scope::new();
        let scope_feed = scope.clone();
        std::thread::Builder::new()
            .name("nabytune-audio".into())
            .spawn(move || {
                // The device lives and dies on this thread (rodio's stream isn't Send).
                let mut output = open_output();
                if output.is_none() {
                    shared.lock().unwrap().error = Some("No audio output".into());
                }
                let mut sink: Option<Sink> = None;
                let mut loaded = false;
                // The loaded track's bytes, to start it again on another device.
                let mut current: Option<Bytes> = None;
                // With no device to play on: where the track was, and whether it was paused,
                // to go on from there once one is back.
                let mut held: Option<(Duration, bool)> = None;
                // The track's normalised volume, and whether the listener muted it.
                let mut volume = 1.0f32;
                let mut muted = std::env::var_os("NABYTUNE_PROTO_MUTE").is_some();
                let probe = std::env::var_os("NABYTUNE_PROTO_FPS").is_some();
                let mut drift: Option<(Instant, Duration, u64)> = None;
                let mut drift_generation = 0u64;
                let mut fading: Option<Fading> = None;
                // The fading-in song's own volume, and when its fade began.
                let mut fade_in: Option<(Instant, Duration)> = None;
                let mut watched = Instant::now();
                // The playing track's position, and since when it has been there.
                let mut still: Option<(Duration, Instant)> = None;
                // Where the sink started in the track, when it was started part-way (on another
                // device): its own position counts from there.
                let mut offset = Duration::ZERO;
                // NABYTUNE_PROTO_REOPEN=1 moves to the default device every 5 s as if it had
                // changed, to test the handover without changing the PC's sound settings.
                let reopen_test = std::env::var_os("NABYTUNE_PROTO_REOPEN").is_some();
                let mut watches = 0u32;
                loop {
                    let wait = if fading.is_some() { 20 } else { 50 };
                    match inbox.recv_timeout(Duration::from_millis(wait)) {
                        Ok(Command::Load { generation, bytes, volume: track_volume, play, fade }) => {
                            volume = track_volume;
                            log(format!("load #{generation}: {} bytes, volume {volume:.2}, play {play}, fade {fade:?}", bytes.len()));
                            if let Some(f) = fading.take() {
                                f.sink.stop();
                            }
                            fade_in = None;
                            if let Some(old) = sink.take() {
                                // Crossfade: the old song goes on, quieter and quieter, under the new.
                                if fade > Duration::ZERO && play && !old.is_paused() && !old.empty() {
                                    let start = old.volume();
                                    fading = Some(Fading { sink: old, from: Instant::now(), length: fade, start });
                                    fade_in = Some((Instant::now(), fade));
                                } else {
                                    old.stop();
                                    scope_feed.clear();
                                }
                            }
                            let bytes = Bytes::new(bytes);
                            current = Some(bytes.clone());
                            held = None;
                            still = None;
                            offset = Duration::ZERO;
                            if output.is_none() {
                                output = open_output();
                            }
                            let mut status = Status { generation, ..Status::default() };
                            match &output {
                                Some(out) => {
                                    // NABYTUNE_PROTO_MUTE=1 starts muted, for testing beside a session that's playing.
                                    let level = if muted || fade_in.is_some() { 0.0 } else { volume };
                                    match start(out, &bytes, &eq_params, &scope_feed, level, !play, Duration::ZERO) {
                                        Ok((s, duration)) => {
                                            status.duration = duration;
                                            sink = Some(s);
                                            loaded = true;
                                        }
                                        Err(e) => {
                                            log(format!("decode failed: {e}"));
                                            status.error = Some(format!("Couldn't decode the stream: {e}"))
                                        }
                                    }
                                }
                                None => {
                                    // Nothing to play on yet: it starts when a device turns up.
                                    held = Some((Duration::ZERO, !play));
                                    status.duration = AudioSource::new(bytes).ok().and_then(|d| d.total_duration());
                                    status.error = Some("No audio output".into());
                                }
                            }
                            *shared.lock().unwrap() = status;
                        }
                        Ok(Command::Play) => {
                            sink.iter().for_each(|s| s.play());
                            if let Some((_, paused)) = &mut held {
                                *paused = false;
                            }
                        }
                        Ok(Command::Pause) => {
                            sink.iter().for_each(|s| s.pause());
                            if let Some((_, paused)) = &mut held {
                                *paused = true;
                            }
                            // Paused mid-crossfade: the old song is done with.
                            if let Some(f) = fading.take() {
                                f.sink.stop();
                            }
                            if fade_in.take().is_some() {
                                sink.iter().for_each(|s| s.set_volume(if muted { 0.0 } else { volume }));
                            }
                        }
                        Ok(Command::Seek(to)) => {
                            if let Some(s) = &sink {
                                match s.try_seek(to) {
                                    // The sink counts from the track's start again.
                                    Ok(()) => offset = Duration::ZERO,
                                    Err(e) => shared.lock().unwrap().error = Some(format!("Seek failed: {e}")),
                                }
                            }
                            if let Some((at, _)) = &mut held {
                                *at = to;
                                shared.lock().unwrap().position = to;
                            }
                            still = None;
                        }
                        Ok(Command::Mute(on)) => {
                            muted = on;
                            if fade_in.is_none() {
                                sink.iter().for_each(|s| s.set_volume(if muted { 0.0 } else { volume }));
                            }
                        }
                        Ok(Command::Volume(to)) => {
                            volume = to;
                            if fade_in.is_none() {
                                sink.iter().for_each(|s| s.set_volume(if muted { 0.0 } else { volume }));
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    // The crossfade, on an equal-power curve: out goes cos, in goes sin.
                    if let Some(f) = &fading {
                        let t = (f.from.elapsed().as_secs_f32() / f.length.as_secs_f32().max(0.01)).min(1.0);
                        let quarter = std::f32::consts::FRAC_PI_2;
                        let level = if muted { 0.0 } else { 1.0 };
                        f.sink.set_volume(level * f.start * (t * quarter).cos().max(0.0));
                        if let (Some(s), Some(_)) = (&sink, fade_in) {
                            s.set_volume(level * volume * (t * quarter).sin());
                        }
                        if t >= 1.0 || f.sink.empty() {
                            if let Some(f) = fading.take() {
                                f.sink.stop();
                            }
                            if fade_in.take().is_some() {
                                sink.iter().for_each(|s| s.set_volume(if muted { 0.0 } else { volume }));
                            }
                        }
                    }
                    if let Some(s) = &sink {
                        let mut status = shared.lock().unwrap();
                        status.position = offset + s.get_pos();
                        // With NABYTUNE_PROTO_FPS set: every 5 s of unbroken playback, the reported
                        // position against the wall clock, to catch a clock that drifts.
                        if probe {
                            let wall = Instant::now();
                            match drift {
                                Some((start, at, n)) if !s.is_paused() && status.generation == drift_generation => {
                                    let elapsed = wall - start;
                                    if elapsed.as_secs() >= n * 5 {
                                        log(format!("drift: wall {:.2}s pos {:.2}s", (at + elapsed).as_secs_f64(), status.position.as_secs_f64()));
                                        drift = Some((start, at, n + 1));
                                    }
                                }
                                _ if s.is_paused() => drift = None,
                                None if status.position > Duration::ZERO => {
                                    drift = Some((wall, status.position, 1));
                                    drift_generation = status.generation;
                                }
                                Some(_) => drift = None,
                                None => {}
                            }
                        }
                        if loaded && s.empty() {
                            status.ended = true;
                            loaded = false;
                        }
                        // Whether the track is moving, as it should when it plays.
                        match still {
                            Some((at, _)) if at == status.position && !s.is_paused() && !s.empty() => {}
                            _ => still = Some((status.position, Instant::now())),
                        }
                    }
                    // Windows' default playback device changed (or the one in use went away, or
                    // stopped taking sound): the track moves to whatever is the default now.
                    if watched.elapsed() >= WATCH {
                        watched = Instant::now();
                        let default = rodio::cpal::default_host().default_output_device();
                        let moved = match (&output, &default) {
                            (Some(out), Some(now)) => !same_device(&out.device, now),
                            (Some(_), None) | (None, Some(_)) => true,
                            (None, None) => false,
                        };
                        watches += 1;
                        let moved = moved || (reopen_test && watches % 5 == 0);
                        let stalled = still.is_some_and(|(_, since)| since.elapsed() >= STALL);
                        if moved || stalled {
                            log(if moved { "default output device changed" } else { "output stalled" });
                            // Where the track was, unless it had played to its end.
                            let resume = match sink.take() {
                                Some(s) => {
                                    let resume = (!s.empty()).then(|| (offset + s.get_pos(), s.is_paused()));
                                    s.stop();
                                    resume
                                }
                                None => held.take(),
                            };
                            if let Some(f) = fading.take() {
                                f.sink.stop();
                            }
                            fade_in = None;
                            scope_feed.clear();
                            still = None;
                            // The old stream closes before the new one opens.
                            drop(output.take());
                            output = open_output();
                            match (&output, &current, resume) {
                                (Some(out), Some(bytes), Some((at, paused))) => {
                                    let level = if muted { 0.0 } else { volume };
                                    match start(out, bytes, &eq_params, &scope_feed, level, paused, at) {
                                        Ok((s, _)) => {
                                            sink = Some(s);
                                            loaded = true;
                                            offset = at;
                                            shared.lock().unwrap().error = None;
                                        }
                                        Err(e) => log(format!("restart on the new device failed: {e}")),
                                    }
                                }
                                (None, _, Some(resume)) => {
                                    held = Some(resume);
                                    shared.lock().unwrap().error = Some("No audio output".into());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            })
            .expect("audio thread");
        Self { commands, status, eq, scope }
    }

    pub fn scope(&self) -> &Arc<dsp::Scope> {
        &self.scope
    }

    pub fn load(&self, generation: u64, bytes: Vec<u8>, volume: f32, play: bool, fade: Duration) {
        let _ = self.commands.send(Command::Load { generation, bytes, volume, play, fade });
    }

    /// The equalizer's settings, shared with the audio thread.
    pub fn eq(&self) -> &Arc<eq::Params> {
        &self.eq
    }
    pub fn play(&self) {
        let _ = self.commands.send(Command::Play);
    }
    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }
    pub fn seek(&self, to: Duration) {
        let _ = self.commands.send(Command::Seek(to));
    }
    pub fn mute(&self, on: bool) {
        let _ = self.commands.send(Command::Mute(on));
    }
    pub fn set_volume(&self, volume: f32) {
        let _ = self.commands.send(Command::Volume(volume));
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}
