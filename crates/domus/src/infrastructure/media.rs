use super::artwork::CoverJpeg;
use super::{flac::inspect_flac, mp3::inspect_mp3_320};
use crate::domain::{AudioFormat, TrackTags};
use crate::{Error, Result};
use id3::frame::{Content, ExtendedText, Picture, PictureType, UniqueFileIdentifier};
use id3::{Encoding, Frame, Tag, TagLike, Version};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::ops::Range;

const MAX_METADATA: usize = 8 * 1024 * 1024;
pub const NORMALIZATION_PROFILE: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedAudio {
    pub profile: u8,
    pub format: AudioFormat,
    pub input_bytes: u64,
    pub input_sha256: String,
    pub tags_sha256: String,
    pub bytes: u64,
    pub sha256: String,
    pub audio_bytes: u64,
    pub audio_sha256: String,
    pub metadata_sha256: String,
    pub cover_sha256: String,
    pub frames: u64,
    pub metadata_frames: u64,
    pub samples_per_channel: u64,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: Option<u8>,
    pub md5_verified: Option<bool>,
}

#[derive(PartialEq, Eq)]
struct AudioProof {
    range: Range<u64>,
    sha256: String,
    frames: u64,
    metadata_frames: u64,
    samples: u64,
    rate: u32,
    channels: u8,
    bits: Option<u8>,
    md5_verified: Option<bool>,
    stream_info: Option<[u8; 34]>,
}

fn inspect(source: &mut (impl Read + Seek), format: AudioFormat) -> Result<AudioProof> {
    match format {
        AudioFormat::Mp3 => {
            let evidence = inspect_mp3_320(source)?;
            Ok(AudioProof {
                range: evidence.mpeg_range,
                sha256: evidence.mpeg_sha256,
                frames: evidence.audio_frames,
                metadata_frames: u64::from(evidence.metadata_frames),
                samples: evidence.samples_per_channel,
                rate: evidence.sample_rate,
                channels: evidence.channels,
                bits: None,
                md5_verified: None,
                stream_info: None,
            })
        }
        AudioFormat::Flac => {
            let evidence = inspect_flac(source)?;
            Ok(AudioProof {
                range: evidence.audio_range,
                sha256: evidence.audio_sha256,
                frames: evidence.frames,
                metadata_frames: 0,
                samples: evidence.samples_per_channel,
                rate: evidence.sample_rate,
                channels: evidence.channels,
                bits: Some(evidence.bits_per_sample),
                md5_verified: Some(evidence.md5_verified),
                stream_info: Some(evidence.stream_info),
            })
        }
    }
}

fn text(tag: &mut Tag, name: &str, value: impl Into<String>) {
    tag.add_frame(
        Frame::with_content(name, Content::Text(value.into())).set_encoding(Some(Encoding::UTF16)),
    );
}

fn extended(tag: &mut Tag, name: &str, value: impl Into<String>) {
    tag.add_frame(
        Frame::with_content(
            "TXXX",
            Content::ExtendedText(ExtendedText {
                description: name.into(),
                value: value.into(),
            }),
        )
        .set_encoding(Some(Encoding::UTF16)),
    );
}

