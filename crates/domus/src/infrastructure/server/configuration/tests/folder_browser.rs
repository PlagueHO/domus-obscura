use super::*;
use crate::infrastructure::server::configuration::BrowseHostFolder;

fn browse(path: &Path) -> BrowseHostFolder {
    BrowseHostFolder {
        path: path.to_string_lossy().into_owned(),
        expected_identity: None,
        cursor: None,
    }
}

#[test]
fn backend_configuration_folder_browser_keeps_bounded_handle_owned_pages() -> TestResult {
    let fixture = Fixture::new(false)?;
    let media = tempfile::tempdir()?;
    for index in 0..300 {
        std::fs::create_dir(media.path().join(format!("Album-{index:03}")))?;
    }
    let first = fixture.source.browse_host_folder(browse(media.path()))?;
    assert_eq!(first.entries.len(), 128);
    assert!(first.entries.iter().all(|item| item.identity.is_some()));
    let identity = first.identity;
    let mut cursor = first.next_cursor;
    let mut names: std::collections::BTreeSet<_> =
        first.entries.into_iter().map(|item| item.name).collect();
    while let Some(next) = cursor {
        let page = fixture.source.browse_host_folder(BrowseHostFolder {
            expected_identity: Some(identity.clone()),
            cursor: Some(next),
            ..browse(media.path())
        })?;
        assert!(page.entries.len() <= 128);
        for entry in page.entries {
            assert!(names.insert(entry.name));
        }
        cursor = page.next_cursor;
    }
    assert_eq!(names.len(), 300);
    assert_eq!(std::fs::read_dir(media.path())?.count(), 300);
    Ok(())
}

#[test]
fn backend_configuration_folder_browser_rejects_stale_and_closed_pages() -> TestResult {
    let fixture = Fixture::new(false)?;
    let media = tempfile::tempdir()?;
    for index in 0..140 {
        std::fs::create_dir(media.path().join(format!("Album-{index}")))?;
    }
    let first = fixture.source.browse_host_folder(browse(media.path()))?;
    let cursor = first.next_cursor.ok_or("missing continuation")?;
    assert!(
        fixture
            .source
            .browse_host_folder(BrowseHostFolder {
                expected_identity: Some("different".into()),
                cursor: Some(cursor.clone()),
                ..browse(media.path())
            })
            .is_err()
    );
    let first = fixture.source.browse_host_folder(browse(media.path()))?;
    let cursor = first.next_cursor.ok_or("missing continuation")?;
    fixture.source.close_host_folder(&cursor)?;
    assert!(
        fixture
            .source
            .browse_host_folder(BrowseHostFolder {
                expected_identity: Some(first.identity),
                cursor: Some(cursor),
                ..browse(media.path())
            })
            .is_err()
    );
    let first = fixture.source.browse_host_folder(browse(media.path()))?;
    std::fs::create_dir(media.path().join("Added-after-page"))?;
    assert!(
        fixture
            .source
            .browse_host_folder(BrowseHostFolder {
                expected_identity: Some(first.identity),
                cursor: first.next_cursor,
                ..browse(media.path())
            })
            .is_err()
    );
    Ok(())
}

#[test]
fn backend_configuration_folder_browser_rejects_private_read_only_and_changed_configuration()
-> TestResult {
    let fixture = Fixture::new(false)?;
    assert!(
        fixture
            .source
            .browse_host_folder(browse(fixture.directory.path()))
            .is_err()
    );
    let child = fixture.directory.path().join("private-child");
    std::fs::create_dir(&child)?;
    assert!(fixture.source.browse_host_folder(browse(&child)).is_err());
    let media = tempfile::tempdir()?;
    let read_only = Fixture::new(true)?;
    assert!(
        read_only
            .source
            .browse_host_folder(browse(media.path()))
            .is_err()
    );
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["origin"] = json!("https://localhost:9443");
    std::fs::write(&fixture.path, serde_json::to_vec(&document)?)?;
    assert!(
        fixture
            .source
            .browse_host_folder(browse(media.path()))
            .is_err()
    );
    Ok(())
}

#[test]
fn backend_configuration_folder_browser_rejects_replaced_names_and_parent_paths() -> TestResult {
    let fixture = Fixture::new(false)?;
    let media = tempfile::tempdir()?;
    let album = media.path().join("Album");
    std::fs::create_dir(&album)?;
    let first = fixture.source.browse_host_folder(browse(&album))?;
    std::fs::rename(&album, media.path().join("Old-Album"))?;
    std::fs::create_dir(&album)?;
    assert!(
        fixture
            .source
            .browse_host_folder(BrowseHostFolder {
                expected_identity: Some(first.identity),
                ..browse(&album)
            })
            .is_err()
    );
    assert!(
        fixture
            .source
            .browse_host_folder(browse(&album.join("..")))
            .is_err()
    );
    assert!(
        fixture
            .source
            .browse_host_folder(browse(Path::new("relative")))
            .is_err()
    );
    Ok(())
}

#[test]
fn backend_configuration_folder_browser_rejects_cursors_from_another_service() -> TestResult {
    let fixture = Fixture::new(false)?;
    let media = tempfile::tempdir()?;
    for index in 0..140 {
        std::fs::create_dir(media.path().join(format!("Album-{index}")))?;
    }
    let first = fixture.source.browse_host_folder(browse(media.path()))?;
    let replacement = ConfigurationService::load(&fixture.path)?.0;
    let current = replacement.browse_host_folder(browse(media.path()))?;
    assert!(
        replacement
            .browse_host_folder(BrowseHostFolder {
                expected_identity: Some(first.identity),
                cursor: first.next_cursor,
                ..browse(media.path())
            })
            .is_err()
    );
    replacement.browse_host_folder(BrowseHostFolder {
        expected_identity: Some(current.identity),
        cursor: current.next_cursor,
        ..browse(media.path())
    })?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn backend_configuration_folder_browser_lists_links_as_unavailable() -> TestResult {
    let fixture = Fixture::new(false)?;
    let media = tempfile::tempdir()?;
    std::os::unix::fs::symlink(fixture.directory.path(), media.path().join("Link"))?;
    let page = fixture.source.browse_host_folder(browse(media.path()))?;
    assert_eq!(page.entries.len(), 1);
    assert!(page.entries[0].identity.is_none());
    assert!(page.entries[0].unavailable_reason.is_some());
    Ok(())
}
