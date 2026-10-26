use crate::domain::ArtworkIssue;
use crate::{Error, Result};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngDecoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, Limits, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};

mod color;
mod jpeg;

const MAX_COMPRESSED_BYTES: usize = 8 * 1024 * 1024;
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const OUTPUT_SIDE: u32 = 1200;
pub(crate) const COVER_PROFILE: u8 = 1;

/// One immutable encoding for both embedded covers and folder.jpg. Callers
/// must not independently resize or re-encode this payload for each track.
pub struct CoverJpeg {
    bytes: Vec<u8>,
    sha256: [u8; 32],
    dimensions: (u32, u32),
}

impl CoverJpeg {
    pub(crate) fn restore(
        bytes: Vec<u8>,
        sha256: [u8; 32],
        dimensions: (u32, u32),
    ) -> Result<Self> {
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        if bytes.is_empty()
            || bytes.len() > MAX_COMPRESSED_BYTES
            || actual != sha256
            || dimensions.0 == 0
            || dimensions.1 == 0
            || dimensions.0 > OUTPUT_SIDE
            || dimensions.1 > OUTPUT_SIDE
        {
            return Err(invalid(
                "saved cover bytes or dimensions differ from their receipt",
            ));
        }
        let image = jpeg::decode(&bytes)?;
        if (image.width(), image.height()) != dimensions {
            return Err(invalid("saved cover dimensions differ from the JPEG"));
        }
        Ok(Self {
            bytes,
            sha256,
            dimensions,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.dimensions
    }
}

/// Shared image normalization for provider and local inputs, independent of
/// album plans or UI state. CPU/blocking work belongs on a bounded worker.
/// Valid RGB/grayscale ICC profiles are converted to sRGB before compositing.
/// Other unsupported inputs and transparent pixels without an explicit
/// background return errors instead of substituting content.
pub fn normalize_cover(input: &[u8], background: Option<[u8; 3]>) -> Result<CoverJpeg> {
    normalize_cover_for_size(input, background, None)
}

pub fn normalize_cover_for_size(
    input: &[u8],
    background: Option<[u8; 3]>,
    size: Option<crate::domain::ArtworkSize>,
) -> Result<CoverJpeg> {
    if input.is_empty() || input.len() > MAX_COMPRESSED_BYTES {
        return Err(invalid("cover must be nonempty and no larger than 8 MiB"));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    let image = match image::guess_format(input).map_err(|_| invalid("cover format is unknown"))? {
        ImageFormat::Jpeg => jpeg::decode(input)?,
        ImageFormat::Png => {
            let (orientation, profile) = verify_png(input)?;
            let decoder = PngDecoder::with_limits(Cursor::new(input), limits)
                .map_err(|_| invalid("cover PNG header is invalid or exceeds decoder limits"))?;
            let image = decode(decoder, orientation)?;
            match profile {
                Some(profile) => profile.convert(image)?,
                None => image,
            }
        }
        _ => return Err(ArtworkIssue::Unsupported.into()),
    };
    if background.is_none() && has_transparency(&image) {
        return Err(ArtworkIssue::Background.into());
    }
    let rgba = image.into_rgba8();
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (source, destination) in rgba.pixels().zip(rgb.pixels_mut()) {
        let alpha = u32::from(source.0[3]);
        let mut channels = [0u8; 3];
        for (index, channel) in channels.iter_mut().enumerate() {
            let color = u32::from(source.0[index]);
            let backdrop = background.map_or(0, |value| u32::from(value[index]));
            let blended = (color * alpha + backdrop * (255 - alpha) + 127) / 255;
            *channel = u8::try_from(blended)
                .map_err(|_| Error::Internal("cover color conversion exceeded its range"))?;
        }
        *destination = Rgb(channels);
    }
    drop(rgba);
    // Composite before filtering so hidden colors in transparent pixels cannot
    // bleed into the resized cover.
    let output_side = size.map_or(OUTPUT_SIDE, crate::domain::ArtworkSize::target_side);
    let rgb = if rgb.width() > output_side || rgb.height() > output_side {
        DynamicImage::ImageRgb8(rgb)
            .resize(output_side, output_side, FilterType::Lanczos3)
            .into_rgb8()
    } else {
        rgb
    };
    let dimensions = rgb.dimensions();
    let mut output = BoundedImage(Vec::new());
    JpegEncoder::new_with_quality(&mut output, 90)
        .encode_image(&rgb)
        .map_err(|_| invalid("cover JPEG encoding failed or exceeded its size limit"))?;
    Ok(CoverJpeg {
        sha256: Sha256::digest(&output.0).into(),
        bytes: output.0,
        dimensions,
    })
}

fn validate_dimensions(width: u32, height: u32, bytes_per_pixel: u64) -> Result<()> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || pixels > MAX_PIXELS
        || pixels
            .checked_mul(bytes_per_pixel)
            .is_none_or(|bytes| bytes > MAX_DECODED_BYTES)
    {
        return Err(invalid(
            "cover dimensions, pixels or decoded bytes exceed their limit",
        ));
    }
    Ok(())
}

fn verify_png(input: &[u8]) -> Result<(Orientation, Option<color::IccProfile>)> {
    let profile = verify_png_container(input)?;
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(input),
        png::Limits {
            bytes: usize::try_from(MAX_DECODED_BYTES)
                .map_err(|_| Error::Internal("cover allocation limit is unsupported"))?,
        },
    );
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let header = decoder
        .read_header_info()
        .map_err(|_| invalid("cover PNG header is invalid or exceeds decoder limits"))?;
    let channels = match header.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba | png::ColorType::Indexed => 4,
    };
    let bytes_per_channel = if header.bit_depth == png::BitDepth::Sixteen {
        2
    } else {
        1
    };
    validate_dimensions(header.width, header.height, channels * bytes_per_channel)?;
    let mut reader = decoder
        .read_info()
        .map_err(|_| invalid("cover PNG metadata is invalid"))?;
    if reader.info().animation_control.is_some() {
        return Err(ArtworkIssue::Animated.into());
    }
    reader
        .finish()
        .map_err(|_| invalid("cover PNG is corrupt or truncated"))?;
    let orientation = reader
        .info()
        .exif_metadata
        .as_deref()
        .map(verify_exif)
        .transpose()
        .map(|orientation| orientation.unwrap_or(Orientation::NoTransforms))?;
    Ok((orientation, profile))
}

