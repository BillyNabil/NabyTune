//! A whole audio file in memory, decoded by symphonia into a rodio source: AAC in
//! (fragmented) MP4 from YouTube, FLAC or ALAC from lossless addons, and the rest symphonia
//! knows. The container is told by its first bytes.
//!
//! The bytes are handed over as a `Cursor`, which tells symphonia the stream's
//! length; YouTube's DASH files are fragmented MP4, and reading them as an
//! unbounded stream doesn't work.

use std::io::Cursor;
use std::time::Duration;

use rodio::source::SeekError;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

pub struct AudioSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    /// The current packet's samples, interleaved.
    buffer: Vec<f32>,
    pos: usize,
    channels: u16,
    rate: u32,
    duration: Option<Duration>,
}

impl AudioSource {
    pub fn new(bytes: Vec<u8>) -> Result<Self, String> {
        let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        // No hint: the bytes say what they are ("ftyp", "fLaC", "OggS", "ID3"…).
        let hint = Hint::new();
        let probed = symphonia::default::get_probe()
            .format(&hint, stream, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
            .map_err(|e| format!("probe: {e}"))?;
        let format = probed.format;
        let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio track")?;
        let params = &track.codec_params;
        let decoder = symphonia::default::get_codecs()
            .make(params, &DecoderOptions::default())
            .map_err(|e| format!("codec: {e}"))?;
        let duration = match (params.n_frames, params.time_base) {
            (Some(frames), Some(base)) => {
                let t = base.calc_time(frames);
                Some(Duration::from_secs(t.seconds) + Duration::from_secs_f64(t.frac))
            }
            _ => None,
        }
        // Fragmented MP4 (DASH, HLS) states no length up front: 0 there means "not said".
        .filter(|d| !d.is_zero());
        let mut source = Self {
            track_id: track.id,
            channels: params.channels.map_or(2, |c| c.count() as u16),
            rate: params.sample_rate.unwrap_or(44_100),
            duration,
            format,
            decoder,
            buffer: Vec::new(),
            pos: 0,
        };
        // The first packet tells the truth about the layout even when the container doesn't.
        if !source.refill() {
            return Err("no audio in the stream".into());
        }
        Ok(source)
    }

    /// Decodes the next packet of our track into `buffer`. False at the end of the stream.
    fn refill(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(_) => return false,
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    self.channels = spec.channels.count() as u16;
                    self.rate = spec.rate;
                    let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    samples.copy_interleaved_ref(decoded);
                    self.buffer.clear();
                    self.buffer.extend_from_slice(samples.samples());
                    self.pos = 0;
                    if !self.buffer.is_empty() {
                        return true;
                    }
                }
                // A damaged packet: skip it, as players do.
                Err(Error::DecodeError(_)) => continue,
                Err(_) => return false,
            }
        }
    }
}

/// AAC overshoots full scale on loud masters (a peak of 1.18 measured on an ordinary
/// track). Hard clipping there crackles; this leaves everything under 0.9 untouched
/// and bends the rest smoothly towards 1.0.
fn soft_limit(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
    }
}

impl Iterator for AudioSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buffer.len() && !self.refill() {
            return None;
        }
        let sample = self.buffer[self.pos];
        self.pos += 1;
        Some(soft_limit(sample))
    }
}

impl rodio::Source for AudioSource {
    /// None: the layout never changes within a track. A frame length here makes rodio
    /// rebuild its sample-rate converter at every frame's end, which with a packet-sized
    /// frame is some 43 seams a second when the device runs at 48 kHz: the crackle.
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
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.format
            .seek(SeekMode::Accurate, SeekTo::Time { time: Time::from(pos.as_secs_f64()), track_id: Some(self.track_id) })
            .map_err(|e| SeekError::Other(Box::new(e)))?;
        self.decoder.reset();
        self.buffer.clear();
        self.pos = 0;
        Ok(())
    }
}
