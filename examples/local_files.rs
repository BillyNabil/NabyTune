//! Tries local_files.rs on songs made up on the spot: WAV files with and without tags, a
//! FLAC file with Vorbis comments, ReplayGain and an embedded cover, a folder cover, hidden
//! folders, a broken file and a folder link that loops back. Then plays them through
//! decoder.rs. `cargo run --example local_files` (or `-- <folder>` to scan a folder of your
//! own, read-only, with the index kept in a temp folder); `cargo test --example local_files`.

#[path = "../src/decoder.rs"]
#[allow(dead_code)]
mod decoder;
#[path = "../src/prefetch.rs"]
#[allow(dead_code)]
mod prefetch;
#[path = "../src/local_files.rs"]
#[allow(dead_code)]
mod local_files;

use std::io::Cursor;
use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};
use rodio::Source;

fn main() -> anyhow::Result<()> {
    if let Some(folder) = std::env::args().nth(1) {
        let temp = TempDir::new("smoke");
        let index = temp.0.join("local-index.json");
        for pass in ["first", "second"] {
            let started = std::time::Instant::now();
            let (tracks, stats) = local_files::scan_with_stats(&[PathBuf::from(&folder)], &index);
            println!("{pass} scan: {} songs in {:?} ({stats:?})", tracks.len(), started.elapsed());
            if pass == "first" {
                for t in tracks.iter().take(40) {
                    println!("  {:>5}  {} — {}  [{}]  {}", local_files::duration_text(t.duration_secs), t.artist, t.title, t.album, t.path.display());
                }
                if let Some(t) = tracks.first() {
                    let source = decoder::AudioSource::new(local_files::read(&t.path)?).map_err(anyhow::Error::msg)?;
                    println!("decoded {}: {} Hz, {} ch, {:?}", t.path.display(), source.sample_rate(), source.channels(), source.total_duration());
                }
            }
        }
        return Ok(());
    }

    let temp = TempDir::new("demo");
    let music = make_library(&temp.0)?;
    let index = temp.0.join("index").join("local-index.json");
    let (tracks, stats) = local_files::scan_with_stats(&[music.clone()], &index);
    println!("first scan: {stats:?}");
    for t in &tracks {
        println!(
            "  {:>5}  {} — {}  [{} / {}]  track {:?} disc {:?} year {:?} gain {:?} cover {}",
            local_files::duration_text(t.duration_secs),
            t.artist,
            t.title,
            t.album,
            t.album_artist,
            t.track_no,
            t.disc_no,
            t.year,
            t.gain_db,
            t.has_cover
        );
    }
    let (_, stats) = local_files::scan_with_stats(&[music], &index);
    println!("second scan: {stats:?}");
    for t in &tracks {
        let cover = local_files::cover(&t.path).map(|c| c.dimensions());
        let samples = decoder::AudioSource::new(local_files::read(&t.path)?).map_err(anyhow::Error::msg)?.count();
        println!("  {}: cover {cover:?}, {samples} samples decoded", t.id());
    }
    Ok(())
}

/// A folder under the system's temp folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("nabytune-local-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // A junction is removed as a link, never followed.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A sine at `freq`, 16-bit stereo 44.1 kHz.
fn sine(freq: f32, secs: f32) -> Vec<i16> {
    let frames = (44_100.0 * secs) as usize;
    (0..frames)
        .flat_map(|i| {
            let s = ((i as f32 / 44_100.0 * freq * std::f32::consts::TAU).sin() * 12_000.0) as i16;
            [s, s]
        })
        .collect()
}

fn write_wav(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // channels
    out.extend_from_slice(&44_100u32.to_le_bytes());
    out.extend_from_slice(&(44_100u32 * 4).to_le_bytes()); // bytes a second
    out.extend_from_slice(&4u16.to_le_bytes()); // bytes a frame
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out)
}

fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in bytes {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
        }
    }
    crc
}

