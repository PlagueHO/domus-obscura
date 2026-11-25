//! First-run local state is an explicit launch effect, never passive validation.
use super::PreparedServer;
use crate::infrastructure::filesystem::{
    FilesystemClient, LibraryDirectory, StorageConfiguration, rename_no_replace, sync_directory,
};
use crate::infrastructure::local_file::ProcessLock;
use crate::infrastructure::permissions::FilePermissions;
use crate::infrastructure::store::Store;
use crate::{Error, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const APPLICATION_DIRECTORY: &str = if cfg!(windows) {
    "DomusObscura"
} else {
    "domus-obscura"
};
const CONFIGURATION: &str = "config.json";
const PENDING_CONFIGURATION: &str = ".config.json.pending";

pub(super) fn prepare() -> Result<PreparedServer> {
    let parent = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
    }
    .ok_or_else(|| {
        Error::InvalidInput("local application-data directory is not configured".into())
    })?;
    prepare_at(&parent.join(APPLICATION_DIRECTORY))
}

pub(super) fn prepare_at(root: &Path) -> Result<PreparedServer> {
    crate::infrastructure::local_file::validate_reference(root)?;
    let parent = root
        .parent()
        .filter(|_| root.file_name().is_some())
        .ok_or_else(|| {
            Error::InvalidInput("application storage requires a named directory".into())
        })?;
    let assets = browser_assets()?;
    ensure_parent(parent)?;
    prepare_root(root, &assets)
}

pub(super) fn initialize_at(root: &Path) -> Result<PathBuf> {
    crate::infrastructure::local_file::validate_reference(root)?;
    let parent = root
        .parent()
        .filter(|_| root.file_name().is_some())
        .ok_or_else(|| {
            Error::InvalidInput("application storage requires a named directory".into())
        })?;
    let assets = browser_assets_path()?;
    ensure_parent(parent)?;
    let (configuration, _directory) = initialize_root(root, &assets, true)?;
    Ok(configuration)
}

fn browser_assets_path() -> Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let packaged = executable
        .parent()
        .ok_or(Error::Internal("executable parent is missing"))?
        .join("browser");
    if packaged.is_dir() || !cfg!(debug_assertions) {
        Ok(packaged)
    } else {
        Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .ok_or(Error::Internal("development workspace root is missing"))?
            .join("frontend")
            .join("dist-app"))
    }
}

fn browser_assets() -> Result<PathBuf> {
    let assets = browser_assets_path()?;
    if !cfg!(debug_assertions) && !assets.is_dir() {
        return Err(Error::InvalidInput(
            "packaged browser assets are missing beside the executable".into(),
        ));
    }
    // Validate assets before creating any persistent state.
    crate::infrastructure::browser::BrowserAssets::load(&assets)?;
    Ok(assets)
}

