//! Decodes an .m4a file with the prototype's decoder and reports what came out,
//! including what rodio's resampler makes of it at a typical device rate.
//! `cargo run --release --example decode -- file.m4a`

#[path = "../src/decoder.rs"]
mod decoder;

use rodio::source::UniformSourceIterator;
use rodio::Source;

/// The largest sample-to-sample step per channel, and how many steps exceed `limit`.
fn steps(samples: &[f32], channels: usize, limit: f32) -> (f32, usize) {
    let mut largest = 0f32;
    let mut over = 0;
    for i in channels..samples.len() {
        let d = (samples[i] - samples[i - channels]).abs();
        largest = largest.max(d);
        over += (d > limit) as usize;
    }
    (largest, over)
}

fn main() {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(&path).expect("read");
    let started = std::time::Instant::now();
    let source = decoder::AudioSource::new(bytes.clone()).expect("open");
    let (rate, channels) = (source.sample_rate(), source.channels());
    println!("opened: {rate} Hz, {channels} ch, duration {:?}", source.total_duration());
    let native: Vec<f32> = source.collect();
    let peak = native.iter().fold(0f32, |m, s| m.max(s.abs()));
    println!("decoded {:.1} s in {:?}, peak {peak:.3}", native.len() as f64 / rate as f64 / channels as f64, started.elapsed());

    let resampled: Vec<f32> = UniformSourceIterator::<_, f32>::new(decoder::AudioSource::new(bytes).unwrap(), 2, 48_000).collect();
    let (n_big, n_over) = steps(&native, channels as usize, 0.5);
    let (r_big, r_over) = steps(&resampled, 2, 0.5);
    println!("44.1 kHz as decoded : largest step {n_big:.3}, steps > 0.5: {n_over}");
    println!("48 kHz via rodio    : largest step {r_big:.3}, steps > 0.5: {r_over}, length {:.1} s", resampled.len() as f64 / 48_000.0 / 2.0);

    let mut again = decoder::AudioSource::new(std::fs::read(&path).unwrap()).unwrap();
    again.try_seek(std::time::Duration::from_secs(120)).expect("seek");
    println!("after seeking to 2:00, {:.1} s remain", again.count() as f64 / rate as f64 / channels as f64);
}