fn verify_png_container(input: &[u8]) -> Result<Option<color::IccProfile>> {
    let error = || invalid("cover PNG is truncated or has trailing data");
    let mut position = 8usize;
    let mut exif = false;
    let mut profile = None;
    let mut image_data = false;
    let mut srgb = false;
    loop {
        let header = input
            .get(position..)
            .and_then(|tail| tail.get(..8))
            .ok_or_else(error)?;
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let end = usize::try_from(length)
            .ok()
            .and_then(|length| position.checked_add(length)?.checked_add(12))
            .filter(|end| *end <= input.len())
            .ok_or_else(error)?;
        match &header[4..] {
            b"IEND" => {
                if length != 0 || end != input.len() {
                    return Err(error());
                }
                return Ok(profile);
            }
            b"eXIf" => {
                if exif {
                    return Err(invalid("cover EXIF orientation metadata is ambiguous"));
                }
                exif = true;
            }
            b"PLTE" | b"IDAT" => image_data = true,
            b"sRGB" => {
                if profile.is_some() {
                    return Err(invalid("image PNG has conflicting color profiles"));
                }
                srgb = true;
            }
            b"iCCP" => {
                if image_data || srgb || profile.is_some() {
                    return Err(invalid(
                        "image PNG color profile is repeated, late or conflicting",
                    ));
                }
                let chunk = &input[position + 4..end - 4];
                let mut crc = u32::MAX;
                for byte in chunk {
                    crc ^= u32::from(*byte);
                    for _ in 0..8 {
                        crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
                    }
                }
                if (!crc).to_be_bytes() != input[end - 4..end] {
                    return Err(invalid("image PNG color profile checksum is invalid"));
                }
                let data = &input[position + 8..end - 4];
                let name_end = data
                    .iter()
                    .position(|byte| *byte == 0)
                    .ok_or_else(|| invalid("image PNG color profile name is invalid"))?;
                if !(1..=79).contains(&name_end)
                    || data.get(name_end + 1) != Some(&0)
                    || data[..name_end]
                        .iter()
                        .any(|byte| !matches!(byte, 32..=126 | 161..=255))
                    || data[0] == b' '
                    || data[name_end - 1] == b' '
                    || data[..name_end].windows(2).any(|pair| pair == b"  ")
                {
                    return Err(invalid("image PNG color profile header is invalid"));
                }
                let compressed = &data[name_end + 2..];
                let mut decoder = flate2::Decompress::new(true);
                let mut bytes = Vec::with_capacity(color::MAX_ICC_BYTES + 1);
                let status = decoder
                    .decompress_vec(compressed, &mut bytes, flate2::FlushDecompress::Finish)
                    .map_err(|_| invalid("image PNG color profile compression is invalid"))?;
                if status != flate2::Status::StreamEnd
                    || bytes.len() > color::MAX_ICC_BYTES
                    || decoder.total_in() != compressed.len() as u64
                {
                    return Err(invalid(
                        "image PNG color profile exceeds its limit, is truncated or has trailing data",
                    ));
                }
                profile = Some(color::IccProfile::parse(&bytes)?);
            }
            _ => {}
        }
        position = end;
    }
}

