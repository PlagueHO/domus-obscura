use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

struct CatalogFake {
    id: &'static str,
    calls: Arc<AtomicUsize>,
    fails: bool,
    wrong_scope: bool,
    excess: bool,
    total: u32,
    empty: bool,
}

impl CatalogProvider for CatalogFake {
    fn id(&self) -> &str {
        self.id
    }
    fn discover(&self, query: DiscoveryQuery) -> BoxFuture<'_, Result<DiscoveryPage>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fails {
                return Err(Error::Conflict("provider unavailable".into()));
            }
            let (offset, limit) = query.bounds();
            Ok(DiscoveryPage {
                offset,
                total: Some(self.total),
                next_offset: Some(offset + 1),
                items: (0..if self.empty {
                    0
                } else if self.excess {
                    usize::from(limit) + 1
                } else {
                    1
                })
                    .map(|_| CatalogItem {
                        entity: EntityRef {
                            provider: if self.wrong_scope { "other" } else { self.id }.into(),
                            id: "release-1".into(),
                        },
                        title: "Album".into(),
                        artist: Some("Artist".into()),
                        date: None,
                    })
                    .collect(),
                source: "generated fixture".into(),
                fetched_at: 1,
            })
        })
    }
    fn metadata(&self, mut release: EntityRef) -> BoxFuture<'_, Result<ReleaseMetadata>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.wrong_scope {
                release.id = "different-release".into();
            }
            Ok(ReleaseMetadata {
                entity: release,
                title: "Album".into(),
                artist: "Artist".into(),
                date: None,
                tracks: Vec::new(),
                source: "generated fixture".into(),
                fetched_at: 1,
            })
        })
    }
}

fn fake(id: &'static str, calls: &Arc<AtomicUsize>, fails: bool) -> CatalogFake {
    CatalogFake {
        id,
        calls: calls.clone(),
        fails,
        wrong_scope: false,
        excess: false,
        total: 50,
        empty: false,
    }
}

fn configuration() -> ServiceConfiguration {
    ServiceConfiguration {
        discovery: vec!["first".into(), "second".into()],
        metadata: vec!["first".into()],
        artwork: Vec::new(),
        acquisition: Vec::new(),
    }
}

fn request() -> DiscoveryRequest {
    DiscoveryRequest {
        providers: None,
        query: DiscoveryQuery::Artists {
            text: "Fixture".into(),
            offset: 0,
            limit: 1,
        },
    }
}

#[tokio::test]
async fn multiple_provider_results_keep_order_and_explicit_partial_failures() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let service = CatalogService::new(
        configuration(),
        vec![
            Arc::new(fake("first", &calls, false)),
            Arc::new(fake("second", &calls, false)),
        ],
    )?;
    let report = service.discover(request()).await?;
    assert_eq!(
        report
            .results
            .iter()
            .map(|result| result.provider.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    let mut selected = request();
    selected.providers = Some(vec!["second".into(), "first".into()]);
    let report = service.discover(selected).await?;
    assert_eq!(report.results[0].provider, "second");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let service = CatalogService::new(
        configuration(),
        vec![
            Arc::new(fake("first", &calls, true)),
            Arc::new(fake("second", &calls, false)),
        ],
    )?;
    let report = service.discover(request()).await?;
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].provider, "second");
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].provider, "first");
    assert!(report.failures[0].error.contains("provider unavailable"));
    let mut selected = request();
    selected.providers = Some(vec!["first".into()]);
    let report = service.discover(selected).await?;
    assert!(report.results.is_empty());
    assert_eq!(report.failures.len(), 1);
    Ok(())
}

