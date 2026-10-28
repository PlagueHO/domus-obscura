use super::*;
use crate::application::{
    add_location,
    sharing::{BindShareFolder, InspectShareFolder, bind_share_folder, inspect_share_folder},
};
use crate::domain::sharing::{ShareAccess, ShareRoot};
use std::io::Read;

use crate::infrastructure::permissions::private_fixture;

struct Fixture {
    _directory: tempfile::TempDir,
    media: PathBuf,
    database: PathBuf,
    store: Store,
    location: i64,
    policy: SharePolicy,
}

impl Fixture {
    fn new() -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("sharing-")
            .tempdir_in(std::env::current_dir()?)?;
        private_fixture::directory(directory.path())
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let media = directory.path().join("media");
        let state = directory.path().join("state");
        std::fs::create_dir(&media)?;
        std::fs::create_dir(&state)?;
        let database = state.join("domus.sqlite");
        let store = Store::open(&database)?;
        let location = add_location(&store, &media, "main")?;
        let inspected = inspect_share_folder(
            &store,
            &InspectShareFolder {
                location_id: location,
                relative_path: String::new(),
            },
        )?;
        assert!(!inspected.bound);
        let bound = bind_share_folder(
            &store,
            &BindShareFolder {
                location_id: location,
                relative_path: String::new(),
                expected_identity: inspected.native_identity,
            },
        )?;
        let policy = SharePolicy {
            access: ShareAccess::All,
            roots: vec![ShareRoot {
                binding_id: bound
                    .binding_id
                    .ok_or(Error::Internal("missing test binding"))?,
                alias: "Music".into(),
            }],
            ..SharePolicy::default()
        };
        Ok(Self {
            _directory: directory,
            media,
            database,
            store,
            location,
            policy,
        })
    }

    fn catalog(&self) -> Result<ShareCatalog> {
        ShareCatalog::build(
            &self.database,
            &self.policy,
            Arc::new(AtomicBool::new(false)),
        )
    }
}

#[test]
fn sharing_catalog_pages_only_supported_files_and_opens_read_only() -> Result<()> {
    let fixture = Fixture::new()?;
    std::fs::create_dir(fixture.media.join("Album"))?;
    std::fs::write(
        fixture.media.join("Album").join("one.mp3"),
        b"encoded audio",
    )?;
    std::fs::write(fixture.media.join("Album").join("two.flac"), b"flac")?;
    std::fs::write(fixture.media.join("Album").join("cover.jpg"), b"jpeg")?;
    std::fs::write(fixture.media.join("secret.json"), b"secret")?;
    std::fs::write(fixture.media.join("other.m4a"), b"unsupported")?;
    std::fs::create_dir(fixture.media.join(".private"))?;
    std::fs::write(fixture.media.join(".private").join("hidden.mp3"), b"secret")?;
    let catalog = fixture.catalog()?;
    assert_eq!(catalog.counts()?, (2, 3));
    let first_directory = catalog.directory_page("", 1, None)?;
    assert_eq!(first_directory[0].wire_path, "Music");
    assert_eq!(first_directory[0].files, 0);
    let album_directory = catalog.directory_page(&first_directory[0].wire_path, 1, None)?;
    assert_eq!(album_directory[0].wire_path, "Music\\Album");
    assert_eq!(album_directory[0].files, 3);
    assert!(catalog.directory_files("Music", 0, 20)?.is_empty());
    assert_eq!(catalog.directory_files("Music\\Album", 0, 20)?.len(), 3);
    assert_eq!(
        catalog.directory_page("", 20, Some("Music\\Album"))?.len(),
        1
    );
    let first = catalog.page(0, 2, None, None)?;
    let rest = catalog.page(
        first
            .last()
            .ok_or(Error::Internal("missing first page"))?
            .id,
        2,
        None,
        None,
    )?;
    assert_eq!(first.len() + rest.len(), 3);
    assert!(
        catalog
            .page(
                rest.last().ok_or(Error::Internal("missing last page"))?.id,
                2,
                None,
                None
            )?
            .is_empty()
    );
    assert_eq!(
        catalog
            .page(0, 20, Some("Music\\Album"), Some("one"))?
            .len(),
        1
    );
    let entry = catalog.file("Music\\Album\\one.mp3")?;
    let mut wrong = entry.clone();
    wrong.bytes += 1;
    assert!(catalog.open(&wrong).is_err());
    wrong = entry.clone();
    wrong.wire_path = "Music\\Album\\another.mp3".into();
    assert!(catalog.open(&wrong).is_err());
    let mut read = catalog.open(&entry)?;
    let mut bytes = Vec::new();
    read.file.read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"encoded audio");
    read.check()?;
    use std::io::Write;
    assert!(read.file.write_all(b"no").is_err());
    assert!(catalog.file("Music\\..\\secret.json").is_err());
    assert!(catalog.page(0, 201, None, None).is_err());
    Ok(())
}

