use crate::domain::{AudioFormat, OptionalTags, TagPolicy, TagRule};
use crate::{Error, Result};
use std::io::{Read, Seek, SeekFrom};

const KEYS: [&str; 4] = ["GENRE", "COMPOSER", "COMMENT", "COPYRIGHT"];

fn invalid() -> Error {
    Error::InvalidInput(
        "source tags are invalid, ambiguous, unsupported, or exceed metadata limits".into(),
    )
}

fn synchsafe(bytes: &[u8]) -> Result<usize> {
    if bytes.iter().any(|byte| byte & 128 != 0) {
        return Err(invalid());
    }
    Ok(bytes
        .iter()
        .fold(0, |size, byte| (size << 7) | usize::from(*byte)))
}

/// Read only bounded source facts from an already retained staging stream.
/// Duplicate detection precedes dependency decoding, which can collapse frames.
pub fn source_tags(
    source: &mut (impl Read + Seek),
    format: AudioFormat,
    policy: &TagPolicy,
) -> Result<OptionalTags> {
    let rules = [
        &policy.genre,
        &policy.composer,
        &policy.comment,
        &policy.copyright,
    ];
    let selected = rules.map(TagRule::needs_review);
    if !selected.into_iter().any(|value| value) {
        return Ok(OptionalTags::default());
    }
    source.seek(SeekFrom::Start(0))?;
    let mut values: [Option<String>; 4] = Default::default();
    let mut language = None;
    match format {
        AudioFormat::Mp3 => {
            let mut header = [0; 10];
            source.read_exact(&mut header)?;
            if &header[..3] == b"ID3" {
                if !matches!(header[3], 3 | 4) || header[4] != 0 || header[5] != 0 {
                    return Err(invalid());
                }
                let size = synchsafe(&header[6..])?;
                if size > super::MAX_METADATA - 10 {
                    return Err(invalid());
                }
                let mut bytes = vec![0; size + 10];
                bytes[..10].copy_from_slice(&header);
                source.read_exact(&mut bytes[10..])?;
                let mut offset = 10;
                let mut count = 0;
                let mut seen = [false; 4];
                while offset < bytes.len() && bytes[offset] != 0 {
                    count += 1;
                    if count > 1024 || bytes.len() - offset < 10 {
                        return Err(invalid());
                    }
                    let frame = &bytes[offset..offset + 10];
                    if !frame[..4]
                        .iter()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
                        || frame[8..] != [0, 0]
                    {
                        return Err(invalid());
                    }
                    let length = if header[3] == 4 {
                        synchsafe(&frame[4..8])?
                    } else {
                        u32::from_be_bytes(frame[4..8].try_into().map_err(|_| invalid())?) as usize
                    };
                    let end = offset
                        .checked_add(10 + length)
                        .filter(|end| *end <= bytes.len())
                        .ok_or_else(invalid)?;
                    if let Some(index) = [b"TCON", b"TCOM", b"COMM", b"TCOP"]
                        .iter()
                        .position(|id| frame[..4] == id[..])
                        && selected[index]
                    {
                        if seen[index] {
                            return Err(invalid());
                        }
                        seen[index] = true;
                        if length > 16384 {
                            return Err(invalid());
                        }
                        let payload = &bytes[offset + 10..end];
                        let encoding = *payload.first().ok_or_else(invalid)?;
                        if header[3] == 3 && encoding > 1 {
                            return Err(invalid());
                        }
                        if index == 2 {
                            if payload.len() < 5 {
                                return Err(invalid());
                            }
                            let code =
                                std::str::from_utf8(&payload[1..4]).map_err(|_| invalid())?;
                            if !code.bytes().all(|byte| byte.is_ascii_lowercase()) {
                                return Err(invalid());
                            }
                            let description = &payload[4..];
                            let prefix = match encoding {
                                0 | 3 if description.starts_with(&[0]) => 1,
                                1 if description.starts_with(&[255, 254, 0, 0])
                                    || description.starts_with(&[254, 255, 0, 0]) =>
                                {
                                    4
                                }
                                1 | 2 if description.starts_with(&[0, 0]) => 2,
                                _ => return Err(invalid()),
                            };
                            values[index] = Some(decode_text(encoding, &description[prefix..])?);
                            language = Some(code.into());
                        } else {
                            values[index] = Some(decode_text(encoding, &payload[1..])?);
                        }
                    }
                    offset = end;
                }
                if bytes[offset..].iter().any(|byte| *byte != 0) {
                    return Err(invalid());
                }
            }
        }
        AudioFormat::Flac => {
            let mut signature = [0; 4];
            source.read_exact(&mut signature)?;
            if &signature != b"fLaC" {
                return Err(invalid());
            }
            let mut total = 4;
            let mut blocks = 0;
            let mut comments = false;
            loop {
                blocks += 1;
                let mut header = [0; 4];
                source.read_exact(&mut header)?;
                let length = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
                total += 4 + length;
                if total > super::MAX_METADATA || blocks > 1024 {
                    return Err(invalid());
                }
                if header[0] & 127 == 4 {
                    if comments {
                        return Err(invalid());
                    }
                    comments = true;
                    let mut bytes = vec![0; length];
                    source.read_exact(&mut bytes)?;
                    let mut remaining = bytes.as_slice();
                    let _vendor = field(&mut remaining)?;
                    let count = number(&mut remaining)?;
                    if count > 1024 {
                        return Err(invalid());
                    }
                    for _ in 0..count {
                        let entry = field(&mut remaining)?;
                        let split = entry
                            .iter()
                            .position(|byte| *byte == b'=')
                            .ok_or_else(invalid)?;
                        if let Some(index) = KEYS
                            .iter()
                            .position(|key| entry[..split].eq_ignore_ascii_case(key.as_bytes()))
                            && selected[index]
                        {
                            if values[index].is_some() {
                                return Err(invalid());
                            }
                            values[index] = Some(
                                std::str::from_utf8(&entry[split + 1..])
                                    .map_err(|_| invalid())?
                                    .to_owned(),
                            );
                        }
                    }
                    if !remaining.is_empty() {
                        return Err(invalid());
                    }
                } else {
                    source.seek(SeekFrom::Current(length as i64))?;
                }
                if header[0] & 128 != 0 {
                    break;
                }
            }
        }
    }
    for value in values.iter().flatten() {
        crate::domain::tag_policy::tag_text(value)?;
    }
    let [genre, composer, comment, copyright] = values;
    let result = OptionalTags {
        genre,
        composer,
        comment,
        copyright,
        comment_language: language,
    };
    policy.resolve(&result)?;
    Ok(result)
}

