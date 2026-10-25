use super::*;

fn generated(progressive: bool, gray: bool) -> Result<Vec<u8>, turbojpeg::Error> {
    let rgb = RgbImage::from_fn(128, 128, |x, y| {
        let value = x
            .wrapping_mul(73)
            .wrapping_add(y.wrapping_mul(137))
            .to_le_bytes()[0];
        Rgb([value, value.wrapping_mul(23), value.wrapping_mul(71)])
    });
    let mut compressor = turbojpeg::Compressor::new()?;
    compressor.set_quality(90)?;
    compressor.set_subsamp(if gray {
        turbojpeg::Subsamp::Gray
    } else {
        turbojpeg::Subsamp::Sub2x2
    })?;
    compressor.set_progressive(progressive)?;
    compressor.compress_to_vec(turbojpeg::Image {
        pixels: rgb.as_raw().as_slice(),
        width: 128,
        pitch: 384,
        height: 128,
        format: turbojpeg::PixelFormat::RGB,
    })
}

fn native_decode(input: &[u8]) -> Result<Vec<u8>, turbojpeg::Error> {
    let mut decoder = turbojpeg::Decompressor::new()?;
    decoder.set_scan_limit(32)?;
    let header = decoder.read_header(input)?;
    assert_eq!((header.width, header.height), (128, 128));
    let mut output = vec![0; 128 * 128 * 3];
    decoder.decompress(
        input,
        turbojpeg::Image {
            pixels: output.as_mut_slice(),
            width: 128,
            pitch: 384,
            height: 128,
            format: turbojpeg::PixelFormat::RGB,
        },
    )?;
    Ok(output)
}

#[test]
fn artwork_native_jpeg_checks_each_scan_for_short_entropy_with_restored_end_marker() -> TestResult {
    for progressive in [false, true] {
        for gray in [false, true] {
            let input = generated(progressive, gray)?;
            native_decode(&input)?;
            let cover = normalize_cover(&input, None)?;
            assert_eq!(cover.dimensions(), (128, 128));
            assert_eq!(cover.bytes(), normalize_cover(&input, None)?.bytes());
            if gray {
                assert!(
                    image::load_from_memory(cover.bytes())?
                        .into_rgb8()
                        .pixels()
                        .all(|pixel| pixel.0[0].abs_diff(pixel.0[1]) <= 2
                            && pixel.0[1].abs_diff(pixel.0[2]) <= 2)
                );
            }
            assert!(native_decode(&input[..input.len() - 1]).is_err());
            rejected(&input[..input.len() - 1], "cover JPEG");
            for (start, end) in entropy_ranges(&input)? {
                for percent in [0, 10, 30, 50, 75] {
                    let cut = start + (end - start) * percent / 100;
                    let mut truncated = input[..cut].to_vec();
                    truncated.extend_from_slice(&[0xff, 0xd9]);
                    assert!(
                        native_decode(&truncated).is_err(),
                        "short entropy was repaired: progressive={progressive}, gray={gray}, scan={start}..{end}, cut={cut}"
                    );
                    rejected(&truncated, "cover JPEG");
                }
            }
        }
    }
    Ok(())
}

#[test]
fn artwork_jpeg_framing_profiles_modes_and_limits_are_checked_before_decode() -> TestResult {
    let input = generated(false, false)?;
    for suffix in [b"trailing".as_slice(), input.as_slice(), &[0xff, 0xd9]] {
        let mut trailing = input.clone();
        trailing.extend_from_slice(suffix);
        rejected(&trailing, "trailing data");
    }
    for length in [0, 1, 2, 3, 4, input.len() / 2] {
        rejected(&input[..length], "cover");
    }
    let mut comment = input.clone();
    comment.splice(2..2, segment(0xfe, b"not an end marker: \xff\xd9")?);
    normalize_cover(&comment, None)?;
    for position in [2, input.len() - 2] {
        let mut profiled = input.clone();
        profiled.splice(position..position, segment(0xe2, b"ICC_PROFILE\0\x01\x01")?);
        rejected(&profiled, "color-management");
    }
    let frame = input
        .windows(2)
        .position(|bytes| bytes == [0xff, 0xc0])
        .ok_or("fixture frame is missing")?;
    for (height, width, reason) in [
        (1u16, 8193u16, "dimensions"),
        (4097, 4097, "pixels"),
        (0, 1, "dimensions"),
    ] {
        let mut oversized = input.clone();
        oversized[frame + 5..frame + 7].copy_from_slice(&height.to_be_bytes());
        oversized[frame + 7..frame + 9].copy_from_slice(&width.to_be_bytes());
        rejected(&oversized, reason);
    }
    for precision in [0, 12, 16] {
        let mut unsupported = input.clone();
        unsupported[frame + 4] = precision;
        rejected(&unsupported, "precision");
    }
    let mut cmyk = input.clone();
    cmyk[frame + 9] = 4;
    rejected(&cmyk, "color space");
    for marker in [0xc1, 0xc3, 0xc9, 0xca, 0xcc, 0xdc, 0xd8] {
        let mut unsupported = input.clone();
        unsupported[frame + 1] = marker;
        rejected(&unsupported, "supported-image");
    }
    for length in [0u16, 1, u16::MAX] {
        let mut malformed = input.clone();
        malformed[frame + 2..frame + 4].copy_from_slice(&length.to_be_bytes());
        rejected(&malformed, "cover JPEG");
    }
    let (entropy, end) = entropy_ranges(&input)?[0];
    let scan = input[..entropy]
        .windows(2)
        .position(|pair| pair == [0xff, 0xda])
        .ok_or("fixture scan is missing")?;
    let mut many_scans = input[..scan].to_vec();
    for _ in 0..33 {
        many_scans.extend_from_slice(&input[scan..end]);
    }
    many_scans.extend_from_slice(&[0xff, 0xd9]);
    rejected(&many_scans, "32-scan limit");
    Ok(())
}

