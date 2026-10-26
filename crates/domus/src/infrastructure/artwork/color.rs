use super::{invalid, validate_dimensions};
use crate::Error;
use crate::Result;
use crate::domain::ArtworkIssue;
use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage};
use moxcms::{
    CmsError, ColorProfile, DataColorSpace, Layout, ParsingOptions, ProfileClass, TransformOptions,
};
use std::collections::HashSet;

pub(super) const MAX_ICC_BYTES: usize = 1024 * 1024;
const MAX_TAGS: usize = 128;

pub(super) struct IccProfile(ColorProfile);

impl IccProfile {
    /// Validate framing before parsing. Bound profile bytes, tag count and each
    /// transform table independently of the compressed image's size.
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        let malformed = || invalid("image ICC profile is malformed or exceeds its limits");
        let word = |offset: usize| -> Result<usize> {
            let data = bytes.get(offset..offset + 4).ok_or_else(malformed)?;
            usize::try_from(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
                .map_err(|_| malformed())
        };
        if bytes.len() < 132
            || bytes.len() > MAX_ICC_BYTES
            || word(0)? != bytes.len()
            || bytes.get(36..40) != Some(b"acsp")
        {
            return Err(invalid("image ICC profile header or byte size is invalid"));
        }
        let count = word(128)?;
        if count == 0 || count > MAX_TAGS || 132 + count * 12 > bytes.len() {
            return Err(invalid("image ICC profile tag count exceeds its limit"));
        }
        let mut signatures = HashSet::new();
        let mut ranges = Vec::new();
        for index in 0..count {
            let tag = 132 + index * 12;
            let start = word(tag + 4)?;
            let length = word(tag + 8)?;
            let end = start.checked_add(length).ok_or_else(malformed)?;
            if !signatures.insert(word(tag)?)
                || start < 132 + count * 12
                || !start.is_multiple_of(4)
                || length < 8
                || end > bytes.len()
                || ranges.iter().any(|&(other_start, other_end)| {
                    start < other_end
                        && other_start < end
                        && (start, end) != (other_start, other_end)
                })
            {
                return Err(invalid(
                    "image ICC profile tag is invalid, repeated or overlapping",
                ));
            }
            ranges.push((start, end));
        }
        let profile = ColorProfile::new_from_slice_with_options(
            bytes,
            ParsingOptions {
                max_profile_size: MAX_ICC_BYTES + 1,
                max_allowed_clut_size: 65536,
                max_allowed_trc_size: 65536,
            },
        )
        .map_err(|_| malformed())?;
        if !matches!(
            profile.color_space,
            DataColorSpace::Rgb | DataColorSpace::Gray
        ) || !matches!(
            profile.profile_class,
            ProfileClass::DisplayDevice | ProfileClass::InputDevice | ProfileClass::OutputDevice
        ) {
            return Err(ArtworkIssue::ColorProfile.into());
        }
        Ok(Self(profile))
    }

    /// Convert rows before alpha compositing or resizing. Preserve 16-bit PNG
    /// precision and alpha; never treat an ICC profile as disposable metadata.
    pub(super) fn convert(&self, image: DynamicImage) -> Result<DynamicImage> {
        let gray = matches!(
            image,
            DynamicImage::ImageLuma8(_)
                | DynamicImage::ImageLumaA8(_)
                | DynamicImage::ImageLuma16(_)
                | DynamicImage::ImageLumaA16(_)
        );
        if (self.0.color_space == DataColorSpace::Gray) != gray {
            return Err(invalid(
                "image ICC color space differs from the decoded pixels",
            ));
        }
        let layout = match &image {
            DynamicImage::ImageLuma8(_) | DynamicImage::ImageLuma16(_) => Layout::Gray,
            DynamicImage::ImageLumaA8(_) | DynamicImage::ImageLumaA16(_) => Layout::GrayAlpha,
            DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgb16(_) => Layout::Rgb,
            DynamicImage::ImageRgba8(_) | DynamicImage::ImageRgba16(_) => Layout::Rgba,
            _ => return Err(ArtworkIssue::ColorProfile.into()),
        };
        let channels = usize::from(image.color().channel_count());
        let stride = usize::try_from(image.width())
            .map_err(|_| invalid("image row size is invalid"))?
            * channels;
        let destination = ColorProfile::new_srgb();
        let options = TransformOptions::default();
        if image.color().bits_per_pixel() / u16::from(image.color().channel_count()) == 16 {
            validate_dimensions(image.width(), image.height(), 8)?;
            let transform = self
                .0
                .create_transform_16bit(layout, &destination, Layout::Rgba, options)
                .map_err(transform_error)?;
            let mut output = ImageBuffer::<Rgba<u16>, Vec<u16>>::new(image.width(), image.height());
            let source: &[u16] = match &image {
                DynamicImage::ImageLuma16(data) => data.as_raw(),
                DynamicImage::ImageLumaA16(data) => data.as_raw(),
                DynamicImage::ImageRgb16(data) => data.as_raw(),
                DynamicImage::ImageRgba16(data) => data.as_raw(),
                _ => return Err(ArtworkIssue::ColorProfile.into()),
            };
            for (source, target) in source.chunks_exact(stride).zip(
                output.as_mut().chunks_exact_mut(
                    usize::try_from(image.width())
                        .map_err(|_| invalid("image row size is invalid"))?
                        * 4,
                ),
            ) {
                transform
                    .transform(source, target)
                    .map_err(|_| invalid("image ICC color conversion failed"))?;
            }
            Ok(DynamicImage::ImageRgba16(output))
        } else {
            validate_dimensions(image.width(), image.height(), 4)?;
            let transform = self
                .0
                .create_transform_8bit(layout, &destination, Layout::Rgba, options)
                .map_err(transform_error)?;
            let mut output = RgbaImage::new(image.width(), image.height());
            for (source, target) in image.as_bytes().chunks_exact(stride).zip(
                output.as_mut().chunks_exact_mut(
                    usize::try_from(image.width())
                        .map_err(|_| invalid("image row size is invalid"))?
                        * 4,
                ),
            ) {
                transform
                    .transform(source, target)
                    .map_err(|_| invalid("image ICC color conversion failed"))?;
            }

            Ok(DynamicImage::ImageRgba8(output))
        }
    }
}

fn transform_error(error: CmsError) -> Error {
    match error {
        CmsError::UnsupportedColorPrimaries(_)
        | CmsError::UnsupportedTrc(_)
        | CmsError::UnsupportedProfileConnection
        | CmsError::UnsupportedChannelConfiguration
        | CmsError::UnsupportedLutRenderingIntent(_) => ArtworkIssue::ColorProfile.into(),
        _ => invalid("image ICC transform is invalid or exceeds its limits"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_color_expansion_keeps_the_decoded_buffer_limit() {
        let source = DynamicImage::ImageLuma16(ImageBuffer::new(4096, 2049));
        let profile = IccProfile(ColorProfile::new_gray_with_gamma(2.2));
        assert!(
            matches!(profile.convert(source), Err(Error::InvalidInput(message))
            if message.contains("decoded bytes"))
        );
    }

    #[test]
    fn artwork_malformed_transform_is_not_a_recoverable_profile_decision() {
        for error in [
            CmsError::InvalidTrcCurve,
            CmsError::InvalidAtoBLut,
            CmsError::DivisionByZero,
        ] {
            assert!(matches!(transform_error(error), Error::InvalidInput(_)));
        }
        assert!(matches!(
            transform_error(CmsError::UnsupportedProfileConnection),
            Error::ArtworkInput(ArtworkIssue::ColorProfile)
        ));
    }
}
