use domus::domain::{AudioFormat, CatalogRelease};
use domus::infrastructure::artwork::{CoverJpeg, normalize_cover};
use domus::infrastructure::catalog::CatalogQuery;
use domus::infrastructure::flac::inspect_flac;
use domus::infrastructure::media::normalize_audio;
use domus::infrastructure::mp3::inspect_mp3_320;
use id3::{Tag, TagLike, Version};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

#[path = "support/media.rs"]
mod fixtures;
use fixtures::{cover_source, flac, mp3};

type TestResult = Result<(), Box<dyn std::error::Error>>;
#[path = "media/journal.rs"]
mod journal;
#[path = "media/profiles.rs"]
mod profiles;
const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
const GROUP: &str = "30000000-0000-4000-8000-000000000001";
const ARTIST: &str = "10000000-0000-4000-8000-000000000001";
const GUEST: &str = "10000000-0000-4000-8000-000000000002";
const TITLE: &str = "Title / <Three>: \u{1f3b5}";
const ALBUM: &str = "Album: \u{e9} / preserved";
const ARTIST_TEXT: &str = "Credited feat. Guest";

fn release(date: Option<&str>) -> Result<CatalogRelease, Box<dyn std::error::Error>> {
    Ok(CatalogQuery::release(RELEASE)?.parse_release(&release_body(date)?)?)
}

fn release_body(date: Option<&str>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let credit = json!([
        {"artist":{"id":ARTIST,"name":"Canonical"},"name":"Credited","joinphrase":" feat. "},
        {"artist":{"id":GUEST,"name":"Guest"},"name":"Guest","joinphrase":""}
    ]);
    let track = |index: u32, position: u16| {
        json!({
            "id":format!("40000000-0000-4000-8000-{index:012x}"),
            "position":position,"number":format!("source-{position}"),
            "title":if index == 3 { TITLE } else { "Another generated track" },
            "artist-credit":credit.clone(),
            "recording":{"id":format!("50000000-0000-4000-8000-{index:012x}"),"video":false}
        })
    };
    Ok(serde_json::to_vec(&json!({
        "id":RELEASE,"title":ALBUM,"date":date,"artist-credit":credit,
        "release-group":{"id":GROUP},
        "media":[
            {"position":1,"track-count":2,"tracks":[track(1,1),track(2,2)]},
            {"position":2,"track-count":1,"tracks":[track(3,1)]}
        ]
    }))?)
}

fn cover() -> Result<CoverJpeg, Box<dyn std::error::Error>> {
    Ok(normalize_cover(&cover_source()?, None)?)
}

