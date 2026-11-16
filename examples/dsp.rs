//! Times the spectrum tap and the vocal cut (the tests live in src/dsp.rs and run with
//! `cargo test --example dsp`).
//! `cargo run --release --example dsp`

#[path = "../src/dsp.rs"]
#[allow(dead_code)]
mod dsp;

use rodio::buffer::SamplesBuffer;
use std::time::{Duration, Instant};

const RATE: u32 = 48_000;

/// Ten seconds of stereo music-ish noise: a little centred tone over a wide hiss.
fn song() -> Vec<f32> {
    let mut x = 0x9e3779b9u32;
    (0..RATE as usize * 10)
        .flat_map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let hiss = (x as f32 / u32::MAX as f32 - 0.5) * 0.2;
            let tone = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin() * 0.3;
            [tone + hiss, tone - hiss]
        })
        .collect()
}

/// Samples per second through `make`'s source, best of three.
fn throughput<I: Iterator<Item = f32>>(samples: &[f32], make: impl Fn(SamplesBuffer<f32>) -> I) -> f64 {
    (0..3)
        .map(|_| {
            let source = SamplesBuffer::new(2, RATE, samples.to_vec());
            let start = Instant::now();
            let sum: f32 = make(source).sum();
            std::hint::black_box(sum);
            samples.len() as f64 / start.elapsed().as_secs_f64()
        })
        .fold(0.0, f64::max)
}

fn main() {
    let samples = song();
    let realtime = (RATE * 2) as f64;
    let plain = throughput(&samples, |s| s);
    let scope = dsp::Scope::new();
    let tap = throughput(&samples, |s| dsp::Tap::new(s, scope.clone()));
    for (name, rate) in [("plain buffer", plain), ("Tap", tap)] {
        println!("{name:20} {:7.1} M samples/s  ({:5.0}x real time at 48 kHz stereo)", rate / 1e6, rate / realtime);
    }

    // The UI's reads, between which the audio thread keeps feeding the scope, as it would.
    let feeder = {
        let scope = scope.clone();
        std::thread::spawn(move || {
            let mut tap = dsp::Tap::new(SamplesBuffer::new(2, RATE, song()), scope);
            let start = Instant::now();
            let mut played = 0usize;
            while start.elapsed() < Duration::from_secs(4) {
                // Keep about real time, in 10 ms device-sized bursts.
                let due = (start.elapsed().as_secs_f64() * RATE as f64 * 2.0) as usize;
                while played < due {
                    if tap.next().is_none() {
                        return;
                    }
                    played += 1;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    let mut times = Vec::new();
    let mut levels = (0.0f32, 0.0f32);
    for _ in 0..180 {
        let start = Instant::now();
        let bands = scope.bands(48);
        let level = scope.level();
        let beat = scope.beat();
        times.push(start.elapsed());
        std::hint::black_box(&bands);
        levels = (levels.0.max(level), levels.1.max(beat));
        std::thread::sleep(Duration::from_millis(16));
    }
    let _ = feeder.join();
    times.sort();
    println!(
        "bands(48)+level+beat per frame: median {:?}, p99 {:?}, max {:?} (max level {:.2}, max beat {:.2})",
        times[times.len() / 2],
        times[times.len() * 99 / 100],
        times[times.len() - 1],
        levels.0,
        levels.1
    );
    // And after the feeding stopped, the readings fall to rest.
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..90 {
        scope.bands(48);
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("1.6 s after the last sample: max band {:.3}, level {:.3}", scope.bands(48).iter().cloned().fold(0.0, f32::max), scope.level());
}
