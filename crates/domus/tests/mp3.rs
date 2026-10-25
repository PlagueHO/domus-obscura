use domus::infrastructure::mp3::inspect_mp3_320;
use sha2::{Digest, Sha256};
use std::io::Cursor;

fn crc16(bytes: impl Iterator<Item = u8>) -> u16 {
    let mut register = 0xffffu32;
    for byte in bytes {
        for bit in (0..8).rev() {
            let feedback = (register >> 15) ^ u32::from((byte >> bit) & 1);
            register = (register << 1) & 0xffff;
            if feedback & 1 != 0 {
                register ^= 0x8005;
            }
        }
    }
    register as u16
}

fn frame(index: u8, rate: u8, mono: bool, padding: bool, protected: bool) -> Vec<u8> {
    let bitrate = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ][usize::from(index)];
    let sample_rate = [44_100, 48_000, 32_000][usize::from(rate)];
    let length = 144_000 * bitrate / sample_rate + usize::from(padding);
    let mut bytes = vec![0; length];
    bytes[..4].copy_from_slice(&[
        0xff,
        if protected { 0xfa } else { 0xfb },
        (index << 4) | (rate << 2) | (u8::from(padding) << 1),
        if mono { 0xc0 } else { 0 },
    ]);
    if protected {
        let side_end = 6 + if mono { 17 } else { 32 };
        let crc = crc16(bytes[2..4].iter().chain(&bytes[6..side_end]).copied());
        bytes[4..6].copy_from_slice(&crc.to_be_bytes());
    }
    bytes
}

fn info_frame(marker: &[u8; 4], audio_frames: u32, mpeg_bytes: u32) -> Vec<u8> {
    let mut bytes = frame(9, 0, false, false, false);
    bytes[36..40].copy_from_slice(marker);
    bytes[40..44].copy_from_slice(&3u32.to_be_bytes());
    bytes[44..48].copy_from_slice(&audio_frames.to_be_bytes());
    bytes[48..52].copy_from_slice(&mpeg_bytes.to_be_bytes());
    bytes
}

#[test]
fn mp3_actual_frames_cover_rates_channels_padding_crc_and_preserved_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(crc16(b"123456789".iter().copied()), 0xaee7);
    for rate in 0..3 {
        for mono in [false, true] {
            for protected in [false, true] {
                let bytes = [
                    frame(14, rate, mono, false, protected),
                    frame(14, rate, mono, true, protected),
                    frame(14, rate, mono, false, protected),
                ]
                .concat();
                let mut source = Cursor::new(bytes.clone());
                let evidence = inspect_mp3_320(&mut source)?;
                assert_eq!(evidence.audio_frames, 3);
                assert_eq!(evidence.metadata_frames, 0);
                assert_eq!(evidence.samples_per_channel, 3456);
                assert_eq!(
                    evidence.sample_rate,
                    [44_100, 48_000, 32_000][usize::from(rate)]
                );
                assert_eq!(evidence.channels, if mono { 1 } else { 2 });
                assert_eq!(evidence.mpeg_range, 0..bytes.len() as u64);
                assert_eq!(
                    evidence.mpeg_sha256,
                    format!("{:x}", Sha256::digest(&bytes))
                );
                assert_eq!(source.into_inner(), bytes);
            }
        }
    }
    Ok(())
}

