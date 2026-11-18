use super::{Store, StoreOpenMode};
use crate::domain::LibraryPath;
use crate::infrastructure::filesystem::{
    DirectoryExpectation, FilesystemClient, ProviderMapping, StorageConfiguration, StorageRole,
    StorageRoot,
};
use crate::{Error, Result};
use std::path::PathBuf;
use std::sync::Arc;

fn fixture() -> Result<(
    tempfile::TempDir,
    PathBuf,
    StorageConfiguration,
    Arc<FilesystemClient>,
)> {
    let root = tempfile::Builder::new()
        .prefix(".store-native-")
        .tempdir_in(std::env::current_dir()?)?;
    let state = root.path().join("state");
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    #[cfg(not(unix))]
    let _ = &mut builder;
    builder.create(&state)?;
    let state_directory = crate::infrastructure::filesystem::LibraryDirectory::open(&state)?;
    crate::infrastructure::filesystem::initialize_selection(&state_directory)?;
    let mut roots = Vec::new();
    for (root_id, role) in [("state", StorageRole::State), ("media", StorageRole::Media)] {
        if role == StorageRole::Media {
            std::fs::create_dir(root.path().join(root_id))?;
        }
        let (expected, _) =
            DirectoryExpectation::inspect(root.path(), &LibraryPath::parse(root_id)?)?;
        roots.push(StorageRoot {
            root_id: root_id.into(),
            binding_revision: 1,
            role,
            provider_scope: "application".into(),
            relative: root_id.into(),
            expected,
        });
    }
    let configuration = StorageConfiguration {
        format_version: 1,
        selection_revision: 1,
        providers: vec![ProviderMapping {
            scope: "application".into(),
            boundary: root.path().to_owned(),
        }],
        roots,
        expected_selection_file: StorageConfiguration::inspect_initial(
            root.path(),
            &LibraryPath::parse("state")?,
        )?
        .expected_selection_file,
        topology_assurances: Vec::new(),
    };
    let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
    Ok((root, state.join("state.db"), configuration, client))
}

#[test]
fn native_store_sources_keep_frozen_selection_with_offline_reservations() -> Result<()> {
    let (root, path, _configuration, client) = fixture()?;
    let media = root.path().join("media");
    std::fs::remove_dir(&media)?;
    let initial = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    let source = initial.source();
    let digest = client.selection_digest().to_owned();
    drop(initial);
    drop(client);
    std::thread::spawn(move || -> Result<()> {
        let reader = source.open(StoreOpenMode::ReadCurrent)?;
        drop(source);
        let source = reader.source();
        let filesystem = source.filesystem()?;
        assert_eq!(filesystem.selection_digest(), digest);
        filesystem.require_current(&"a".repeat(64))?;
        assert!(filesystem.require_current(&"b".repeat(64)).is_err());
        assert_eq!(filesystem.root_path("media", StorageRole::Media)?, media);
        assert!(filesystem.open("media", StorageRole::Media).is_err());
        assert!(!media.exists());
        Ok(())
    })
    .join()
    .map_err(|_| Error::Internal("selected Store owner panicked"))??;
    Ok(())
}

fn count(store: &Store) -> Result<i64> {
    Ok(store
        .connection
        .query_row("SELECT count(*) FROM native_store_probe", [], |row| {
            row.get(0)
        })?)
}

