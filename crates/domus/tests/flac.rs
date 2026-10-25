use domus::infrastructure::flac::inspect_flac;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read, Seek, SeekFrom};
use symphonia::core::checksum::Md5;
use symphonia::core::io::Monitor;

#[derive(Clone, Copy)]
enum Mode {
    Constant,
    Verbatim,
    Fixed,
    Rice2,
    Escape,
    EscapeValues,
    RiceValues,
    OutOfRangePredictor,
    OutOfRangeStereo,
    Lpc,
    Wasted,
}

#[derive(Clone, Copy)]
struct Spec {
    bits: u8,
    channels: u8,
    variable: bool,
    assignment: u8,
}

fn crc(bytes: &[u8], width: u32, polynomial: u32) -> u32 {
    let mut value = 0;
    for byte in bytes {
        for bit in (0..8).rev() {
            let feedback = (value >> (width - 1)) ^ u32::from((byte >> bit) & 1);
            value = (value << 1) & ((1 << width) - 1);
            if feedback & 1 != 0 {
                value ^= polynomial;
            }
        }
    }
    value
}

fn footer(frame: &mut [u8]) {
    let end = frame.len() - 2;
    let value = crc(&frame[..end], 16, 0x8005) as u16;
    frame[end..].copy_from_slice(&value.to_be_bytes());
}

fn sample(mode: Mode, index: u32) -> i32 {
    match mode {
        Mode::Constant => -3,
        Mode::Verbatim | Mode::EscapeValues | Mode::RiceValues => (index % 7) as i32 - 3,
        Mode::Fixed => 1,
        Mode::Lpc => 2,
        Mode::Wasted => -4,
        Mode::OutOfRangePredictor => 32_767,
        Mode::OutOfRangeStereo => 0,
        Mode::Rice2 | Mode::Escape => 0,
    }
}

fn coded_number(number: u64) -> Vec<u8> {
    if number < 128 {
        return vec![number as u8];
    }
    for width in 2..=7 {
        if number < 1 << (5 * width + 1) {
            let mut bytes = vec![(0xffu8 << (8 - width)) | (number >> (6 * (width - 1))) as u8];
            for index in (0..width - 1).rev() {
                bytes.push(0x80 | ((number >> (6 * index)) & 63) as u8);
            }
            return bytes;
        }
    }
    panic!("fixture number exceeds FLAC coding bounds");
}

