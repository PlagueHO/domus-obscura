use crate::domain::{AudioFormat, AudioProfile};
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::ops::Range;
use symphonia::core::checksum::Crc16Ansi;
use symphonia::core::codecs::audio::well_known::CODEC_ID_MP3;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
use symphonia::core::io::Monitor;
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration, Timestamp};

const MAX_TAG_BYTES: u64 = 8 * 1024 * 1024;
const SAMPLES_PER_FRAME: u64 = 1152;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mp3Evidence {
    pub mpeg_range: Range<u64>,
    pub mpeg_sha256: String,
    pub audio_frames: u64,
    pub metadata_frames: u8,
    pub sample_rate: u32,
    pub channels: u8,
    /// Physical decoded samples, before any encoder-delay or padding trim.
    pub samples_per_channel: u64,
}

fn invalid(message: &str) -> Error {
    Error::InvalidInput(message.into())
}

fn read_at<const N: usize>(source: &mut (impl Read + Seek), offset: u64) -> Result<[u8; N]> {
    source.seek(SeekFrom::Start(offset))?;
    let mut bytes = [0; N];
    source.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn mpeg_range(source: &mut (impl Read + Seek)) -> Result<Range<u64>> {
    let length = source.seek(SeekFrom::End(0))?;
    if length < 4 || length > i64::MAX as u64 {
        return Err(invalid("MP3 file length is outside the supported range"));
    }
    let mut start = 0;
    if read_at::<3>(source, 0)? == *b"ID3" {
        let header = read_at::<10>(source, 0)?;
        let allowed_flags = match header[3] {
            2 => 0xc0,
            3 => 0xe0,
            4 => 0xf0,
            _ => return Err(invalid("unsupported ID3 version")),
        };
        if header[4] == 0xff
            || header[5] & !allowed_flags != 0
            || header[6..10].iter().any(|byte| byte & 0x80 != 0)
        {
            return Err(invalid("invalid ID3 header"));
        }
        let size = header[6..10]
            .iter()
            .fold(0u64, |size, byte| (size << 7) | u64::from(*byte));
        start = 10 + size;
        if header[3] == 4 && header[5] & 0x10 != 0 {
            if start + 10 > length || start + 10 > MAX_TAG_BYTES {
                return Err(invalid("ID3 footer exceeds the bounded metadata range"));
            }
            let footer = read_at::<10>(source, start)?;
            if footer[..3] != *b"3DI" || footer[3..] != header[3..] {
                return Err(invalid("ID3 footer does not match its header"));
            }
            start += 10;
        }
    }
    if start >= length || start > MAX_TAG_BYTES {
        return Err(invalid("MP3 metadata is oversized or leaves no audio"));
    }
    Ok(start..length)
}

fn trailing_tags_start(source: &mut (impl Read + Seek), length: u64) -> Result<u64> {
    let mut end = length;
    if end >= 128 && read_at::<3>(source, end - 128)? == *b"TAG" {
        end -= 128;
    }
    if end >= 32 {
        let footer = read_at::<32>(source, end - 32)?;
        if footer[..8] == *b"APETAGEX" {
            let version = le_u32(&footer[8..12]);
            let size = u64::from(le_u32(&footer[12..16]));
            let flags = le_u32(&footer[20..24]);
            if !matches!(version, 1000 | 2000)
                || !(32..=MAX_TAG_BYTES).contains(&size)
                || size > end
                || flags & !0x8000_0000 != 0
                || footer[24..].iter().any(|byte| *byte != 0)
            {
                return Err(invalid("invalid or oversized APE footer"));
            }
            end -= size;
            if flags & 0x8000_0000 != 0 {
                if version != 2000 || end < 32 {
                    return Err(invalid("invalid APE header range"));
                }
                end -= 32;
                let header = read_at::<32>(source, end)?;
                if header[..20] != footer[..20]
                    || le_u32(&header[20..24]) != flags | 0x2000_0000
                    || header[24..] != footer[24..]
                {
                    return Err(invalid("APE header does not match its footer"));
                }
            }
        }
    }
    Ok(end)
}

#[derive(Clone, Copy)]
struct Header {
    bitrate: u32,
    sample_rate: u32,
    channels: u8,
    length: usize,
    side_start: usize,
    main_start: usize,
}

fn header(bytes: [u8; 4]) -> Result<Header> {
    if bytes[0] != 0xff || bytes[1] & 0xfe != 0xfa || bytes[3] & 3 == 2 {
        return Err(invalid("expected a valid MPEG-1 Layer III frame"));
    }
    let index = usize::from(bytes[2] >> 4);
    if index == 0 || index == 15 || bytes[2] & 0x0c == 0x0c {
        return Err(invalid("reserved or free-format MP3 header is unsupported"));
    }
    let bitrate = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ][index];
    let sample_rate = [44_100, 48_000, 32_000][usize::from((bytes[2] >> 2) & 3)];
    let channels = if bytes[3] >> 6 == 3 { 1 } else { 2 };
    let length = (144_000 * bitrate / sample_rate + u32::from((bytes[2] >> 1) & 1)) as usize;
    let side_start = if bytes[1] & 1 == 0 { 6 } else { 4 };
    let main_start = side_start + if channels == 1 { 17 } else { 32 };
    Ok(Header {
        bitrate,
        sample_rate,
        channels,
        length,
        side_start,
        main_start,
    })
}

