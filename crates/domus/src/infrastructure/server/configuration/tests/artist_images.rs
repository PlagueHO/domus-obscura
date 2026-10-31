use super::*;

#[test]
fn artist_images_configuration_defaults_disabled_and_requires_selected_shared_cache() -> TestResult
{
    let fixture = Fixture::new(false)?;
    let active = fixture.source.read()?;
    assert!(active.active.services.artist_images.is_empty());
    for selected in ["wikimedia-commons", "uninstalled"] {
        let command = section_command(
            &fixture.source,
            json!([{
                "section":"music_providers", "services":{
                    "discovery":[], "metadata":[], "artwork":[], "acquisition":[],
                    "artist_images":[selected]
                }
            }]),
        )?;
        assert!(!fixture.source.validate_sections(command)?.errors.is_empty());
    }
    let root = fixture.directory.path().join("cache");
    std::fs::create_dir(&root)?;
    let database = fixture.directory.path().join("state.db");
    let mut store = Store::open_current_existing(&database)?;
    drop(crate::infrastructure::cache::FileCache::open(
        &mut store,
        &root,
        crate::infrastructure::cache::CacheLimits::default(),
    )?);
    let command = section_command(
        &fixture.source,
        json!([
            {"section":"music_providers","services":{
                "discovery":[], "metadata":[], "artwork":[], "acquisition":[],
                "artist_images":["wikimedia-commons"]
            }},
            {"section":"storage", "database":active.saved.database, "cache":{
                "root":root,"item_bytes":9437184,"total_bytes":134217728,"entries":64
            }}
        ]),
    )?;
    let before = std::fs::read(&fixture.path)?;
    let result = fixture.source.validate_sections(command)?;
    assert!(
        result.errors.is_empty(),
        "{}",
        serde_json::to_string(&result.errors)?
    );
    assert_eq!(std::fs::read(&fixture.path)?, before);
    let raw = rusqlite::Connection::open(database)?;
    assert_eq!(
        raw.query_row("SELECT generation FROM commons_gate WHERE id=1", [], |r| {
            r.get::<_, i64>(0)
        })?,
        0
    );
    Ok(())
}