fn ensure_parent(path: &Path) -> Result<()> {
    crate::infrastructure::local_file::validate_reference(path)?;
    let mut missing = Vec::new();
    let mut existing = path;
    let mut directory = loop {
        match LibraryDirectory::open(existing) {
            Ok(directory) => {
                directory.require_local()?;
                FilePermissions::current()?.check(&directory.directory_file()?, false)?;
                break directory;
            }
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    existing
                        .file_name()
                        .ok_or(Error::Internal("data directory name is missing"))?
                        .to_owned(),
                );
                existing = existing
                    .parent()
                    .ok_or(Error::Internal("data directory parent is missing"))?;
                if missing.len() > 32 {
                    return Err(Error::InvalidInput(
                        "application-data directory exceeds the depth limit".into(),
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    };
    for name in missing.into_iter().rev() {
        directory.require_local()?;
        match directory.create_dir_with(&name, &private_directory_builder()) {
            Ok(()) => sync_directory(&directory)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        directory = directory.open_dir_nofollow(Path::new(&name))?;
        directory.require_local()?;
        FilePermissions::current()?.check(&directory.directory_file()?, true)?;
    }
    Ok(())
}

fn private_directory_builder() -> cap_std::fs::DirBuilder {
    let mut builder = cap_std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use cap_std::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    #[cfg(not(unix))]
    let _ = &mut builder;
    builder
}

fn prepare_root(root: &Path, assets: &Path) -> Result<PreparedServer> {
    let (configuration, _directory) = initialize_root(root, assets, false)?;
    PreparedServer::load(&configuration)
}

fn initialize_root(
    root: &Path,
    assets: &Path,
    fresh_only: bool,
) -> Result<(PathBuf, LibraryDirectory)> {
    initialize_root_options(root, assets, fresh_only, false)
}

pub(super) fn initialize_developer_at(root: &Path) -> Result<PathBuf> {
    crate::infrastructure::local_file::validate_reference(root)?;
    let parent = root
        .parent()
        .ok_or(Error::InvalidInput("setup parent missing".into()))?;
    let directory = LibraryDirectory::open(parent)?;
    let name = root
        .file_name()
        .ok_or(Error::InvalidInput("setup name missing".into()))?;
    match directory.symlink_metadata(name) {
        Ok(_) => {
            return Err(Error::Conflict(
                "fresh developer target exists; nothing was changed".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(initialize_root_options(root, &browser_assets()?, true, true)?.0)
}

fn initialize_root_options(
    root: &Path,
    assets: &Path,
    fresh_only: bool,
    developer: bool,
) -> Result<(PathBuf, LibraryDirectory)> {
    let parent = root
        .parent()
        .ok_or_else(|| Error::InvalidInput("application storage parent is missing".into()))?;
    let directory = LibraryDirectory::open(parent)?;
    directory.require_local()?;
    let permissions = FilePermissions::current()?;
    permissions.check(&directory.directory_file()?, false)?;
    let name = root
        .file_name()
        .ok_or_else(|| Error::InvalidInput("application storage name is missing".into()))?;
    match directory.create_dir_with(name, &private_directory_builder()) {
        Ok(()) => sync_directory(&directory)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let directory = directory.open_dir_nofollow(Path::new(name))?;
    directory.require_local()?;
    permissions.check(&directory.directory_file()?, true)?;
    let configuration = root.join(CONFIGURATION);
    match directory.symlink_metadata(CONFIGURATION) {
        Ok(_) if fresh_only => {
            return Err(Error::Conflict(
                "fresh setup requires empty application storage; existing configuration was preserved".into(),
            ));
        }
        Ok(_) => return Ok((configuration, directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    require_unused_root(&directory, false)?;
    let _lock = ProcessLock::acquire_in(&directory, "local-startup")?;
    // Another first launch can finish before this process acquires the lock.
    match directory.symlink_metadata(CONFIGURATION) {
        Ok(_) if fresh_only => {
            return Err(Error::Conflict(
                "fresh setup requires empty application storage; existing configuration was preserved".into(),
            ));
        }
        Ok(_) => return Ok((configuration, directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    require_unused_root(&directory, true)?;
    // Retain interrupted first-run output for inspection, never overwrite it.
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut pending = directory
        .open_with(PENDING_CONFIGURATION, &options)?
        .into_std();
    permissions.check(&pending, true)?;
    directory.create_dir_with("state", &private_directory_builder())?;
    sync_directory(&directory)?;
    let state = directory.open_dir_nofollow("state")?;
    state.require_local()?;
    permissions.check(&state.directory_file()?, true)?;
    crate::infrastructure::filesystem::initialize_selection(&state)?;
    let mut storage = StorageConfiguration::inspect_initial_in(
        &directory,
        Path::new("."),
        &crate::domain::LibraryPath::parse("state")?,
    )?;
    let mut document = serde_json::json!({
        "environment": "single_user",
        "system_settings_read_only": !developer,
        "services": {
            "discovery": [], "metadata": [], "artwork": [], "acquisition": [], "artist_images": []
        },
        "database": super::configuration::default_database(),
        "storage": storage,
        "listen": "127.0.0.1:8080",
        "origin": "http://127.0.0.1:8080",
        "browser_assets": assets
    });
    if developer {
        document["production_worker"] =
            serde_json::json!({"queue_capacity": 64, "acquisition": null});
    }
    let mut bytes = serde_json::to_vec_pretty(&document)?;
    bytes.push(b'\n');
    pending.write_all(&bytes)?;
    pending.sync_all()?;
    drop(pending);
    rename_no_replace(&directory, PENDING_CONFIGURATION, CONFIGURATION)?;
    sync_directory(&directory)?;
    storage.resolve_paths(root);
    let client = FilesystemClient::new(storage, super::configuration::digest(&bytes))?;
    let _application = client.open_application(root)?;
    let database = root.join(super::configuration::default_database());
    let _state = client.open_state(&database)?;
    let store = Store::open_scoped(
        &database,
        &client,
        crate::infrastructure::store::StoreOpenMode::Create,
    )?;
    drop(store);
    Ok((configuration, directory))
}

fn require_unused_root(directory: &LibraryDirectory, owns_lock: bool) -> Result<()> {
    for entry in directory.entries()? {
        let entry = entry?;
        if !owns_lock || entry.file_name() != ".domus-local-startup.lock" {
            return Err(Error::Conflict(
                "application storage contains existing state without configuration; select an empty directory for explicit fresh setup".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepare_in(parent: &Path, assets: &Path) -> Result<PreparedServer> {
        prepare_root(&parent.join(APPLICATION_DIRECTORY), assets)
    }

    fn storage_directory(parent: &Path) -> Result<PathBuf> {
        LibraryDirectory::open(parent)?
            .create_dir_with(APPLICATION_DIRECTORY, &private_directory_builder())?;
        Ok(parent.join(APPLICATION_DIRECTORY))
    }

    fn generated_browser(parent: &Path) -> Result<PathBuf> {
        let assets = parent.join("browser");
        std::fs::create_dir(&assets)?;
        std::fs::create_dir(assets.join("assets"))?;
        std::fs::write(
            assets.join("index.html"),
            "<!doctype html><title>Domus</title>",
        )?;
        Ok(assets)
    }

    #[test]
    fn single_user_first_launch_needs_no_tls_and_retains_state()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let assets = generated_browser(parent.path())?;
        let first = prepare_in(parent.path(), &assets)?;
        assert!(first.tls.is_none());
        assert!(first.secret.is_none());
        assert!(first.configuration.certificate_file.is_none());
        assert!(first.configuration.private_key_file.is_none());
        let revision = first.configuration_source.active_revision().to_owned();
        let root = parent.path().join(APPLICATION_DIRECTORY);
        assert_eq!(
            first.configuration.database,
            root.join("state").join("state.db")
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(CONFIGURATION))?)?;
        assert_eq!(
            saved["database"],
            serde_json::to_value(Path::new("state").join("state.db"))?
        );
        assert!(!root.join("service.json").exists());
        assert!(!root.join("state.db").exists());
        drop(first);
        let next = prepare_in(parent.path(), &assets)?;
        assert_eq!(next.configuration_source.active_revision(), revision);
        assert!(next.configuration.system_settings_read_only);
        assert!(next.configuration.services.acquisition.is_empty());
        Ok(())
    }

    #[test]
    fn single_user_explicit_storage_root_keeps_its_exact_name_and_relative_layout()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fixture = crate::infrastructure::permissions::private_test_directory()?;
        let assets = generated_browser(fixture.path())?;
        let root = fixture.path().join("selected-storage");
        let host = prepare_root(&root, &assets)?;
        assert_eq!(
            host.configuration.database,
            root.join("state").join("state.db")
        );
        let before = std::fs::read(root.join(CONFIGURATION))?;
        drop(host);
        let restarted = prepare_root(&root, &assets)?;
        assert_eq!(
            restarted.configuration.database,
            root.join("state").join("state.db")
        );
        assert_eq!(std::fs::read(root.join(CONFIGURATION))?, before);
        assert!(!root.join(APPLICATION_DIRECTORY).exists());
        assert!(!root.join("state.db").exists());
        assert!(!root.join("service.json").exists());
        assert!(!root.join(PENDING_CONFIGURATION).exists());
        Ok(())
    }

    #[test]
    fn single_user_default_database_is_resolved_without_rewriting_operator_json()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fixture = crate::infrastructure::permissions::private_test_directory()?;
        let assets = generated_browser(fixture.path())?;
        let root = fixture.path().join("selected-storage");
        drop(prepare_root(&root, &assets)?);
        let path = root.join(CONFIGURATION);
        let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        document
            .as_object_mut()
            .ok_or("configuration is not an object")?
            .remove("database");
        let bytes = serde_json::to_vec(&document)?;
        std::fs::write(&path, &bytes)?;
        let source = super::super::configuration::ConfigurationService::open(&path)?;
        assert!(source.validate_saved()?.errors.is_empty());
        let restarted = PreparedServer::load(&path)?;
        assert_eq!(
            restarted.configuration.database,
            root.join("state").join("state.db")
        );
        assert_eq!(std::fs::read(&path)?, bytes);
        assert!(!root.join("state.db").exists());
        Ok(())
    }

    #[test]
    fn single_user_external_binding_refuses_replaced_roots_before_sqlite_access()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for application in [false, true] {
            let fixture = crate::infrastructure::permissions::private_test_directory()?;
            let assets = generated_browser(fixture.path())?;
            let root = fixture.path().join("selected-storage");
            drop(prepare_root(&root, &assets)?);
            let configuration = std::fs::read(root.join(CONFIGURATION))?;
            let original = std::fs::read(root.join("state").join("state.db"))?;
            let retained = fixture.path().join("retained");
            if application {
                std::fs::rename(&root, &retained)?;
                LibraryDirectory::open(fixture.path())?
                    .create_dir_with("selected-storage", &private_directory_builder())?;
                std::fs::write(root.join(CONFIGURATION), &configuration)?;
                // Keep the original state leaf to catch a leaf-only comparison.
                std::fs::rename(retained.join("state"), root.join("state"))?;
            } else {
                std::fs::rename(root.join("state"), &retained)?;
                LibraryDirectory::open(&root)?
                    .create_dir_with("state", &private_directory_builder())?;
                std::fs::write(root.join("state").join("state.db"), b"replacement evidence")?;
            }
            assert!(matches!(
                PreparedServer::load(&root.join(CONFIGURATION)),
                Err(Error::Conflict(_))
            ));
            assert_eq!(std::fs::read(root.join(CONFIGURATION))?, configuration);
            if application {
                assert_eq!(
                    std::fs::read(root.join("state").join("state.db"))?,
                    original
                );
            } else {
                assert_eq!(std::fs::read(retained.join("state.db"))?, original);
                assert_eq!(
                    std::fs::read(root.join("state").join("state.db"))?,
                    b"replacement evidence"
                );
                assert_eq!(std::fs::read_dir(root.join("state"))?.count(), 1);
            }
        }
        Ok(())
    }

    #[test]
    fn single_user_creates_missing_data_parents_without_following_links()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let data = parent.path().join(".local").join("share");
        ensure_parent(&data)?;
        assert!(data.is_dir());
        ensure_parent(&data)?;
        Ok(())
    }

    #[test]
    fn single_user_ignores_inactive_secure_file_references()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let database = parent.path().join("state.db");
        Store::open(&database)?;
        let configuration = parent.path().join("service.json");
        let absent = parent.path().join("never-read").join("secret");
        std::fs::write(
            &configuration,
            serde_json::to_vec(&serde_json::json!({
                "environment": "single_user", "database": database,
                "storage": StorageConfiguration::inspect_initial(parent.path(), &crate::domain::LibraryPath::parse("")?)?,
                "listen": "127.0.0.1:8080", "origin": "http://127.0.0.1:8080",
                "services": { "discovery": [], "metadata": [], "artwork": [], "acquisition": [], "artist_images": [] },
                "client_secret_file": absent, "private_key_file": absent, "certificate_file": absent
            }))?,
        )?;
        let host = PreparedServer::load(&configuration)?;
        assert!(host.tls.is_none());
        assert!(host.secret.is_none());
        assert!(!absent.exists());
        Ok(())
    }

    #[test]
    fn single_user_keeps_interrupted_first_run_output()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let root = storage_directory(parent.path())?;
        std::fs::write(root.join(PENDING_CONFIGURATION), b"interrupted")?;
        assert!(prepare_in(parent.path(), parent.path()).is_err());
        assert_eq!(
            std::fs::read(root.join(PENDING_CONFIGURATION))?,
            b"interrupted"
        );
        assert!(!root.join(CONFIGURATION).exists());
        assert!(!root.join("state.db").exists());
        assert!(!root.join(".domus-local-startup.lock").exists());
        Ok(())
    }

    #[test]
    fn single_user_fresh_setup_refuses_existing_database_without_changes()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let root = storage_directory(parent.path())?;
        let database = root.join("state.db");
        drop(Store::open(&database)?);
        let before = std::fs::read(&database)?;
        let names = || -> std::io::Result<std::collections::BTreeSet<_>> {
            std::fs::read_dir(&root)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect()
        };
        let before_names = names()?;
        let result = prepare_in(parent.path(), parent.path());
        assert!(
            matches!(&result, Err(Error::Conflict(_))),
            "unexpected setup result: {:?}",
            result.err()
        );
        assert_eq!(std::fs::read(&database)?, before);
        assert_eq!(names()?, before_names);
        assert!(!root.join(CONFIGURATION).exists());
        Ok(())
    }

    #[test]
    fn single_user_fresh_setup_preserves_orphaned_files_and_media()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for name in [
            "state.db-wal",
            "state.db-shm",
            "state.db-journal",
            ".service.json.pending",
            ".config.json.pending",
            "service.json",
            "state",
            ".domus-local-startup.lock",
            "existing-media",
        ] {
            let parent = crate::infrastructure::permissions::private_test_directory()?;
            let root = storage_directory(parent.path())?;
            let existing = root.join(name);
            let evidence = if matches!(name, "existing-media" | "state") {
                std::fs::create_dir(&existing)?;
                existing.join("album.flac")
            } else {
                existing
            };
            std::fs::write(&evidence, b"preserve existing bytes")?;
            let result = prepare_in(parent.path(), parent.path());
            assert!(
                matches!(&result, Err(Error::Conflict(_))),
                "unexpected setup result for {name}: {:?}",
                result.err()
            );
            assert_eq!(std::fs::read(&evidence)?, b"preserve existing bytes");
            assert_eq!(std::fs::read_dir(&root)?.count(), 1);
            assert!(!root.join(CONFIGURATION).exists());
            assert!(!root.join("state.db").exists());
        }
        Ok(())
    }

    #[test]
    fn single_user_startup_keeps_invalid_existing_configuration()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let root = storage_directory(parent.path())?;
        let file = root.join(CONFIGURATION);
        std::fs::write(&file, b"invalid saved secure configuration")?;
        assert!(prepare_in(parent.path(), parent.path()).is_err());
        assert_eq!(std::fs::read(&file)?, b"invalid saved secure configuration");
        assert!(!root.join("state.db").exists());
        assert!(!root.join(".domus-local-startup.lock").exists());
        Ok(())
    }
}