#[derive(Default)]
struct InfoFrame {
    audio_frames: Option<u32>,
    mpeg_bytes: Option<u32>,
}

fn info_frame(frame: &[u8], header: Header) -> Result<Option<InfoFrame>> {
    if frame[header.side_start..header.main_start]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Ok(None);
    }
    let marker = frame.get(header.main_start..header.main_start + 4);
    if marker != Some(b"Xing") && marker != Some(b"Info") {
        if frame.get(36..40) == Some(b"VBRI") {
            return Err(invalid("VBRI metadata needs separate validated support"));
        }
        return Ok(None);
    }
    let mut cursor = header.main_start + 4;
    let mut take = |count: usize| -> Result<&[u8]> {
        let bytes = frame
            .get(cursor..cursor + count)
            .ok_or_else(|| invalid("truncated Xing/Info metadata"))?;
        cursor += count;
        Ok(bytes)
    };
    let flags = take(4)?;
    let flags = u32::from_be_bytes([flags[0], flags[1], flags[2], flags[3]]);
    if flags & !15 != 0 {
        return Err(invalid("reserved Xing/Info metadata flags"));
    }
    let mut evidence = InfoFrame::default();
    if flags & 1 != 0 {
        let bytes = take(4)?;
        evidence.audio_frames = Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
    }
    if flags & 2 != 0 {
        let bytes = take(4)?;
        evidence.mpeg_bytes = Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
    }
    if flags & 4 != 0 {
        let table = take(100)?;
        if table.windows(2).any(|pair| pair[0] > pair[1]) {
            return Err(invalid("Xing/Info seek table is not monotonic"));
        }
    }
    if flags & 8 != 0 {
        take(4)?;
    }
    Ok(Some(evidence))
}

/// Read-only, bounded validation of every MPEG frame, not an average-bitrate
/// check. The hash covers the complete unchanged MPEG range, including its
/// optional first Xing/Info frame. Existing tag contents are not trusted.
pub fn inspect_mp3_320(source: &mut (impl Read + Seek)) -> Result<Mp3Evidence> {
    inspect_mp3(source, AudioProfile::Mp3Cbr320)
}