#[test]
fn sharing_visibility_guards_exclude_publication_and_rebuild() -> Result<()> {
    let fixture = Fixture::new()?;
    std::fs::write(fixture.media.join("one.mp3"), b"audio")?;
    let catalog = fixture.catalog()?;
    let read = catalog.open(&catalog.file("Music\\one.mp3")?)?;
    assert!(publication_visibility_write(&fixture.store).is_err());
    assert!(fixture.catalog().is_err());
    drop(read);
    let writer = publication_visibility_write(&fixture.store)?;
    assert!(catalog.check().is_err());
    assert!(catalog.page(0, 20, None, None).is_err());
    drop(writer);
    let newer = fixture.catalog()?;
    assert_eq!(newer.counts()?, (1, 1));
    assert!(catalog.check().is_err());
    Ok(())
}

#[test]
fn sharing_changes_and_hardlink_aliases_fail_closed() -> Result<()> {
    let fixture = Fixture::new()?;
    let file = fixture.media.join("one.mp3");
    std::fs::write(&file, b"audio")?;
    let catalog = fixture.catalog()?;
    let entry = catalog.file("Music\\one.mp3")?;
    std::fs::write(&file, b"changed")?;
    assert!(catalog.open(&entry).is_err());
    let outside = fixture._directory.path().join("outside.mp3");
    std::fs::hard_link(&file, &outside)?;
    assert!(fixture.catalog().is_err());
    assert_eq!(std::fs::read(outside)?, b"changed");
    let connection = fixture.store.sharing_connection();
    let error: Option<String> = connection.query_row(
        "SELECT last_error FROM share_catalogs ORDER BY id DESC LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    assert!(error.is_some());
    Ok(())
}

#[test]
fn sharing_binding_never_silently_adopts_a_replaced_selected_folder() -> Result<()> {
    let fixture = Fixture::new()?;
    std::fs::create_dir(fixture.media.join("Selected"))?;
    let request = InspectShareFolder {
        location_id: fixture.location,
        relative_path: "Selected".into(),
    };
    let inspected = inspect_share_folder(&fixture.store, &request)?;
    bind_share_folder(
        &fixture.store,
        &BindShareFolder {
            location_id: fixture.location,
            relative_path: "Selected".into(),
            expected_identity: inspected.native_identity,
        },
    )?;
    std::fs::rename(fixture.media.join("Selected"), fixture.media.join("Old"))?;
    std::fs::create_dir(fixture.media.join("Selected"))?;
    let changed = inspect_share_folder(&fixture.store, &request)?;
    assert!(!changed.bound);
    assert!(
        bind_share_folder(
            &fixture.store,
            &BindShareFolder {
                location_id: fixture.location,
                relative_path: "Selected".into(),
                expected_identity: changed.native_identity,
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn sharing_catalog_limits_stop_without_exposing_a_partial_generation() -> Result<()> {
    let mut fixture = Fixture::new()?;
    std::fs::write(fixture.media.join("one.mp3"), b"one")?;
    std::fs::write(fixture.media.join("two.mp3"), b"two")?;
    fixture.policy.limits.catalog_files = 1;
    assert!(fixture.catalog().is_err());
    let complete: bool = fixture.store.sharing_connection().query_row(
        "SELECT EXISTS(SELECT 1 FROM share_catalogs WHERE complete=1)",
        [],
        |row| row.get(0),
    )?;
    assert!(!complete);
    Ok(())
}

#[test]
fn sharing_catalog_encoded_entry_budget_is_configurable_and_fail_closed() -> Result<()> {
    let mut fixture = Fixture::new()?;
    for index in 0..16 {
        std::fs::write(fixture.media.join(format!("track-{index}.mp3")), b"audio")?;
    }
    fixture.policy.limits.catalog_bytes = 1024;
    assert!(fixture.catalog().is_err());
    let complete: bool = fixture.store.sharing_connection().query_row(
        "SELECT EXISTS(SELECT 1 FROM share_catalogs WHERE complete=1)",
        [],
        |row| row.get(0),
    )?;
    assert!(!complete);
    fixture.policy.limits.catalog_bytes = 1024 * 1024;
    assert_eq!(fixture.catalog()?.counts()?.1, 16);
    Ok(())
}

#[test]
fn sharing_namespace_collisions_fail_instead_of_truncating() -> Result<()> {
    let fixture = Fixture::new()?;
    std::fs::write(fixture.media.join("one.mp3"), b"one")?;
    let mut policy = fixture.policy.clone();
    policy.roots[0].alias = "Bad.".into();
    assert!(
        ShareCatalog::build(&fixture.database, &policy, Arc::new(AtomicBool::new(false))).is_err()
    );
    Ok(())
}

#[test]
fn sharing_visibility_cross_process_probe() -> Result<()> {
    let Some(database) = std::env::var_os("DOMUS_SHARING_LOCK_DATABASE") else {
        return Ok(());
    };
    let store = Store::open_read_current(Path::new(&database))?;
    let lock = publication_visibility_write(&store);
    assert_eq!(
        lock.is_ok(),
        std::env::var_os("DOMUS_SHARING_EXPECT_WRITE").is_some()
    );
    Ok(())
}

#[test]
fn sharing_visibility_lock_is_cross_process_and_survives_reader_exit() -> Result<()> {
    let fixture = Fixture::new()?;
    let catalog = fixture.catalog()?;
    let guard = catalog.check()?;
    let child = |write: bool| -> Result<()> {
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                "infrastructure::shares::tests::sharing_visibility_cross_process_probe",
            ])
            .env("DOMUS_SHARING_LOCK_DATABASE", &fixture.database);
        if write {
            command.env("DOMUS_SHARING_EXPECT_WRITE", "1");
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    child(false)?;
    drop(guard);
    child(true)?;
    Ok(())
}

#[test]
fn sharing_scan_completion_invalidates_a_retained_catalog() -> Result<()> {
    let mut fixture = Fixture::new()?;
    std::fs::write(fixture.media.join("one.mp3"), b"audio")?;
    let catalog = fixture.catalog()?;
    crate::application::scan_location(&mut fixture.store, fixture.location)?;
    assert!(matches!(catalog.check(), Err(Error::Conflict(_))));
    Ok(())
}

#[test]
fn sharing_replaced_descendants_fail_before_browse_or_open() -> Result<()> {
    let fixture = Fixture::new()?;
    let album = fixture.media.join("Album");
    std::fs::create_dir(&album)?;
    std::fs::write(album.join("one.mp3"), b"original")?;
    let catalog = fixture.catalog()?;
    let shared = catalog.file("Music\\Album\\one.mp3")?;
    std::fs::rename(&album, fixture.media.join("Moved"))?;
    std::fs::create_dir(&album)?;
    std::fs::write(album.join("one.mp3"), b"replaced")?;
    assert!(catalog.directory_page("Music", 20, None).is_err());
    assert!(catalog.directory_files("Music\\Album", 0, 20).is_err());
    assert!(catalog.open(&shared).is_err());
    assert_eq!(std::fs::read(album.join("one.mp3"))?, b"replaced");
    Ok(())
}

#[cfg(unix)]
#[test]
fn sharing_symlink_and_case_collision_cannot_escape_or_alias_the_catalog() -> Result<()> {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new()?;
    let outside = fixture._directory.path().join("outside.mp3");
    std::fs::write(&outside, b"outside sentinel")?;
    symlink(&outside, fixture.media.join("escape.mp3"))?;
    assert!(fixture.catalog().is_err());
    std::fs::remove_file(fixture.media.join("escape.mp3"))?;
    std::fs::write(fixture.media.join("Track.mp3"), b"one")?;
    std::fs::write(fixture.media.join("track.mp3"), b"two")?;
    assert!(fixture.catalog().is_err());
    assert_eq!(std::fs::read(outside)?, b"outside sentinel");
    Ok(())
}
