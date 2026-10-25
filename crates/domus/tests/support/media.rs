use id3::{Tag, TagLike, Version};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::io::Cursor;
use symphonia::core::checksum::{Crc8Ccitt, Crc16Ansi};
use symphonia::core::io::Monitor;

pub fn cover_source() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 20, Rgb([10, 100, 240])))
        .write_to(&mut png, ImageFormat::Png)?;
    Ok(png.into_inner())
}

pub fn mp3() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut tag = Tag::new();
    tag.set_title("remove old title");
    tag.set_genre("remove old genre");
    tag.add_frame(id3::frame::ExtendedText {
        description: "REPLAYGAIN_TRACK_GAIN".into(),
        value: "-99 dB".into(),
    });
    let mut bytes = Vec::new();
    tag.write_to(&mut bytes, Version::Id3v24)?;
    let mut frame = vec![0; 1044];
    frame[..4].copy_from_slice(&[0xff, 0xfb, 0xe0, 0]);
    for _ in 0..3 {
        bytes.extend_from_slice(&frame);
    }
    bytes.extend_from_slice(b"TAG");
    bytes.extend_from_slice(&[0; 125]);
    Ok(bytes)
}

pub fn flac() -> Vec<u8> {
    let mut info = [0; 34];
    info[..2].copy_from_slice(&16u16.to_be_bytes());
    info[2..4].copy_from_slice(&16u16.to_be_bytes());
    let packed = (44_100u64 << 44) | (15 << 36) | 16;
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    info[18..].copy_from_slice(&[
        0x70, 0xbc, 0x8f, 0x4b, 0x72, 0xa8, 0x69, 0x21, 0x46, 0x8b, 0xf8, 0xe8, 0x44, 0x1d, 0xce,
        0x51,
    ]);
    let mut old = metaflac::block::VorbisComment::new();
    old.comments
        .insert("COMMENT".into(), vec!["remove old comments".into()]);
    let old = old.to_bytes();
    let mut bytes = b"fLaC\x00\x00\x00\x22".to_vec();
    bytes.extend_from_slice(&info);
    bytes.push(0x84);
    bytes.extend_from_slice(&(old.len() as u32).to_be_bytes()[1..]);
    bytes.extend_from_slice(&old);
    let mut frame = vec![0xff, 0xf8, 0x60, 0, 0, 15];
    let mut header_crc = Crc8Ccitt::new(0);
    header_crc.process_buf_bytes(&frame);
    frame.push(header_crc.crc());
    frame.extend_from_slice(&[0, 0, 0]);
    let mut frame_crc = Crc16Ansi::new(0);
    frame_crc.process_buf_bytes(&frame);
    frame.extend_from_slice(&frame_crc.crc().to_be_bytes());
    bytes.extend(frame);
    bytes
}
