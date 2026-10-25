use super::{invalid, validate_dimensions, verify_exif};
use crate::domain::ArtworkIssue;
use crate::{Error, Result};
use image::metadata::Orientation;
use image::{DynamicImage, RgbImage};
use turbojpeg::{Colorspace, Decompressor, Image, PixelFormat};

const MAX_SCANS: u32 = 32;

pub(super) fn decode(input: &[u8]) -> Result<DynamicImage> {
    let (width, height, orientation) = verify_container(input)?;
    let mut decoder = Decompressor::new()
        .map_err(|_| Error::Internal("cover JPEG decoder initialization failed"))?;
    decoder
        .set_scan_limit(MAX_SCANS)
        .map_err(|_| Error::Internal("cover JPEG scan limit could not be set"))?;
    let header = decoder
        .read_header(input)
        .map_err(|_| invalid("cover JPEG header is invalid"))?;
    if header.width != usize::try_from(width).map_err(|_| invalid("cover JPEG width is invalid"))?
        || header.height
            != usize::try_from(height).map_err(|_| invalid("cover JPEG height is invalid"))?
        || header.is_lossless
        || header.is_arithmetic
        || !matches!(
            header.colorspace,
            Colorspace::RGB | Colorspace::YCbCr | Colorspace::Gray
        )
    {
        return Err(ArtworkIssue::Unsupported.into());
    }
    let mut rgb = RgbImage::new(width, height);
    decoder
        .decompress(
            input,
            Image {
                pixels: rgb.as_mut(),
                width: header.width,
                pitch: header.width * 3,
                height: header.height,
                format: PixelFormat::RGB,
            },
        )
        .map_err(|_| invalid("cover JPEG is corrupt or truncated"))?;
    let mut image = DynamicImage::ImageRgb8(rgb);
    image.apply_orientation(orientation);
    Ok(image)
}

/// Check framing and limits before native decoding. The native decoder must
/// also finish without warnings; an EOI marker alone cannot prove completeness.
fn verify_container(input: &[u8]) -> Result<(u32, u32, Orientation)> {
    if !input.starts_with(&[0xff, 0xd8]) {
        return Err(invalid("cover JPEG start marker is invalid"));
    }
    let mut position = 2;
    let mut frame = None;
    let mut orientation = None;
    let mut scans = 0;
    loop {
        let marker = next_marker(input, &mut position)?;
        if marker == 0xd9 {
            if position != input.len() {
                return Err(invalid("cover JPEG has trailing data or multiple images"));
            }
            let (width, height) = frame.ok_or_else(|| invalid("cover JPEG frame is missing"))?;
            if scans == 0 {
                return Err(invalid("cover JPEG scan is missing"));
            }
            return Ok((
                width,
                height,
                orientation.unwrap_or(Orientation::NoTransforms),
            ));
        }
        if !matches!(
            marker,
            0xc0 | 0xc2 | 0xc4 | 0xda | 0xdb | 0xdd | 0xe0..=0xef | 0xfe
        ) {
            return Err(ArtworkIssue::Unsupported.into());
        }
        let length = input
            .get(position..position + 2)
            .ok_or_else(|| invalid("cover JPEG segment is truncated"))?;
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        if length < 2 {
            return Err(invalid("cover JPEG segment length is invalid"));
        }
        let data = input
            .get(position + 2..position + length)
            .ok_or_else(|| invalid("cover JPEG segment is truncated"))?;
        position += length;
        match marker {
            0xc0 | 0xc2 => {
                if frame.is_some() || data.len() < 6 {
                    return Err(invalid("cover JPEG frame is invalid or repeated"));
                }
                if data[0] != 8 || !matches!(data[5], 1 | 3) {
                    return Err(ArtworkIssue::Unsupported.into());
                }
                if data.len() != 6 + usize::from(data[5]) * 3 {
                    return Err(invalid("cover JPEG frame length is invalid"));
                }
                let height = u32::from(u16::from_be_bytes([data[1], data[2]]));
                let width = u32::from(u16::from_be_bytes([data[3], data[4]]));
                validate_dimensions(width, height, 3)?;
                frame = Some((width, height));
            }
            0xda => {
                if frame.is_none() || data.is_empty() {
                    return Err(invalid("cover JPEG scan has no valid frame"));
                }
                scans += 1;
                if scans > MAX_SCANS {
                    return Err(invalid("cover JPEG exceeds its 32-scan limit"));
                }
                skip_entropy(input, &mut position)?;
            }
            0xe1 if data.starts_with(b"Exif\0\0") => {
                if orientation.is_some() {
                    return Err(invalid("cover EXIF orientation metadata is ambiguous"));
                }
                orientation = Some(verify_exif(&data[6..])?);
            }
            0xe2 if data.starts_with(b"ICC_PROFILE\0") => {
                return Err(ArtworkIssue::ColorProfile.into());
            }
            _ => {}
        }
    }
}

fn next_marker(input: &[u8], position: &mut usize) -> Result<u8> {
    if input.get(*position) != Some(&0xff) {
        return Err(invalid("cover JPEG marker is missing or truncated"));
    }
    while input.get(*position) == Some(&0xff) {
        *position += 1;
    }
    let marker = *input
        .get(*position)
        .ok_or_else(|| invalid("cover JPEG marker is truncated"))?;
    *position += 1;
    Ok(marker)
}

fn skip_entropy(input: &[u8], position: &mut usize) -> Result<()> {
    while let Some(byte) = input.get(*position) {
        if *byte != 0xff {
            *position += 1;
            continue;
        }
        let start = *position;
        let marker = next_marker(input, position)?;
        if !matches!(marker, 0 | 0xd0..=0xd7) {
            *position = start;
            return Ok(());
        }
    }
    Err(invalid("cover JPEG entropy data is truncated"))
}
