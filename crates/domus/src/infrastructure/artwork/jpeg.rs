use super::color::{IccProfile, MAX_ICC_BYTES};
use super::{invalid, validate_dimensions, verify_exif};
use crate::domain::ArtworkIssue;
use crate::{Error, Result};
use image::metadata::Orientation;
use image::{DynamicImage, RgbImage};
use turbojpeg::{Colorspace, Decompressor, Image, PixelFormat};

const MAX_SCANS: u32 = 32;

pub(super) fn decode(input: &[u8]) -> Result<DynamicImage> {
    let VerifiedJpeg {
        width,
        height,
        orientation,
        profile,
    } = verify_container(input)?;
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
    let image = if header.colorspace == Colorspace::Gray && profile.is_some() {
        let mut gray = image::GrayImage::new(width, height);
        decoder
            .decompress(
                input,
                Image {
                    pixels: gray.as_mut(),
                    width: header.width,
                    pitch: header.width,
                    height: header.height,
                    format: PixelFormat::GRAY,
                },
            )
            .map_err(|_| invalid("cover JPEG is corrupt or truncated"))?;
        DynamicImage::ImageLuma8(gray)
    } else {
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
        DynamicImage::ImageRgb8(rgb)
    };
    let mut image = match profile {
        Some(profile) => profile.convert(image)?,
        None => image,
    };
    image.apply_orientation(orientation);
    Ok(image)
}

/// Check framing and limits before native decoding. The native decoder must
/// also finish without warnings; an EOI marker alone cannot prove completeness.
struct VerifiedJpeg {
    width: u32,
    height: u32,
    orientation: Orientation,
    profile: Option<IccProfile>,
}

fn verify_container(input: &[u8]) -> Result<VerifiedJpeg> {
    if !input.starts_with(&[0xff, 0xd8]) {
        return Err(invalid("cover JPEG start marker is invalid"));
    }
    let mut position = 2;
    let mut frame = None;
    let mut orientation = None;
    let mut scans = 0;
    let mut icc_chunks: Vec<Option<&[u8]>> = Vec::new();
    let mut icc_bytes = 0usize;
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
            let profile = if icc_chunks.is_empty() {
                None
            } else {
                let mut bytes = Vec::with_capacity(icc_bytes);
                for chunk in icc_chunks {
                    bytes.extend_from_slice(
                        chunk.ok_or_else(|| invalid("image JPEG ICC profile is incomplete"))?,
                    );
                }
                Some(IccProfile::parse(&bytes)?)
            };
            return Ok(VerifiedJpeg {
                width,
                height,
                orientation: orientation.unwrap_or(Orientation::NoTransforms),
                profile,
            });
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
                if data.len() <= 14 || data[12] == 0 || data[13] == 0 || data[12] > data[13] {
                    return Err(invalid("image JPEG ICC profile chunk is invalid"));
                }
                let count = usize::from(data[13]);
                if icc_chunks.is_empty() {
                    icc_chunks.resize(count, None);
                }
                let slot = usize::from(data[12] - 1);
                icc_bytes += data.len() - 14;
                if icc_chunks.len() != count
                    || icc_chunks[slot].is_some()
                    || icc_bytes > MAX_ICC_BYTES
                {
                    return Err(invalid(
                        "image JPEG ICC profile is repeated, inconsistent or exceeds its limit",
                    ));
                }
                icc_chunks[slot] = Some(&data[14..]);
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
