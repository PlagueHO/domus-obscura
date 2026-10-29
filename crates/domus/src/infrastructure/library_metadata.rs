//! Bounded header/tag inspection. No decoder or acquisition validator runs here.
use crate::domain::library::{LibraryMetadata, LibraryTags};
use crate::{Error, Result};
use id3::TagLike;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};

const MAX_METADATA: usize = 8 * 1024 * 1024;
const MAX_VALUES: usize = 100;
const MAX_FRAMES: usize = 1024;

fn invalid(message: &str) -> Error {
    Error::InvalidInput(message.into())
}

fn limit() -> Error {
    invalid("metadata resource limit exceeded")
}

fn values(text: &str) -> Result<Vec<String>> {
    let mut values = Vec::new();
    for (index, value) in text.split('\0').enumerate() {
        if index >= MAX_VALUES || value.len() > 4096 || value.chars().any(|c| c.is_control()) {
            return Err(limit());
        }
        if !value.trim().is_empty() {
            values.push(value.to_owned());
        }
    }
    Ok(values)
}

fn one(text: Option<&str>) -> Result<Option<String>> {
    text.map(values)
        .transpose()
        .map(|value| value.and_then(|v| v.into_iter().next()))
}

pub(crate) fn modified_ns(file: &File) -> Result<Option<String>> {
    let modified = match file
        .metadata()?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
    {
        Ok(value) => value.as_nanos().to_string(),
        Err(error) => format!("-{}", error.duration().as_nanos()),
    };
    Ok(Some(modified))
}

/// Every failure becomes a durable diagnostic; callers still record file presence.
pub(crate) fn inspect(file: &mut File, name: &str) -> LibraryMetadata {
    let mut observation = LibraryMetadata::default();
    let result = inspect_inner(file, name, &mut observation);
    if let Err(error) = result {
        let status = match &error {
            Error::Io(_) => "unreadable",
            Error::Conflict(_) => "changed",
            Error::InvalidInput(message) if message == "metadata resource limit exceeded" => {
                "limit"
            }
            _ => "invalid",
        };
        observation.tags = LibraryTags::default();
        observation.audio = Default::default();
        observation.status = status.into();
        observation.error = Some(error.to_string());
    }
    observation
}

fn inspect_inner(file: &mut File, name: &str, observation: &mut LibraryMetadata) -> Result<()> {
    let identity = super::filesystem::native_file_identity(file)?;
    let metadata = file.metadata()?;
    let modified = modified_ns(file)?;
    observation.native_identity = Some(identity.clone());
    observation.size = Some(i64::try_from(metadata.len()).map_err(|_| limit())?);
    observation.modified_ns = modified.clone();
    let extension = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "mp3" => mp3(file, observation)?,
        "flac" => flac(file, observation)?,
        "m4a" | "aac" | "ogg" | "opus" | "wav" | "aiff" | "wma" | "ape" => {
            observation.audio.format = Some(extension);
            observation.status = "unsupported".into();
        }
        _ => observation.status = "asset".into(),
    }
    if identity != super::filesystem::native_file_identity(file)?
        || metadata.len() != file.metadata()?.len()
        || modified != modified_ns(file)?
    {
        return Err(Error::Conflict(
            "file changed during metadata inspection".into(),
        ));
    }
    if observation.status.is_empty() {
        observation.status = if observation.tags.album.is_some()
            || !observation.tags.artists.is_empty()
            || !observation.tags.album_artists.is_empty()
            || observation.tags.title.is_some()
        {
            "tagged"
        } else {
            "missing_tags"
        }
        .into();
    }
    if observation
        .tags
        .identifiers
        .keys()
        .any(|key| key.len() > 128)
        || serde_json::to_vec(&observation.tags)?.len() > 64 * 1024
    {
        return Err(limit());
    }
    Ok(())
}