#[tokio::test]
async fn invalid_selection_and_cross_provider_ids_fail_before_any_provider_effect() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let service = CatalogService::new(
        configuration(),
        vec![
            Arc::new(fake("first", &calls, false)),
            Arc::new(fake("second", &calls, false)),
        ],
    )?;
    for providers in [
        Vec::new(),
        vec!["first".into(), "first".into()],
        vec!["absent".into()],
    ] {
        let mut selected = request();
        selected.providers = Some(providers);
        assert!(service.discover(selected).await.is_err());
    }
    for limit in [0, 26] {
        let mut selected = request();
        selected.query = DiscoveryQuery::Artists {
            text: "Fixture".into(),
            offset: 0,
            limit,
        };
        assert!(service.discover(selected).await.is_err());
    }
    let mut selected = request();
    selected.query = DiscoveryQuery::ArtistReleases {
        artist: EntityRef {
            provider: "first".into(),
            id: "artist-1".into(),
        },
        offset: 0,
        limit: 1,
        filters: Default::default(),
    };
    assert!(service.discover(selected).await.is_err());
    assert!(
        service
            .metadata(EntityRef {
                provider: "second".into(),
                id: "release-1".into()
            })
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn configuration_rejects_uninstalled_duplicates_and_wrong_roles() -> TestResult {
    let defaults: ServiceConfiguration = serde_json::from_str("{}")?;
    defaults.validate_builtins()?;
    for json in [
        r#"{"discovery":["spotify"]}"#,
        r#"{"metadata":["musicbrainz","musicbrainz"]}"#,
        r#"{"artwork":["musicbrainz"]}"#,
        r#"{"acquisition":["Soulseek"]}"#,
    ] {
        assert!(
            serde_json::from_str::<ServiceConfiguration>(json)?
                .validate_builtins()
                .is_err()
        );
    }
    let disabled: ServiceConfiguration =
        serde_json::from_str(r#"{"discovery":[],"metadata":[],"artwork":[],"acquisition":[]}"#)?;
    disabled.validate_builtins()?;
    assert!(
        disabled
            .require(Capability::Acquisition, "soulseek")
            .is_err()
    );
    assert!(serde_json::from_str::<ServiceConfiguration>(r#"{"secret":"not-a-setting"}"#).is_err());
    let calls = Arc::new(AtomicUsize::new(0));
    assert!(
        CatalogService::new(
            configuration(),
            vec![Arc::new(fake("first", &calls, false))]
        )
        .is_err()
    );
    assert!(
        CatalogService::new(
            configuration(),
            vec![
                Arc::new(fake("first", &calls, false)),
                Arc::new(fake("first", &calls, false))
            ]
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn malformed_provider_results_are_explicit_without_losing_other_results() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    for excessive in [true, false] {
        let mut bad = fake("first", &calls, false);
        bad.excess = excessive;
        bad.wrong_scope = !excessive;
        let service = CatalogService::new(
            configuration(),
            vec![Arc::new(bad), Arc::new(fake("second", &calls, false))],
        )?;
        let report = service.discover(request()).await?;
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].provider, "second");
        assert_eq!(report.failures[0].provider, "first");
    }
    let mut bad = fake("first", &calls, false);
    bad.wrong_scope = true;
    let service = CatalogService::new(
        configuration(),
        vec![Arc::new(bad), Arc::new(fake("second", &calls, false))],
    )?;
    assert!(
        service
            .metadata(EntityRef {
                provider: "first".into(),
                id: "release-1".into()
            })
            .await
            .is_err()
    );
    Ok(())
}

struct ArtworkFake {
    calls: Arc<AtomicUsize>,
    wrong_scope: bool,
}

#[tokio::test]
async fn pagination_rejects_false_counts_but_preserves_empty_filtered_pages() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    for empty in [false, true] {
        let mut first = fake("first", &calls, false);
        first.empty = empty;
        if !empty {
            first.total = 0;
        }
        let service = CatalogService::new(
            configuration(),
            vec![Arc::new(first), Arc::new(fake("second", &calls, false))],
        )?;
        let report = service.discover(request()).await?;
        if empty {
            assert!(report.failures.is_empty());
            assert!(report.results[0].page.items.is_empty());
            assert_eq!(report.results[0].page.next_offset, Some(1));
        } else {
            assert_eq!(report.failures[0].provider, "first");
            assert_eq!(report.results[0].provider, "second");
        }
    }
    Ok(())
}
impl ArtworkProvider for ArtworkFake {
    fn id(&self) -> &str {
        "pictures"
    }
    fn fetch(&self, request: ArtworkRequest) -> BoxFuture<'_, Result<ArtworkResult>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ArtworkResult {
                provider: "pictures".into(),
                release: request.release,
                sources: vec!["fixture".into()],
                content: if request.image_id.is_some() {
                    let bytes = vec![1, 2, 3];
                    use sha2::{Digest, Sha256};
                    ArtworkContent::Image {
                        sha256: format!("{:x}", Sha256::digest(&bytes)),
                        bytes,
                    }
                } else {
                    ArtworkContent::Listing(vec![ArtworkImage {
                        entity: EntityRef {
                            provider: if self.wrong_scope {
                                "other"
                            } else {
                                "pictures"
                            }
                            .into(),
                            id: "cover-1".into(),
                        },
                        source: "fixture".into(),
                        front: true,
                        provider_approved: Some(true),
                    }])
                },
            })
        })
    }
}

