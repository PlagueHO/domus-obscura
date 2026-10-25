use domus::Error;
use domus::infrastructure::artwork::normalize_cover;
use image::{DynamicImage, ImageBuffer, ImageEncoder, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use sha2::{Digest, Sha256};
use std::io::Cursor;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[path = "artwork/native.rs"]
mod native;

fn encode(image: &DynamicImage, format: ImageFormat) -> Result<Vec<u8>, image::ImageError> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format)?;
    Ok(bytes.into_inner())
}

fn rejected(input: &[u8], reason: &str) {
    let matched = match normalize_cover(input, None) {
        Err(Error::InvalidInput(message)) => message.contains(reason),
        Err(Error::ArtworkInput(issue)) => issue.to_string().contains(reason),
        _ => false,
    };
    assert!(
        matched,
        "expected cover rejection containing: {reason}; format: {:?}; bytes: {}",
        image::guess_format(input),
        input.len()
    );
}

#[test]
fn artwork_dimensions_ratio_no_enlargement_and_one_payload() -> TestResult {
    for (width, height, expected) in [
        (1600, 800, (1200, 600)),
        (800, 1600, (600, 1200)),
        (1200, 1200, (1200, 1200)),
        (400, 200, (400, 200)),
        (1, 1, (1, 1)),
        (8192, 1, (1200, 1)),
        (1, 8192, (1, 1200)),
    ] {
        let image =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([10, 80, 140])));
        for format in [ImageFormat::Png, ImageFormat::Jpeg] {
            let input = encode(&image, format)?;
            let cover = normalize_cover(&input, None)?;
            assert_eq!(image::guess_format(cover.bytes())?, ImageFormat::Jpeg);
            let output = image::load_from_memory(cover.bytes())?;
            assert_eq!((output.width(), output.height()), expected);
            assert_eq!(cover.dimensions(), expected);
            let digest: [u8; 32] = Sha256::digest(cover.bytes()).into();
            assert_eq!(cover.sha256(), &digest);
            assert!(output.width() <= width && output.height() <= height);
            if width == 400 && height == 200 {
                assert_eq!(cover.bytes(), normalize_cover(&input, None)?.bytes());
            }
        }
    }
    let image = DynamicImage::ImageRgb8(RgbImage::new(1601, 901));
    let cover = normalize_cover(&encode(&image, ImageFormat::Png)?, None)?;
    let (width, height) = cover.dimensions();
    assert_eq!(width, 1200);
    assert!((f64::from(height) - f64::from(width) * 901.0 / 1601.0).abs() <= 1.0);
    Ok(())
}

#[test]
fn artwork_rejects_empty_oversized_unsupported_corrupt_and_truncated_input() -> TestResult {
    rejected(&[], "nonempty");
    rejected(&vec![0; 8 * 1024 * 1024 + 1], "8 MiB");
    rejected(b"not an image", "unknown");
    rejected(b"GIF89a", "supported-image");
    let input = encode(
        &DynamicImage::ImageRgb8(RgbImage::new(32, 16)),
        ImageFormat::Png,
    )?;
    rejected(&input[..input.len() / 2], "cover");
    rejected(&input[..input.len() - 1], "cover");
    let mut truncated = input[..input.len() / 2].to_vec();
    truncated.extend_from_slice(&input[input.len() - 12..]);
    rejected(&truncated, "cover");
    let mut trailing = input.clone();
    trailing.extend_from_slice(b"unexpected data");
    rejected(&trailing, "trailing data");
    for suffix in [input.as_slice(), &input[input.len() - 12..]] {
        let mut trailing = input.clone();
        trailing.extend_from_slice(suffix);
        rejected(&trailing, "trailing data");
    }
    let mut oversized_chunk = input.clone();
    oversized_chunk[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    rejected(&oversized_chunk, "truncated");
    let mut corrupt = input;
    corrupt[29] ^= 1;
    rejected(&corrupt, "header");
    Ok(())
}

#[test]
fn artwork_checks_side_pixels_and_decoded_bytes_before_pixel_decode() -> TestResult {
    let input = encode(
        &DynamicImage::ImageRgb8(RgbImage::new(1, 1)),
        ImageFormat::Png,
    )?;
    let mut header = input[16..29].to_vec();
    header[..4].copy_from_slice(&8193u32.to_be_bytes());
    let mut oversized = input.clone();
    oversized.splice(8..33, png_chunk(b"IHDR", &header));
    rejected(&oversized, "dimensions");
    header[..4].copy_from_slice(&4097u32.to_be_bytes());
    header[4..8].copy_from_slice(&4097u32.to_be_bytes());
    let mut oversized = input;
    oversized.splice(8..33, png_chunk(b"IHDR", &header));
    rejected(&oversized, "pixels");

    let image = DynamicImage::ImageRgba16(ImageBuffer::from_pixel(1, 1, Rgba([0, 0, 0, u16::MAX])));
    let mut input = encode(&image, ImageFormat::Png)?;
    let mut header = input[16..29].to_vec();
    header[..4].copy_from_slice(&4096u32.to_be_bytes());
    header[4..8].copy_from_slice(&4096u32.to_be_bytes());
    input.splice(8..33, png_chunk(b"IHDR", &header));
    rejected(&input, "decoded bytes");
    Ok(())
}

#[test]
fn artwork_transparency_needs_explicit_color_and_composites_before_resize() -> TestResult {
    let transparent = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([0, 255, 0, 128])));
    let input = encode(&transparent, ImageFormat::Png)?;
    rejected(&input, "background");
    let cover = normalize_cover(&input, Some([255, 255, 255]))?;
    let output = image::load_from_memory(cover.bytes())?.into_rgb8();
    assert_color(output.get_pixel(4, 4).0, [127, 255, 127]);

    let almost_opaque =
        DynamicImage::ImageRgba16(ImageBuffer::from_pixel(1, 1, Rgba([0, 0, 0, u16::MAX - 1])));
    rejected(&encode(&almost_opaque, ImageFormat::Png)?, "background");
    let opaque =
        DynamicImage::ImageRgba16(ImageBuffer::from_pixel(1, 1, Rgba([0, 0, 0, u16::MAX])));
    normalize_cover(&encode(&opaque, ImageFormat::Png)?, None)?;

    let image = RgbaImage::from_fn(1600, 8, |x, _| {
        if x % 2 == 0 {
            Rgba([0, 0, 0, 0])
        } else {
            Rgba([255, 255, 255, 255])
        }
    });
    let cover = normalize_cover(
        &encode(&DynamicImage::ImageRgba8(image), ImageFormat::Png)?,
        Some([255, 255, 255]),
    )?;
    assert!(
        image::load_from_memory(cover.bytes())?
            .into_rgb8()
            .pixels()
            .all(|pixel| pixel.0.iter().all(|value| *value >= 250))
    );
    Ok(())
}

