use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::ops::Range;
use symphonia::core::checksum::{Crc8Ccitt, Crc16Ansi};
use symphonia::core::codecs::audio::well_known::CODEC_ID_FLAC;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
use symphonia::core::io::Monitor;
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration, Timestamp};

const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FRAME_BYTES: usize = 0xff_ffff;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlacEvidence {
    pub stream_info: [u8; 34],
    pub audio_range: Range<u64>,
    pub audio_sha256: String,
    pub frames: u64,
    pub samples_per_channel: u64,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub md5_verified: bool,
}

fn invalid(message: &'static str) -> Error {
    Error::InvalidInput(message.into())
}

struct StreamInfo {
    bytes: [u8; 34],
    min_block: u32,
    max_block: u32,
    min_frame: usize,
    max_frame: usize,
    rate: u32,
    channels: u8,
    bits: u8,
    samples: u64,
}

fn size24(bytes: &[u8]) -> usize {
    usize::from(bytes[0]) << 16 | usize::from(bytes[1]) << 8 | usize::from(bytes[2])
}

fn stream_info(bytes: [u8; 34]) -> Result<StreamInfo> {
    let min_block = u32::from(u16::from_be_bytes([bytes[0], bytes[1]]));
    let max_block = u32::from(u16::from_be_bytes([bytes[2], bytes[3]]));
    let mut packed = [0; 8];
    packed.copy_from_slice(&bytes[10..18]);
    let packed = u64::from_be_bytes(packed);
    let info = StreamInfo {
        min_block,
        max_block,
        min_frame: size24(&bytes[4..7]),
        max_frame: size24(&bytes[7..10]),
        rate: (packed >> 44) as u32,
        channels: ((packed >> 41) & 7) as u8 + 1,
        bits: ((packed >> 36) & 31) as u8 + 1,
        samples: packed & 0xf_ffff_ffff,
        bytes,
    };
    if info.min_block < 16
        || info.max_block < info.min_block
        || info.rate == 0
        || info.bits < 4
        || (info.max_frame != 0 && info.min_frame > info.max_frame)
    {
        return Err(invalid("invalid FLAC STREAMINFO values"));
    }
    if info.rate > 655_350 {
        return Err(invalid(
            "FLAC sample rate exceeds the current decoder's supported range",
        ));
    }
    Ok(info)
}

fn metadata(source: &mut (impl Read + Seek), length: u64) -> Result<StreamInfo> {
    source.seek(SeekFrom::Start(0))?;
    let mut marker = [0; 4];
    source.read_exact(&mut marker)?;
    if marker != *b"fLaC" {
        return Err(invalid("expected a native FLAC stream"));
    }
    let mut info = None;
    let mut position = 4u64;
    let mut unique = 0u8;
    for index in 0..128 {
        let mut header = [0; 4];
        source.read_exact(&mut header)?;
        let kind = header[0] & 0x7f;
        let size = size24(&header[1..]);
        position += 4 + size as u64;
        if position > length || position > MAX_METADATA_BYTES || kind == 127 {
            return Err(invalid(
                "FLAC metadata exceeds its file or validation bounds",
            ));
        }
        if kind == 0 {
            if index != 0 || size != 34 {
                return Err(invalid("FLAC requires one first 34-byte STREAMINFO block"));
            }
            let mut bytes = [0; 34];
            source.read_exact(&mut bytes)?;
            info = Some(stream_info(bytes)?);
        } else {
            if index == 0 {
                return Err(invalid("FLAC STREAMINFO must be first"));
            }
            if matches!(kind, 3..=5) {
                let flag = 1 << kind;
                if unique & flag != 0 {
                    return Err(invalid("duplicate unique FLAC metadata block"));
                }
                unique |= flag;
            }
            source.seek(SeekFrom::Current(size as i64))?;
        }
        if header[0] & 0x80 != 0 {
            return info.ok_or_else(|| invalid("FLAC STREAMINFO is missing"));
        }
    }
    Err(invalid("too many FLAC metadata blocks"))
}

struct FrameBits<'a, R> {
    source: &'a mut R,
    bytes: Vec<u8>,
    current: u8,
    available: u32,
}