fn synchsafe(bytes: &[u8]) -> Result<usize> {
    if bytes.iter().any(|byte| byte & 0x80 != 0) {
        return Err(invalid("invalid ID3 synchsafe size"));
    }
    Ok(bytes
        .iter()
        .fold(0usize, |value, byte| (value << 7) | usize::from(*byte)))
}

fn mp3(file: &mut File, observation: &mut LibraryMetadata) -> Result<()> {
    observation.audio.format = Some("mp3".into());
    let mut header = [0; 10];
    file.read_exact(&mut header)?;
    let mut audio_start = 0;
    if &header[..3] == b"ID3" {
        let size = synchsafe(&header[6..10])?;
        if size > MAX_METADATA - 10 {
            return Err(limit());
        }
        if !matches!(header[3], 2..=4) || header[5] != 0 {
            return Err(invalid("unsupported ID3 version or tag flags"));
        }
        let mut bytes = vec![0; size + 10];
        bytes[..10].copy_from_slice(&header);
        file.read_exact(&mut bytes[10..])?;
        // Check frame sizes/count before the dependency allocates frame objects.
        let (prefix, width) = if header[3] == 2 { (6, 3) } else { (10, 4) };
        let mut offset = 10;
        let mut count = 0;
        while offset + prefix <= bytes.len() && bytes[offset] != 0 {
            count += 1;
            if count > MAX_FRAMES {
                return Err(limit());
            }
            let length = if header[3] == 2 {
                bytes[offset + 3..offset + 6]
                    .iter()
                    .fold(0usize, |n, b| (n << 8) | usize::from(*b))
            } else if header[3] == 4 {
                synchsafe(&bytes[offset + 4..offset + 8])?
            } else {
                u32::from_be_bytes(
                    bytes[offset + 4..offset + 8]
                        .try_into()
                        .map_err(|_| invalid("invalid ID3 frame"))?,
                ) as usize
            };
            if width == 4 && bytes[offset + 8..offset + 10] != [0, 0] {
                return Err(invalid("unsupported ID3 frame flags"));
            }
            offset = offset
                .checked_add(prefix + length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| invalid("truncated ID3 frame"))?;
        }
        let tag = id3::Tag::read_from2(Cursor::new(&bytes))
            .map_err(|error| invalid(&format!("invalid ID3 metadata: {error}")))?;
        let tags = &mut observation.tags;
        tags.title = one(tag.title())?;
        tags.album = one(tag.album())?;
        tags.artists = tag.artist().map(values).transpose()?.unwrap_or_default();
        tags.album_artists = tag
            .album_artist()
            .map(values)
            .transpose()?
            .unwrap_or_default();
        tags.date = one(tag
            .get("TDRC")
            .or_else(|| tag.get("TYER"))
            .and_then(|f| f.content().text()))?;
        tags.track = one(tag.get("TRCK").and_then(|f| f.content().text()))?;
        tags.disc = one(tag.get("TPOS").and_then(|f| f.content().text()))?;
        tags.embedded_cover = tag.pictures().next().is_some();
        for field in tag.extended_texts() {
            if field
                .description
                .to_ascii_lowercase()
                .starts_with("musicbrainz")
            {
                if tags.identifiers.len() >= 16 {
                    return Err(limit());
                }
                tags.identifiers
                    .insert(field.description.clone(), values(&field.value)?);
            }
        }
        audio_start = bytes.len() as u64;
    }
    file.seek(SeekFrom::Start(audio_start))?;
    let mut probe = Vec::new();
    file.take(64 * 1024).read_to_end(&mut probe)?;
    // Two matching MPEG Layer III frame headers reduce accidental sync matches.
    for offset in 0..probe.len().saturating_sub(4) {
        if let Some((rate, channels, bitrate, length)) = mpeg_header(&probe[offset..offset + 4]) {
            let next = offset + length;
            if next + 4 <= probe.len()
                && mpeg_header(&probe[next..next + 4])
                    .is_some_and(|(r, c, _, _)| r == rate && c == channels)
            {
                observation.audio.sample_rate = Some(rate);
                observation.audio.channels = Some(channels);
                observation.audio.bitrate_kbps = Some(bitrate);
                return Ok(());
            }
        }
    }
    Err(invalid(
        "no consistent MPEG Layer III headers within bounded probe",
    ))
}

