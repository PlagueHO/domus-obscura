use domus::domain::{AudioFormat, OutputSettings};
use domus::infrastructure::artwork::normalize_cover;
use domus::infrastructure::catalog::CatalogQuery;
use domus::infrastructure::flac::inspect_flac;
use domus::infrastructure::media::{NormalizedAudio, normalize_audio};
use domus::{Error, Result};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use symphonia::core::checksum::{Crc8Ccitt, Crc16Ansi, Md5};
use symphonia::core::io::Monitor;
use tempfile::TempDir;

const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
const ARTIST_ID: &str = "10000000-0000-4000-8000-000000000001";
const ARTIST: &str = "The Test Signals";
const ALBUM: &str = "Loopback: Three Tones";
const TITLES: [&str; 3] = ["First / Light", "Second Signal", "Third: Return"];
const RATE: u32 = 44_100;
const BLOCK: u16 = 1024;
const FRAMES: u8 = 128;

#[derive(Clone, Serialize)]
pub struct AlbumReport {
    pub source: &'static str,
    pub directory: PathBuf,
    pub format: &'static str,
    pub cover_sha256: String,
    pub tracks: Vec<TrackReport>,
}

#[derive(Clone, Serialize)]
pub struct TrackReport {
    pub number: usize,
    pub filename: String,
    pub title: String,
    pub artist: &'static str,
    pub album: &'static str,
    pub disc: u16,
    pub disc_track: u16,
    pub duration_seconds: f64,
    pub downloaded_sha256: String,
    pub evidence: NormalizedAudio,
    pub tags_verified: bool,
}

pub struct AlbumTest {
    pub report: AlbumReport,
    pub audio: Vec<Vec<u8>>,
    _directory: TempDir,
}

/// Generate actual audible native FLAC with verbatim subframes. This fixture
/// encoder is not part of the application's no-transcoding media pipeline.
fn tone(frequency: f64) -> Vec<u8> {
    let total = usize::from(BLOCK) * usize::from(FRAMES);
    let mut md5 = Md5::default();
    let mut frames = Vec::new();
    for frame_number in 0..FRAMES {
        // Browser demuxers need the explicit 44.1 kHz rate for timestamps.
        let mut frame = vec![0xff, 0xf8, 0x79, 0x08, frame_number];
        frame.extend_from_slice(&(BLOCK - 1).to_be_bytes());
        let mut crc = Crc8Ccitt::new(0);
        crc.process_buf_bytes(&frame);
        frame.push(crc.crc());
        frame.push(0x02); // One 16-bit mono verbatim subframe.
        for offset in 0..usize::from(BLOCK) {
            let index = usize::from(frame_number) * usize::from(BLOCK) + offset;
            let fade = (index.min(total - 1 - index) as f64 / 2205.0).min(1.0);
            let sample = ((std::f64::consts::TAU * frequency * index as f64 / f64::from(RATE))
                .sin()
                * 4096.0
                * fade) as i16;
            frame.extend_from_slice(&sample.to_be_bytes());
            md5.process_buf_bytes(&sample.to_le_bytes());
        }
        let mut crc = Crc16Ansi::new(0);
        crc.process_buf_bytes(&frame);
        frame.extend_from_slice(&crc.crc().to_be_bytes());
        frames.extend_from_slice(&frame);
    }
    let mut info = [0u8; 34];
    info[..2].copy_from_slice(&BLOCK.to_be_bytes());
    info[2..4].copy_from_slice(&BLOCK.to_be_bytes());
    let packed = (u64::from(RATE) << 44) | (15 << 36) | total as u64;
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    info[18..].copy_from_slice(&md5.md5());
    let mut file = b"fLaC\x80\x00\x00\x22".to_vec();
    file.extend_from_slice(&info);
    file.extend_from_slice(&frames);
    file
}

fn release_body() -> Result<Vec<u8>> {
    let credit = json!([{"artist":{"id":ARTIST_ID,"name":ARTIST},"name":ARTIST,"joinphrase":""}]);
    let track = |index: usize, position: u16| {
        json!({
            "id":format!("40000000-0000-4000-8000-{:012}", index + 1),
            "position":position, "number":position.to_string(), "title":TITLES[index],
            "artist-credit":credit,
            "recording":{"id":format!("50000000-0000-4000-8000-{:012}", index + 1),"video":false}
        })
    };
    Ok(serde_json::to_vec(&json!({
        "id":RELEASE,"title":ALBUM,"date":"2026-10-25","artist-credit":credit,
        "release-group":{"id":"30000000-0000-4000-8000-000000000001"},
        "media":[
            {"position":1,"track-count":2,"tracks":[track(0,1),track(1,2)]},
            {"position":2,"track-count":1,"tracks":[track(2,1)]}
        ]
    }))?)
}