impl<R: Read> FrameBits<'_, R> {
    fn refill(&mut self) -> Result<()> {
        if self.bytes.len() == MAX_FRAME_BYTES {
            return Err(invalid("FLAC frame exceeds the bounded frame size"));
        }
        let mut byte = [0];
        self.source.read_exact(&mut byte)?;
        self.current = byte[0];
        self.bytes.push(byte[0]);
        self.available = 8;
        Ok(())
    }

    fn bits(&mut self, mut count: u32) -> Result<u32> {
        let mut value = 0;
        while count != 0 {
            if self.available == 0 {
                self.refill()?;
            }
            let take = count.min(self.available);
            let mask = (1u32 << take) - 1;
            value = (value << take) | ((u32::from(self.current) >> (self.available - take)) & mask);
            self.available -= take;
            count -= take;
        }
        Ok(value)
    }

    fn signed(&mut self, count: u32) -> Result<i64> {
        Ok(i64::from(
            (self.bits(count)? << (32 - count)) as i32 >> (32 - count),
        ))
    }

    fn unary(&mut self, maximum: u32) -> Result<u32> {
        let mut value = 0u32;
        loop {
            if self.available == 0 {
                self.refill()?;
            }
            let zeros = (self.current << (8 - self.available))
                .leading_zeros()
                .min(self.available);
            value = value
                .checked_add(zeros)
                .filter(|value| *value <= maximum)
                .ok_or_else(|| invalid("FLAC unary value exceeds its coding bounds"))?;
            self.available -= zeros;
            if self.available != 0 {
                self.available -= 1;
                return Ok(value);
            }
        }
    }

    fn coded_number(&mut self) -> Result<u64> {
        let first = self.bits(8)? as u8;
        let width = first.leading_ones();
        if width == 0 {
            return Ok(u64::from(first));
        }
        if !(2..=7).contains(&width) {
            return Err(invalid("invalid FLAC coded frame/sample number"));
        }
        let mut value = u64::from(first & ((1 << (7 - width)) - 1));
        for _ in 1..width {
            let next = self.bits(8)?;
            if next & 0xc0 != 0x80 {
                return Err(invalid("invalid FLAC coded-number continuation"));
            }
            value = (value << 6) | u64::from(next & 0x3f);
        }
        let minimum = [
            0,
            0,
            0x80,
            0x800,
            0x1_0000,
            0x20_0000,
            0x400_0000,
            0x8000_0000,
        ][width as usize];
        if value < minimum || value > 0xf_ffff_ffff {
            return Err(invalid("overlong or oversized FLAC coded number"));
        }
        Ok(value)
    }
}

struct Frame {
    bytes: Vec<u8>,
    block: u32,
    variable: bool,
    number: u64,
}

fn residual<R: Read>(frame: &mut FrameBits<'_, R>, block: u32, order: u32) -> Result<Vec<i64>> {
    let parameter_bits = match frame.bits(2)? {
        0 => 4,
        1 => 5,
        _ => return Err(invalid("reserved FLAC residual coding method")),
    };
    let partitions = 1 << frame.bits(4)?;
    if !block.is_multiple_of(partitions) || block / partitions <= order {
        return Err(invalid("invalid FLAC residual partition size"));
    }
    let mut values = Vec::with_capacity((block - order) as usize);
    for partition in 0..partitions {
        let samples = block / partitions - if partition == 0 { order } else { 0 };
        let parameter = frame.bits(parameter_bits)?;
        if parameter == (1 << parameter_bits) - 1 {
            let width = frame.bits(5)?;
            for _ in 0..samples {
                values.push(if width == 0 { 0 } else { frame.signed(width)? });
            }
        } else {
            for _ in 0..samples {
                let quotient = frame.unary(0xffff_fffe >> parameter)?;
                let remainder = frame.bits(parameter)?;
                let folded = (u64::from(quotient) << parameter) | u64::from(remainder);
                if folded > 0xffff_fffe {
                    return Err(invalid("FLAC residual exceeds the signed coding range"));
                }
                values.push((folded >> 1) as i64 ^ -((folded & 1) as i64));
            }
        }
    }
    Ok(values)
}