/// FLAC's UTF-8-like frame number.
fn coded_number(n: u32) -> Vec<u8> {
    match n {
        0..=0x7F => vec![n as u8],
        0x80..=0x7FF => vec![0xC0 | (n >> 6) as u8, 0x80 | (n & 0x3F) as u8],
        _ => vec![0xE0 | (n >> 12) as u8, 0x80 | ((n >> 6) & 0x3F) as u8, 0x80 | (n & 0x3F) as u8],
    }
}

/// An uncompressed (verbatim) FLAC file, 16-bit stereo 44.1 kHz, with only its STREAMINFO.
fn write_flac(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    const BLOCK: usize = 4096;
    let frames = samples.len() / 2;
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0, 0, 34]); // last block, STREAMINFO, 34 bytes
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&[0; 6]); // frame sizes unknown
    let packed: u64 = (44_100u64 << 44) | (1 << 41) | (15 << 36) | frames as u64;
    out.extend_from_slice(&packed.to_be_bytes());
    out.extend_from_slice(&[0; 16]); // no MD5
    for (number, chunk) in samples.chunks(BLOCK * 2).enumerate() {
        let len = chunk.len() / 2;
        let start = out.len();
        out.extend_from_slice(&[0xFF, 0xF8, 0x79, 0x18]); // fixed blocks; size at end; 44.1 kHz; L/R; 16-bit
        out.extend_from_slice(&coded_number(number as u32));
        out.extend_from_slice(&((len - 1) as u16).to_be_bytes());
        out.push(crc8(&out[start..]));
        for ch in 0..2 {
            out.push(0x02); // verbatim subframe
            for frame in chunk.chunks(2) {
                out.extend_from_slice(&frame[ch].to_be_bytes());
            }
        }
        let crc = crc16(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    std::fs::write(path, out)
}

fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
    let mut bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).expect("png");
    bytes
}

fn picture(bytes: Vec<u8>, kind: PictureType) -> Picture {
    Picture::unchecked(bytes).pic_type(kind).mime_type(MimeType::Png).build()
}