fn id3_metadata(tags: &TrackTags, cover: &CoverJpeg) -> Result<Vec<u8>> {
    let mut tag = Tag::with_version(Version::Id3v23);
    for (name, value) in [
        ("TIT2", &tags.title),
        ("TPE1", &tags.artist),
        ("TPE2", &tags.album_artist),
        ("TALB", &tags.album),
    ] {
        text(&mut tag, name, value);
    }
    text(
        &mut tag,
        "TRCK",
        format!("{}/{}", tags.track, tags.track_total),
    );
    text(
        &mut tag,
        "TPOS",
        format!("{}/{}", tags.disc, tags.disc_total),
    );
    extended(
        &mut tag,
        "ORIGINALTRACKNUMBER",
        tags.original_track.to_string(),
    );
    if let Some(date) = &tags.date {
        text(&mut tag, "TYER", &date[..4]);
        if date.len() == 10 {
            text(&mut tag, "TDAT", format!("{}{}", &date[8..10], &date[5..7]));
        }
        extended(&mut tag, "DATE", date);
    }
    for (name, value) in [
        ("MusicBrainz Album Id", tags.release_id.clone()),
        (
            "MusicBrainz Release Group Id",
            tags.release_group_id.clone(),
        ),
        (
            "MusicBrainz Release Track Id",
            tags.release_track_id.clone(),
        ),
        ("MusicBrainz Artist Id", tags.artist_ids.join("/")),
        (
            "MusicBrainz Album Artist Id",
            tags.album_artist_ids.join("/"),
        ),
    ] {
        extended(&mut tag, name, value);
    }
    tag.add_frame(UniqueFileIdentifier {
        owner_identifier: "http://musicbrainz.org".into(),
        identifier: tags.recording_id.as_bytes().to_vec(),
    });
    tag.add_frame(
        Frame::with_content(
            "APIC",
            Content::Picture(Picture {
                mime_type: "image/jpeg".into(),
                picture_type: PictureType::CoverFront,
                description: String::new(),
                data: cover.bytes().to_vec(),
            }),
        )
        .set_encoding(Some(Encoding::UTF16)),
    );
    let mut bytes = Vec::new();
    id3::Encoder::new()
        .version(Version::Id3v23)
        .padding(0)
        .encode(&tag, &mut bytes)
        .map_err(|_| Error::InvalidInput("fresh ID3v2.3 metadata could not be encoded".into()))?;
    Ok(bytes)
}

fn sized_string(bytes: &mut Vec<u8>, text: &str, little_endian: bool) -> Result<()> {
    let length = u32::try_from(text.len())
        .map_err(|_| Error::InvalidInput("media text exceeds its encoded length".into()))?;
    bytes.extend_from_slice(&if little_endian {
        length.to_le_bytes()
    } else {
        length.to_be_bytes()
    });
    bytes.extend_from_slice(text.as_bytes());
    Ok(())
}

fn block(bytes: &mut Vec<u8>, kind: u8, contents: &[u8]) -> Result<()> {
    if contents.len() > 0xff_ffff {
        return Err(Error::InvalidInput(
            "FLAC metadata block exceeds its encoded length".into(),
        ));
    }
    bytes.push(kind);
    bytes.extend_from_slice(&(contents.len() as u32).to_be_bytes()[1..]);
    bytes.extend_from_slice(contents);
    Ok(())
}

fn flac_metadata(tags: &TrackTags, cover: &CoverJpeg, stream_info: &[u8; 34]) -> Result<Vec<u8>> {
    let mut entries = vec![
        ("TITLE", tags.title.clone()),
        ("ARTIST", tags.artist.clone()),
        ("ALBUMARTIST", tags.album_artist.clone()),
        ("ALBUM", tags.album.clone()),
        ("TRACKNUMBER", tags.track.to_string()),
        ("TRACKTOTAL", tags.track_total.to_string()),
        ("DISCNUMBER", tags.disc.to_string()),
        ("DISCTOTAL", tags.disc_total.to_string()),
        ("ORIGINALTRACKNUMBER", tags.original_track.to_string()),
        ("MUSICBRAINZ_ALBUMID", tags.release_id.clone()),
        ("MUSICBRAINZ_RELEASEGROUPID", tags.release_group_id.clone()),
        ("MUSICBRAINZ_TRACKID", tags.recording_id.clone()),
        ("MUSICBRAINZ_RELEASETRACKID", tags.release_track_id.clone()),
    ];
    if let Some(date) = &tags.date {
        entries.push(("DATE", date.clone()));
    }
    for (key, values) in [
        ("MUSICBRAINZ_ARTISTID", &tags.artist_ids),
        ("MUSICBRAINZ_ALBUMARTISTID", &tags.album_artist_ids),
    ] {
        for value in values {
            entries.push((key, value.clone()));
        }
    }
    let mut comments = Vec::new();
    sized_string(
        &mut comments,
        concat!("Domus Obscura ", env!("CARGO_PKG_VERSION")),
        true,
    )?;
    comments.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (key, value) in entries {
        sized_string(&mut comments, &format!("{key}={value}"), true)?;
    }
    let mut picture = Vec::new();
    picture.extend_from_slice(&3u32.to_be_bytes());
    sized_string(&mut picture, "image/jpeg", false)?;
    sized_string(&mut picture, "", false)?;
    for value in [cover.dimensions().0, cover.dimensions().1, 24, 0] {
        picture.extend_from_slice(&value.to_be_bytes());
    }
    picture.extend_from_slice(&(cover.bytes().len() as u32).to_be_bytes());
    picture.extend_from_slice(cover.bytes());
    let mut bytes = b"fLaC".to_vec();
    block(&mut bytes, 0, stream_info)?;
    block(&mut bytes, 4, &comments)?;
    block(&mut bytes, 0x86, &picture)?;
    Ok(bytes)
}