struct SourceServer(tokio::task::JoinHandle<std::io::Result<()>>);
impl Drop for SourceServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn run() -> Result<AlbumTest> {
    let originals = tokio::task::spawn_blocking(|| {
        [220.0, 330.0, 440.0]
            .into_iter()
            .map(tone)
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|_| Error::Internal("sample audio generation failed"))?;
    let source = Arc::new(originals.clone());
    let app = axum::Router::new().route(
        "/track/{number}",
        axum::routing::get(
            move |axum::extract::Path(number): axum::extract::Path<usize>| {
                let source = source.clone();
                async move {
                    match number.checked_sub(1).and_then(|index| source.get(index)) {
                        Some(bytes) => (
                            axum::http::StatusCode::OK,
                            [(axum::http::header::CONTENT_TYPE, "audio/flac")],
                            bytes.clone(),
                        ),
                        None => (
                            axum::http::StatusCode::NOT_FOUND,
                            [(axum::http::header::CONTENT_TYPE, "text/plain")],
                            Vec::new(),
                        ),
                    }
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let source_task = SourceServer(tokio::spawn(
        async move { axum::serve(listener, app).await },
    ));
    domus::infrastructure::tls::initialize_crypto()?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| Error::Internal("sample download client failed"))?;
    let mut downloads = Vec::new();
    for (index, original) in originals.iter().enumerate() {
        let mut response = client
            .get(format!("http://{address}/track/{}", index + 1))
            .send()
            .await
            .map_err(|_| Error::Remote("sample album download failed"))?;
        if response.status() != reqwest::StatusCode::OK
            || response.content_length() != Some(original.len() as u64)
        {
            return Err(Error::Remote(
                "sample album response does not match the source",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Remote("sample album stream was interrupted"))?
        {
            if chunk.len() > original.len() - bytes.len() {
                return Err(Error::Remote(
                    "sample album download exceeds its exact size",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() != original.len() || Sha256::digest(&bytes) != Sha256::digest(original) {
            return Err(Error::Remote("sample album download checksum failed"));
        }
        downloads.push(bytes);
    }
    source_task.0.abort();
    tokio::task::spawn_blocking(move || finish(downloads))
        .await
        .map_err(|_| Error::Internal("sample album processing failed"))?
}

fn verify_tags(bytes: &[u8], index: usize, cover: &[u8]) -> Result<()> {
    let tag = metaflac::Tag::read_from(&mut Cursor::new(bytes)).map_err(|_| {
        Error::InvalidInput("independent sample tag reader rejected the output".into())
    })?;
    let expected = [
        ("TITLE", TITLES[index].into()),
        ("ARTIST", ARTIST.into()),
        ("ALBUMARTIST", ARTIST.into()),
        ("ALBUM", ALBUM.into()),
        ("TRACKNUMBER", (index + 1).to_string()),
        ("TRACKTOTAL", "3".into()),
        ("DISCNUMBER", if index == 2 { "2" } else { "1" }.into()),
        ("DISCTOTAL", "2".into()),
        (
            "ORIGINALTRACKNUMBER",
            if index == 1 { "2" } else { "1" }.into(),
        ),
        ("DATE", "2026-10-25".into()),
        ("MUSICBRAINZ_ALBUMID", RELEASE.into()),
        (
            "MUSICBRAINZ_RELEASEGROUPID",
            "30000000-0000-4000-8000-000000000001".into(),
        ),
        (
            "MUSICBRAINZ_TRACKID",
            format!("50000000-0000-4000-8000-{:012}", index + 1),
        ),
        (
            "MUSICBRAINZ_RELEASETRACKID",
            format!("40000000-0000-4000-8000-{:012}", index + 1),
        ),
        ("MUSICBRAINZ_ARTISTID", ARTIST_ID.into()),
        ("MUSICBRAINZ_ALBUMARTISTID", ARTIST_ID.into()),
    ];
    for (key, value) in expected {
        if tag.get_vorbis(key).map(|items| items.collect::<Vec<_>>()) != Some(vec![value.as_str()])
        {
            return Err(Error::InvalidInput(format!(
                "sample output tag mismatch: {key}"
            )));
        }
    }
    let pictures = tag.pictures().collect::<Vec<_>>();
    if pictures.len() != 1
        || pictures[0].data != cover
        || pictures[0].mime_type != "image/jpeg"
        || pictures[0].picture_type != metaflac::block::PictureType::CoverFront
    {
        return Err(Error::InvalidInput(
            "sample embedded artwork does not match folder.jpg".into(),
        ));
    }
    Ok(())
}

fn finish(downloads: Vec<Vec<u8>>) -> Result<AlbumTest> {
    let directory = tempfile::Builder::new()
        .prefix("domus-obscura-album-test-")
        .tempdir()?;
    let release = CatalogQuery::release(RELEASE)?.parse_release(&release_body()?)?;
    let settings = OutputSettings {
        format: AudioFormat::Flac,
        ..OutputSettings::default()
    };
    let outputs = release.selection(None)?.preview(&settings)?;
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(96, 96)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|_| Error::Internal("sample artwork generation failed"))?;
    let cover = normalize_cover(png.get_ref(), None)?;
    let album = outputs[0]
        .relative_folder
        .split('/')
        .fold(directory.path().to_path_buf(), |parent, component| {
            parent.join(component)
        });
    std::fs::create_dir_all(&album)?;
    let inputs = directory.path().join("downloaded-inputs");
    std::fs::create_dir(&inputs)?;
    let mut tracks = Vec::new();
    let mut audio = Vec::new();
    for (index, input) in downloads.into_iter().enumerate() {
        let input_hash = format!("{:x}", Sha256::digest(&input));
        let mut raw = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(inputs.join(format!("track-{}.flac", index + 1)))?;
        raw.write_all(&input)?;
        raw.sync_all()?;
        let tags = release.track_tags((index + 1) as u32)?;
        let path = album.join(&outputs[index].filename);
        let mut file = std::fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let evidence = normalize_audio(
            &mut Cursor::new(&input),
            &mut file,
            AudioFormat::Flac,
            &tags,
            &cover,
        )?;
        file.sync_all()?;
        drop(file);
        let bytes = std::fs::read(&path)?;
        verify_tags(&bytes, index, cover.bytes())?;
        let decoded = inspect_flac(&mut Cursor::new(&bytes))?;
        if !decoded.md5_verified
            || evidence.sha256 != format!("{:x}", Sha256::digest(&bytes))
            || evidence.input_sha256 != input_hash
            || evidence.audio_sha256 != decoded.audio_sha256
        {
            return Err(Error::InvalidInput(
                "sample album final verification failed".into(),
            ));
        }
        tracks.push(TrackReport {
            number: index + 1,
            filename: outputs[index].filename.clone(),
            title: TITLES[index].into(),
            artist: ARTIST,
            album: ALBUM,
            disc: if index == 2 { 2 } else { 1 },
            disc_track: if index == 1 { 2 } else { 1 },
            duration_seconds: decoded.samples_per_channel as f64 / f64::from(decoded.sample_rate),
            downloaded_sha256: input_hash,
            evidence,
            tags_verified: true,
        });
        audio.push(bytes);
    }
    let mut jpeg = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(album.join("folder.jpg"))?;
    jpeg.write_all(cover.bytes())?;
    jpeg.sync_all()?;
    drop(jpeg);
    if std::fs::read(album.join("folder.jpg"))? != cover.bytes()
        || std::fs::read_dir(&album)?.count() != 4
    {
        return Err(Error::InvalidInput(
            "sample output album contains unexpected files".into(),
        ));
    }
    let report = AlbumReport {
        source: "Generated three-tone album downloaded over loopback HTTP; not Soulseek or a live release.",
        directory: album,
        format: "flac",
        cover_sha256: format!("{:x}", Sha256::digest(cover.bytes())),
        tracks,
    };
    let mut manifest = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(directory.path().join("verification.json"))?;
    manifest.write_all(&serde_json::to_vec_pretty(&report)?)?;
    manifest.sync_all()?;
    Ok(AlbumTest {
        report,
        audio,
        _directory: directory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn generated_album_downloads_normalizes_and_preserves_all_tone_samples()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let result = run().await?;
        assert_eq!(result.report.tracks.len(), 3);
        assert_eq!(
            result
                .report
                .directory
                .file_name()
                .and_then(|name| name.to_str()),
            Some("LOOPBACK_ THREE TONES")
        );
        assert!(
            result.report.directory.ends_with(
                std::path::Path::new("T")
                    .join("THE TEST SIGNALS")
                    .join("LOOPBACK_ THREE TONES")
            )
        );
        assert_eq!(
            result.report.tracks[0].filename,
            "The Test Signals - Loopback_ Three Tones - 01 - First _ Light.flac"
        );
        for (index, track) in result.report.tracks.iter().enumerate() {
            assert!(track.filename.contains(&format!(" - {:02} - ", index + 1)));
            assert!(track.tags_verified);
            assert_eq!(track.evidence.md5_verified, Some(true));
            assert_eq!(
                track.evidence.samples_per_channel,
                u64::from(BLOCK) * u64::from(FRAMES)
            );
            assert_eq!(track.evidence.input_sha256, track.downloaded_sha256);
            assert!(track.duration_seconds > 2.9);
            assert!(result.report.directory.join(&track.filename).is_file());
            let original = tone([220.0, 330.0, 440.0][index]);
            let before = inspect_flac(&mut Cursor::new(&original))?;
            let after = inspect_flac(&mut Cursor::new(&result.audio[index]))?;
            assert_eq!(original[before.audio_range.start as usize + 2] & 0x0f, 9);
            assert_eq!(before.audio_sha256, after.audio_sha256);
            assert_eq!(
                &original[before.audio_range.start as usize..],
                &result.audio[index][after.audio_range.start as usize..]
            );
        }
        assert_eq!(result.report.tracks[2].disc, 2);
        assert_eq!(result.report.tracks[2].disc_track, 1);
        let first_path = result.report.directory.clone();
        drop(result);
        assert!(!first_path.exists());
        Ok(())
    }

    #[test]
    fn corrupt_download_or_tags_cannot_form_a_successful_result()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut bad = tone(220.0);
        let last = bad.last_mut().ok_or("missing audio frame")?;
        *last ^= 1;
        assert!(finish(vec![bad]).is_err());
        assert!(verify_tags(&tone(220.0), 0, b"not a JPEG").is_err());
        Ok(())
    }
}