fn mpeg_header(bytes: &[u8]) -> Option<(u32, u8, u32, usize)> {
    let bits = u32::from_be_bytes(bytes.try_into().ok()?);
    if bits >> 21 != 0x7ff || (bits >> 17) & 3 != 1 {
        return None;
    }
    let version = (bits >> 19) & 3;
    let sample = ((bits >> 10) & 3) as usize;
    let bitrate = ((bits >> 12) & 15) as usize;
    if version == 1 || sample == 3 || bitrate == 0 || bitrate == 15 {
        return None;
    }
    let rates = [44100, 48000, 32000];
    let rate = rates[sample]
        / match version {
            3 => 1,
            2 => 2,
            _ => 4,
        };
    let table = if version == 3 {
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ]
    } else {
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
    };
    let kbps = table[bitrate];
    let length =
        ((if version == 3 { 144000 } else { 72000 }) * kbps / rate + ((bits >> 9) & 1)) as usize;
    Some((rate, if (bits >> 6) & 3 == 3 { 1 } else { 2 }, kbps, length))
}

fn sized<'a>(bytes: &mut &'a [u8]) -> Result<&'a [u8]> {
    if bytes.len() < 4 {
        return Err(invalid("truncated FLAC comments"));
    }
    let length = u32::from_le_bytes(
        bytes[..4]
            .try_into()
            .map_err(|_| invalid("invalid FLAC length"))?,
    ) as usize;
    *bytes = &bytes[4..];
    if length > bytes.len() {
        return Err(invalid("truncated FLAC comment"));
    }
    let value = &bytes[..length];
    *bytes = &bytes[length..];
    Ok(value)
}