fn subframe<R: Read>(frame: &mut FrameBits<'_, R>, block: u32, mut bits: u32) -> Result<Vec<i64>> {
    if frame.bits(1)? != 0 {
        return Err(invalid("FLAC subframe padding bit must be zero"));
    }
    let kind = frame.bits(6)?;
    let wasted = if frame.bits(1)? != 0 {
        frame.unary(bits - 2)? + 1
    } else {
        0
    };
    bits -= wasted;
    let mut values = Vec::with_capacity(block as usize);
    let order = match kind {
        0 => {
            let value = frame.signed(bits)? << wasted;
            return Ok(vec![value; block as usize]);
        }
        1 => {
            for _ in 0..block {
                values.push(frame.signed(bits)? << wasted);
            }
            return Ok(values);
        }
        8..=12 => kind - 8,
        32..=63 => kind - 31,
        _ => return Err(invalid("reserved FLAC subframe type")),
    };
    if order > block {
        return Err(invalid("FLAC predictor order exceeds its block size"));
    }
    for _ in 0..order {
        values.push(frame.signed(bits)?);
    }
    let (coefficients, shift) = if kind >= 32 {
        let precision = frame.bits(4)? + 1;
        if precision == 16 {
            return Err(invalid("reserved FLAC predictor precision"));
        }
        let shift = frame.bits(5)?;
        if shift & 16 != 0 {
            return Err(invalid("FLAC predictor shift must not be negative"));
        }
        let mut coefficients = Vec::with_capacity(order as usize);
        for _ in 0..order {
            coefficients.push(frame.signed(precision)?);
        }
        (coefficients, shift)
    } else {
        let coefficients: &[i64] = match order {
            0 => &[],
            1 => &[1],
            2 => &[2, -1],
            3 => &[3, -3, 1],
            4 => &[4, -6, 4, -1],
            _ => return Err(invalid("invalid FLAC fixed predictor order")),
        };
        (coefficients.to_vec(), 0)
    };
    let limit = 1i64 << (bits - 1);
    for residual in residual(frame, block, order)? {
        let prediction = coefficients
            .iter()
            .zip(values.iter().rev())
            .map(|(coefficient, previous)| coefficient * previous)
            .sum::<i64>()
            >> shift;
        let value = prediction + residual;
        if !(-limit..limit).contains(&value) {
            return Err(invalid(
                "FLAC decoded sample exceeds its subframe bit depth",
            ));
        }
        values.push(value);
    }
    for value in &mut values {
        *value <<= wasted;
    }
    Ok(values)
}

fn read_frame(source: &mut impl Read, info: &StreamInfo) -> Result<Frame> {
    let mut frame = FrameBits {
        source,
        bytes: Vec::with_capacity(8192),
        current: 0,
        available: 0,
    };
    if frame.bits(15)? != 0x7ffc {
        return Err(invalid("FLAC frame sync or reserved bit is invalid"));
    }
    let variable = frame.bits(1)? != 0;
    let block_code = frame.bits(4)?;
    let rate_code = frame.bits(4)?;
    let assignment = frame.bits(4)?;
    let depth_code = frame.bits(3)?;
    if frame.bits(1)? != 0 || assignment > 10 {
        return Err(invalid("reserved FLAC frame header value"));
    }
    let number = frame.coded_number()?;
    if !variable && number > 0x7fff_ffff {
        return Err(invalid("FLAC fixed frame number exceeds 31 bits"));
    }
    let block = match block_code {
        1 => 192,
        2..=5 => 576 << (block_code - 2),
        6 => frame.bits(8)? + 1,
        7 => frame.bits(16)? + 1,
        8..=15 => 256 << (block_code - 8),
        _ => return Err(invalid("reserved FLAC block size")),
    };
    let rate = match rate_code {
        0 => info.rate,
        1..=11 => [
            0, 88_200, 176_400, 192_000, 8_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
            96_000,
        ][rate_code as usize],
        12 => frame.bits(8)? * 1000,
        13 => frame.bits(16)?,
        14 => frame.bits(16)? * 10,
        _ => return Err(invalid("reserved FLAC sample rate")),
    };
    let bits = match depth_code {
        0 => u32::from(info.bits),
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return Err(invalid("reserved FLAC bit depth")),
    };
    let channels = if assignment < 8 { assignment + 1 } else { 2 };
    if block > info.max_block
        || rate != info.rate
        || channels != u32::from(info.channels)
        || bits != u32::from(info.bits)
    {
        return Err(invalid("FLAC frame properties do not match STREAMINFO"));
    }
    if bits == 32 && assignment >= 8 {
        return Err(invalid(
            "32-bit FLAC stereo decorrelation needs separate decoder support",
        ));
    }
    let mut crc = Crc8Ccitt::new(0);
    crc.process_buf_bytes(&frame.bytes);
    if frame.bits(8)? as u8 != crc.crc() {
        return Err(invalid("FLAC frame header CRC does not match"));
    }
    let mut decoded = Vec::with_capacity(channels as usize);
    for channel in 0..channels {
        let extra = (assignment == 8 || assignment == 10) && channel == 1
            || assignment == 9 && channel == 0;
        decoded.push(subframe(&mut frame, block, bits + u32::from(extra))?);
    }
    if assignment >= 8 {
        let limit = 1i64 << (bits - 1);
        for (&first, &second) in decoded[0].iter().zip(&decoded[1]) {
            let (left, right) = match assignment {
                8 => (first, first - second),
                9 => (first + second, second),
                10 => {
                    let mid = (first << 1) | (second & 1);
                    ((mid + second) >> 1, (mid - second) >> 1)
                }
                _ => return Err(invalid("invalid FLAC stereo assignment")),
            };
            if !(-limit..limit).contains(&left) || !(-limit..limit).contains(&right) {
                return Err(invalid("FLAC decoded stereo sample exceeds its bit depth"));
            }
        }
    }
    while frame.available != 0 {
        if frame.bits(1)? != 0 {
            return Err(invalid("FLAC frame alignment padding must be zero"));
        }
    }
    let mut crc = Crc16Ansi::new(0);
    crc.process_buf_bytes(&frame.bytes);
    if frame.bits(16)? as u16 != crc.crc() {
        return Err(invalid("FLAC frame CRC does not match"));
    }
    if frame.bytes.len() < info.min_frame
        || info.max_frame != 0 && frame.bytes.len() > info.max_frame
    {
        return Err(invalid("FLAC frame size does not match STREAMINFO"));
    }
    Ok(Frame {
        bytes: frame.bytes,
        block,
        variable,
        number,
    })
}

