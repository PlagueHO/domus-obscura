//! Original retrieval and image processing, without plan or approval authority.
use super::{ArtworkContent, ArtworkRequest, ArtworkService};
use crate::domain::ArtworkSize;
use crate::{Error, Result};
use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageOptions {
    pub size: Option<ArtworkSize>,
    pub background: Option<[u8; 3]>,
}

/// Internal JPEG payload. This is not a serializable tool result or approval.
pub struct ProcessedImage {
    pub jpeg: Vec<u8>,
    pub sha256: [u8; 32],
    pub width: u32,
    pub height: u32,
    pub profile: u8,
}

/// Native decoding, color conversion and cache I/O run behind this boundary.
/// Implementations must bound blocking work and reject repaired image input.
pub trait ImageProcessor: Send + Sync {
    fn process(
        &self,
        original: Vec<u8>,
        options: ImageOptions,
    ) -> BoxFuture<'_, Result<ProcessedImage>>;
}

/// Original identity and processing choices travel with the JPEG. Persistence
/// must independently recheck fresh provider evidence and the current plan.
pub struct RetrievedImage {
    pub request: ArtworkRequest,
    pub sources: Vec<String>,
    pub original_sha256: [u8; 32],
    pub options: ImageOptions,
    pub image: ProcessedImage,
}

pub struct ImageService {
    artwork: Arc<ArtworkService>,
    processor: Arc<dyn ImageProcessor>,
}

impl ImageService {
    pub fn new(artwork: Arc<ArtworkService>, processor: Arc<dyn ImageProcessor>) -> Self {
        Self { artwork, processor }
    }