/// The made-up library:
///
/// music/
///   Daft Artist - Tagless Song.wav     no tags: artist and title from the name
///   tagged-info.wav                    RIFF INFO tags
///   tagged-id3.wav                     ID3v2 tags with a front cover (red, 40x30)
///   broken.mp3                         not audio
///   notes.txt
///   Album/01 - first.flac              Vorbis comments, ReplayGain, back + front covers
///   Album/07 just a name.wav           no tags; cover.JPG beside it (green)
///   Album/loop -> music (junction)
///   .hidden/skipped.wav
///   secret/skipped.wav                 hidden attribute
fn make_library(root: &Path) -> anyhow::Result<PathBuf> {
    let music = root.join("music");
    let album = music.join("Album");
    std::fs::create_dir_all(&album)?;
    let wav = sine(440.0, 1.5);

    write_wav(&music.join("Daft Artist - Tagless Song.wav"), &wav)?;

    let info = music.join("tagged-info.wav");
    write_wav(&info, &wav)?;
    let mut tag = Tag::new(TagType::RiffInfo);
    tag.set_title("Info Title".into());
    tag.set_artist("Info Artist".into());
    tag.set_album("Info Album".into());
    tag.save_to_path(&info, WriteOptions::default())?;

    let id3 = music.join("tagged-id3.wav");
    write_wav(&id3, &wav)?;
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title("Id3 Title".into());
    tag.set_artist("Id3 Artist".into());
    tag.set_album("Id3 Album".into());
    tag.insert_text(ItemKey::RecordingDate, "1999-03-01".into());
    tag.set_track(4);
    tag.set_track_total(12);
    tag.push_picture(picture(png(40, 30, [255, 0, 0]), PictureType::CoverFront));
    tag.save_to_path(&id3, WriteOptions::default())?;

    std::fs::write(music.join("broken.mp3"), "this is not audio, only words ".repeat(60))?;
    std::fs::write(music.join("notes.txt"), "not a song")?;

    let flac = album.join("01 - first.flac");
    write_flac(&flac, &sine(330.0, 1.0))?;
    let mut tag = Tag::new(TagType::VorbisComments);
    tag.set_title("Flac Title".into());
    tag.set_artist("Flac Artist".into());
    tag.set_album("Flac Album".into());
    tag.insert_text(ItemKey::AlbumArtist, "Various Artists".into());
    tag.insert_text(ItemKey::TrackNumber, "1".into());
    tag.insert_text(ItemKey::DiscNumber, "2".into());
    tag.insert_text(ItemKey::RecordingDate, "2004".into());
    tag.insert_text(ItemKey::ReplayGainTrackGain, "-6.54 dB".into());
    tag.push_picture(picture(png(10, 10, [0, 0, 255]), PictureType::CoverBack));
    tag.push_picture(picture(png(20, 20, [255, 255, 0]), PictureType::CoverFront));
    tag.save_to_path(&flac, WriteOptions::default())?;

    write_wav(&album.join("07 just a name.wav"), &wav)?;
    let green = image::RgbImage::from_pixel(16, 16, image::Rgb([0, 255, 0]));
    image::DynamicImage::ImageRgb8(green).save_with_format(album.join("cover.JPG"), image::ImageFormat::Jpeg)?;

    let dot = music.join(".hidden");
    std::fs::create_dir_all(&dot)?;
    write_wav(&dot.join("skipped.wav"), &wav)?;
    let secret = music.join("secret");
    std::fs::create_dir_all(&secret)?;
    write_wav(&secret.join("skipped.wav"), &wav)?;

    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("attrib").arg("+h").arg(&secret).status()?;
        // A junction needs no rights; it loops the scan back to the top.
        Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(album.join("loop")).arg(&music).output()?;
    }
    Ok(music)
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_files::{LocalTrack, ScanStats};

    fn by_title<'a>(tracks: &'a [LocalTrack], title: &str) -> &'a LocalTrack {
        tracks.iter().find(|t| t.title == title).unwrap_or_else(|| panic!("no {title:?} in {tracks:#?}"))
    }

    #[test]
    fn scans_reads_tags_and_reuses_the_index() {
        let temp = TempDir::new("scan");
        let music = make_library(&temp.0).unwrap();
        let index = temp.0.join("index").join("local-index.json");
        #[cfg(windows)]
        assert!(music.join("Album").join("loop").join("Album").join("loop").exists(), "the junction loops");

        let (tracks, stats) = local_files::scan_with_stats(&[music.clone()], &index);
        assert_eq!(stats, ScanStats { found: 6, read: 6, reused: 0, failed: 1 }, "{tracks:#?}");
        assert_eq!(tracks.len(), 5);
        assert!(index.exists());
        assert!(tracks.iter().all(|t| !t.path.to_string_lossy().contains("skipped")));
        assert!(tracks.iter().all(|t| !t.path.to_string_lossy().contains("loop")), "the junction is walked once, as the folder itself");

        let tagless = by_title(&tracks, "Tagless Song");
        assert_eq!(tagless.artist, "Daft Artist");
        assert_eq!(tagless.album, "");
        assert!(!tagless.has_cover);
        assert!((tagless.duration_secs - 1.5).abs() < 0.01, "{}", tagless.duration_secs);

        let plain = by_title(&tracks, "just a name");
        assert_eq!(plain.artist, local_files::UNKNOWN_ARTIST);
        assert_eq!(plain.track_no, Some(7));

        let info = by_title(&tracks, "Info Title");
        assert_eq!((info.artist.as_str(), info.album.as_str()), ("Info Artist", "Info Album"));

        let id3 = by_title(&tracks, "Id3 Title");
        assert_eq!((id3.artist.as_str(), id3.album.as_str(), id3.album_artist.as_str()), ("Id3 Artist", "Id3 Album", "Id3 Artist"));
        assert_eq!((id3.track_no, id3.year), (Some(4), Some(1999)));
        assert!(id3.has_cover);

        let flac = by_title(&tracks, "Flac Title");
        assert_eq!(flac.artist, "Flac Artist");
        assert_eq!(flac.album_artist, "Various Artists");
        assert_eq!((flac.track_no, flac.disc_no, flac.year), (Some(1), Some(2), Some(2004)));
        assert_eq!(flac.gain_db, Some(-6.54));
        assert!(flac.has_cover);
        assert!((flac.duration_secs - 1.0).abs() < 0.01, "{}", flac.duration_secs);

        // Second scan: everything from the index, the same songs.
        let (again, stats) = local_files::scan_with_stats(&[music.clone()], &index);
        assert_eq!(stats, ScanStats { found: 6, read: 0, reused: 6, failed: 1 });
        assert_eq!(again, tracks);

        // A file retagged is read again; the rest still come from the index.
        let mut tag = Tag::new(TagType::RiffInfo);
        tag.set_title("Renamed".into());
        tag.set_artist("Info Artist".into());
        tag.save_to_path(&info.path, WriteOptions::default()).unwrap();
        let (third, stats) = local_files::scan_with_stats(&[music.clone()], &index);
        assert_eq!(stats.read, 1, "{stats:?}");
        assert_eq!(stats.reused, 5);
        assert!(third.iter().any(|t| t.title == "Renamed"));

        // A damaged or foreign index is ignored, not trusted.
        std::fs::write(&index, "{ not json").unwrap();
        let (fourth, stats) = local_files::scan_with_stats(&[music.clone()], &index);
        assert_eq!((stats.read, fourth.len()), (6, 5));

        // The same folder picked twice, or a folder and its subfolder, gives each song once.
        let both = local_files::scan(&[music.clone(), music.join("Album"), music.clone()], &index);
        assert_eq!(both.len(), 5);

        // A folder that isn't there is no error.
        assert!(local_files::scan(&[temp.0.join("missing")], &temp.0.join("other.json")).is_empty());

        // In the background.
        let handle = local_files::scan_in_background(vec![music], index).unwrap();
        assert_eq!(handle.join().unwrap().len(), 5);
    }

    #[test]
    fn covers_come_from_the_file_then_the_folder() {
        let temp = TempDir::new("cover");
        let music = make_library(&temp.0).unwrap();

        let id3 = local_files::cover(&music.join("tagged-id3.wav")).expect("embedded cover");
        assert_eq!(id3.dimensions(), (40, 30));
        assert_eq!(id3.get_pixel(5, 5).0, [255, 0, 0, 255]);

        // The front cover, though the back one comes first.
        let flac = local_files::cover(&music.join("Album").join("01 - first.flac")).expect("flac cover");
        assert_eq!(flac.dimensions(), (20, 20));
        assert_eq!(flac.get_pixel(5, 5).0, [255, 255, 0, 255]);

        let folder = local_files::cover(&music.join("Album").join("07 just a name.wav")).expect("cover.JPG");
        assert_eq!(folder.dimensions(), (16, 16));
        let [r, g, b, _] = folder.get_pixel(8, 8).0;
        assert!(r < 30 && g > 220 && b < 30, "{:?}", (r, g, b));

        assert!(local_files::cover(&music.join("Daft Artist - Tagless Song.wav")).is_none());
        assert!(local_files::cover(&temp.0.join("nothing.wav")).is_none());
    }

    #[test]
    fn big_covers_are_scaled_down() {
        let temp = TempDir::new("bigcover");
        let file = temp.0.join("big.wav");
        write_wav(&file, &sine(440.0, 0.2)).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(picture(png(2400, 1600, [9, 9, 9]), PictureType::CoverFront));
        tag.save_to_path(&file, WriteOptions::default()).unwrap();
        assert_eq!(local_files::cover(&file).unwrap().dimensions(), (1200, 800));
    }

    #[test]
    fn every_scanned_file_decodes() {
        let temp = TempDir::new("decode");
        let music = make_library(&temp.0).unwrap();
        let tracks = local_files::scan(&[music], &temp.0.join("index.json"));
        assert_eq!(tracks.len(), 5);
        for t in &tracks {
            let source = decoder::AudioSource::new(local_files::read(&t.path).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", t.path.display()));
            assert_eq!((source.sample_rate(), source.channels()), (44_100, 2), "{}", t.path.display());
            let decoded = source.count() as f64 / 2.0 / 44_100.0;
            assert!((decoded - t.duration_secs).abs() < 0.05, "{}: decoded {decoded} s, tags say {}", t.path.display(), t.duration_secs);
        }
        assert!(local_files::read(&temp.0.join("nothing.wav")).is_err());
    }

    #[test]
    fn ids_and_paths() {
        let t = LocalTrack {
            path: PathBuf::from(r"C:\Music\A - B.flac"),
            title: "B".into(),
            artist: "A".into(),
            album: String::new(),
            album_artist: "A".into(),
            track_no: None,
            disc_no: None,
            year: None,
            duration_secs: 0.0,
            has_cover: false,
            gain_db: None,
            modified: 0,
            size: 0,
        };
        assert_eq!(t.id(), r"file:C:\Music\A - B.flac");
        assert_eq!(local_files::path_of(&t.id()), Some(t.path.clone()));
        assert_eq!(local_files::path_of("dQw4w9WgXcQ"), None);
        assert_eq!(local_files::path_of("file:"), None);
        assert!(local_files::is_supported(Path::new("x.FLAC")));
        assert!(local_files::is_supported(Path::new("x.m4a")));
        assert!(!local_files::is_supported(Path::new("x.opus")));
        assert!(!local_files::is_supported(Path::new("flac")));
        assert_eq!(local_files::extension_hint(Path::new("x.AAC")).as_deref(), Some("aac"));
    }

    #[test]
    fn durations() {
        assert_eq!(local_files::duration_text(187.2), "3:07");
        assert_eq!(local_files::duration_text(59.6), "1:00");
        assert_eq!(local_files::duration_text(0.0), "0:00");
        assert_eq!(local_files::duration_text(f64::NAN), "0:00");
        assert_eq!(local_files::duration_text(-5.0), "0:00");
        assert_eq!(local_files::duration_text(3723.0), "1:02:03");
    }

    #[test]
    fn search_needs_every_word() {
        let make = |title: &str, artist: &str, album: &str| LocalTrack {
            path: PathBuf::from(format!("{title}.mp3")),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: artist.into(),
            track_no: None,
            disc_no: None,
            year: None,
            duration_secs: 0.0,
            has_cover: false,
            gain_db: None,
            modified: 0,
            size: 0,
        };
        let tracks = vec![
            make("Harder Better Faster", "Daft Punk", "Discovery"),
            make("One More Time", "Daft Punk", "Discovery"),
            make("Time", "Pink Floyd", "The Dark Side of the Moon"),
            make("Café del Mar", "Energy 52", "Café"),
        ];
        let titles = |q: &str| local_files::search(&tracks, q).iter().map(|t| t.title.as_str()).collect::<Vec<_>>();
        assert_eq!(titles("daft TIME"), ["One More Time"]);
        assert_eq!(titles("time"), ["One More Time", "Time"]);
        assert_eq!(titles("discovery"), ["Harder Better Faster", "One More Time"]);
        assert_eq!(titles("CAFÉ"), ["Café del Mar"]);
        assert_eq!(titles("pink moon"), ["Time"]);
        assert!(titles("daft floyd").is_empty());
        assert!(titles("   ").is_empty());
    }
}