/// Validate the selected MPEG-1 profile from every decoded audio frame.
/// A metadata frame's bitrate does not contribute to CBR or VBR evidence.
pub fn inspect_mp3(source: &mut (impl Read + Seek), profile: AudioProfile) -> Result<Mp3Evidence> {
    if profile.format() != AudioFormat::Mp3 {
        return Err(invalid("MP3 inspection requires an MP3 profile"));
    }
    let mut source = BufReader::with_capacity(64 * 1024, source);
    let mut range = mpeg_range(&mut source)?;
    let file_length = range.end;
    source.seek(SeekFrom::Start(range.start))?;
    let mut position = range.start;
    let mut digest = Sha256::new();
    let mut frame = [0; 1441];
    let mut stream = None;
    let mut info: Option<InfoFrame> = None;
    let mut audio_frames = 0u64;
    let mut first_bitrate = None;
    let mut varying_bitrate = false;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(
            AudioCodecParameters::new().for_codec(CODEC_ID_MP3),
            &AudioDecoderOptions::default().gapless(false).verify(true),
        )
        .map_err(|_| Error::Internal("MP3 decoder initialization failed"))?;
    while position < range.end {
        if range.end - position < 4 {
            return Err(invalid("MP3 has a partial final frame header"));
        }
        source.read_exact(&mut frame[..4])?;
        let header = match header([frame[0], frame[1], frame[2], frame[3]]) {
            Ok(header) => header,
            Err(error) => {
                if trailing_tags_start(&mut source, file_length)? != position
                    || range.start + file_length - position > MAX_TAG_BYTES
                {
                    return Err(error);
                }
                range.end = position;
                break;
            }
        };
        if position + header.length as u64 > range.end {
            return Err(invalid("MP3 frame extends beyond the audio range"));
        }
        if let Some((sample_rate, channels)) = stream {
            if sample_rate != header.sample_rate || channels != header.channels {
                return Err(invalid(
                    "MP3 sample rate or channel count changes within the stream",
                ));
            }
        } else {
            stream = Some((header.sample_rate, header.channels));
        }
        source.read_exact(&mut frame[4..header.length])?;
        let bytes = &frame[..header.length];
        if header.side_start == 6 {
            let mut crc = Crc16Ansi::new(0xffff);
            crc.process_buf_bytes(&bytes[2..4]);
            crc.process_buf_bytes(&bytes[6..header.main_start]);
            if crc.crc() != u16::from_be_bytes([bytes[4], bytes[5]]) {
                return Err(invalid("MP3 protected frame CRC does not match"));
            }
        }
        if let Some(metadata) = info_frame(bytes, header)? {
            if position != range.start {
                return Err(invalid(
                    "Xing/Info metadata is only allowed in the first MPEG frame",
                ));
            }
            info = Some(metadata);
        } else {
            if profile
                .bitrate()
                .is_some_and(|bitrate| header.bitrate != bitrate)
            {
                return Err(invalid(
                    "MP3 audio frame bitrate differs from the selected CBR profile",
                ));
            }
            if let Some(first) = first_bitrate {
                varying_bitrate |= first != header.bitrate;
            } else {
                first_bitrate = Some(header.bitrate);
            }
            let packet = Packet::new(0, Timestamp::ZERO, Duration::from(SAMPLES_PER_FRAME), bytes);
            let decoded = decoder
                .decode(&packet)
                .map_err(|_| invalid("MP3 audio frame cannot be decoded"))?;
            if decoded.frames() != SAMPLES_PER_FRAME as usize {
                return Err(invalid("MP3 audio frame did not decode completely"));
            }
            audio_frames += 1;
        }
        digest.update(bytes);
        position += header.length as u64;
    }
    if audio_frames == 0 {
        return Err(invalid("MP3 contains no audio frames"));
    }
    if profile == AudioProfile::Mp3Vbr && !varying_bitrate {
        return Err(invalid(
            "MP3 VBR requires observed audio-frame bitrate variation",
        ));
    }
    if let Some(info) = &info
        && (info
            .audio_frames
            .is_some_and(|count| u64::from(count) != audio_frames)
            || info
                .mpeg_bytes
                .is_some_and(|count| u64::from(count) != range.end - range.start))
    {
        return Err(invalid(
            "Xing/Info counts do not match the observed MPEG frames",
        ));
    }
    if decoder.finalize().verify_ok == Some(false) {
        return Err(invalid("MP3 decoder verification failed"));
    }
    if source.seek(SeekFrom::End(0))? != file_length {
        return Err(Error::Conflict(
            "MP3 file length changed during validation".into(),
        ));
    }
    let (sample_rate, channels) = stream.ok_or_else(|| invalid("MP3 has no MPEG stream"))?;
    Ok(Mp3Evidence {
        mpeg_range: range,
        mpeg_sha256: format!("{:x}", digest.finalize()),
        audio_frames,
        metadata_frames: u8::from(info.is_some()),
        sample_rate,
        channels,
        samples_per_channel: audio_frames
            .checked_mul(SAMPLES_PER_FRAME)
            .ok_or_else(|| invalid("MP3 sample count exceeds the supported range"))?,
    })
}
