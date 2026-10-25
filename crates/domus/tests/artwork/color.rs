use super::*;
use std::io::Write;

fn png_profile(
    image: &DynamicImage,
    profile: Vec<u8>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    encoder.set_icc_profile(profile)?;
    encoder.write_image(
        image.as_bytes(),
        image.width(),
        image.height(),
        image.color().into(),
    )?;
    Ok(bytes)
}

fn jpeg_profile(
    input: &[u8],
    chunks: &[(u8, u8, &[u8])],
    position: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut result = input.to_vec();
    let mut segments = Vec::new();
    for &(number, count, data) in chunks {
        let length = u16::try_from(16 + data.len())?;
        segments.extend_from_slice(&[0xff, 0xe2]);
        segments.extend_from_slice(&length.to_be_bytes());
        segments.extend_from_slice(b"ICC_PROFILE\0");
        segments.extend_from_slice(&[number, count]);
        segments.extend_from_slice(data);
    }
    result.splice(position..position, segments);
    Ok(result)
}

#[test]
fn artwork_converts_rgb_profiles_in_png_and_split_jpeg_without_changing_unprofiled_inputs()
-> TestResult {
    let input = DynamicImage::ImageRgb8(RgbImage::from_pixel(24, 16, Rgb([120, 140, 180])));
    let profile = icc::profile(moxcms::ColorProfile::new_adobe_rgb())?;
    let jpeg = encode(&input, ImageFormat::Jpeg)?;
    let split = profile.len() / 2;
    for bytes in [
        png_profile(&input, profile.clone())?,
        jpeg_profile(&jpeg, &[(1, 1, &profile)], 2)?,
        jpeg_profile(
            &jpeg,
            &[(2, 2, &profile[split..]), (1, 2, &profile[..split])],
            2,
        )?,
        jpeg_profile(&jpeg, &[(1, 1, &profile)], jpeg.len() - 2)?,
    ] {
        let cover = normalize_cover(&bytes, None)?;
        assert_eq!(cover.dimensions(), (24, 16));
        let output = image::load_from_memory(cover.bytes())?.into_rgb8();
        // Adobe RGB (1998) matrix and gamma mapped to sRGB, not the original RGB bytes.
        assert_color(output.get_pixel(12, 8).0, [111, 141, 183]);
        assert_eq!(cover.bytes(), normalize_cover(&bytes, None)?.bytes());
        let mut decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(cover.bytes()))?;
        assert!(image::ImageDecoder::icc_profile(&mut decoder)?.is_none());
    }
    for format in [ImageFormat::Jpeg, ImageFormat::Png] {
        let cover = normalize_cover(&encode(&input, format)?, None)?;
        assert_color(
            image::load_from_memory(cover.bytes())?
                .into_rgb8()
                .get_pixel(12, 8)
                .0,
            [120, 140, 180],
        );
    }
    Ok(())
}

#[test]
fn artwork_color_conversion_preserves_gray_alpha_and_sixteen_bit_inputs() -> TestResult {
    let rgb16 = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(
        8,
        8,
        Rgb([80u16 * 257, 140 * 257, 180 * 257]),
    ));
    let cover = normalize_cover(
        &png_profile(&rgb16, icc::profile(moxcms::ColorProfile::new_adobe_rgb())?)?,
        None,
    )?;
    assert_color(
        image::load_from_memory(cover.bytes())?
            .into_rgb8()
            .get_pixel(4, 4)
            .0,
        [9, 141, 183],
    );
    let transparent =
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([80, 140, 180, 128])));
    let bytes = png_profile(
        &transparent,
        icc::profile(moxcms::ColorProfile::new_adobe_rgb())?,
    )?;
    rejected(&bytes, "background");
    let cover = normalize_cover(&bytes, Some([255, 255, 255]))?;
    assert_color(
        image::load_from_memory(cover.bytes())?
            .into_rgb8()
            .get_pixel(4, 4)
            .0,
        [132, 198, 219],
    );

    let gray = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(8, 8, image::Luma([128])));
    let profile = icc::profile(moxcms::ColorProfile::new_gray_with_gamma(1.0))?;
    for bytes in [
        png_profile(&gray, profile.clone())?,
        jpeg_profile(&encode(&gray, ImageFormat::Jpeg)?, &[(1, 1, &profile)], 2)?,
    ] {
        let cover = normalize_cover(&bytes, None)?;
        assert_color(
            image::load_from_memory(cover.bytes())?
                .into_rgb8()
                .get_pixel(4, 4)
                .0,
            [188, 188, 188],
        );
    }
    Ok(())
}