fn make_frame(spec: Spec, block: u32, number: u64, mode: Mode) -> Vec<u8> {
    let mut frame = vec![
        0xff,
        0xf8 | u8::from(spec.variable),
        0x60,
        spec.assignment << 4,
    ];
    frame.extend(coded_number(number));
    frame.push((block - 1) as u8);
    frame.push(crc(&frame, 8, 7) as u8);
    let mut bits = Vec::new();
    let mut push = |value: i64, count: u32| {
        for bit in (0..count).rev() {
            bits.push(((value as u64 >> bit) & 1) as u8);
        }
    };
    for channel in 0..spec.channels {
        let extra = (spec.assignment == 8 || spec.assignment == 10) && channel == 1
            || spec.assignment == 9 && channel == 0;
        let depth = u32::from(spec.bits) + u32::from(extra);
        let side = spec.assignment >= 8 && extra;
        let kind = match mode {
            Mode::Constant | Mode::OutOfRangeStereo => 0,
            Mode::Verbatim => 2,
            Mode::Fixed | Mode::OutOfRangePredictor => 0x12,
            Mode::Lpc => 0x40,
            Mode::Wasted => 1,
            Mode::Rice2 | Mode::Escape | Mode::EscapeValues | Mode::RiceValues => 0x10,
        };
        push(kind, 8);
        match mode {
            Mode::Constant => push(if side { 0 } else { -3 }, depth),
            Mode::OutOfRangeStereo => push(if channel == 0 { 7 } else { -8 }, depth),
            Mode::Verbatim => {
                for index in 0..block {
                    push(i64::from(sample(mode, index)), depth);
                }
            }
            Mode::Wasted => {
                push(1, 1);
                push(-2, depth - 1);
            }
            _ => {
                let order = u32::from(matches!(
                    mode,
                    Mode::Fixed | Mode::Lpc | Mode::OutOfRangePredictor
                ));
                if order == 1 {
                    push(i64::from(sample(mode, 0)), depth);
                }
                if matches!(mode, Mode::Lpc) {
                    push(1, 4);
                    push(0, 5);
                    push(1, 2);
                }
                let rice2 = matches!(mode, Mode::Rice2);
                push(i64::from(rice2), 2);
                push(i64::from(rice2), 4);
                let partitions = if rice2 { 2 } else { 1 };
                for partition in 0..partitions {
                    let count = block / partitions - if partition == 0 { order } else { 0 };
                    if matches!(mode, Mode::Escape | Mode::EscapeValues) {
                        push(15, 4);
                        let width = if matches!(mode, Mode::EscapeValues) {
                            4
                        } else {
                            0
                        };
                        push(width, 5);
                        for index in 0..count {
                            push(i64::from(sample(mode, index)), width as u32);
                        }
                    } else {
                        let parameter = if matches!(mode, Mode::RiceValues) {
                            2
                        } else {
                            0
                        };
                        push(parameter, if rice2 { 5 } else { 4 });
                        for index in 0..count {
                            let value = if matches!(mode, Mode::OutOfRangePredictor) {
                                1
                            } else if matches!(mode, Mode::RiceValues) {
                                sample(mode, index)
                            } else {
                                0
                            };
                            let folded = if value < 0 { -2 * value - 1 } else { 2 * value };
                            for _ in 0..folded >> parameter {
                                push(0, 1);
                            }
                            push(1, 1);
                            push(i64::from(folded & ((1 << parameter) - 1)), parameter as u32);
                        }
                    }
                }
            }
        }
    }
    while !bits.len().is_multiple_of(8) {
        bits.push(0);
    }
    for group in bits.as_chunks::<8>().0 {
        frame.push(group.iter().fold(0, |value, bit| (value << 1) | bit));
    }
    frame.extend_from_slice(&[0, 0]);
    footer(&mut frame);
    frame
}

fn make_stream(spec: Spec, blocks: &[u32], mode: Mode) -> (Vec<u8>, usize) {
    let mut pcm_md5 = Md5::default();
    for block in blocks {
        for index in 0..*block {
            for _ in 0..spec.channels {
                let value = sample(mode, index).to_le_bytes();
                pcm_md5.process_buf_bytes(&value[..usize::from(spec.bits.div_ceil(8))]);
            }
        }
    }
    let mut info = [0; 34];
    info[..2].copy_from_slice(&16u16.to_be_bytes());
    info[2..4].copy_from_slice(&(blocks[0] as u16).to_be_bytes());
    let packed = (44_100u64 << 44)
        | (u64::from(spec.channels - 1) << 41)
        | (u64::from(spec.bits - 1) << 36)
        | blocks.iter().map(|block| u64::from(*block)).sum::<u64>();
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    info[18..].copy_from_slice(&pcm_md5.md5());
    let mut bytes = b"fLaC\x80\x00\x00\x22".to_vec();
    bytes.extend_from_slice(&info);
    let start = bytes.len();
    let mut samples = 0;
    for (index, block) in blocks.iter().enumerate() {
        let number = if spec.variable { samples } else { index as u64 };
        bytes.extend_from_slice(&make_frame(spec, *block, number, mode));
        samples += u64::from(*block);
    }
    (bytes, start)
}

