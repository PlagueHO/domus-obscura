use super::*;
use crate::application::services::images::{ImageOptions, ImageService};
use crate::application::services::{ArtworkService, ServiceConfiguration};
use crate::domain::ArtworkSize;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) fn fixture() -> Result<(ArtistImageSource, Vec<u8>)> {
    let image = image::RgbImage::from_pixel(800, 400, image::Rgb([30, 80, 120]));
    let mut original = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut original)
        .encode_image(&image)
        .map_err(|_| Error::Internal("fixture encoding failed"))?;
    let source = ArtistImageSource {
        file: "File:Generated.jpg".into(),
        entity_id: "1".into(),
        revision: "2".into(),
        timestamp: "2026-01-01T00:00:00Z".into(),
        sha1: Some(format!("{:x}", sha1::Sha1::digest(&original))),
        bytes: original.len() as u32,
        width: 800,
        height: 400,
        mime: "image/jpeg".into(),
        original_url: "https://upload.wikimedia.org/wikipedia/commons/a/ab/Generated.jpg".into(),
        rights: ImageRights {
            title: "Generated photo".into(),
            creator: "Generated fixture".into(),
            credit: "Generated for tests".into(),
            license: "CC BY-SA 4.0".into(),
            license_url: "https://creativecommons.org/licenses/by-sa/4.0/".into(),
            source_url: "https://commons.wikimedia.org/wiki/File:Generated.jpg".into(),
            notices: String::new(),
        },
    };
    Ok((source, original))
}

pub(crate) fn artist() -> EntityRef {
    EntityRef {
        provider: "musicbrainz".into(),
        id: "01234567-89ab-cdef-0123-456789abcdef".into(),
    }
}

pub(crate) fn photo() -> Result<ArtistPhoto> {
    let (source, original) = fixture()?;
    Ok(ArtistPhoto {
        artist: artist(),
        provider: "wikimedia-commons".into(),
        source_sha256: source.fingerprint()?,
        source,
        original_sha256: format!("{:x}", Sha256::digest(original)),
        jpeg_sha256: "a".repeat(64),
        width: 400,
        height: 200,
        fetched_at: 1,
        expires_at: u64::MAX,
    })
}

pub(crate) struct Provider {
    pub id: &'static str,
    pub source: ArtistImageSource,
    pub original: Vec<u8>,
    pub inspections: AtomicUsize,
    pub downloads: AtomicUsize,
    pub missing: bool,
    pub stale: bool,
    pub fail: bool,
}

impl Provider {
    pub(crate) fn new(id: &'static str) -> Result<Self> {
        let (source, original) = fixture()?;
        Ok(Self {
            id,
            source,
            original,
            inspections: AtomicUsize::new(0),
            downloads: AtomicUsize::new(0),
            missing: false,
            stale: false,
            fail: false,
        })
    }
}

impl ArtistImageProvider for Provider {
    fn id(&self) -> &str {
        self.id
    }
    fn inspect(&self, _: String) -> BoxFuture<'_, Result<Option<ArtistImageEvidence>>> {
        Box::pin(async move {
            self.inspections.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(Error::Remote("fixture provider failed"));
            }
            if self.missing {
                return Ok(None);
            }
            Ok(Some(ArtistImageEvidence {
                source: self.source.clone(),
                fetched_at: 1,
                expires_at: if self.stale { 1 } else { u64::MAX },
            }))
        })
    }
    fn original(&self, _: ArtistImageSource) -> BoxFuture<'_, Result<Vec<u8>>> {
        Box::pin(async move {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            Ok(self.original.clone())
        })
    }
}

pub(crate) fn service(provider: Arc<dyn ArtistImageProvider>) -> Result<ImageService> {
    let configuration = ServiceConfiguration {
        artwork: Vec::new(),
        artist_images: vec![provider.id().into()],
        ..Default::default()
    };
    Ok(ImageService::new(
        Arc::new(ArtworkService::new(configuration.clone(), Vec::new())?),
        Arc::new(crate::infrastructure::artwork::NativeImageProcessor::new(
            None,
        )),
    )
    .with_artists(ArtistImageProviders::new(configuration, vec![provider])?))
}

pub(crate) fn selection(provider: &str) -> CheckArtistPhoto {
    CheckArtistPhoto {
        artist: artist(),
        provider: provider.into(),
        file: "File:Generated.jpg".into(),
    }
}

#[tokio::test]
async fn artist_images_replace_provider_without_changing_caller_and_preserve_sizes() -> Result<()> {
    for id in ["wikimedia-commons", "replacement-fixture"] {
        let mut provider = Provider::new(id)?;
        if id == "replacement-fixture" {
            provider.source.file = "opaque-photo-reference".into();
            provider.source.entity_id = "opaque-object".into();
            provider.source.revision = "v2".into();
            provider.source.sha1 = None;
        }
        let mut request = selection(id);
        request.file = provider.source.file.clone();
        let provider = Arc::new(provider);
        let images = service(provider)?;
        for (size, width, height) in [
            (ArtworkSize::Small, 200, 100),
            (ArtworkSize::Recommended, 400, 200),
            (ArtworkSize::Large, 800, 400),
        ] {
            let (photo, image) = images
                .retrieve_artist(
                    request.clone(),
                    ImageOptions {
                        size: Some(size),
                        background: None,
                    },
                )
                .await?
                .ok_or(Error::Internal("fixture photo missing"))?;
            assert_eq!(photo.provider, id);
            assert_eq!(photo.artist, artist());
            assert_eq!((image.width, image.height), (width, height));
            assert_eq!(photo.source.rights.license, "CC BY-SA 4.0");
            assert_eq!(photo.source_sha256, photo.source.fingerprint()?);
        }
    }
    Ok(())
}