#[test]
fn artwork_exif_rejects_ambiguous_or_invalid_orientation_in_both_formats() -> TestResult {
    let image = DynamicImage::ImageRgb8(RgbImage::new(2, 1));
    let valid = exif_orientation(1);
    let mut invalids = vec![b"bad".to_vec(), valid[..12].to_vec()];
    for value in [0, 9, 255] {
        invalids.push(exif_orientation(value)[6..].to_vec());
    }
    let mut wrong_type = valid[6..].to_vec();
    wrong_type[12] = 4;
    invalids.push(wrong_type);
    let mut duplicate = valid[6..valid.len() - 4].to_vec();
    duplicate[8] = 2;
    duplicate.extend_from_slice(&valid[16..28]);
    duplicate.extend_from_slice(&[0; 4]);
    invalids.push(duplicate);
    let mut bad_offset = valid[6..].to_vec();
    bad_offset[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    invalids.push(bad_offset);
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        let original = encode(&image, format)?;
        let mut duplicate = original.clone();
        if format == ImageFormat::Png {
            duplicate.splice(33..33, png_chunk(b"eXIf", &valid[6..]).repeat(2));
        } else {
            duplicate.splice(2..2, segment(0xe1, &valid)?.repeat(2));
        }
        rejected(&duplicate, "ambiguous");
        for metadata in &invalids {
            let mut input = original.clone();
            if format == ImageFormat::Png {
                input.splice(33..33, png_chunk(b"eXIf", metadata));
            } else {
                let data = [b"Exif\0\0".as_slice(), metadata].concat();
                input.splice(2..2, segment(0xe1, &data)?);
            }
            rejected(&input, "EXIF");
        }
        for metadata in [
            b"II\x2a\0\x08\0\0\0\0\0\0\0\0\0".as_slice(),
            b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0".as_slice(),
        ] {
            let mut input = original.clone();
            if format == ImageFormat::Png {
                input.splice(33..33, png_chunk(b"eXIf", metadata));
            } else {
                input.splice(
                    2..2,
                    segment(0xe1, &[b"Exif\0\0".as_slice(), metadata].concat())?,
                );
            }
            let cover = normalize_cover(&input, None)?;
            assert_eq!(
                cover.dimensions(),
                if metadata.starts_with(b"MM") {
                    (1, 2)
                } else {
                    (2, 1)
                }
            );
        }
    }
    Ok(())
}

pub(super) fn segment(marker: u8, data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = vec![0xff, marker];
    bytes.extend_from_slice(&u16::try_from(data.len() + 2)?.to_be_bytes());
    bytes.extend_from_slice(data);
    Ok(bytes)
}

fn entropy_ranges(input: &[u8]) -> Result<Vec<(usize, usize)>, Box<dyn std::error::Error>> {
    let mut ranges = Vec::new();
    let mut position = 2;
    while position < input.len() {
        assert_eq!(input[position], 0xff);
        let marker = input[position + 1];
        if marker == 0xd9 {
            break;
        }
        let length = usize::from(u16::from_be_bytes([
            input[position + 2],
            input[position + 3],
        ]));
        position += 2 + length;
        if marker == 0xda {
            let start = position;
            while position + 1 < input.len() {
                if input[position] == 0xff && input[position + 1] != 0 {
                    break;
                }
                position += if input[position] == 0xff { 2 } else { 1 };
            }
            ranges.push((start, position));
        }
    }
    if ranges.is_empty() {
        return Err("generated JPEG has no entropy scans".into());
    }
    Ok(ranges)
}