#[test]
fn mp3_info_frame_is_counted_separately_from_320_kbps_audio()
-> Result<(), Box<dyn std::error::Error>> {
    let audio = [
        frame(14, 0, false, false, false),
        frame(14, 0, false, true, false),
    ]
    .concat();
    let length = audio.len() + frame(9, 0, false, false, false).len();
    for marker in [b"Info", b"Xing"] {
        let mut bytes = info_frame(marker, 2, length as u32);
        bytes.extend_from_slice(&audio);
        let evidence = inspect_mp3_320(&mut Cursor::new(&bytes))?;
        assert_eq!(evidence.audio_frames, 2);
        assert_eq!(evidence.metadata_frames, 1);
        assert_eq!(evidence.samples_per_channel, 2304);
        assert_eq!(
            evidence.mpeg_sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        let first_length = length - audio.len();
        assert!(inspect_mp3_320(&mut Cursor::new(&bytes[..first_length])).is_err());
        for offset in [44, 48] {
            let mut changed = bytes.clone();
            changed[offset + 3] ^= 1;
            assert!(inspect_mp3_320(&mut Cursor::new(changed)).is_err());
        }
        let reordered = [&audio[..], &bytes[..first_length]].concat();
        assert!(inspect_mp3_320(&mut Cursor::new(reordered)).is_err());
        bytes[4] = 1;
        assert!(inspect_mp3_320(&mut Cursor::new(bytes)).is_err());
    }
    Ok(())
}

#[test]
fn mp3_rejects_non_320_audio_in_every_position_and_format_changes() {
    for bitrate in 1..14 {
        for bad_position in 0..3 {
            let bytes = (0..3)
                .flat_map(|position| {
                    frame(
                        if position == bad_position {
                            bitrate
                        } else {
                            14
                        },
                        0,
                        false,
                        false,
                        false,
                    )
                })
                .collect::<Vec<_>>();
            assert!(inspect_mp3_320(&mut Cursor::new(bytes)).is_err());
        }
    }
    let good = frame(14, 0, false, false, false);
    for changed in [
        frame(14, 1, false, false, false),
        frame(14, 0, true, false, false),
    ] {
        let bytes = [&good[..], &changed].concat();
        assert!(inspect_mp3_320(&mut Cursor::new(bytes)).is_err());
    }
    for (offset, mask) in [(0, 1), (1, 8), (1, 2), (2, 0xe0), (3, 2)] {
        let mut changed = good.clone();
        changed[offset] ^= mask;
        assert!(inspect_mp3_320(&mut Cursor::new(changed)).is_err());
    }
}

#[test]
fn mp3_rejects_short_frames_junk_broken_crc_and_undecodable_side_information() {
    let first = frame(14, 0, false, false, false);
    let second = frame(14, 0, false, true, false);
    let full = [&first[..], &second].concat();
    for cut in [
        1,
        3,
        4,
        20,
        first.len() - 1,
        first.len() + 1,
        full.len() - 1,
    ] {
        assert!(inspect_mp3_320(&mut Cursor::new(&full[..cut])).is_err());
    }
    for junk in [&[0][..], b"junk", &[0xff, 0xfb, 0xe0]] {
        let bytes = [&full[..], junk].concat();
        assert!(inspect_mp3_320(&mut Cursor::new(bytes)).is_err());
    }
    let mut protected = frame(14, 0, false, false, true);
    protected[4] ^= 1;
    assert!(inspect_mp3_320(&mut Cursor::new(protected)).is_err());
    let mut invalid = first;
    invalid[4..36].fill(0xff);
    assert!(inspect_mp3_320(&mut Cursor::new(invalid)).is_err());
}

fn ape_footer(has_header: bool, is_header: bool) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(b"APETAGEX");
    bytes[8..12].copy_from_slice(&2000u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&32u32.to_le_bytes());
    let flags = (u32::from(has_header) << 31) | (u32::from(is_header) << 29);
    bytes[20..24].copy_from_slice(&flags.to_le_bytes());
    bytes
}

#[test]
fn mp3_tag_framing_does_not_change_the_encoded_audio_hash() -> Result<(), Box<dyn std::error::Error>>
{
    let audio = [
        frame(14, 0, false, false, false),
        frame(14, 0, false, false, false),
    ]
    .concat();
    let original = inspect_mp3_320(&mut Cursor::new(&audio))?;
    for version in 2..=4 {
        let footer = version == 4;
        let mut bytes = vec![
            b'I',
            b'D',
            b'3',
            version,
            0,
            if footer { 0x10 } else { 0 },
            0,
            0,
            0,
            0,
        ];
        if footer {
            bytes.extend_from_slice(b"3DI\x04\x00\x10\x00\x00\x00\x00");
        }
        let start = bytes.len() as u64;
        bytes.extend_from_slice(&audio);
        let end = bytes.len() as u64;
        bytes.extend_from_slice(&ape_footer(true, true));
        bytes.extend_from_slice(&ape_footer(true, false));
        bytes.extend_from_slice(b"TAG");
        bytes.extend_from_slice(&[0; 125]);
        let evidence = inspect_mp3_320(&mut Cursor::new(&bytes))?;
        assert_eq!(evidence.mpeg_range, start..end);
        assert_eq!(evidence.mpeg_sha256, original.mpeg_sha256);
        assert_eq!(evidence.audio_frames, original.audio_frames);
    }
    Ok(())
}