#[test]
fn artwork_applies_all_eight_exif_orientations_before_or_after_image_data() -> TestResult {
    let colors = [[230, 20, 10], [20, 230, 10], [10, 20, 230], [230, 230, 10]];
    let image = DynamicImage::ImageRgb8(RgbImage::from_fn(60, 40, |x, y| {
        Rgb(colors[usize::from(y >= 20) * 2 + usize::from(x >= 30)])
    }));
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        let original = encode(&image, format)?;
        let positions = if format == ImageFormat::Png {
            [33, original.len() - 12]
        } else {
            [2, original.len() - 2]
        };
        for position in positions {
            for (index, (left, right)) in [
                (0, 1),
                (1, 0),
                (3, 2),
                (2, 3),
                (0, 2),
                (2, 0),
                (3, 1),
                (1, 3),
            ]
            .into_iter()
            .enumerate()
            {
                let orientation = u8::try_from(index + 1)?;
                let exif = exif_orientation(orientation);
                let mut input = original.clone();
                let metadata = if format == ImageFormat::Png {
                    png_chunk(b"eXIf", &exif[6..])
                } else {
                    native::segment(0xe1, &exif)?
                };
                input.splice(position..position, metadata);
                let cover = normalize_cover(&input, None)?;
                let output = image::load_from_memory(cover.bytes())?.into_rgb8();
                assert_eq!(
                    output.dimensions(),
                    if orientation >= 5 { (40, 60) } else { (60, 40) }
                );
                assert_color(output.get_pixel(4, 4).0, colors[left]);
                assert_color(output.get_pixel(output.width() - 5, 4).0, colors[right]);
                let mut decoder = image::ImageReader::new(Cursor::new(cover.bytes()))
                    .with_guessed_format()?
                    .into_decoder()?;
                assert_eq!(
                    image::ImageDecoder::orientation(&mut decoder)?,
                    image::metadata::Orientation::NoTransforms
                );
            }
        }
    }
    Ok(())
}

#[test]
fn artwork_profiles_and_animation_require_decisions() -> TestResult {
    let image = DynamicImage::ImageRgb8(RgbImage::new(2, 1));
    let mut input = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut input);
    encoder.set_icc_profile(vec![0; 128])?;
    encoder.write_image(
        image.as_bytes(),
        image.width(),
        image.height(),
        image.color().into(),
    )?;
    rejected(&input, "color-management");

    let mut input = encode(&image, ImageFormat::Png)?;
    let animation = png_chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]);
    let mut frame = vec![0; 26];
    frame[4..8].copy_from_slice(&2u32.to_be_bytes());
    frame[8..12].copy_from_slice(&1u32.to_be_bytes());
    frame[21] = 1;
    frame[23] = 1;
    input.splice(
        33..33,
        animation.into_iter().chain(png_chunk(b"fcTL", &frame)),
    );
    rejected(&input, "animated");
    Ok(())
}

fn assert_color(actual: [u8; 3], expected: [u8; 3]) {
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 8),
        "{actual:?} differs from {expected:?}"
    );
}

fn exif_orientation(value: u8) -> Vec<u8> {
    let mut bytes = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    bytes.extend_from_slice(&[value, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::new();
    chunk.extend_from_slice(
        &u32::try_from(data.len())
            .unwrap_or_else(|_| panic!("fixture chunk is too large"))
            .to_be_bytes(),
    );
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(data);
    let mut crc = u32::MAX;
    for byte in &chunk[4..] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    chunk.extend_from_slice(&(!crc).to_be_bytes());
    chunk
}
