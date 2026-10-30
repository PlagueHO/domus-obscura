use super::*;
use domus::domain::{AudioProfile, AudioSelection};
use domus::infrastructure::media::normalize_audio_with_profile;
use domus::infrastructure::mp3::inspect_mp3;
use symphonia::core::checksum::Crc16Ansi;
use symphonia::core::io::Monitor;

pub(super) fn frames(
    bitrates: &[u32],
    protected: bool,
    padded: bool,
    rate_index: u8,
    mono: bool,
) -> Vec<u8> {
    let rates = [44_100, 48_000, 32_000];
    let mut bytes = Vec::new();
    for bitrate in bitrates {
        let index = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ]
        .iter()
        .position(|value| value == bitrate)
        .unwrap_or_else(|| panic!("unsupported generated fixture bitrate"));
        let length = 144_000 * bitrate / rates[usize::from(rate_index)] + u32::from(padded);
        let mut frame = vec![0; length as usize];
        frame[..4].copy_from_slice(&[
            0xff,
            if protected { 0xfa } else { 0xfb },
            ((index as u8) << 4) | (rate_index << 2) | (u8::from(padded) << 1),
            if mono { 0xc0 } else { 0 },
        ]);
        if protected {
            let mut crc = Crc16Ansi::new(0xffff);
            crc.process_buf_bytes(&frame[2..4]);
            crc.process_buf_bytes(&frame[6..if mono { 23 } else { 38 }]);
            frame[4..6].copy_from_slice(&crc.crc().to_be_bytes());
        }
        bytes.extend(frame);
    }
    bytes
}

#[test]
fn media_profiles_validate_every_cbr_frame_and_preserve_encoded_audio() -> TestResult {
    let tags = release(None)?.track_tags(1)?;
    let cover = cover()?;
    for profile in AudioProfile::ALL
        .into_iter()
        .filter(|profile| profile.bitrate().is_some())
    {
        let bitrate = profile.bitrate().ok_or("missing CBR bitrate")?;
        for (protected, padded, rate, mono) in [
            (false, false, 0, false),
            (true, true, 1, true),
            (true, false, 2, false),
        ] {
            let input = frames(&[bitrate; 3], protected, padded, rate, mono);
            let before = inspect_mp3(&mut Cursor::new(&input), profile)?;
            let mut output = Cursor::new(Vec::new());
            let receipt = normalize_audio_with_profile(
                &mut Cursor::new(&input),
                &mut output,
                AudioFormat::Mp3,
                Some(AudioSelection::new(profile)),
                &tags,
                &cover,
            )?;
            let after = inspect_mp3(&mut output, profile)?;
            assert_eq!(before.mpeg_sha256, after.mpeg_sha256);
            assert_eq!(receipt.audio_sha256, before.mpeg_sha256);
            assert_eq!(receipt.audio, Some(AudioSelection::new(profile)));
            output.set_position(0);
            assert_eq!(
                Tag::read_from2(&mut output)?.title(),
                Some("Another generated track")
            );
            let mut wrong = frames(
                &[bitrate, if bitrate == 320 { 128 } else { 320 }, bitrate],
                protected,
                padded,
                rate,
                mono,
            );
            assert!(inspect_mp3(&mut Cursor::new(&wrong), profile).is_err());
            wrong.pop();
            assert!(inspect_mp3(&mut Cursor::new(wrong), AudioProfile::Mp3Vbr).is_err());
            if protected {
                let mut corrupt = input.clone();
                corrupt[4] ^= 1;
                assert!(inspect_mp3(&mut Cursor::new(corrupt), profile).is_err());
            }
        }
    }
    Ok(())
}

#[test]
fn media_profiles_vbr_requires_real_variation_not_a_header_claim() -> TestResult {
    let profile = AudioProfile::Mp3Vbr;
    for protected in [false, true] {
        let input = frames(&[32, 128, 256, 320], protected, true, 0, false);
        let before = inspect_mp3(&mut Cursor::new(&input), profile)?;
        assert_eq!(before.audio_frames, 4);
        assert!(inspect_mp3_320(&mut Cursor::new(&input)).is_err());
        let mut output = Cursor::new(Vec::new());
        let receipt = normalize_audio_with_profile(
            &mut Cursor::new(&input),
            &mut output,
            AudioFormat::Mp3,
            Some(AudioSelection::new(profile)),
            &release(None)?.track_tags(1)?,
            &cover()?,
        )?;
        assert_eq!(receipt.audio_sha256, before.mpeg_sha256);
        assert_eq!(inspect_mp3(&mut output, profile)?.audio_frames, 4);
    }
    let mut input = frames(&[128, 128, 128, 128], false, false, 0, false);
    input[36..40].copy_from_slice(b"Xing");
    input[40..44].copy_from_slice(&3u32.to_be_bytes());
    input[44..48].copy_from_slice(&3u32.to_be_bytes());
    let size = input.len() as u32;
    input[48..52].copy_from_slice(&size.to_be_bytes());
    assert!(inspect_mp3(&mut Cursor::new(&input), profile).is_err());
    assert_eq!(
        inspect_mp3(&mut Cursor::new(&input), AudioProfile::Mp3Cbr128)?.metadata_frames,
        1
    );
    input[48..52].copy_from_slice(&(size + 1).to_be_bytes());
    assert!(inspect_mp3(&mut Cursor::new(&input), AudioProfile::Mp3Cbr128).is_err());
    Ok(())
}