#[test]
fn mp3_rejects_oversized_ambiguous_and_inconsistent_metadata() {
    let audio = frame(14, 0, false, false, false);
    for prefix in [
        b"ID3\x04\x00\x00\x7f\x7f\x7f\x7f".as_slice(),
        b"ID3\x03\x00\x01\x00\x00\x00\x00".as_slice(),
        b"ID3\x05\x00\x00\x00\x00\x00\x00".as_slice(),
        b"ID3\x04\x00\x10\x00\x00\x00\x00bad footer".as_slice(),
    ] {
        let bytes = [prefix, &audio].concat();
        assert!(inspect_mp3_320(&mut Cursor::new(bytes)).is_err());
    }
    let mut bad_ape = ape_footer(false, false);
    bad_ape[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(inspect_mp3_320(&mut Cursor::new([&audio[..], &bad_ape].concat())).is_err());
    let mut info = info_frame(b"Info", 1, 0);
    info[40] = 0x80;
    assert!(inspect_mp3_320(&mut Cursor::new([&info[..], &audio].concat())).is_err());
    let mut vbri = audio.clone();
    vbri[36..40].copy_from_slice(b"VBRI");
    assert!(inspect_mp3_320(&mut Cursor::new(vbri)).is_err());
    for bytes in [&[][..], b"not MP3", b"TAG", b"ID3"] {
        assert!(inspect_mp3_320(&mut Cursor::new(bytes)).is_err());
    }
}

#[test]
fn mp3_tag_markers_in_frame_stuffing_are_not_mistaken_for_trailing_tags()
-> Result<(), Box<dyn std::error::Error>> {
    let audio = frame(14, 0, false, false, false);
    let mut with_id3_marker = audio.clone();
    let offset = with_id3_marker.len() - 128;
    with_id3_marker[offset..offset + 3].copy_from_slice(b"TAG");
    let mut with_ape_marker = audio.clone();
    let offset = with_ape_marker.len() - 32;
    with_ape_marker[offset..].copy_from_slice(&ape_footer(false, false));
    for bytes in [with_id3_marker, with_ape_marker] {
        let evidence = inspect_mp3_320(&mut Cursor::new(&bytes))?;
        assert_eq!(evidence.mpeg_range, 0..bytes.len() as u64);
        assert_eq!(evidence.audio_frames, 1);
    }
    let non_320 = frame(9, 0, false, false, false);
    let mut forged_footer = ape_footer(false, false);
    forged_footer[12..16].copy_from_slice(&(non_320.len() as u32 + 32).to_le_bytes());
    let bytes = [&audio[..], &non_320, &forged_footer].concat();
    assert!(inspect_mp3_320(&mut Cursor::new(bytes)).is_err());
    Ok(())
}

fn nonzero_spectral_frame() -> Vec<u8> {
    let mut bytes = frame(14, 0, false, false, false);
    let mut bits = Vec::new();
    let mut push = |value: u32, width: u32| {
        for bit in (0..width).rev() {
            bits.push(((value >> bit) & 1) as u8);
        }
    };
    push(0, 20);
    for _ in 0..4 {
        push(3, 12);
        push(1, 9);
        push(210, 8);
        push(0, 4);
        push(0, 1);
        for _ in 0..3 {
            push(1, 5);
        }
        push(0, 4);
        push(0, 3);
        push(0, 3);
    }
    assert_eq!(bits.len(), 256);
    for (index, bits) in bits.as_chunks::<8>().0.iter().enumerate() {
        bytes[4 + index] = bits.iter().fold(0, |value, bit| (value << 1) | bit);
    }
    bytes[36..38].copy_from_slice(&[0x49, 0x20]);
    bytes
}

#[test]
fn mp3_generated_nonzero_spectral_audio_decodes_without_changing_encoded_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    use symphonia::core::codecs::audio::well_known::CODEC_ID_MP3;
    use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
    use symphonia::core::packet::Packet;
    use symphonia::core::units::{Duration, Timestamp};
    let frame = nonzero_spectral_frame();
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(
        AudioCodecParameters::new().for_codec(CODEC_ID_MP3),
        &AudioDecoderOptions::default().gapless(false),
    )?;
    let packet = Packet::new(0, Timestamp::ZERO, Duration::from(1152u64), &frame[..]);
    let decoded = decoder.decode(&packet)?;
    let mut samples = vec![0f32; decoded.samples_interleaved()];
    decoded.copy_to_slice_interleaved(&mut samples);
    assert!(samples.iter().all(|sample| sample.is_finite()));
    assert!(samples.iter().any(|sample| *sample != 0.0));
    let audio = [&frame[..], &frame, &frame].concat();
    let mut source = Cursor::new(audio.clone());
    let evidence = inspect_mp3_320(&mut source)?;
    assert_eq!(evidence.audio_frames, 3);
    assert_eq!(
        evidence.mpeg_sha256,
        format!("{:x}", Sha256::digest(&audio))
    );
    assert_eq!(source.into_inner(), audio);
    Ok(())
}

#[test]
fn mp3_streaming_reads_are_bounded_for_a_longer_generated_stream()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Seek, SeekFrom};
    struct Tracked {
        source: Cursor<Vec<u8>>,
        largest_read: usize,
    }
    impl Read for Tracked {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.largest_read = self.largest_read.max(bytes.len());
            self.source.read(bytes)
        }
    }
    impl Seek for Tracked {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.source.seek(position)
        }
    }
    let frame = nonzero_spectral_frame();
    let bytes = frame.repeat(4096);
    let mut source = Tracked {
        source: Cursor::new(bytes),
        largest_read: 0,
    };
    let evidence = inspect_mp3_320(&mut source)?;
    assert_eq!(evidence.audio_frames, 4096);
    assert_eq!(evidence.samples_per_channel, 4096 * 1152);
    assert!(source.largest_read <= 64 * 1024);
    Ok(())
}