#[tokio::test]
async fn shared_commands_dispatch_without_approval_or_binary_output() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let catalog = CatalogService::new(
        configuration(),
        vec![
            Arc::new(fake("first", &calls, false)),
            Arc::new(fake("second", &calls, false)),
        ],
    )?;
    let artwork = ArtworkService::new(
        ServiceConfiguration {
            artwork: vec!["pictures".into()],
            ..configuration()
        },
        vec![Arc::new(ArtworkFake {
            calls: calls.clone(),
            wrong_scope: false,
        })],
    )?;
    for json in [
        r#"{"operation":"discover","request":{"query":{"operation":"artists","text":"Fixture","offset":0,"limit":1}}}"#,
        r#"{"operation":"metadata","release":{"provider":"first","id":"release-1"}}"#,
        r#"{"operation":"artwork_list","provider":"pictures","release":{"provider":"first","id":"release-1"}}"#,
        r#"{"operation":"artwork_fetch","provider":"pictures","release":{"provider":"first","id":"release-1"},"image_id":"cover-1"}"#,
    ] {
        let command: ServiceRequest = serde_json::from_str(json)?;
        let roundtrip: ServiceRequest = serde_json::from_str(&serde_json::to_string(&command)?)?;
        let response = serde_json::to_value(roundtrip.execute(&catalog, &artwork).await?)?;
        assert!(response["operation"].is_string());
        assert!(response.get("bytes").is_none());
        assert!(response.get("approved").is_none());
    }
    let bad_artwork = ArtworkService::new(
        ServiceConfiguration {
            artwork: vec!["pictures".into()],
            ..configuration()
        },
        vec![Arc::new(ArtworkFake {
            calls: calls.clone(),
            wrong_scope: true,
        })],
    )?;
    assert!(
        bad_artwork
            .fetch(ArtworkRequest {
                provider: "pictures".into(),
                release: EntityRef {
                    provider: "first".into(),
                    id: "release-1".into()
                },
                image_id: None
            })
            .await
            .is_err()
    );
    let before = calls.load(Ordering::SeqCst);
    assert!(
        artwork
            .fetch(ArtworkRequest {
                provider: "disabled".into(),
                release: EntityRef {
                    provider: "first".into(),
                    id: "release-1".into()
                },
                image_id: None
            })
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), before);
    for json in [
        r#"{"operation":"approve","job_id":1}"#,
        r#"{"operation":"metadata","release":{"provider":"first","id":"release-1"},"database":"injected.db"}"#,
        r#"{"operation":"artwork_list","provider":"pictures","release":{"provider":"first","id":"release-1"},"url":"https://example.invalid/cover"}"#,
    ] {
        assert!(serde_json::from_str::<ServiceRequest>(json).is_err());
    }
    Ok(())
}