#[tokio::test]
async fn artist_images_reject_unknown_identity_missing_corrupt_and_stale_sources() -> Result<()> {
    let options = ImageOptions {
        size: Some(ArtworkSize::Recommended),
        background: None,
    };
    let provider = Arc::new(Provider::new("wikimedia-commons")?);
    let images = service(provider.clone())?;
    let mut unknown = selection("wikimedia-commons");
    unknown.artist.provider = "name".into();
    assert!(images.retrieve_artist(unknown, options).await.is_err());
    assert!(
        images
            .retrieve_artist(selection("uninstalled"), options)
            .await
            .is_err()
    );
    assert_eq!(provider.inspections.load(Ordering::SeqCst), 0);
    for kind in ["missing", "stale", "corrupt", "mismatch", "failed"] {
        let mut provider = Provider::new("wikimedia-commons")?;
        match kind {
            "missing" => provider.missing = true,
            "stale" => provider.stale = true,
            "corrupt" => {
                provider.original = vec![1; provider.original.len()];
            }
            "mismatch" => provider.source.file = "File:Other.jpg".into(),
            _ => provider.fail = true,
        }
        let provider = Arc::new(provider);
        let images = service(provider.clone())?;
        let result = images
            .retrieve_artist(selection("wikimedia-commons"), options)
            .await;
        if kind == "missing" {
            assert!(result?.is_none());
        } else {
            assert!(result.is_err());
        }
        if kind != "corrupt" {
            assert_eq!(provider.downloads.load(Ordering::SeqCst), 0);
        }
    }
    Ok(())
}

#[test]
fn artist_images_rights_and_file_boundaries_fail_closed() -> Result<()> {
    let (source, _) = fixture()?;
    source.validate()?;
    for file in [
        "https://example.org/a.jpg",
        "File:a|File:b",
        "File:../x.jpg",
        "File:",
        "File:a#b.jpg",
    ] {
        assert!(validate_file(file).is_err());
    }

    Ok(())
}

#[tokio::test]
async fn artist_images_small_original_is_not_enlarged_and_variants_are_provider_scoped()
-> Result<()> {
    use crate::application::services::cache::testing::MemoryCache;
    use crate::infrastructure::artwork::NativeImageProcessor;
    let cache = Arc::new(MemoryCache::default());
    for id in ["wikimedia-commons", "replacement-fixture"] {
        let mut provider = Provider::new(id)?;
        let mut original = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut original)
            .encode_image(&image::RgbImage::from_pixel(
                80,
                40,
                image::Rgb([30, 80, 120]),
            ))
            .map_err(|_| Error::Internal("small fixture encoding failed"))?;
        provider.source.width = 80;
        provider.source.height = 40;
        provider.source.bytes = original.len() as u32;
        provider.source.sha1 = Some(format!("{:x}", sha1::Sha1::digest(&original)));
        provider.original = original;
        let configuration = ServiceConfiguration {
            artwork: Vec::new(),
            artist_images: vec![id.into()],
            ..Default::default()
        };
        let images = ImageService::new(
            Arc::new(ArtworkService::new(configuration.clone(), Vec::new())?),
            Arc::new(NativeImageProcessor::new(Some(cache.clone()))),
        )
        .with_artists(ArtistImageProviders::new(
            configuration,
            vec![Arc::new(provider)],
        )?);
        for size in [
            ArtworkSize::Small,
            ArtworkSize::Recommended,
            ArtworkSize::Large,
        ] {
            let (photo, _) = images
                .retrieve_artist(
                    selection(id),
                    ImageOptions {
                        size: Some(size),
                        background: None,
                    },
                )
                .await?
                .ok_or(Error::Internal("small fixture missing"))?;
            assert_eq!((photo.width, photo.height), (80, 40));
        }
    }
    assert_eq!(cache.puts.load(Ordering::SeqCst), 6);
    Ok(())
}
#[test]
fn artist_images_supported_licenses_require_complete_exact_credit() -> Result<()> {
    let (source, _) = fixture()?;
    for (license, url) in [
        (
            "CC0 1.0",
            "https://creativecommons.org/publicdomain/zero/1.0/",
        ),
        ("CC BY 3.0", "https://creativecommons.org/licenses/by/3.0/"),
        ("CC BY 4.0", "https://creativecommons.org/licenses/by/4.0/"),
        (
            "CC BY-SA 3.0",
            "https://creativecommons.org/licenses/by-sa/3.0/",
        ),
        (
            "CC BY-SA 4.0",
            "https://creativecommons.org/licenses/by-sa/4.0/",
        ),
    ] {
        let mut rights = source.rights.clone();
        rights.license = license.into();
        rights.license_url = url.into();
        rights.validate()?;
        rights.license_url = "https://example.org/false-license".into();
        assert!(rights.validate().is_err());
    }
    let mut rights = source.rights.clone();
    rights.creator.clear();
    assert!(rights.validate().is_err());
    rights = source.rights;
    rights.license = "Fair use".into();
    assert!(rights.validate().is_err());
    Ok(())
}