fn verify_exif(input: &[u8]) -> Result<Orientation> {
    let error = || invalid("cover EXIF orientation metadata is invalid or ambiguous");
    let little = match input.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err(error()),
    };
    let read16 = |position: usize| -> Result<u16> {
        let bytes = input
            .get(position..)
            .and_then(|tail| tail.get(..2))
            .ok_or_else(error)?;
        let bytes = [bytes[0], bytes[1]];
        Ok(if little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    let read32 = |position: usize| -> Result<u32> {
        let bytes = input
            .get(position..)
            .and_then(|tail| tail.get(..4))
            .ok_or_else(error)?;
        let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
        Ok(if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    };
    if read16(2)? != 42 {
        return Err(error());
    }
    let offset = usize::try_from(read32(4)?).map_err(|_| error())?;
    if offset == 0 {
        return Ok(Orientation::NoTransforms);
    }
    if offset < 8 {
        return Err(error());
    }
    let count = usize::from(read16(offset)?);
    let start = offset + 2;
    let bytes = count * 12;
    if input
        .get(start..)
        .and_then(|tail| tail.get(..bytes + 4))
        .is_none()
    {
        return Err(error());
    }
    let mut orientation = None;
    for index in 0..count {
        let entry = start + index * 12;
        if read16(entry)? == 0x0112 {
            if orientation.is_some() || read16(entry + 2)? != 3 || read32(entry + 4)? != 1 {
                return Err(error());
            }
            let value = u8::try_from(read16(entry + 8)?).map_err(|_| error())?;
            orientation = Some(Orientation::from_exif(value).ok_or_else(error)?);
        }
    }
    Ok(orientation.unwrap_or(Orientation::NoTransforms))
}

fn decode(decoder: impl ImageDecoder, orientation: Orientation) -> Result<DynamicImage> {
    let (width, height) = decoder.dimensions();
    validate_dimensions(
        width,
        height,
        u64::from(decoder.color_type().bytes_per_pixel()),
    )?;
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|_| invalid("cover image is corrupt or truncated"))?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn has_transparency(image: &DynamicImage) -> bool {
    match image {
        DynamicImage::ImageRgba8(image) => image.pixels().any(|pixel| pixel.0[3] != u8::MAX),
        DynamicImage::ImageLumaA8(image) => image.pixels().any(|pixel| pixel.0[1] != u8::MAX),
        DynamicImage::ImageRgba16(image) => image.pixels().any(|pixel| pixel.0[3] != u16::MAX),
        DynamicImage::ImageLumaA16(image) => image.pixels().any(|pixel| pixel.0[1] != u16::MAX),
        _ => image.color().has_alpha(),
    }
}

fn invalid(message: &str) -> Error {
    Error::InvalidInput(message.into())
}

struct BoundedImage(Vec<u8>);

impl Write for BoundedImage {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_COMPRESSED_BYTES - self.0.len() {
            return Err(std::io::Error::other("encoded cover exceeds 8 MiB"));
        }

        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_encoder_buffer_enforces_exact_limit() -> std::io::Result<()> {
        let mut output = BoundedImage(Vec::new());
        output.write_all(&vec![0; MAX_COMPRESSED_BYTES - 1])?;
        output.write_all(&[1])?;
        assert!(output.write_all(&[2]).is_err());
        assert_eq!(output.0.len(), MAX_COMPRESSED_BYTES);
        assert_eq!(output.0.last(), Some(&1));
        Ok(())
    }
}