    /// Reuse the provider's original/cache rules, then create the selected JPEG.
    /// A missing image stays missing; corruption and provider errors stay errors.
    pub async fn retrieve(
        &self,
        request: ArtworkRequest,
        options: ImageOptions,
    ) -> Result<Option<RetrievedImage>> {
        if request.image_id.is_none() {
            return Err(Error::InvalidInput(
                "image retrieval requires an explicit provider image ID".into(),
            ));
        }
        let fetched = self.artwork.fetch(request.clone()).await?;
        let original = match fetched.content {
            ArtworkContent::Image { bytes, .. } => bytes,
            ArtworkContent::Missing => return Ok(None),
            ArtworkContent::Listing(_) => {
                return Err(Error::Internal("image retrieval returned a listing"));
            }
        };
        let original_sha256 = Sha256::digest(&original).into();
        let image = self.processor.process(original, options).await?;
        let target = options.size.map_or(1200, ArtworkSize::target_side);
        if image.jpeg.is_empty()
            || image.jpeg.len() > 8 * 1024 * 1024
            || image.sha256 != <[u8; 32]>::from(Sha256::digest(&image.jpeg))
            || image.width == 0
            || image.height == 0
            || image.width > target
            || image.height > target
        {
            return Err(Error::Internal(
                "image processor returned an invalid JPEG result",
            ));
        }
        Ok(Some(RetrievedImage {
            request,
            sources: fetched.sources,
            original_sha256,
            options,
            image,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::{
        ArtworkProvider, ArtworkResult, EntityRef, ServiceConfiguration,
    };
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    struct Provider {
        calls: AtomicUsize,
        missing: bool,
    }

    impl ArtworkProvider for Provider {
        fn id(&self) -> &str {
            "pictures"
        }

        fn fetch(&self, request: ArtworkRequest) -> BoxFuture<'_, Result<ArtworkResult>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let bytes = b"generated original".to_vec();
                Ok(ArtworkResult {
                    provider: request.provider,
                    release: request.release,
                    sources: vec!["generated-source".into()],
                    content: if self.missing {
                        ArtworkContent::Missing
                    } else {
                        ArtworkContent::Image {
                            sha256: format!("{:x}", Sha256::digest(&bytes)),
                            bytes,
                        }
                    },
                })
            })
        }
    }

    #[derive(Default)]
    struct Processor {
        calls: Mutex<Vec<(Vec<u8>, ImageOptions)>>,
        failure: bool,
        malformed: bool,
    }

    impl ImageProcessor for Processor {
        fn process(
            &self,
            original: Vec<u8>,
            options: ImageOptions,
        ) -> BoxFuture<'_, Result<ProcessedImage>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .map_err(|_| Error::Internal("test processor lock"))?
                    .push((original, options));
                if self.failure {
                    return Err(crate::domain::ArtworkIssue::Background.into());
                }
                let jpeg = b"generated processor output".to_vec();
                Ok(ProcessedImage {
                    sha256: Sha256::digest(&jpeg).into(),
                    jpeg,
                    width: if self.malformed { 801 } else { 100 },
                    height: 50,
                    profile: 1,
                })
            })
        }
    }

    fn request() -> ArtworkRequest {
        ArtworkRequest {
            provider: "pictures".into(),
            release: EntityRef {
                provider: "catalog".into(),
                id: "entity-1".into(),
            },
            image_id: Some("original-1".into()),
        }
    }

    fn service(provider: Arc<Provider>, processor: Arc<Processor>) -> Result<ImageService> {
        let config = ServiceConfiguration {
            artwork: vec!["pictures".into()],
            ..Default::default()
        };
        Ok(ImageService::new(
            Arc::new(ArtworkService::new(config, vec![provider])?),
            processor,
        ))
    }

    #[tokio::test]
    async fn image_retrieval_preserves_provider_identity_original_and_processing_choices()
    -> Result<()> {
        let provider = Arc::new(Provider {
            calls: AtomicUsize::new(0),
            missing: false,
        });
        let processor = Arc::new(Processor::default());
        let service = service(provider.clone(), processor.clone())?;
        for size in [
            ArtworkSize::Small,
            ArtworkSize::Recommended,
            ArtworkSize::Large,
        ] {
            let options = ImageOptions {
                size: Some(size),
                background: Some([1, 2, 3]),
            };
            let image = service
                .retrieve(request(), options)
                .await?
                .ok_or(Error::Internal("missing image"))?;
            assert_eq!(image.request.provider, "pictures");
            assert_eq!(image.request.release.provider, "catalog");
            assert_eq!(image.request.image_id.as_deref(), Some("original-1"));
            assert_eq!(
                image.original_sha256,
                <[u8; 32]>::from(Sha256::digest(b"generated original"))
            );
            assert_eq!(image.sources, ["generated-source"]);
            assert_eq!(image.options, options);
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
        let calls = processor
            .calls
            .lock()
            .map_err(|_| Error::Internal("test processor lock"))?;
        assert_eq!(calls.len(), 3);
        assert!(
            calls
                .iter()
                .all(|(bytes, options)| bytes == b"generated original"
                    && options.background == Some([1, 2, 3]))
        );
        Ok(())
    }

    #[tokio::test]
    async fn image_retrieval_does_not_process_missing_or_invalid_selections() -> Result<()> {
        let provider = Arc::new(Provider {
            calls: AtomicUsize::new(0),
            missing: true,
        });
        let processor = Arc::new(Processor::default());
        let service = service(provider.clone(), processor.clone())?;
        let options = ImageOptions {
            size: Some(ArtworkSize::Recommended),
            background: None,
        };
        assert!(service.retrieve(request(), options).await?.is_none());
        let mut listing = request();
        listing.image_id = None;
        assert!(service.retrieve(listing, options).await.is_err());
        let mut disabled = request();
        disabled.provider = "uninstalled".into();
        assert!(service.retrieve(disabled, options).await.is_err());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert!(
            processor
                .calls
                .lock()
                .map_err(|_| Error::Internal("test processor lock"))?
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn image_retrieval_preserves_typed_processing_failures_without_retry() -> Result<()> {
        for (failure, malformed) in [(true, false), (false, true)] {
            let provider = Arc::new(Provider {
                calls: AtomicUsize::new(0),
                missing: false,
            });
            let processor = Arc::new(Processor {
                failure,
                malformed,
                ..Default::default()
            });
            let service = service(provider.clone(), processor.clone())?;
            let result = service
                .retrieve(
                    request(),
                    ImageOptions {
                        size: Some(ArtworkSize::Small),
                        background: None,
                    },
                )
                .await;
            if failure {
                assert!(matches!(
                    result,
                    Err(Error::ArtworkInput(crate::domain::ArtworkIssue::Background))
                ));
            } else {
                assert!(matches!(result, Err(Error::Internal(_))));
            }
            assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                processor
                    .calls
                    .lock()
                    .map_err(|_| Error::Internal("test processor lock"))?
                    .len(),
                1
            );
        }
        Ok(())
    }
}
