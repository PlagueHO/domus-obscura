use super::{ConfigurationService, MAXIMUM};
use crate::infrastructure::filesystem::{LibraryDirectory, sync_directory};
use crate::infrastructure::permissions::FilePermissions;
use crate::infrastructure::store::StoreOpenMode;
use crate::{Error, Result};
use std::path::PathBuf;

/// A narrowly constructed document can be published only after all managed
/// children stop. Publication uncertainty retains evidence and leaves them stopped.
pub(super) struct PreparedConfigurationActivation {
    pub(super) path: PathBuf,
    pub(super) bytes: Vec<u8>,
    pub(super) expected: String,
    pub(super) directory: LibraryDirectory,
    pub(super) pending: String,
}

impl PreparedConfigurationActivation {
    pub(super) fn validate(&self) -> Result<ConfigurationService> {
        let mut configuration = ConfigurationService::parse(&self.bytes)?;
        ConfigurationService::resolve_paths(
            &mut configuration,
            self.path
                .parent()
                .ok_or(Error::Internal("configuration parent missing"))?,
        );
        ConfigurationService::normalize(&mut configuration).map_err(super::configuration_error)?;
        ConfigurationService::check_references(&configuration)
            .map_err(super::configuration_error)?;
        let source = ConfigurationService::new(&self.path, &self.bytes, configuration)?;
        let store = source.open_store(StoreOpenMode::ReadCurrent)?;
        store.require_primary_library_idle()?;
        source.check_files(&source.active, &store)?;
        if let Some(cache) = &source.active.cache {
            cache.inspect(&store, &source.protected(&source.active), true)?;
        }
        Ok(source)
    }

    pub(super) fn publish(&self, source: &ConfigurationService) -> Result<()> {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        use std::io::{Read, Write};
        FilePermissions::current()?.check(&self.directory.directory_file()?, true)?;
        let name = self
            .path
            .file_name()
            .ok_or(Error::Internal("configuration name missing"))?;
        let mut current = Vec::new();
        let file = self.directory.open_file(name)?;
        FilePermissions::current()?.check(&file, true)?;
        file.take(MAXIMUM + 1).read_to_end(&mut current)?;
        source.require_revision(&self.expected, &current)?;
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = self
            .directory
            .open_with(&self.pending, &options)?
            .into_std();
        FilePermissions::current()?.check(&file, true)?;
        file.write_all(&self.bytes)?;
        file.sync_all()?;
        drop(file);
        let current = source.read_file(&self.directory)?.1;
        source.require_revision(&self.expected, &current)?;
        self.directory
            .rename(&self.pending, &self.directory, name)?;
        sync_directory(&self.directory)?;
        if source.read_file(&self.directory)?.1 != self.bytes {
            return Err(Error::Conflict(
                "Configuration publication is unconfirmed. Preserve activation evidence and inspect the host before restarting.".into(),
            ));
        }
        Ok(())
    }
}
