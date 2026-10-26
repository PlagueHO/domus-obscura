use super::*;
use crate::application::services::images::{ImageOptions, ProcessedImage, RetrievedImage};
use crate::application::services::{ArtworkRequest, EntityRef};
use crate::domain::{ArtworkSize, OutputSettings};
use crate::infrastructure::catalog::{CatalogQuery, CatalogResult};
use serde_json::json;

#[test]
fn image_retrieval_persistence_rechecks_plan_original_profile_and_fresh_evidence()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let state = tempfile::tempdir()?;
    let media = tempfile::tempdir()?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let location = crate::application::add_location(&store, media.path(), "main")?;
    crate::application::scan_location(&mut store, location)?;
    let settings = store.save_settings(None, &OutputSettings::default())?;
    let release = "20000000-0000-4000-8000-000000000001";
    let credit = json!([{"name":"Generated artist","artist":{
        "id":"10000000-0000-4000-8000-000000000001","name":"Generated artist"
    }}]);
    let body = serde_json::to_vec(&json!({
        "id":release,"title":"Generated album","artist-credit":credit,
        "release-group":{"id":"30000000-0000-4000-8000-000000000001"},
        "media":[{"position":1,"track-count":1,"tracks":[{
            "id":"40000000-0000-4000-8000-000000000001","position":1,"number":"1",
            "title":"Generated track","artist-credit":credit,
            "recording":{"id":"50000000-0000-4000-8000-000000000001","video":false}
        }]}]
    }))?;
    let query = CatalogQuery::release(release)?;
    let now: i64 = store
        .connection
        .query_row("SELECT unixepoch()", [], |row| row.get(0))?;
    let hash = Sha256::digest(&body).to_vec();
    store.connection.execute(
        "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
         VALUES(?1,?2,?3,?4,?4+86400,?4*1000)",
        params![query.source(), body, hash, now],
    )?;
    let exact = CatalogResult {
        data: query.parse_release(&body)?,
        source: query.source().into(),
        payload_sha256: hash,
        fetched_at: now,
        cached: true,
    };
    let job = store.enqueue(1)?;
    let plan = store.prepare_catalog_plan(
        super::super::PlanRequest {
            job_id: job,
            expected_version: 1,
            settings_version: settings,
            format: None,
            edition: exact.data.selection(None)?,
            locations: vec![location],
            accepts_incomplete_coverage: false,
        },
        exact,
    )?;
    store.approve_plan(job, plan.version)?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1000, 500).write_to(&mut png, image::ImageFormat::Png)?;
    let listing_source = format!("https://coverartarchive.org/release/{release}/");
    let image_source = format!("{listing_source}1");
    let listing = serde_json::to_vec(&json!({
        "release":format!("https://musicbrainz.org/release/{release}"),
        "images":[{"id":"1","edit":1,"types":["Front"],"front":true,"back":false,
            "approved":false,"comment":"","image":format!("{image_source}.png")}]
    }))?;
    for (key, bytes, source) in [
        (listing_source.clone(), listing, listing_source),
        (
            format!("{image_source}#mbid-{release}-1.png"),
            png.get_ref().clone(),
            image_source,
        ),
    ] {
        store.connection.execute(
            "INSERT INTO caa_cache(cache_key,body,sha256,sources,fetched_at,expires_at,accessed_at_ms)
             VALUES(?1,?2,?3,?4,?5,?5+86400,?5*1000)",
            params![key,bytes,&Sha256::digest(&bytes)[..],serde_json::to_string(&vec![source])?,now],
        )?;
    }
    let retrieved = || -> Result<RetrievedImage> {
        let jpeg = normalize_cover_for_size(png.get_ref(), None, Some(ArtworkSize::Recommended))?;
        let (width, height) = jpeg.dimensions();
        Ok(RetrievedImage {
            request: ArtworkRequest {
                provider: "cover-art-archive".into(),
                release: EntityRef {
                    provider: "musicbrainz".into(),
                    id: release.into(),
                },
                image_id: Some("1".into()),
            },
            sources: vec![],
            original_sha256: Sha256::digest(png.get_ref()).into(),
            options: ImageOptions {
                size: Some(ArtworkSize::Recommended),
                background: None,
            },
            image: ProcessedImage {
                sha256: *jpeg.sha256(),
                width,
                height,
                profile: COVER_PROFILE,
                jpeg: jpeg.into_bytes(),
            },
        })
    };
    for mutation in 0..6 {
        let mut image = retrieved()?;
        match mutation {
            0 => image.original_sha256[0] ^= 1,
            1 => image.options.size = Some(ArtworkSize::Small),
            2 => image.image.profile += 1,
            3 => image.request.release.id = "20000000-0000-4000-8000-000000000002".into(),
            4 => image.image.jpeg[0] ^= 1,
            _ => image.image.width = 401,
        }
        assert!(
            store
                .prepare_retrieved_caa_artwork_or_ask(job, 2, Ok(image))
                .is_err()
        );
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM artwork_candidates", [], |row| row
                .get::<_, i64>(0))?,
        0
    );
    store.connection.execute(
        "UPDATE caa_cache SET fetched_at=unixepoch()-86401,expires_at=unixepoch()-1",
        [],
    )?;
    assert!(
        store
            .prepare_retrieved_caa_artwork_or_ask(job, 2, Ok(retrieved()?))
            .is_err()
    );
    store.connection.execute(
        "UPDATE caa_cache SET fetched_at=?1,expires_at=?1+86400",
        [now],
    )?;
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM questions", [], |row| row
                .get::<_, i64>(0),)?,
        0
    );
    assert!(matches!(
        store.prepare_retrieved_caa_artwork_or_ask(
            job,
            2,
            Err(crate::domain::ArtworkIssue::Background.into()),
        ),
        Err(Error::ArtworkRequired(_))
    ));
    let candidate = store.prepare_retrieved_caa_artwork_or_ask(job, 2, Ok(retrieved()?))?;
    assert_eq!((candidate.width, candidate.height), (400, 200));
    assert!(candidate.approved_at.is_none());
    assert!(store.approved_artwork(job, 2).is_err());
    store.prepare_plan(super::super::PlanRequest {
        job_id: job,
        expected_version: 2,
        settings_version: settings,
        format: None,
        edition: plan.plan.edition,
        locations: vec![location],
        accepts_incomplete_coverage: false,
    })?;
    assert!(
        store
            .prepare_retrieved_caa_artwork_or_ask(job, 2, Ok(retrieved()?))
            .is_err()
    );
    Ok(())
}