#[test]
fn media_fresh_id3v23_preserves_text_numbers_ids_dates_cover_and_encoded_audio() -> TestResult {
    let cover = cover()?;
    for date in [None, Some("2024"), Some("2024-02"), Some("2024-02-29")] {
        let release = release(date)?;
        let tags = release.track_tags(3)?;
        let original = mp3()?;
        let before = inspect_mp3_320(&mut Cursor::new(&original))?;
        let mut source = Cursor::new(original.clone());
        let mut output = Cursor::new(Vec::new());
        let evidence = normalize_audio(&mut source, &mut output, AudioFormat::Mp3, &tags, &cover)?;
        assert_eq!(
            evidence.input_sha256,
            format!("{:x}", Sha256::digest(&original))
        );
        assert_eq!(source.into_inner(), original);
        assert_eq!(&output.get_ref()[..6], b"ID3\x03\x00\x00");
        let after = inspect_mp3_320(&mut output)?;
        assert_eq!(after.mpeg_sha256, before.mpeg_sha256);
        assert_eq!(after.audio_frames, before.audio_frames);
        assert_eq!(
            &output.get_ref()[after.mpeg_range.start as usize..],
            &original[before.mpeg_range.start as usize..before.mpeg_range.end as usize]
        );
        assert_eq!(
            evidence.sha256,
            format!("{:x}", Sha256::digest(output.get_ref()))
        );
        assert_eq!(evidence.audio_sha256, before.mpeg_sha256);
        assert_eq!(evidence.bytes, output.get_ref().len() as u64);
        assert_eq!(
            evidence.metadata_sha256,
            format!(
                "{:x}",
                Sha256::digest(&output.get_ref()[..after.mpeg_range.start as usize])
            )
        );
        let tag = Tag::read_from2(&mut Cursor::new(output.get_ref()))?;
        assert_eq!(tag.version(), Version::Id3v23);
        assert_eq!(tag.title(), Some(TITLE));
        assert_eq!(tag.album(), Some(ALBUM));
        assert_eq!(tag.artist(), Some(ARTIST_TEXT));
        assert_eq!(tag.album_artist(), Some(ARTIST_TEXT));
        assert_eq!((tag.track(), tag.total_tracks()), (Some(3), Some(3)));
        assert_eq!((tag.disc(), tag.total_discs()), (Some(2), Some(2)));
        assert_eq!(tag.genre(), None);
        let texts = tag
            .extended_texts()
            .map(|item| (item.description.as_str(), item.value.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(texts.get("ORIGINALTRACKNUMBER"), Some(&"1"));
        assert_eq!(texts.get("MusicBrainz Album Id"), Some(&RELEASE));
        assert_eq!(texts.get("MusicBrainz Release Group Id"), Some(&GROUP));
        assert_eq!(
            texts.get("MusicBrainz Release Track Id"),
            Some(&"40000000-0000-4000-8000-000000000003")
        );
        assert_eq!(
            texts.get("MusicBrainz Artist Id").copied(),
            Some(format!("{ARTIST}/{GUEST}").as_str())
        );
        assert_eq!(
            texts.get("MusicBrainz Album Artist Id"),
            texts.get("MusicBrainz Artist Id")
        );
        assert_eq!(texts.get("DATE").copied(), date);
        assert!(!texts.contains_key("REPLAYGAIN_TRACK_GAIN"));
        let identifiers = tag.unique_file_identifiers().collect::<Vec<_>>();
        assert_eq!(identifiers.len(), 1);
        assert_eq!(identifiers[0].owner_identifier, "http://musicbrainz.org");
        assert_eq!(
            identifiers[0].identifier,
            b"50000000-0000-4000-8000-000000000003"
        );
        let pictures = tag.pictures().collect::<Vec<_>>();
        assert_eq!(pictures.len(), 1);
        assert_eq!(
            pictures[0].picture_type,
            id3::frame::PictureType::CoverFront
        );
        assert_eq!(pictures[0].mime_type, "image/jpeg");
        assert_eq!(pictures[0].data, cover.bytes());
        let mut position = 10;
        let mut year = None;
        let mut day_month = None;
        while position < after.mpeg_range.start as usize {
            let bytes = output.get_ref();
            let id = &bytes[position..position + 4];
            let size = u32::from_be_bytes(bytes[position + 4..position + 8].try_into()?) as usize;
            let data = &bytes[position + 10..position + 10 + size];
            if id.starts_with(b"T") || id == b"APIC" {
                assert_eq!(data[0], 1);
            }
            if id == b"TYER" {
                year = Some(tag.year());
            }
            if id == b"TDAT" {
                day_month = Some(
                    tag.get("TDAT")
                        .ok_or("missing date")?
                        .content()
                        .text()
                        .ok_or("invalid date")?
                        .to_owned(),
                );
            }
            assert_ne!(id, b"TDRC");
            position += 10 + size;
        }
        assert_eq!(year, date.map(|_| Some(2024)));
        assert_eq!(
            day_month.as_deref(),
            date.filter(|value| value.len() == 10).map(|_| "2902")
        );
    }
    Ok(())
}

#[test]
fn media_fresh_flac_keeps_streaminfo_and_audio_and_writes_only_comments_and_one_front_picture()
-> TestResult {
    let cover = cover()?;
    for date in [None, Some("2024"), Some("2024-02"), Some("2024-02-29")] {
        let tags = release(date)?.track_tags(3)?;
        let original = flac();
        let before = inspect_flac(&mut Cursor::new(&original))?;
        let mut source = Cursor::new(original.clone());
        let mut output = Cursor::new(Vec::new());
        let evidence = normalize_audio(&mut source, &mut output, AudioFormat::Flac, &tags, &cover)?;
        assert_eq!(
            evidence.input_sha256,
            format!("{:x}", Sha256::digest(&original))
        );
        assert_eq!(source.into_inner(), original);
        let after = inspect_flac(&mut output)?;
        assert_eq!(after.stream_info, before.stream_info);
        assert_eq!(after.audio_sha256, before.audio_sha256);
        assert!(after.md5_verified);
        assert_eq!(
            &output.get_ref()[after.audio_range.start as usize..],
            &original[before.audio_range.start as usize..]
        );
        assert_eq!(
            evidence.sha256,
            format!("{:x}", Sha256::digest(output.get_ref()))
        );
        assert_eq!(
            evidence.cover_sha256,
            format!("{:x}", Sha256::digest(cover.bytes()))
        );
        let tag = metaflac::Tag::read_from(&mut Cursor::new(output.get_ref()))?;
        assert_eq!(tag.blocks().count(), 3);
        assert_eq!(
            tag.get_streaminfo().ok_or("missing STREAMINFO")?.to_bytes(),
            before.stream_info
        );
        for (key, expected) in [
            ("TITLE", TITLE),
            ("ALBUM", ALBUM),
            ("ARTIST", ARTIST_TEXT),
            ("ALBUMARTIST", ARTIST_TEXT),
            ("TRACKNUMBER", "3"),
            ("TRACKTOTAL", "3"),
            ("DISCNUMBER", "2"),
            ("DISCTOTAL", "2"),
            ("ORIGINALTRACKNUMBER", "1"),
            ("MUSICBRAINZ_ALBUMID", RELEASE),
            ("MUSICBRAINZ_RELEASEGROUPID", GROUP),
            (
                "MUSICBRAINZ_TRACKID",
                "50000000-0000-4000-8000-000000000003",
            ),
            (
                "MUSICBRAINZ_RELEASETRACKID",
                "40000000-0000-4000-8000-000000000003",
            ),
        ] {
            assert_eq!(
                tag.get_vorbis(key)
                    .ok_or("missing comment")?
                    .collect::<Vec<_>>(),
                vec![expected]
            );
        }
        assert_eq!(
            tag.get_vorbis("DATE")
                .map(|items| items.collect::<Vec<_>>()),
            date.map(|value| vec![value])
        );
        for key in ["MUSICBRAINZ_ARTISTID", "MUSICBRAINZ_ALBUMARTISTID"] {
            assert_eq!(
                tag.get_vorbis(key)
                    .ok_or("missing artist IDs")?
                    .collect::<Vec<_>>(),
                vec![ARTIST, GUEST]
            );
        }
        assert!(tag.get_vorbis("COMMENT").is_none());
        let pictures = tag.pictures().collect::<Vec<_>>();
        assert_eq!(pictures.len(), 1);
        assert_eq!(
            pictures[0].picture_type,
            metaflac::block::PictureType::CoverFront
        );
        assert_eq!(pictures[0].mime_type, "image/jpeg");
        assert_eq!(
            (
                pictures[0].width,
                pictures[0].height,
                pictures[0].depth,
                pictures[0].num_colors
            ),
            (40, 20, 24, 0)
        );
        assert_eq!(pictures[0].data, cover.bytes());
    }
    Ok(())
}

#[test]
fn media_invalid_sources_and_nonempty_destinations_do_not_write_output() -> TestResult {
    let tags = release(None)?.track_tags(1)?;
    let cover = cover()?;
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let mut source = Cursor::new(b"not audio".to_vec());
        let mut output = Cursor::new(Vec::new());
        assert!(normalize_audio(&mut source, &mut output, format, &tags, &cover).is_err());
        assert!(output.into_inner().is_empty());
        let mut output = Cursor::new(b"existing output".to_vec());
        assert!(normalize_audio(&mut source, &mut output, format, &tags, &cover).is_err());
        assert_eq!(output.into_inner(), b"existing output");
    }
    Ok(())
}

#[test]
fn media_catalog_metadata_rejects_missing_ids_controls_bad_dates_and_mappings() -> TestResult {
    let original = release(None)?;
    assert!(original.track_tags(0).is_err());
    assert!(original.track_tags(4).is_err());
    for index in 0..8 {
        let mut bad = original.clone();
        match index {
            0 => bad.summary.date = Some("2023-02-29".into()),
            1 => bad.summary.title = "bad\0title".into(),
            2 => bad.credits.clear(),
            3 => bad.media[0].position = 2,
            4 => bad.media[1].tracks[0].recording_id = "missing".into(),
            5 => bad.media[1].tracks[0].credits[0].artist_id = "missing".into(),
            6 => bad.media[1].tracks[0].position = 2,
            7 => bad.media[1].tracks[0].video = None,
            _ => return Err("invalid fixture".into()),
        }
        assert!(bad.track_tags(3).is_err(), "case {index}");
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Fault {
    Write,
    Read,
    Corrupt,
}

struct FaultOutput {
    bytes: Cursor<Vec<u8>>,
    fault: Fault,
    written: bool,
}

impl Read for FaultOutput {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if matches!(self.fault, Fault::Read) {
            return Err(std::io::Error::other("injected read failure"));
        }
        self.bytes.read(output)
    }
}

impl Seek for FaultOutput {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.bytes.seek(position)
    }
}

impl Write for FaultOutput {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if matches!(self.fault, Fault::Write) {
            return Err(std::io::Error::other("injected write failure"));
        }
        let result = self.bytes.write(input)?;
        if !self.written && matches!(self.fault, Fault::Corrupt) {
            self.bytes.get_mut()[24] ^= 1;
        }
        self.written = true;
        Ok(result)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn media_io_failures_and_corrupted_written_metadata_never_return_verified_output() -> TestResult {
    let tags = release(None)?.track_tags(1)?;
    let cover = cover()?;
    for fault in [Fault::Read, Fault::Write, Fault::Corrupt] {
        let mut source = Cursor::new(mp3()?);
        let mut output = FaultOutput {
            bytes: Cursor::new(Vec::new()),
            fault,
            written: false,
        };
        let error = normalize_audio(&mut source, &mut output, AudioFormat::Mp3, &tags, &cover)
            .err()
            .ok_or("expected output failure")?;
        match fault {
            Fault::Read | Fault::Write => assert!(error.to_string().contains("injected")),
            Fault::Corrupt => assert!(error.to_string().contains("bytes changed")),
        }
    }
    Ok(())
}

#[test]
fn media_same_length_source_changes_between_inspection_and_copy_are_rejected() -> TestResult {
    struct ChangingSource {
        bytes: Cursor<Vec<u8>>,
        end_seeks: u8,
    }
    impl Read for ChangingSource {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            self.bytes.read(output)
        }
    }
    impl Seek for ChangingSource {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            let result = self.bytes.seek(position)?;
            if position == SeekFrom::End(0) {
                self.end_seeks += 1;
                if self.end_seeks == 3 {
                    let index = self.bytes.get_ref().len() - 200;
                    self.bytes.get_mut()[index] ^= 1;
                }
            }
            Ok(result)
        }
    }
    let tags = release(None)?.track_tags(1)?;
    let cover = cover()?;
    let mut source = ChangingSource {
        bytes: Cursor::new(mp3()?),
        end_seeks: 0,
    };
    let mut output = Cursor::new(Vec::new());
    let error = normalize_audio(&mut source, &mut output, AudioFormat::Mp3, &tags, &cover)
        .err()
        .ok_or("expected changed source failure")?;
    assert!(error.to_string().contains("source audio changed"));
    Ok(())
}