#[test]
fn flac_coding_modes_decode_crc_md5_and_preserve_encoded_frames()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(crc(b"123456789", 8, 7), 0xf4);
    assert_eq!(crc(b"123456789", 16, 0x8005), 0xfee8);
    for bits in [4, 8, 12, 16, 20, 24, 32] {
        for channels in [1, 2] {
            for variable in [false, true] {
                for mode in [
                    Mode::Constant,
                    Mode::Verbatim,
                    Mode::Fixed,
                    Mode::Rice2,
                    Mode::Escape,
                    Mode::EscapeValues,
                    Mode::RiceValues,
                    Mode::Lpc,
                    Mode::Wasted,
                ] {
                    let spec = Spec {
                        bits,
                        channels,
                        variable,
                        assignment: channels - 1,
                    };
                    let (bytes, start) = make_stream(spec, &[16, 8], mode);
                    let mut source = Cursor::new(bytes.clone());
                    let evidence = inspect_flac(&mut source)?;
                    assert_eq!(evidence.frames, 2);
                    assert_eq!(evidence.samples_per_channel, 24);
                    assert_eq!(evidence.sample_rate, 44_100);
                    assert_eq!(evidence.channels, channels);
                    assert_eq!(evidence.bits_per_sample, bits);
                    assert!(evidence.md5_verified);
                    assert_eq!(evidence.audio_range, start as u64..bytes.len() as u64);
                    assert_eq!(
                        evidence.audio_sha256,
                        format!("{:x}", Sha256::digest(&bytes[start..]))
                    );
                    assert_eq!(source.into_inner(), bytes);
                }
            }
        }
    }
    Ok(())
}

#[test]
fn flac_unknown_md5_is_explicit_and_bad_md5_is_rejected() -> Result<(), Box<dyn std::error::Error>>
{
    let spec = Spec {
        bits: 16,
        channels: 1,
        variable: false,
        assignment: 0,
    };
    let (mut bytes, _) = make_stream(spec, &[16], Mode::Escape);
    assert_eq!(
        &bytes[26..42],
        &[
            0x70, 0xbc, 0x8f, 0x4b, 0x72, 0xa8, 0x69, 0x21, 0x46, 0x8b, 0xf8, 0xe8, 0x44, 0x1d,
            0xce, 0x51
        ]
    );
    inspect_flac(&mut Cursor::new(&bytes))?;
    bytes[26] ^= 1;
    assert!(inspect_flac(&mut Cursor::new(&bytes)).is_err());
    bytes[26..42].fill(0);
    assert!(!inspect_flac(&mut Cursor::new(bytes))?.md5_verified);
    Ok(())
}

#[test]
fn flac_truncation_trailing_data_counts_and_crc_cannot_be_repaired_silently() {
    let spec = Spec {
        bits: 16,
        channels: 2,
        variable: false,
        assignment: 1,
    };
    let (bytes, start) = make_stream(spec, &[16, 16], Mode::Verbatim);
    let frame_length = (bytes.len() - start) / 2;
    for cut in [
        1,
        4,
        8,
        41,
        start,
        start + 1,
        start + 6,
        start + frame_length - 1,
        start + frame_length,
        bytes.len() - 1,
    ] {
        assert!(
            inspect_flac(&mut Cursor::new(&bytes[..cut])).is_err(),
            "cut {cut}"
        );
    }
    for extra in [&[0][..], &[0, 0], b"fLaC", &bytes[..]] {
        assert!(inspect_flac(&mut Cursor::new([&bytes[..], extra].concat())).is_err());
    }
    for offset in [25, start + 6, bytes.len() - 1] {
        let mut bad = bytes.clone();
        bad[offset] ^= 1;
        assert!(inspect_flac(&mut Cursor::new(bad)).is_err());
    }
}