/// Independently consume FLAC coding boundaries and CRCs before PCM decoding.
/// Never rely on a playback reader's resynchronization or end-of-stream repair.
pub fn inspect_flac(source: &mut (impl Read + Seek)) -> Result<FlacEvidence> {
    let mut source = BufReader::with_capacity(64 * 1024, source);
    let length = source.seek(SeekFrom::End(0))?;
    if !(42..=i64::MAX as u64).contains(&length) {
        return Err(invalid("FLAC file length is outside the supported range"));
    }
    let info = metadata(&mut source, length)?;
    let start = source.stream_position()?;
    let mut position = start;
    let mut digest = Sha256::new();
    let mut params = AudioCodecParameters::new();
    params
        .for_codec(CODEC_ID_FLAC)
        .with_extra_data(info.bytes.to_vec().into_boxed_slice());
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(
            &params,
            &AudioDecoderOptions::default().gapless(false).verify(true),
        )
        .map_err(|_| invalid("FLAC STREAMINFO cannot initialize the decoder"))?;
    let mut samples = 0u64;
    let mut frames = 0u64;
    let mut strategy: Option<(bool, u32)> = None;
    let mut previous_block = None;
    while position < length {
        let frame = read_frame(&mut source, &info)?;
        if let Some((variable, nominal)) = strategy {
            if variable != frame.variable
                || !variable && (previous_block != Some(nominal) || frame.block > nominal)
            {
                return Err(invalid("FLAC blocking strategy changes within the stream"));
            }
        } else {
            strategy = Some((frame.variable, frame.block));
        }
        if frame.number != if frame.variable { samples } else { frames } {
            return Err(invalid("FLAC frame/sample numbering is not continuous"));
        }
        position += frame.bytes.len() as u64;
        if position < length && frame.block < info.min_block {
            return Err(invalid(
                "a non-final FLAC block is below the minimum block size",
            ));
        }
        digest.update(&frame.bytes);
        let packet = Packet::new(0, Timestamp::ZERO, Duration::from(frame.block), frame.bytes);
        let decoded = decoder
            .decode(&packet)
            .map_err(|_| invalid("FLAC frame cannot be decoded"))?;
        if decoded.frames() != frame.block as usize {
            return Err(invalid("FLAC frame did not decode completely"));
        }
        samples += u64::from(frame.block);
        frames += 1;
        previous_block = Some(frame.block);
    }
    if frames == 0 || position != length || info.samples != 0 && samples != info.samples {
        return Err(invalid(
            "FLAC frame range or total sample count is incomplete",
        ));
    }
    let md5_present = info.bytes[18..].iter().any(|byte| *byte != 0);
    let verified = decoder.finalize().verify_ok;
    if verified == Some(false) || md5_present && verified != Some(true) {
        return Err(invalid("FLAC decoded MD5 does not match STREAMINFO"));
    }
    if source.seek(SeekFrom::End(0))? != length {
        return Err(Error::Conflict(
            "FLAC file length changed during validation".into(),
        ));
    }
    Ok(FlacEvidence {
        stream_info: info.bytes,
        audio_range: start..length,
        audio_sha256: format!("{:x}", digest.finalize()),
        frames,
        samples_per_channel: samples,
        sample_rate: info.rate,
        channels: info.channels,
        bits_per_sample: info.bits,
        md5_verified: md5_present,
    })
}