fn flac(file: &mut File, observation: &mut LibraryMetadata) -> Result<()> {
    observation.audio.format = Some("flac".into());
    let mut magic = [0; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"fLaC" {
        return Err(invalid("invalid FLAC signature"));
    }
    let mut total = 4;
    for index in 0..MAX_FRAMES {
        let mut header = [0; 4];
        file.read_exact(&mut header)?;
        let length = usize::from_be_bytes({
            let mut size = [0; size_of::<usize>()];
            let start = size.len() - 3;
            size[start..].copy_from_slice(&header[1..]);
            size
        });
        total += length + 4;
        if total > MAX_METADATA {
            return Err(limit());
        }
        let kind = header[0] & 127;
        if index == 0 && (kind != 0 || length != 34) {
            return Err(invalid("FLAC requires first STREAMINFO"));
        }
        if matches!(kind, 0 | 4) {
            let mut block = vec![0; length];
            file.read_exact(&mut block)?;
            if kind == 0 {
                if index != 0 {
                    return Err(invalid("duplicate FLAC STREAMINFO"));
                }
                let info = u64::from_be_bytes(
                    block[10..18]
                        .try_into()
                        .map_err(|_| invalid("invalid STREAMINFO"))?,
                );
                let rate = (info >> 44) as u32;
                let samples = info & 0xf_ffff_ffff;
                if rate == 0 {
                    return Err(invalid("invalid FLAC sample rate"));
                }
                observation.audio.sample_rate = Some(rate);
                observation.audio.channels = Some(((info >> 41) & 7) as u8 + 1);
                observation.audio.bits_per_sample = Some(((info >> 36) & 31) as u8 + 1);
                observation.audio.duration_ms = if samples > 0 {
                    Some(samples * 1000 / u64::from(rate))
                } else {
                    None
                };
            } else {
                let mut bytes = block.as_slice();
                sized(&mut bytes)?;
                if bytes.len() < 4 {
                    return Err(invalid("truncated FLAC comment count"));
                }
                let count = u32::from_le_bytes(
                    bytes[..4]
                        .try_into()
                        .map_err(|_| invalid("invalid comment count"))?,
                ) as usize;
                bytes = &bytes[4..];
                if count > MAX_FRAMES {
                    return Err(limit());
                }
                for _ in 0..count {
                    let text = std::str::from_utf8(sized(&mut bytes)?)
                        .map_err(|_| invalid("invalid FLAC UTF-8"))?;
                    let (key, value) = text
                        .split_once('=')
                        .ok_or_else(|| invalid("invalid FLAC comment"))?;
                    let key = key.to_ascii_uppercase();
                    let tags = &mut observation.tags;
                    match key.as_str() {
                        "TITLE" => tags.title = one(Some(value))?,
                        "ARTIST" => tags.artists.extend(values(value)?),
                        "ALBUMARTIST" | "ALBUM ARTIST" => tags.album_artists.extend(values(value)?),
                        "ALBUM" => tags.album = one(Some(value))?,
                        "DATE" => tags.date = one(Some(value))?,
                        "TRACKNUMBER" | "TRACKTOTAL" | "TOTALTRACKS" | "DISCNUMBER"
                        | "DISCTOTAL" | "TOTALDISCS" => {
                            tags.identifiers
                                .entry(key.clone())
                                .or_default()
                                .extend(values(value)?);
                            if key == "TRACKNUMBER" {
                                tags.track = one(Some(value))?;
                            }
                            if key == "DISCNUMBER" {
                                tags.disc = one(Some(value))?;
                            }
                        }
                        _ if key.starts_with("MUSICBRAINZ_") => {
                            tags.identifiers
                                .entry(key)
                                .or_default()
                                .extend(values(value)?);
                        }
                        _ => {}
                    }
                    if tags.artists.len() > MAX_VALUES
                        || tags.album_artists.len() > MAX_VALUES
                        || tags.identifiers.len() > 16
                        || tags.identifiers.values().any(|v| v.len() > MAX_VALUES)
                    {
                        return Err(limit());
                    }
                }
            }
        } else {
            if kind == 6 {
                observation.tags.embedded_cover = true;
            }
            let end = file
                .stream_position()?
                .checked_add(length as u64)
                .ok_or_else(limit)?;
            if end > file.metadata()?.len() {
                return Err(invalid("truncated FLAC block"));
            }
            file.seek(SeekFrom::Start(end))?;
        }
        if header[0] & 128 != 0 {
            for (number, total, alternate) in [
                (&mut observation.tags.track, "TRACKTOTAL", "TOTALTRACKS"),
                (&mut observation.tags.disc, "DISCTOTAL", "TOTALDISCS"),
            ] {
                if let Some(value) = number.as_mut().filter(|value| !value.contains('/'))
                    && let Some(total) = observation
                        .tags
                        .identifiers
                        .get(total)
                        .or_else(|| observation.tags.identifiers.get(alternate))
                        .and_then(|values| values.first())
                {
                    value.push('/');
                    value.push_str(total);
                }
            }
            return Ok(());
        }
    }
    Err(limit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_text_values_reject_limits_before_copying_and_preserve_valid_text() -> Result<()> {
        assert_eq!(
            values(" Artist A \0\0Artist B")?,
            [" Artist A ", "Artist B"]
        );
        assert_eq!(
            values(&vec!["Artist"; MAX_VALUES].join("\0"))?.len(),
            MAX_VALUES
        );
        for text in [
            vec!["Artist"; MAX_VALUES + 1].join("\0"),
            "\0".repeat(MAX_VALUES),
            "a".repeat(4097),
        ] {
            assert!(matches!(values(&text), Err(Error::InvalidInput(message))
                if message == "metadata resource limit exceeded"));
        }
        Ok(())
    }
}