/// Write only to a new, private, empty staging stream. On any error its
/// contents are unusable and must not be published. This does not mutate the
/// source, create a path, authorize replacement, or make the output durable.
pub fn normalize_audio(
    source: &mut (impl Read + Seek),
    destination: &mut (impl Read + Write + Seek),
    format: AudioFormat,
    tags: &TrackTags,
    cover: &CoverJpeg,
) -> Result<NormalizedAudio> {
    if destination.seek(SeekFrom::End(0))? != 0 {
        return Err(Error::Conflict(
            "audio normalization requires an empty destination".into(),
        ));
    }
    let source_length = source.seek(SeekFrom::End(0))?;
    let original = inspect(source, format)?;
    let metadata = match original.stream_info.as_ref() {
        Some(stream_info) => flac_metadata(tags, cover, stream_info)?,
        None => id3_metadata(tags, cover)?,
    };
    if metadata.len() > MAX_METADATA {
        return Err(Error::InvalidInput(
            "fresh audio metadata exceeds 8 MiB".into(),
        ));
    }
    destination.write_all(&metadata)?;
    let mut complete_hash = Sha256::new();
    complete_hash.update(&metadata);
    let mut audio_hash = Sha256::new();
    source.seek(SeekFrom::Start(0))?;
    let audio_bytes = original.range.end - original.range.start;
    let mut remaining = source_length;
    let mut input_hash = Sha256::new();
    let mut position = 0;
    let mut buffer = [0; 64 * 1024];
    while remaining != 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        source.read_exact(&mut buffer[..count])?;
        input_hash.update(&buffer[..count]);
        let start = original.range.start.max(position);
        let end = original.range.end.min(position + count as u64);
        if start < end {
            let audio = &buffer[(start - position) as usize..(end - position) as usize];
            destination.write_all(audio)?;
            audio_hash.update(audio);
            complete_hash.update(audio);
        }
        position += count as u64;
        remaining -= count as u64;
    }
    if format!("{:x}", audio_hash.finalize()) != original.sha256
        || source.seek(SeekFrom::End(0))? != source_length
    {
        return Err(Error::Conflict(
            "source audio changed during normalization".into(),
        ));
    }
    destination.flush()?;
    let length = metadata.len() as u64 + audio_bytes;
    if destination.seek(SeekFrom::End(0))? != length {
        return Err(Error::Conflict(
            "normalized audio length differs from the written payload".into(),
        ));
    }
    let mut verified = inspect(destination, format)?;
    if verified.range != (metadata.len() as u64..length) {
        return Err(Error::Conflict(
            "normalized audio framing differs from the written payload".into(),
        ));
    }
    verified.range = original.range.clone();
    if verified != original {
        return Err(Error::Conflict(
            "normalized audio differs from the validated source".into(),
        ));
    }
    let expected_hash = format!("{:x}", complete_hash.finalize());
    destination.seek(SeekFrom::Start(0))?;
    let mut verified_hash = Sha256::new();
    let mut remaining = length;
    while remaining != 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        destination.read_exact(&mut buffer[..count])?;
        verified_hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if format!("{:x}", verified_hash.finalize()) != expected_hash
        || destination.seek(SeekFrom::End(0))? != length
    {
        return Err(Error::Conflict(
            "normalized file bytes changed during verification".into(),
        ));
    }
    Ok(NormalizedAudio {
        profile: NORMALIZATION_PROFILE,
        format,
        input_bytes: source_length,
        input_sha256: format!("{:x}", input_hash.finalize()),
        tags_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(tags)?)),
        bytes: length,
        sha256: expected_hash,
        audio_bytes,
        audio_sha256: original.sha256,
        metadata_sha256: format!("{:x}", Sha256::digest(&metadata)),
        cover_sha256: format!("{:x}", Sha256::digest(cover.bytes())),
        frames: original.frames,
        metadata_frames: original.metadata_frames,
        samples_per_channel: original.samples,
        sample_rate: original.rate,
        channels: original.channels,
        bits_per_sample: original.bits,
        md5_verified: original.md5_verified,
    })
}
