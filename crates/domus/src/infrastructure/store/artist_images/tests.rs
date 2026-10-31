use super::*;
use crate::application::services::artist_images::tests::{artist, photo};

pub(crate) fn state() -> Result<tempfile::TempDir> {
    let directory = tempfile::Builder::new()
        .prefix("artist-images-")
        .tempdir_in(std::env::current_dir()?)?;
    crate::infrastructure::permissions::private_fixture::directory(directory.path())
        .map_err(|error| Error::Conflict(format!("private photo fixture failed: {error}")))?;
    Ok(directory)
}

#[test]
fn artist_images_bindings_survive_restart_and_reject_stale_writes_and_tombstone_recreation()
-> Result<()> {
    let directory = state()?;
    let database = directory.path().join("state.db");
    let mut store = Store::open(&database)?;
    let photo = photo()?;
    let saved = store.save_artist_photo(&photo, None)?;
    assert_eq!(saved.version, 1);
    assert!(store.save_artist_photo(&photo, None).is_err());
    assert!(store.remove_artist_photo(saved.id, 9).is_err());
    drop(store);
    let mut store = Store::open_current_existing(&database)?;
    assert_eq!(
        store
            .artist_photo_binding(&artist(), "wikimedia-commons")?
            .ok_or(Error::Internal("fixture binding missing"))?
            .version,
        1
    );
    assert_eq!(store.artist_photo_bindings(0, 1)?.len(), 1);
    assert!(store.artist_photo_bindings(0, 51).is_err());
    assert!(store.artist_photo_bindings(saved.id, 1)?.is_empty());
    let second = store.save_artist_photo(&photo, Some(1))?;
    assert_eq!(second.version, 2);
    store.remove_artist_photo(saved.id, 2)?;
    assert!(
        store
            .artist_photo_binding(&artist(), "wikimedia-commons")?
            .is_none()
    );
    assert!(store.save_artist_photo(&photo, None).is_err());
    assert!(store.save_artist_photo(&photo, Some(2)).is_err());
    assert_eq!(
        store.artist_photo_version(&artist(), "wikimedia-commons")?,
        Some(3)
    );
    assert_eq!(store.save_artist_photo(&photo, Some(3))?.version, 4);
    Ok(())
}

#[tokio::test]
async fn artist_images_commons_admission_keeps_live_lock_and_durable_longer_backoff() -> Result<()>
{
    let directory = state()?;
    let database = directory.path().join("state.db");
    let mut store = Store::open(&database)?;
    let mut other = Store::open_current_existing(&database)?;
    let mut permit = store.claim_commons()?;
    assert!(other.claim_commons().is_err());
    permit.start().await?;
    assert!(permit.start().await.is_err());
    permit.delay(&store, 120)?;
    permit.delay(&store, 1)?;
    store.complete_commons(permit, true)?;
    let remaining = other.claim_commons();
    assert!(matches!(
        remaining,
        Err(Error::Catalog {
            retry_after_seconds: 120,
            ..
        })
    ));
    let catalog: i64 =
        store
            .connection
            .query_row("SELECT generation FROM catalog_gate WHERE id=1", [], |r| {
                r.get(0)
            })?;
    assert_eq!(catalog, 0);
    Ok(())
}

#[tokio::test]
async fn artist_images_commons_abandonment_and_unsupported_delays_do_not_reset_admission()
-> Result<()> {
    let directory = state()?;
    let mut store = Store::open(&directory.path().join("state.db"))?;
    let mut permit = store.claim_commons()?;
    permit.start().await?;
    permit.delay(&store, u32::MAX)?;
    drop(permit);
    assert!(matches!(
        store.claim_commons(),
        Err(Error::Catalog {
            retry_after_seconds: u32::MAX,
            ..
        })
    ));
    store.resume_commons()?;
    assert!(matches!(
        store.claim_commons(),
        Err(Error::Catalog {
            retry_after_seconds: 1,
            ..
        })
    ));
    assert!(store.resume_commons().is_err());
    Ok(())
}