#[test]
fn artwork_rejects_malformed_incomplete_duplicate_and_oversized_color_profiles() -> TestResult {
    let input = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([80, 140, 180])));
    let profile = icc::profile(moxcms::ColorProfile::new_srgb())?;
    let jpeg = encode(&input, ImageFormat::Jpeg)?;
    for chunks in [
        vec![(0, 1, profile.as_slice())],
        vec![(1, 0, profile.as_slice())],
        vec![(2, 1, profile.as_slice())],
        vec![(1, 2, profile.as_slice())],
        vec![(1, 1, profile.as_slice()), (1, 1, profile.as_slice())],
        vec![(1, 2, profile.as_slice()), (2, 3, profile.as_slice())],
    ] {
        rejected(&jpeg_profile(&jpeg, &chunks, 2)?, "ICC profile");
    }
    let mut malformed = Vec::new();
    malformed.push(vec![0; 128]);
    malformed.push(profile[..profile.len() - 1].to_vec());
    let mut trailing = profile.clone();
    trailing.push(0);
    malformed.push(trailing);
    for (offset, value) in [(128, 129u32), (136, u32::MAX), (140, u32::MAX)] {
        let mut bytes = profile.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        malformed.push(bytes);
    }
    let mut duplicate = profile.clone();
    let signature = duplicate[132..136].to_vec();
    duplicate[144..148].copy_from_slice(&signature);
    malformed.push(duplicate);
    for bytes in malformed {
        rejected(&png_profile(&input, bytes.clone())?, "ICC profile");
        rejected(&jpeg_profile(&jpeg, &[(1, 1, &bytes)], 2)?, "ICC profile");
    }
    let oversized = vec![0; 1024 * 1024 + 1];
    rejected(&png_profile(&input, oversized.clone())?, "profile");
    let chunks = oversized.chunks(60000).collect::<Vec<_>>();
    let numbered = chunks
        .iter()
        .enumerate()
        .map(|(index, bytes)| {
            Ok((
                u8::try_from(index + 1)?,
                u8::try_from(chunks.len())?,
                *bytes,
            ))
        })
        .collect::<Result<Vec<_>, std::num::TryFromIntError>>()?;
    rejected(&jpeg_profile(&jpeg, &numbered, 2)?, "ICC profile");
    Ok(())
}

#[test]
fn artwork_png_profile_compression_checksums_order_and_color_space_are_checked() -> TestResult {
    let input = DynamicImage::ImageRgb8(RgbImage::new(8, 8));
    let profile = icc::profile(moxcms::ColorProfile::new_srgb())?;
    let png = encode(&input, ImageFormat::Png)?;
    let mut compressed =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    compressed.write_all(&profile)?;
    let compressed = compressed.finish()?;
    let data = [b"Profile\0\0".as_slice(), &compressed].concat();
    let chunk = png_chunk(b"iCCP", &data);
    let mut good = png.clone();
    good.splice(33..33, chunk.clone());
    normalize_cover(&good, None)?;
    let mut crc = good.clone();
    crc[33 + chunk.len() - 1] ^= 1;
    rejected(&crc, "checksum");
    let mut duplicate = good.clone();
    duplicate.splice(33..33, chunk.clone());
    rejected(&duplicate, "repeated");
    let mut late = png.clone();
    late.splice(late.len() - 12..late.len() - 12, chunk);
    rejected(&late, "late");
    let mut trailing_data = data.clone();
    trailing_data.push(0);
    let mut trailing = png.clone();
    trailing.splice(33..33, png_chunk(b"iCCP", &trailing_data));
    rejected(&trailing, "trailing data");
    for removed in [1, 4, compressed.len() / 2] {
        let mut truncated = png.clone();
        let data = [
            b"Profile\0\0".as_slice(),
            &compressed[..compressed.len() - removed],
        ]
        .concat();
        truncated.splice(33..33, png_chunk(b"iCCP", &data));
        rejected(&truncated, "truncated");
    }
    let mut corrupt_data = data;
    let end = corrupt_data.len() - 1;
    corrupt_data[end] ^= 1;
    let mut corrupt = png;
    corrupt.splice(33..33, png_chunk(b"iCCP", &corrupt_data));
    rejected(&corrupt, "compression");
    rejected(
        &png_profile(
            &input,
            icc::profile(moxcms::ColorProfile::new_gray_with_gamma(2.2))?,
        )?,
        "color space",
    );
    Ok(())
}