#[test]
fn flac_nonzero_alignment_and_noncontinuous_numbering_are_rejected_even_with_valid_crcs() {
    let spec = Spec {
        bits: 16,
        channels: 1,
        variable: false,
        assignment: 0,
    };
    let (bytes, start) = make_stream(spec, &[16], Mode::Lpc);
    let mut bad = bytes.clone();
    let padding = bad.len() - 3;
    bad[padding] |= 1;
    footer(&mut bad[start..]);
    assert!(inspect_flac(&mut Cursor::new(bad)).is_err());
    let mut bad = bytes.clone();
    bad[start + 4] = 1;
    bad[start + 6] = crc(&bad[start..start + 6], 8, 7) as u8;
    footer(&mut bad[start..]);
    assert!(inspect_flac(&mut Cursor::new(bad)).is_err());
    let mut bad = bytes;
    bad[start + 4] = 0xc0;
    bad.insert(start + 5, 0x80);
    bad[start + 7] = crc(&bad[start..start + 7], 8, 7) as u8;
    footer(&mut bad[start..]);
    assert!(inspect_flac(&mut Cursor::new(bad)).is_err());
}

#[test]
fn flac_stereo_decorrelation_is_checked_and_unsupported_33_bit_sides_are_explicit()
-> Result<(), Box<dyn std::error::Error>> {
    for bits in [16, 24, 31] {
        for assignment in [8, 9, 10] {
            let spec = Spec {
                bits,
                channels: 2,
                variable: false,
                assignment,
            };
            let (bytes, _) = make_stream(spec, &[16], Mode::Constant);
            assert!(inspect_flac(&mut Cursor::new(bytes))?.md5_verified);
        }
    }
    let spec = Spec {
        bits: 32,
        channels: 2,
        variable: false,
        assignment: 8,
    };
    let (bytes, _) = make_stream(spec, &[16], Mode::Constant);
    let error = inspect_flac(&mut Cursor::new(bytes))
        .err()
        .ok_or("expected unsupported side width")?;
    assert!(
        error
            .to_string()
            .contains("32-bit FLAC stereo decorrelation")
    );
    Ok(())
}

#[test]
fn flac_metadata_framing_has_exact_bounds_and_one_first_streaminfo() {
    let spec = Spec {
        bits: 16,
        channels: 1,
        variable: false,
        assignment: 0,
    };
    let (bytes, start) = make_stream(spec, &[16], Mode::Constant);
    let mut duplicate = bytes.clone();
    duplicate[4] = 0;
    duplicate.splice(start..start, bytes[4..start].iter().copied());
    assert!(inspect_flac(&mut Cursor::new(duplicate)).is_err());
    for (offset, value) in [(4, 0x81), (4, 0xff), (7, 33), (8, 0), (9, 0)] {
        let mut bad = bytes.clone();
        bad[offset] = value;
        if bad != bytes {
            assert!(
                inspect_flac(&mut Cursor::new(bad)).is_err(),
                "offset {offset}"
            );
        }
    }
    let mut oversized = bytes.clone();
    oversized[4] = 0;
    oversized.splice(start..start, [0x81, 0xff, 0xff, 0xff]);
    assert!(inspect_flac(&mut Cursor::new(oversized)).is_err());
    let mut zero_rate = bytes;
    zero_rate[18..20].fill(0);
    zero_rate[20] &= 0x0f;
    assert!(inspect_flac(&mut Cursor::new(zero_rate)).is_err());
}

#[test]
fn flac_explicit_rates_use_the_correct_units_and_decoder_limits_are_explicit()
-> Result<(), Box<dyn std::error::Error>> {
    let spec = Spec {
        bits: 16,
        channels: 1,
        variable: false,
        assignment: 0,
    };
    for (code, rate, extension) in [
        (12, 48_000, vec![48]),
        (13, 65_535, vec![255, 255]),
        (14, 655_350, vec![255, 255]),
        (0, 1_048_575, vec![]),
    ] {
        let (mut bytes, start) = make_stream(spec, &[16], Mode::Constant);
        let mut packed = [0; 8];
        packed.copy_from_slice(&bytes[18..26]);
        let packed = (u64::from_be_bytes(packed) & ((1 << 44) - 1)) | (rate << 44);
        bytes[18..26].copy_from_slice(&packed.to_be_bytes());
        bytes[start + 2] |= code;
        bytes.splice(start + 6..start + 6, extension.iter().copied());
        let header_end = start + 6 + extension.len();
        bytes[header_end] = crc(&bytes[start..header_end], 8, 7) as u8;
        footer(&mut bytes[start..]);
        let result = inspect_flac(&mut Cursor::new(bytes));
        if rate > 655_350 {
            assert!(
                result
                    .err()
                    .ok_or("expected decoder rate limit")?
                    .to_string()
                    .contains("supported range")
            );
        } else {
            assert_eq!(result?.sample_rate, rate as u32);
        }
    }
    Ok(())
}