fn decode_text(encoding: u8, bytes: &[u8]) -> Result<String> {
    let mut text = match encoding {
        0 => bytes.iter().map(|byte| char::from(*byte)).collect(),
        3 => std::str::from_utf8(bytes)
            .map_err(|_| invalid())?
            .to_owned(),
        1 | 2 => {
            let (bytes, little) = if encoding == 2 {
                (bytes, false)
            } else if let Some(bytes) = bytes.strip_prefix(&[255, 254]) {
                (bytes, true)
            } else if let Some(bytes) = bytes.strip_prefix(&[254, 255]) {
                (bytes, false)
            } else {
                return Err(invalid());
            };
            if !bytes.len().is_multiple_of(2) {
                return Err(invalid());
            }
            let units = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    if little {
                        u16::from_le_bytes([pair[0], pair[1]])
                    } else {
                        u16::from_be_bytes([pair[0], pair[1]])
                    }
                })
                .collect::<Vec<_>>();
            String::from_utf16(&units).map_err(|_| invalid())?
        }
        _ => return Err(invalid()),
    };
    if text.ends_with('\0') {
        text.pop();
    }
    Ok(text)
}

fn number(bytes: &mut &[u8]) -> Result<usize> {
    if bytes.len() < 4 {
        return Err(invalid());
    }
    let value = u32::from_le_bytes(bytes[..4].try_into().map_err(|_| invalid())?) as usize;
    *bytes = &bytes[4..];
    Ok(value)
}

fn field<'a>(bytes: &mut &'a [u8]) -> Result<&'a [u8]> {
    let length = number(bytes)?;
    if length > bytes.len() {
        return Err(invalid());
    }
    let (value, rest) = bytes.split_at(length);
    *bytes = rest;
    Ok(value)
}