#[test]
fn selected_inputs_admit_the_actual_parent_before_opening_a_file() -> Result<()> {
    let (root, path, _, client) = fixture()?;
    let store = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    let source = store.source();
    let input = root.path().join("state").join("input.json");
    std::fs::write(&input, b"selected input")?;
    assert_eq!(source.read_input(&input, 32, false)?, b"selected input");
    assert!(matches!(
        source.read_input(&input, 4, false),
        Err(Error::InvalidInput(_))
    ));
    for outside in [
        root.path().join("media").join("missing.json"),
        root.path().join("missing.json"),
    ] {
        assert!(matches!(
            source.read_input(&outside, 32, false),
            Err(Error::Conflict(_))
        ));
        assert!(!outside.exists());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "private container probe; requires a fresh generated volume and exact parent invocation"]
fn logical_store_container_probe() -> Result<()> {
    let root = PathBuf::from(
        std::env::var_os("DOMUS_STORAGE_PROOF_ROOT")
            .ok_or_else(|| Error::InvalidInput("container proof root is missing".into()))?,
    );
    let mode = std::env::var("DOMUS_STORAGE_PROOF_MODE")
        .map_err(|_| Error::InvalidInput("container proof mode is missing".into()))?;
    if !root.is_absolute()
        || root.file_name() != Some(std::ffi::OsStr::new("proof"))
        || !matches!(
            mode.as_str(),
            "initialize" | "writer" | "reader" | "recreated"
        )
    {
        return Err(Error::InvalidInput(
            "invalid private container proof invocation".into(),
        ));
    }
    let wait = |name: &str| -> Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !root.join(name).try_exists()? {
            if std::time::Instant::now() >= deadline {
                return Err(Error::Busy(
                    "container proof peer did not reach its boundary",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(())
    };
    let app = root.join("app");
    let path = app.join("state").join("state.db");
    let configuration_path = app.join("selection.json");
    let mut configuration = if mode == "initialize" {
        if std::fs::read_dir(&root)?.next().is_some() {
            return Err(Error::Conflict(
                "container proof requires a fresh empty volume".into(),
            ));
        }
        use std::os::unix::fs::DirBuilderExt;
        for relative in ["app", "app/state", "media"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(relative))?;
        }
        let directory =
            crate::infrastructure::filesystem::LibraryDirectory::open(&app.join("state"))?;
        crate::infrastructure::filesystem::initialize_selection(&directory)?;
        let mut configuration =
            StorageConfiguration::inspect_initial(&app, &LibraryPath::parse("state")?)?;
        configuration.providers[0].boundary = root.clone();
        configuration.roots.clear();
        for (root_id, role, relative) in [
            ("application", StorageRole::Application, "app"),
            ("state", StorageRole::State, "app/state"),
            ("media", StorageRole::Media, "media"),
        ] {
            let (expected, _) =
                DirectoryExpectation::inspect(&root, &LibraryPath::parse(relative)?)?;
            configuration.roots.push(StorageRoot {
                root_id: root_id.into(),
                binding_revision: 1,
                role,
                provider_scope: "application".into(),
                relative: relative.into(),
                expected,
            });
        }
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&configuration_path)?;
        file.write_all(&serde_json::to_vec(&configuration)?)?;
        std::fs::write(
            root.join("media").join("generated.txt"),
            b"retained container media",
        )?;
        configuration
    } else {
        serde_json::from_slice::<StorageConfiguration>(&std::fs::read(&configuration_path)?)?
    };
    configuration.providers[0].boundary = root.clone();
    let client = FilesystemClient::new(
        configuration,
        if mode == "initialize" {
            "a".repeat(64)
        } else {
            "b".repeat(64)
        },
    )?;
    let _application = client.open_application(&app)?;
    let mut store = Store::open_scoped(
        &path,
        &client,
        if mode == "initialize" {
            StoreOpenMode::Create
        } else {
            StoreOpenMode::Current
        },
    )?;
    if mode == "initialize" {
        let media = root.join("media");
        let location =
            store.add_location(media.to_str().ok_or(Error::Internal("proof path"))?, "main")?;
        crate::application::scan_location(&mut store, location)?;
        store.connection.execute_batch(
            "CREATE TABLE native_store_probe(value INTEGER NOT NULL);
             INSERT INTO native_store_probe VALUES(1);",
        )?;
    } else {
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("initialize-result.json"))?)?;
        let saved: String = store.connection.query_row(
            "SELECT body FROM filesystem_roots WHERE root='storage-root:media'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&saved)?,
            original["logical_root"]
        );
        assert_eq!(store.location(1)?.root, "storage-root:media");
        match mode.as_str() {
            "writer" => {
                std::fs::write(root.join("writer-ready"), b"ready")?;
                wait("reader-ready")?;
                for value in 2..=21 {
                    store
                        .connection
                        .execute("INSERT INTO native_store_probe VALUES(?1)", [value])?;
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                wait("reader-done")?;
                assert_eq!(count(&store)?, 21);
            }
            "reader" => {
                wait("writer-ready")?;
                let snapshot = store.source().open(StoreOpenMode::Snapshot)?;
                assert_eq!(count(&snapshot)?, 1);
                std::fs::write(root.join("reader-ready"), b"ready")?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                while count(&store)? != 21 {
                    if std::time::Instant::now() >= deadline {
                        return Err(Error::Busy("container writer did not finish"));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                assert_eq!(count(&snapshot)?, 1);
                crate::application::scan_location(&mut store, 1)?;
                drop(snapshot);
                std::fs::write(root.join("reader-done"), b"done")?;
            }
            "recreated" => {
                assert_eq!(count(&store)?, 21);
                crate::application::check_location_availability(&store, 1)?;
                crate::application::scan_location(&mut store, 1)?;
                assert_eq!(
                    std::fs::read(root.join("media").join("generated.txt"))?,
                    b"retained container media"
                );
            }
            _ => unreachable!(),
        }
    }
    let saved: String = store.connection.query_row(
        "SELECT body FROM filesystem_roots WHERE root='storage-root:media'",
        [],
        |row| row.get(0),
    )?;
    let result = serde_json::json!({
        "mode":mode,
        "logical_root":serde_json::from_str::<serde_json::Value>(&saved)?,
        "native_root":store.open_registered_root("storage-root:media", "media")?.native_root(),
        "rows":count(&store)?,
    });
    use std::io::Write;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(format!("{mode}-result.json")))?;
    output.write_all(&serde_json::to_vec(&result)?)?;
    Ok(())
}

#[test]
fn logical_store_binding_survives_provider_relocation_without_rewriting_saved_roots() -> Result<()>
{
    let (root, path, mut configuration, client) = fixture()?;
    drop(client);
    for (root_id, role) in [
        ("cache", StorageRole::Cache),
        ("staging", StorageRole::Staging),
        ("certificate", StorageRole::Certificate),
    ] {
        let directory = root.path().join(root_id);
        std::fs::create_dir(&directory)?;
        crate::infrastructure::permissions::private_fixture::directory(&directory)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let (expected, _) =
            DirectoryExpectation::inspect(root.path(), &LibraryPath::parse(root_id)?)?;
        configuration.roots.push(StorageRoot {
            root_id: root_id.into(),
            binding_revision: 1,
            role,
            provider_scope: "application".into(),
            relative: root_id.into(),
            expected,
        });
    }
    let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
    let media = root.path().join("media");
    std::fs::write(media.join("sentinel.txt"), b"unchanged media")?;
    std::fs::create_dir(media.join("Album"))?;
    std::fs::write(
        media.join("Album").join("one.mp3"),
        b"generated shared bytes",
    )?;
    let mut store = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    let location = store.add_location(
        media
            .to_str()
            .ok_or(Error::Internal("fixture path is not Unicode"))?,
        "main",
    )?;
    crate::application::scan_location(&mut store, location)?;
    let inspected = crate::application::sharing::inspect_share_folder(
        &store,
        &crate::application::sharing::InspectShareFolder {
            location_id: location,
            relative_path: "Album".into(),
        },
    )?;
    let shared = crate::application::sharing::bind_share_folder(
        &store,
        &crate::application::sharing::BindShareFolder {
            location_id: location,
            relative_path: "Album".into(),
            expected_identity: inspected.native_identity.clone(),
        },
    )?;
    let policy = crate::domain::sharing::SharePolicy {
        access: crate::domain::sharing::ShareAccess::All,
        roots: vec![crate::domain::sharing::ShareRoot {
            binding_id: shared
                .binding_id
                .ok_or(Error::Internal("missing selected share binding"))?,
            alias: "Music".into(),
        }],
        ..Default::default()
    };
    let catalog = crate::infrastructure::shares::ShareCatalog::build(
        &store.source(),
        &policy,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;
    assert_eq!(catalog.counts()?, (1, 1));
    drop(catalog);
    let cache = crate::infrastructure::cache::FileCache::open(
        &mut store,
        &root.path().join("cache"),
        Default::default(),
    )?;
    drop(cache);
    let staging = crate::infrastructure::staging::StagingRoot::open(
        &mut store,
        &root.path().join("staging"),
    )?;
    let cleanup_binding = staging.cleanup_binding()?;
    drop(staging);
    let certificate = root.path().join("certificate");
    let directory = crate::infrastructure::filesystem::LibraryDirectory::open(&certificate)?;
    store.bind_certificate_root(&certificate, directory.native_root())?;
    drop(directory);
    let key = store.location(location)?.root;
    assert_eq!(key, "storage-root:media");
    let saved: (String, String) = store.connection.query_row(
        "SELECT identity, body FROM filesystem_roots WHERE root = ?1",
        [&key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert!(saved.0.starts_with("logical-v1:"));
    let body: serde_json::Value = serde_json::from_str(&saved.1)?;
    assert!(body.get("ancestors").is_none());
    assert!(body["root"].get("provider_scope").is_some());
    drop(store);
    drop(client);

    let parent = tempfile::Builder::new()
        .prefix(".relocated-store-")
        .tempdir_in(std::env::current_dir()?)?;
    let relocated = parent.path().join("provider");
    std::fs::rename(root.path(), &relocated)?;
    configuration.providers[0].boundary = relocated.clone();
    let client = FilesystemClient::new(configuration, "b".repeat(64))?;
    let mut store = Store::open_scoped(
        &relocated.join("state").join("state.db"),
        &client,
        StoreOpenMode::Current,
    )?;
    crate::application::check_location_availability(&store, location)?;
    crate::application::scan_location(&mut store, location)?;
    let inspected_again = crate::application::sharing::inspect_share_folder(
        &store,
        &crate::application::sharing::InspectShareFolder {
            location_id: location,
            relative_path: "Album".into(),
        },
    )?;
    assert!(inspected_again.bound);
    assert_eq!(inspected_again.native_identity, inspected.native_identity);
    let catalog = crate::infrastructure::shares::ShareCatalog::build(
        &store.source(),
        &policy,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;
    assert_eq!(catalog.counts()?, (1, 1));
    drop(catalog);
    let cache = crate::infrastructure::cache::FileCache::open(
        &mut store,
        &relocated.join("cache"),
        Default::default(),
    )?;
    drop(cache);
    let staging =
        crate::infrastructure::staging::StagingRoot::open(&mut store, &relocated.join("staging"))?;
    assert_eq!(staging.cleanup_binding()?, cleanup_binding);
    drop(staging);
    store.inspect_certificate_root(&relocated.join("certificate"))?;
    let reopened: (String, String) = store.connection.query_row(
        "SELECT identity, body FROM filesystem_roots WHERE root = ?1",
        [&key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(reopened, saved);
    assert_eq!(store.location(location)?.root, key);
    assert_eq!(
        std::fs::read(relocated.join("media").join("sentinel.txt"))?,
        b"unchanged media"
    );
    Ok(())
}

#[test]
fn logical_store_binding_refuses_a_changed_root_without_rebinding_or_losing_observations()
-> Result<()> {
    let (root, path, mut configuration, client) = fixture()?;
    let media = root.path().join("media");
    std::fs::write(media.join("sentinel.txt"), b"unchanged media")?;
    let mut store = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    let location = store.add_location(
        media
            .to_str()
            .ok_or(Error::Internal("fixture path is not Unicode"))?,
        "main",
    )?;
    crate::application::scan_location(&mut store, location)?;
    let body: String = store.connection.query_row(
        "SELECT body FROM filesystem_roots WHERE root = 'storage-root:media'",
        [],
        |row| row.get(0),
    )?;
    let retained = store.open_registered_root("storage-root:media", "media")?;
    drop(store);
    drop(client);
    configuration
        .roots
        .iter_mut()
        .find(|root| root.role == StorageRole::Media)
        .ok_or(Error::Internal("missing fixture media"))?
        .binding_revision += 1;
    let changed = FilesystemClient::new(configuration, "b".repeat(64))?;
    assert!(Store::open_scoped(&path, &changed, StoreOpenMode::Current).is_err());
    drop(retained);
    let mut store = Store::open_scoped(&path, &changed, StoreOpenMode::Current)?;
    assert!(crate::application::scan_location(&mut store, location).is_err());
    assert_ne!(store.location(location)?.coverage, "complete");
    let observations: i64 = store.connection.query_row(
        "SELECT count(*) FROM entries WHERE location_id = ?1 AND present = 1",
        [location],
        |row| row.get(0),
    )?;
    assert_eq!(observations, 1);
    let unchanged: String = store.connection.query_row(
        "SELECT body FROM filesystem_roots WHERE root = 'storage-root:media'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(unchanged, body);
    assert_eq!(
        std::fs::read(media.join("sentinel.txt"))?,
        b"unchanged media"
    );
    Ok(())
}

#[test]
fn logical_store_never_adopts_an_existing_native_binding() -> Result<()> {
    let (root, path, _configuration, client) = fixture()?;
    let media = root.path().join("media");
    let mut legacy = Store::open(&path)?;
    let location = legacy.add_location(
        media
            .to_str()
            .ok_or(Error::Internal("fixture path is not Unicode"))?,
        "main",
    )?;
    crate::application::scan_location(&mut legacy, location)?;
    drop(legacy);
    let mut selected = Store::open_scoped(&path, &client, StoreOpenMode::Current)?;
    assert!(crate::application::scan_location(&mut selected, location).is_err());
    assert_ne!(selected.location(location)?.coverage, "complete");
    let body: String =
        selected
            .connection
            .query_row("SELECT body FROM filesystem_roots", [], |row| row.get(0))?;
    let saved: serde_json::Value = serde_json::from_str(&body)?;
    assert!(saved.get("ancestors").is_some());
    assert!(saved.get("format_version").is_none());
    Ok(())
}

#[test]
fn native_store_preserves_modes_and_independent_snapshots() -> Result<()> {
    let (_root, path, _configuration, client) = fixture()?;
    assert!(Store::open_scoped(&path, &client, StoreOpenMode::Current).is_err());
    assert!(!path.exists());
    let writer = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    writer.connection.execute_batch(
        "CREATE TABLE native_store_probe(value INTEGER); INSERT INTO native_store_probe VALUES(1);",
    )?;
    let snapshot = writer.source().open(StoreOpenMode::Snapshot)?;
    assert_eq!(count(&snapshot)?, 1);
    writer
        .connection
        .execute("INSERT INTO native_store_probe VALUES(2)", [])?;
    assert_eq!(count(&snapshot)?, 1);
    for mode in [
        StoreOpenMode::Existing,
        StoreOpenMode::Current,
        StoreOpenMode::ReadCurrent,
    ] {
        let reader = Store::open_scoped(&path, &client, mode)?;
        assert_eq!(count(&reader)?, 2);
    }
    let source = writer.source();
    std::thread::spawn(move || -> Result<()> {
        let store = source.open(StoreOpenMode::Current)?;
        store
            .connection
            .execute("INSERT INTO native_store_probe VALUES(3)", [])?;
        Ok(())
    })
    .join()
    .map_err(|_| Error::Internal("independent Store owner panicked"))??;
    assert_eq!(count(&writer)?, 3);
    assert_eq!(count(&snapshot)?, 1);
    assert!(
        snapshot
            .connection
            .execute("INSERT INTO native_store_probe VALUES(3)", [])
            .is_err()
    );
    assert_eq!(
        writer
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))?,
        "wal"
    );
    for (pragma, expected) in [
        ("foreign_keys", 1),
        ("synchronous", 2),
        ("busy_timeout", 5000),
    ] {
        assert_eq!(
            writer
                .connection
                .pragma_query_value(None, pragma, |row| row.get::<_, i64>(0))?,
            expected
        );
    }
    Ok(())
}

#[test]
fn native_store_selected_existing_modes_preserve_legacy_schema() -> Result<()> {
    let (_root, path, _configuration, client) = fixture()?;
    let initial = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    initial.connection.execute_batch(
        "CREATE TABLE retained_evidence(value TEXT);
         INSERT INTO retained_evidence VALUES('unchanged');
         PRAGMA user_version=12;",
    )?;
    let source = initial.source();
    drop(initial);
    let before = std::fs::read(&path)?;
    for mode in [
        StoreOpenMode::Existing,
        StoreOpenMode::Current,
        StoreOpenMode::ReadCurrent,
        StoreOpenMode::Snapshot,
    ] {
        assert!(source.open(mode).is_err());
        assert!(Store::open_scoped(&path, &client, mode).is_err());
        assert_eq!(std::fs::read(&path)?, before);
    }
    let raw =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        12
    );
    assert_eq!(
        raw.query_row("SELECT value FROM retained_evidence", [], |row| row
            .get::<_, String>(0))?,
        "unchanged"
    );
    Ok(())
}

#[test]
fn native_store_spills_temp_data_inside_its_private_child() -> Result<()> {
    let (_root, path, _configuration, client) = fixture()?;
    let store = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    store.connection.execute_batch(
        "PRAGMA temp_store=FILE; PRAGMA temp.cache_size=8;
         CREATE TEMP TABLE spill(value);
         WITH RECURSIVE source(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM source WHERE x<2048)
         INSERT INTO spill SELECT hex(zeroblob(512)) FROM source;",
    )?;
    let statistics = store.connection.context().statistics();
    assert!(statistics.temporary_opens > 0);
    assert!(statistics.temporary_bytes_written > 0);
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM spill", [], |row| row.get::<_, i64>(0))?,
        2048
    );
    store.connection.context().check()?;
    Ok(())
}

#[test]
fn native_store_refuses_replaced_roots_and_missing_temp_without_repair() -> Result<()> {
    let (root, path, configuration, client) = fixture()?;
    drop(Store::open_scoped(&path, &client, StoreOpenMode::Create)?);
    drop(client);
    let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
    let state = path
        .parent()
        .ok_or(Error::Internal("fixture database parent"))?;
    let before = std::fs::read(&path)?;
    std::fs::rename(state.join("sqlite-tmp"), state.join("retained-tmp"))?;
    for mode in [
        StoreOpenMode::Existing,
        StoreOpenMode::Current,
        StoreOpenMode::ReadCurrent,
        StoreOpenMode::Snapshot,
    ] {
        assert!(Store::open_scoped(&path, &client, mode).is_err());
        assert!(!state.join("sqlite-tmp").exists());
        assert_eq!(std::fs::read(&path)?, before);
    }
    drop(client);
    std::fs::rename(state, root.path().join("original-state"))?;
    std::fs::create_dir(state)?;
    std::fs::write(&path, &before)?;
    let client = FilesystemClient::new(configuration, "a".repeat(64))?;
    assert!(Store::open_scoped(&path, &client, StoreOpenMode::Create).is_err());
    assert_eq!(std::fs::read(&path)?, before);
    assert!(!state.join("sqlite-tmp").exists());
    Ok(())
}

#[test]
fn native_store_source_keeps_state_locks_on_admitted_authority() -> Result<()> {
    let (root, path, _configuration, client) = fixture()?;
    let initial = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    let source = initial.source();
    drop(initial);
    let state = path
        .parent()
        .ok_or(Error::Internal("fixture database parent"))?;
    let retained = root.path().join("retained-state");
    #[cfg(windows)]
    {
        assert!(std::fs::rename(state, &retained).is_err());
        assert!(std::fs::rename(state.join("sqlite-tmp"), state.join("retained-tmp")).is_err());
        let _lock = source.state_lock("native-probe")?;
        assert!(state.join(".domus-native-probe.lock").is_file());
        assert!(source.open(StoreOpenMode::ReadCurrent).is_ok());
    }
    #[cfg(unix)]
    {
        std::fs::rename(state, &retained)?;
        std::fs::create_dir(state)?;
        assert!(source.open(StoreOpenMode::Current).is_err());
        assert!(source.state_lock("native-probe").is_err());
        assert!(!state.join(".domus-native-probe.lock").exists());
        assert!(!retained.join(".domus-native-probe.lock").exists());
        assert_eq!(std::fs::read_dir(state)?.count(), 0);
    }
    Ok(())
}

#[test]
fn native_store_fences_changed_selection_through_filesystem_and_lock_owners() -> Result<()> {
    let (_root, path, mut configuration, client) = fixture()?;
    let initial = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    let equivalent = FilesystemClient::new(configuration.clone(), "b".repeat(64))?;
    drop(Store::open_scoped(
        &path,
        &equivalent,
        StoreOpenMode::ReadCurrent,
    )?);
    drop(equivalent);
    let source = initial.source();
    let lock = source.state_lock("selection-probe")?;
    drop(initial);
    drop(source);
    configuration.selection_revision += 1;
    let changed = FilesystemClient::new(configuration, "b".repeat(64))?;
    assert!(matches!(
        Store::open_scoped(&path, &changed, StoreOpenMode::ReadCurrent),
        Err(Error::NativeStorage(native_sqlite::Error::SelectionInUse))
    ));
    drop(client);
    assert!(matches!(
        Store::open_scoped(&path, &changed, StoreOpenMode::ReadCurrent),
        Err(Error::NativeStorage(native_sqlite::Error::SelectionInUse))
    ));
    drop(lock);
    drop(Store::open_scoped(
        &path,
        &changed,
        StoreOpenMode::ReadCurrent,
    )?);
    Ok(())
}

#[test]
fn native_store_fences_topology_assurances_separately_from_their_selection_digest() -> Result<()> {
    use crate::infrastructure::filesystem::{RootRevision, TopologyAssurance};
    let (_root, path, mut configuration, client) = fixture()?;
    let initial = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    let digest = configuration.selection_digest()?;
    configuration.topology_assurances.push(TopologyAssurance {
        left: RootRevision {
            root_id: "media".into(),
            binding_revision: 1,
        },
        right: RootRevision {
            root_id: "state".into(),
            binding_revision: 1,
        },
        selection_digest: digest.clone(),
    });
    assert_eq!(configuration.selection_digest()?, digest);
    let changed = FilesystemClient::new(configuration, "b".repeat(64))?;
    assert!(matches!(
        Store::open_scoped(&path, &changed, StoreOpenMode::ReadCurrent),
        Err(Error::NativeStorage(native_sqlite::Error::SelectionInUse))
    ));
    drop(initial);
    drop(client);
    drop(Store::open_scoped(
        &path,
        &changed,
        StoreOpenMode::ReadCurrent,
    )?);
    Ok(())
}

#[test]
fn native_store_refuses_missing_authority_and_a_copied_selection_record() -> Result<()> {
    use std::io::Write;
    let (_root, path, configuration, client) = fixture()?;
    let state = path
        .parent()
        .ok_or(Error::Internal("fixture state parent"))?;
    let selection = state.join(native_sqlite::SELECTION_FILE);
    let initial_record = std::fs::read(&selection)?;
    let mut unbound = configuration.clone();
    unbound.expected_selection_file = None;
    let missing = FilesystemClient::new(unbound, "b".repeat(64))?;
    assert!(Store::open_scoped(&path, &missing, StoreOpenMode::Create).is_err());
    assert!(!path.exists());
    assert!(!state.join("sqlite-tmp").exists());
    assert_eq!(std::fs::read(&selection)?, initial_record);
    let store = Store::open_scoped(&path, &client, StoreOpenMode::Create)?;
    drop(store);
    let before = std::fs::read(&path)?;
    let record = std::fs::read(&selection)?;
    // Release every owner before changing only this generated fixture.
    drop(client);
    std::fs::rename(&selection, state.join("retained-selection"))?;
    let directory = crate::infrastructure::filesystem::LibraryDirectory::open(state)?;
    crate::infrastructure::filesystem::initialize_selection(&directory)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&selection)?
        .write_all(&record)?;
    let copied = FilesystemClient::new(configuration, "a".repeat(64))?;
    assert!(Store::open_scoped(&path, &copied, StoreOpenMode::ReadCurrent).is_err());
    assert_eq!(std::fs::read(&selection)?, record);
    assert_eq!(std::fs::read(&path)?, before);
    Ok(())
}

#[test]
fn native_store_refuses_reserved_database_names_before_fresh_effects() -> Result<()> {
    let (root, path, _configuration, client) = fixture()?;
    let state = path
        .parent()
        .ok_or(Error::Internal("fixture state parent"))?;
    let selection = state.join(native_sqlite::SELECTION_FILE);
    let record = std::fs::read(&selection)?;
    let bare = root.path().join("unselected");
    std::fs::create_dir(&bare)?;
    for name in [
        native_sqlite::SELECTION_FILE.to_owned(),
        native_sqlite::SELECTION_FILE.to_ascii_uppercase(),
    ] {
        assert!(Store::open_scoped(&state.join(&name), &client, StoreOpenMode::Create).is_err());
        assert!(Store::open(&bare.join(&name)).is_err());
        assert_eq!(std::fs::read(&selection)?, record);
        assert_eq!(std::fs::read_dir(state)?.count(), 1);
        assert_eq!(std::fs::read_dir(&bare)?.count(), 0);
    }
    Ok(())
}
