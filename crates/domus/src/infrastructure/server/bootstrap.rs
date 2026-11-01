//! First-run local state is an explicit launch effect, never passive validation.
use super::PreparedServer;
use crate::infrastructure::filesystem::{LibraryDirectory, rename_no_replace, sync_directory};
use crate::infrastructure::local_file::ProcessLock;
use crate::infrastructure::permissions::FilePermissions;
use crate::infrastructure::store::Store;
use crate::{Error, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) fn prepare() -> Result<PreparedServer> {
    let parent = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
    }
    .ok_or_else(|| {
        Error::InvalidInput("local application-data directory is not configured".into())
    })?;
    let executable = std::env::current_exe()?;
    let packaged = executable
        .parent()
        .ok_or(Error::Internal("executable parent is missing"))?
        .join("browser");
    let assets = if packaged.is_dir() {
        packaged
    } else if cfg!(debug_assertions) {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .ok_or(Error::Internal("development workspace root is missing"))?
            .join("frontend")
            .join("dist-app")
    } else {
        return Err(Error::InvalidInput(
            "packaged browser assets are missing beside the executable".into(),
        ));
    };
    // Validate assets before creating any persistent state.
    crate::infrastructure::browser::BrowserAssets::load(&assets)?;
    ensure_parent(&parent)?;
    prepare_in(&parent, &assets)
}

fn ensure_parent(path: &Path) -> Result<()> {
    crate::infrastructure::local_file::validate_reference(path)?;
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        match LibraryDirectory::open(existing) {
            Ok(directory) => {
                FilePermissions::current()?
                    .check(&directory.into_directory().into_std_file(), false)?;
                break;
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
    }
    let mut current = existing.to_path_buf();
    for name in missing.into_iter().rev() {
        let directory = LibraryDirectory::open(&current)?.into_directory();
        match directory.create_dir_with(&name, &private_directory_builder()) {
            Ok(()) => sync_directory(&directory)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        current.push(name);
        let created = LibraryDirectory::open(&current)?.into_directory();
        FilePermissions::current()?.check(&created.into_std_file(), true)?;
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

pub(super) fn prepare_in(parent: &Path, assets: &Path) -> Result<PreparedServer> {
    let directory = LibraryDirectory::open(parent)?.into_directory();
    let permissions = FilePermissions::current()?;
    permissions.check(&directory.try_clone()?.into_std_file(), false)?;
    let name = "DomusObscura";
    match directory.create_dir_with(name, &private_directory_builder()) {
        Ok(()) => sync_directory(&directory)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let root = parent.join(name);
    let directory = LibraryDirectory::open(&root)?.into_directory();
    permissions.check(&directory.try_clone()?.into_std_file(), true)?;
    let database = root.join("state.db");
    let _lock = ProcessLock::acquire(&database, "local-startup")?;
    let configuration = root.join("service.json");
    match directory.symlink_metadata("service.json") {
        Ok(_) => return PreparedServer::load(&configuration),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
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
        .open_with(".service.json.pending", &options)?
        .into_std();
    permissions.check(&pending, true)?;
    match directory.symlink_metadata("state.db") {
        Ok(_) => {
            let file =
                LibraryDirectory::open(&root)?.open_file(std::ffi::OsStr::new("state.db"))?;
            permissions.check(&file, false)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let store = Store::open(&database)?;
    drop(store);
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "environment": "single_user",
        "system_settings_read_only": true,
        "services": {
            "discovery": [], "metadata": [], "artwork": [], "acquisition": [], "artist_images": []
        },
        "database": database,
        "listen": "127.0.0.1:8080",
        "origin": "http://127.0.0.1:8080",
        "browser_assets": assets
    }))?;
    pending.write_all(&bytes)?;
    pending.write_all(b"\n")?;
    pending.sync_all()?;
    drop(pending);
    rename_no_replace(&directory, ".service.json.pending", "service.json")?;
    sync_directory(&directory)?;
    PreparedServer::load(&configuration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_user_first_launch_needs_no_tls_and_retains_state()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let assets = parent.path().join("browser");
        std::fs::create_dir(&assets)?;
        std::fs::create_dir(assets.join("assets"))?;
        std::fs::write(
            assets.join("index.html"),
            "<!doctype html><title>Domus</title>",
        )?;
        let first = prepare_in(parent.path(), &assets)?;
        assert!(first.tls.is_none());
        assert!(first.secret.is_none());
        assert!(first.configuration.certificate_file.is_none());
        assert!(first.configuration.private_key_file.is_none());
        let revision = first.configuration_source.active_revision().to_owned();
        drop(first);
        let next = prepare_in(parent.path(), &assets)?;
        assert_eq!(next.configuration_source.active_revision(), revision);
        assert!(next.configuration.system_settings_read_only);
        assert!(next.configuration.services.acquisition.is_empty());
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
        let root = parent.path().join("DomusObscura");
        std::fs::create_dir(&root)?;
        std::fs::write(root.join(".service.json.pending"), b"interrupted")?;
        assert!(prepare_in(parent.path(), parent.path()).is_err());
        assert_eq!(
            std::fs::read(root.join(".service.json.pending"))?,
            b"interrupted"
        );
        assert!(!root.join("service.json").exists());
        assert!(!root.join("state.db").exists());
        Ok(())
    }

    #[test]
    fn single_user_startup_keeps_invalid_existing_configuration()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let root = parent.path().join("DomusObscura");
        std::fs::create_dir(&root)?;
        let file = root.join("service.json");
        std::fs::write(&file, b"invalid saved secure configuration")?;
        assert!(prepare_in(parent.path(), parent.path()).is_err());
        assert_eq!(std::fs::read(&file)?, b"invalid saved secure configuration");
        assert!(!root.join("state.db").exists());
        Ok(())
    }
}