#[test]
fn flac_predictor_and_block_constraints_are_checked_before_decoding()
-> Result<(), Box<dyn std::error::Error>> {
    let spec = Spec {
        bits: 16,
        channels: 1,
        variable: false,
        assignment: 0,
    };
    for (mode, mask, expected) in [(Mode::Lpc, 8, "shift"), (Mode::Fixed, 16, "partition")] {
        let (mut bytes, start) = make_stream(spec, &[16], mode);
        bytes[start + 7 + 3] |= mask;
        footer(&mut bytes[start..]);
        let error = inspect_flac(&mut Cursor::new(bytes))
            .err()
            .ok_or("expected coding error")?;
        assert!(error.to_string().contains(expected), "{error}");
    }
    for variable in [false, true] {
        let spec = Spec { variable, ..spec };
        let (bytes, _) = make_stream(spec, &[16, 8, 16], Mode::Constant);
        assert!(inspect_flac(&mut Cursor::new(bytes)).is_err());
    }
    Ok(())
}

#[test]
fn flac_reconstruction_rejects_out_of_range_samples_before_the_playback_decoder()
-> Result<(), Box<dyn std::error::Error>> {
    for (spec, mode, expected) in [
        (
            Spec {
                bits: 16,
                channels: 1,
                variable: false,
                assignment: 0,
            },
            Mode::OutOfRangePredictor,
            "subframe bit depth",
        ),
        (
            Spec {
                bits: 4,
                channels: 2,
                variable: false,
                assignment: 8,
            },
            Mode::OutOfRangeStereo,
            "stereo sample",
        ),
    ] {
        let (bytes, _) = make_stream(spec, &[16], mode);
        let error = inspect_flac(&mut Cursor::new(bytes))
            .err()
            .ok_or("expected sample range error")?;
        assert!(error.to_string().contains(expected), "{error}");
    }
    Ok(())
}

#[test]
fn flac_long_stream_has_bounded_reads_and_canonical_multibyte_frame_numbers()
-> Result<(), Box<dyn std::error::Error>> {
    struct BoundedSource {
        source: Cursor<Vec<u8>>,
        largest: usize,
    }
    impl Read for BoundedSource {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            self.largest = self.largest.max(output.len());
            assert!(output.len() <= 64 * 1024);
            self.source.read(output)
        }
    }
    impl Seek for BoundedSource {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.source.seek(position)
        }
    }
    for variable in [false, true] {
        let spec = Spec {
            bits: 24,
            channels: 2,
            variable,
            assignment: 1,
        };
        let (bytes, start) = make_stream(spec, &[64; 4096], Mode::Verbatim);
        let expected = format!("{:x}", Sha256::digest(&bytes[start..]));
        let mut source = BoundedSource {
            source: Cursor::new(bytes.clone()),
            largest: 0,
        };
        let evidence = inspect_flac(&mut source)?;
        assert_eq!(evidence.frames, 4096);
        assert_eq!(evidence.samples_per_channel, 262_144);
        assert_eq!(evidence.audio_sha256, expected);
        assert!(evidence.md5_verified);
        assert_eq!(source.largest, 64 * 1024);
        assert_eq!(source.source.into_inner(), bytes);
    }
    Ok(())
}
