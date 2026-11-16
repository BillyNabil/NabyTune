//! The audio engine: one thread owning the output device (WASAPI through
//! cpal) and a rodio sink fed by symphonia's AAC decoder. The UI talks to it
//! with commands and reads a shared status; it never blocks on audio.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{OutputStream, Sink, Source};

use crate::decoder::AudioSource;
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
    from: std::time::Instant,
    length: Duration,
    start: f32,
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
                let (_stream, handle) = match OutputStream::try_default() {
                    Ok(pair) => {
                        log("output device open");
                        pair
                    }
                    Err(e) => {
                        log(format!("no output device: {e}"));
                        shared.lock().unwrap().error = Some(format!("No audio output: {e}"));
                        return;
                    }
                };
                let mut sink: Option<Sink> = None;
                let mut loaded = false;
                // The track's normalised volume, and whether the listener muted it.
                let mut volume = 1.0f32;
                let mut muted = std::env::var_os("NABYTUNE_PROTO_MUTE").is_some();
                let probe = std::env::var_os("NABYTUNE_PROTO_FPS").is_some();
                let mut drift: Option<(std::time::Instant, Duration, u64)> = None;
                let mut drift_generation = 0u64;
                let mut fading: Option<Fading> = None;
                // The fading-in song's own volume, and when its fade began.
                let mut fade_in: Option<(std::time::Instant, Duration)> = None;
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
                                    fading = Some(Fading { sink: old, from: std::time::Instant::now(), length: fade, start });
                                    fade_in = Some((std::time::Instant::now(), fade));
                                } else {
                                    old.stop();
                                    scope_feed.clear();
                                }
                            }
                            let mut status = Status { generation, ..Status::default() };
                            match AudioSource::new(bytes).and_then(|decoder| {
                                let duration = decoder.total_duration();
                                log(format!(
                                    "decoder: {} Hz, {} ch, duration {duration:?}",
                                    decoder.sample_rate(),
                                    decoder.channels()
                                ));
                                let s = Sink::try_new(&handle).map_err(|e| e.to_string())?;
                                // NABYTUNE_PROTO_MUTE=1 starts muted, for testing beside a session that's playing.
                                s.set_volume(if muted || fade_in.is_some() { 0.0 } else { volume });
                                if !play {
                                    s.pause();
                                }
                                // The equalizer, then the visualizer hears what's heard.
                                s.append(dsp::Tap::new(eq::Equalized::new(decoder, eq_params.clone()), scope_feed.clone()));
                                Ok((s, duration))
                            }) {
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
                            *shared.lock().unwrap() = status;
                        }
                        Ok(Command::Play) => sink.iter().for_each(|s| s.play()),
                        Ok(Command::Pause) => {
                            sink.iter().for_each(|s| s.pause());
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
                                if let Err(e) = s.try_seek(to) {
                                    shared.lock().unwrap().error = Some(format!("Seek failed: {e}"));
                                }
                            }
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
                        status.position = s.get_pos();
                        // With NABYTUNE_PROTO_FPS set: every 5 s of unbroken playback, the reported
                        // position against the wall clock, to catch a clock that drifts.
                        if probe {
                            let wall = std::time::Instant::now();
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
