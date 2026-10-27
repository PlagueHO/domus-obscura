//! The sole access boundary for backend JSON. Validation never activates settings.
use super::load_tls;
use crate::application::services::ServiceConfiguration;
use crate::infrastructure::filesystem::{
    LibraryDirectory, NativeRoot, is_unsafe_link, native_file_identity, require_local_disk,
    sync_directory,
};
use crate::infrastructure::permissions::FilePermissions;
use crate::infrastructure::store::Store;
use crate::{Error, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use openidconnect::ClientSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAXIMUM: u64 = 16384;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Environment {
    #[default]
    Production,
    Development,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Configuration {
    #[serde(default)]
    pub(super) environment: Environment,
    #[serde(default = "system_settings_read_only_by_default")]
    pub(super) system_settings_read_only: bool,
    #[serde(default)]
    pub(super) services: ServiceConfiguration,
    pub(super) cache: Option<crate::infrastructure::cache::CacheConfiguration>,
    pub(super) database: PathBuf,
    pub(super) listen: std::net::SocketAddr,
    pub(super) origin: String,
    pub(super) tenant_id: Option<String>,
    pub(super) object_id: Option<String>,
    pub(super) client_id: Option<String>,
    pub(super) client_secret_file: Option<PathBuf>,
    pub(super) certificate_file: PathBuf,
    pub(super) private_key_file: PathBuf,
    pub(super) browser_assets: Option<PathBuf>,
    pub(super) local_acquisition: Option<crate::infrastructure::worker::LocalAcquisition>,
}

fn system_settings_read_only_by_default() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendCacheConfiguration {
    pub root: String,
    pub item_bytes: u32,
    pub total_bytes: u64,
    pub entries: u16,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendLocalAcquisition {
    pub staging_root: String,
    pub credentials_file_configured: bool,
    pub server: String,
    pub acknowledge_plaintext_login: bool,
}

/// Hidden fields never become browser-readable, including after replacement.
#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendConfiguration {
    pub environment: String,
    pub system_settings_read_only: bool,
    pub services: ServiceConfiguration,
    pub cache: Option<BackendCacheConfiguration>,
    pub database: String,
    pub listen: String,
    pub origin: String,
    pub tenant_id_configured: bool,
    pub object_id_configured: bool,
    pub client_id_configured: bool,
    pub client_secret_file_configured: bool,
    pub certificate_file: String,
    pub private_key_file_configured: bool,
    pub browser_assets: Option<String>,
    pub local_acquisition: Option<BackendLocalAcquisition>,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendConfigurationState {
    pub active: BackendConfiguration,
    pub saved: BackendConfiguration,
    pub active_revision: String,
    pub saved_revision: String,
    pub restart_required: bool,
    /// False means this exact revision passed passive validation, not activation.
    pub saved_requires_validation: bool,
    pub changed_sections: Vec<ConfigurationSection>,
    pub reload: ConfigurationReloadState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationSection {
    General,
    Identity,
    Tls,
    Storage,
    MusicProviders,
    MusicAcquisition,
}

/// Each variant owns a complete section. Omitted sections are never replaced.
#[derive(Deserialize, ts_rs::TS)]
#[serde(tag = "section", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigurationSectionUpdate {
    General {
        environment: String,
        listen: String,
        origin: String,
        browser_assets: Option<String>,
    },
    Identity {
        tenant_id: ConfigurationReplacement,
        object_id: ConfigurationReplacement,
        client_id: ConfigurationReplacement,
        client_secret_file: ConfigurationReplacement,
    },
    Tls {
        certificate_file: String,
        private_key_file: ConfigurationReplacement,
    },
    Storage {
        database: String,
        cache: Option<BackendCacheConfiguration>,
    },
    MusicProviders {
        services: ServiceConfiguration,
    },
    MusicAcquisition {
        local_acquisition: Option<BackendLocalAcquisitionDraft>,
    },
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSectionCommand {
    pub expected_revision: String,
    pub sections: Vec<ConfigurationSectionUpdate>,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationReloadCommand {
    pub expected_revision: String,
}

#[derive(Clone, Default, Serialize, ts_rs::TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConfigurationReloadState {
    #[default]
    Idle,
    Pending {
        revision: String,
        origin: String,
    },
    Active {
        revision: String,
    },
    Failed {
        revision: String,
        message: String,
    },
}

#[derive(Default)]
struct ReloadControl {
    sender: Option<tokio::sync::mpsc::Sender<String>>,
    state: ConfigurationReloadState,
}

enum ConfigurationUpdate {
    Full(Box<BackendConfigurationDraft>),
    Sections(Vec<ConfigurationSectionUpdate>),
}

#[derive(Default, Deserialize, ts_rs::TS)]
#[serde(
    tag = "action",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ConfigurationReplacement {
    #[default]
    Keep,
    Replace(String),
    Clear,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendLocalAcquisitionDraft {
    pub staging_root: String,
    pub credentials_file: ConfigurationReplacement,
    pub server: String,
    pub acknowledge_plaintext_login: bool,
}

/// This contract intentionally has no operator-policy field or raw secret value.
#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendConfigurationDraft {
    pub environment: String,
    pub services: ServiceConfiguration,
    pub cache: Option<BackendCacheConfiguration>,
    pub database: String,
    pub listen: String,
    pub origin: String,
    pub tenant_id: ConfigurationReplacement,
    pub object_id: ConfigurationReplacement,
    pub client_id: ConfigurationReplacement,
    pub client_secret_file: ConfigurationReplacement,
    pub certificate_file: String,
    pub private_key_file: ConfigurationReplacement,
    pub browser_assets: Option<String>,
    pub local_acquisition: Option<BackendLocalAcquisitionDraft>,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendConfigurationCommand {
    pub expected_revision: String,
    pub draft: BackendConfigurationDraft,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendConfigurationIssue {
    pub field: String,
    pub message: String,
}

#[derive(Serialize, ts_rs::TS)]
pub struct BackendConfigurationResult {
    pub errors: Vec<BackendConfigurationIssue>,
    /// Only a successful save returns saved state. Validation does not save.
    pub state: Option<BackendConfigurationState>,
}

/// Startup and HTTP share this service. Its native source and active snapshot
/// cannot be changed by a caller. All updates are restart-only.
pub(crate) struct ConfigurationService {
    path: PathBuf,
    parent: NativeRoot,
    pub(super) active: Configuration,
    active_document: Configuration,
    active_revision: String,
    validated_revision: Mutex<Option<String>>,
    writer: Arc<Mutex<Option<File>>>,
    reload: Arc<Mutex<ReloadControl>>,
    #[cfg(test)]
    failure: Mutex<Option<SaveFailure>>,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveFailure {
    Write,
    Sync,
    Replace,
    AfterReplace,
    CrashAfterSync,
}

impl ConfigurationService {
    pub(crate) fn output_settings(
        store: &Store,
    ) -> Result<Option<crate::infrastructure::store::SettingsVersion>> {
        store.settings()
    }

    pub(crate) fn save_output_settings(
        store: &mut Store,
        expected: Option<i64>,
        settings: &crate::domain::OutputSettings,
    ) -> Result<i64> {
        store.save_settings(expected, settings)
    }

    pub(crate) fn library_roots(
        store: &Store,
        page: crate::domain::Page,
    ) -> Result<Vec<crate::infrastructure::store::Location>> {
        store.locations(page)
    }

    pub(crate) fn register_library_root(
        store: &mut Store,
        root: &str,
        kind: &str,
    ) -> Result<crate::infrastructure::store::Location> {
        if root.len() > 4096 || !Path::new(root).is_absolute() {
            return Err(Error::InvalidInput(
                "library root must be an absolute server path of at most 4096 bytes".into(),
            ));
        }
        let id = crate::application::add_location(store, Path::new(root), kind)?;
        store.location(id)
    }

    pub(crate) fn require_editable(&self) -> Result<()> {
        let control = self
            .reload
            .lock()
            .map_err(|_| Error::Internal("configuration reload lock failed"))?;
        if matches!(control.state, ConfigurationReloadState::Pending { .. }) {
            return Err(Error::Busy("configuration reload is pending"));
        }
        let (_, bytes) = self.read_file(&self.directory()?)?;
        if self.active.system_settings_read_only || Self::parse(&bytes)?.system_settings_read_only {
            return Err(Error::Conflict("system settings are read-only".into()));
        }
        Ok(())
    }

    pub(super) fn load(path: &Path) -> Result<(Self, Configuration, String)> {
        valid_reference(path)?;
        let bytes = crate::infrastructure::local_file::read_local(path, MAXIMUM, false)?;
        let mut configuration = Self::parse(&bytes)?;
        let hostname = Self::normalize(&mut configuration).map_err(configuration_error)?;
        Self::check_references(&configuration).map_err(configuration_error)?;
        let service = Self::new(path, &bytes, configuration.clone())?;
        Ok((service, configuration, hostname))
    }

    fn new(path: &Path, bytes: &[u8], active: Configuration) -> Result<Self> {
        let name = path.file_name().and_then(|name| name.to_str());
        if name.is_none_or(|name| name.contains([':', '\\', '/']) || name.is_empty()) {
            return Err(Error::InvalidInput(
                "configuration file name is invalid".into(),
            ));
        }
        let parent = LibraryDirectory::open(
            path.parent()
                .ok_or(Error::Internal("configuration parent is missing"))?,
        )?
        .native_root()
        .clone();
        Ok(Self {
            path: path.to_owned(),
            parent,
            active,
            active_document: Self::parse(bytes)?,
            active_revision: digest(bytes),
            validated_revision: Mutex::new(None),
            writer: Arc::new(Mutex::new(None)),
            reload: Arc::new(Mutex::new(ReloadControl::default())),
            #[cfg(test)]
            failure: Mutex::new(None),
        })
    }

    fn parse(bytes: &[u8]) -> Result<Configuration> {
        serde_json::from_slice(bytes)
            .map_err(|_| Error::InvalidInput("saved backend configuration JSON is invalid".into()))
    }

    fn directory(&self) -> Result<LibraryDirectory> {
        let directory = LibraryDirectory::open(
            self.path
                .parent()
                .ok_or(Error::Internal("configuration parent is missing"))?,
        )?;
        if directory.native_root() != &self.parent {
            return Err(Error::Conflict(
                "configuration parent identity changed".into(),
            ));
        }
        Ok(directory)
    }

    fn read_file(&self, directory: &LibraryDirectory) -> Result<(File, Vec<u8>)> {
        let file = directory.open_file(
            self.path
                .file_name()
                .ok_or(Error::Internal("configuration name is missing"))?,
        )?;
        FilePermissions::current()?.check(&file, false)?;
        let mut bytes = Vec::new();
        (&file).take(MAXIMUM + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAXIMUM {
            return Err(Error::InvalidInput("configuration exceeds 16 KiB".into()));
        }
        Ok((file, bytes))
    }

    pub(crate) fn read(&self) -> Result<BackendConfigurationState> {
        let (_, bytes) = self.read_file(&self.directory()?)?;
        self.state(&bytes)
    }

    fn state(&self, bytes: &[u8]) -> Result<BackendConfigurationState> {
        let saved = Self::parse(bytes)?;
        let revision = digest(bytes);
        let validated = self
            .validated_revision
            .lock()
            .map_err(|_| Error::Internal("configuration validation lock failed"))?;
        Ok(BackendConfigurationState {
            active: summary(&self.active),
            saved: summary(&saved),
            active_revision: self.active_revision.clone(),
            restart_required: revision != self.active_revision,
            saved_requires_validation: revision != self.active_revision
                && validated.as_ref() != Some(&revision),
            saved_revision: revision,
            changed_sections: Self::changed_sections(&self.active_document, &saved)?,
            reload: self
                .reload
                .lock()
                .map_err(|_| Error::Internal("configuration reload lock failed"))?
                .state
                .clone(),
        })
    }

    /// Does not create a lock file, mutate SQLite, or retain submitted secrets.
    pub(crate) fn validate(
        &self,
        command: BackendConfigurationCommand,
    ) -> Result<BackendConfigurationResult> {
        self.validate_update(
            command.expected_revision,
            ConfigurationUpdate::Full(Box::new(command.draft)),
        )
    }

    pub(crate) fn validate_sections(
        &self,
        command: ConfigurationSectionCommand,
    ) -> Result<BackendConfigurationResult> {
        self.validate_update(
            command.expected_revision,
            ConfigurationUpdate::Sections(command.sections),
        )
    }

    fn validate_update(
        &self,
        expected: String,
        update: ConfigurationUpdate,
    ) -> Result<BackendConfigurationResult> {
        let (_, bytes) = self.read_file(&self.directory()?)?;
        self.require_revision(&expected, &bytes)?;
        let errors = self
            .proposed_update(update, &bytes)
            .err()
            .into_iter()
            .collect();
        let (_, current) = self.read_file(&self.directory()?)?;
        self.require_revision(&expected, &current)?;
        Ok(BackendConfigurationResult {
            errors,
            state: None,
        })
    }

    pub(crate) fn save(
        &self,
        command: BackendConfigurationCommand,
    ) -> Result<BackendConfigurationResult> {
        self.save_update(
            command.expected_revision,
            ConfigurationUpdate::Full(Box::new(command.draft)),
        )
    }

    pub(crate) fn save_sections(
        &self,
        command: ConfigurationSectionCommand,
    ) -> Result<BackendConfigurationResult> {
        self.save_update(
            command.expected_revision,
            ConfigurationUpdate::Sections(command.sections),
        )
    }

    fn save_update(
        &self,
        expected: String,
        update: ConfigurationUpdate,
    ) -> Result<BackendConfigurationResult> {
        let reload = self
            .reload
            .lock()
            .map_err(|_| Error::Internal("configuration reload lock failed"))?;
        if matches!(reload.state, ConfigurationReloadState::Pending { .. }) {
            return Err(Error::Busy("configuration reload is pending"));
        }
        if self.active.system_settings_read_only {
            return Err(Error::Conflict(
                "system settings are read-only in active configuration".into(),
            ));
        }
        let mut retained = self
            .writer
            .try_lock()
            .map_err(|_| Error::Busy("configuration save is already running"))?;
        let opened = self.directory()?;
        let directory = opened.into_directory();
        let permissions = FilePermissions::current()?;
        permissions.check(&directory.try_clone()?.into_std_file(), false)?;
        let name = self
            .path
            .file_name()
            .ok_or(Error::Internal("configuration name is missing"))?;
        let lock_name = format!(".{}.lock", name.to_string_lossy());
        let marker = match directory.open_with(&lock_name, &file_options(true)) {
            Ok(file) => file.into_std(),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => directory
                .open_with(&lock_name, &file_options(false))?
                .into_std(),
            Err(error) => return Err(error.into()),
        };
        if is_unsafe_link(&marker.metadata()?) || !marker.metadata()?.is_file() {
            return Err(Error::InvalidInput(
                "configuration lock must be a regular file".into(),
            ));
        }
        permissions.check(&marker, false)?;
        if let Some(previous) = retained.as_ref() {
            if native_file_identity(previous)? != native_file_identity(&marker)? {
                return Err(Error::Conflict(
                    "configuration lock identity changed; restart before saving".into(),
                ));
            }
        } else {
            *retained = Some(marker.try_clone()?);
        }
        match marker.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(Error::Busy("configuration is locked by another writer"));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        let _lock = WriterLock(marker);
        let (original, bytes) = self.read_file(&self.directory()?)?;
        self.require_revision(&expected, &bytes)?;
        if Self::parse(&bytes)?.system_settings_read_only {
            return Err(Error::Conflict(
                "system settings are read-only in saved configuration".into(),
            ));
        }
        let proposed = match self.proposed_update(update, &bytes) {
            Ok(proposed) => proposed,
            Err(error) => {
                return Ok(BackendConfigurationResult {
                    errors: vec![error],
                    state: None,
                });
            }
        };
        let mut output = serde_json::to_vec_pretty(&proposed)
            .map_err(|_| Error::Internal("configuration serialization failed"))?;
        let changed = changed_fields(&Self::parse(&bytes)?, &proposed)?;
        output.push(b'\n');
        if output.len() as u64 > MAXIMUM {
            return Err(Error::InvalidInput("configuration exceeds 16 KiB".into()));
        }
        let temporary_name = format!(".{}.{}.pending", name.to_string_lossy(), digest(&output));
        let mut temporary = directory
            .open_with(&temporary_name, &file_options(true))?
            .into_std();
        let result = (|| -> Result<()> {
            permissions.copy(&original, &mut temporary)?;
            #[cfg(test)]
            self.fail_save(SaveFailure::Write)?;
            temporary.write_all(&output)?;
            #[cfg(test)]
            self.fail_save(SaveFailure::Sync)?;
            temporary.sync_all()?;
            #[cfg(test)]
            self.fail_save(SaveFailure::CrashAfterSync)?;
            let (current, current_bytes) = self.read_file(&self.directory()?)?;
            self.require_revision(&expected, &current_bytes)?;
            if native_file_identity(&current)? != native_file_identity(&original)? {
                return Err(Error::Conflict(
                    "configuration target identity changed".into(),
                ));
            }
            drop(current);
            drop(original);
            drop(temporary);
            #[cfg(test)]
            self.fail_save(SaveFailure::Replace)?;
            directory.rename(&temporary_name, &directory, name)?;
            #[cfg(test)]
            self.fail_save(SaveFailure::AfterReplace).map_err(|_| Error::Conflict(
                "configuration was replaced but directory sync failed; reload and review before retrying".into()
            ))?;
            sync_directory(&directory).map_err(|_| Error::Conflict(
                "configuration was replaced but directory sync failed; reload and review before retrying".into()
            ))?;
            let (_, actual) = self.read_file(&self.directory()?).map_err(|_| Error::Conflict(
                "configuration replacement completed but verification failed; reload and review before retrying".into()
            ))?;
            if actual != output {
                return Err(Error::Conflict(
                    "configuration changed after replacement; reload before retrying".into(),
                ));
            }
            Ok(())
        })();
        if result.is_err() {
            match directory.remove_file(&temporary_name) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Error::Conflict("configuration save failed and its temporary file needs operator cleanup; reload before retrying".into())),
            }
        }
        result?;
        *self
            .validated_revision
            .lock()
            .map_err(|_| Error::Internal("configuration validation lock failed"))? =
            Some(digest(&output));
        eprintln!(
            "backend configuration saved; changed fields: {}",
            changed.join(", ")
        );
        drop(reload);
        Ok(BackendConfigurationResult {
            errors: Vec::new(),
            state: Some(self.state(&output)?),
        })
    }

    #[cfg(test)]
    fn fail_save(&self, point: SaveFailure) -> Result<()> {
        let mut failure = self
            .failure
            .lock()
            .map_err(|_| Error::Internal("test failure lock failed"))?;
        if *failure == Some(point) {
            if point == SaveFailure::CrashAfterSync {
                std::process::exit(42);
            }
            *failure = None;
            return Err(std::io::Error::other("injected configuration persistence failure").into());
        }
        Ok(())
    }

    fn require_revision(&self, expected: &str, bytes: &[u8]) -> Result<()> {
        if expected != digest(bytes) {
            return Err(Error::Conflict(
                "saved configuration changed; reload and review the draft before retrying".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn attach_reload(&self, sender: tokio::sync::mpsc::Sender<String>) -> Result<()> {
        self.reload
            .lock()
            .map_err(|_| Error::Internal("configuration reload lock failed"))?
            .sender = Some(sender);
        Ok(())
    }

    pub(crate) fn request_reload(
        &self,
        command: ConfigurationReloadCommand,
    ) -> Result<ConfigurationReloadState> {
        let mut control = self
            .reload
            .lock()
            .map_err(|_| Error::Internal("configuration reload lock failed"))?;
        if matches!(control.state, ConfigurationReloadState::Pending { .. }) {
            return Err(Error::Busy("configuration reload is already pending"));
        }
        let (_, bytes) = self.read_file(&self.directory()?)?;
        self.require_revision(&command.expected_revision, &bytes)?;
        let saved = Self::parse(&bytes)?;
        if self.active.system_settings_read_only || saved.system_settings_read_only {
            return Err(Error::Conflict("system settings are read-only".into()));
        }
        self.proposed(Self::draft_from(summary(&saved)), &bytes)
            .map_err(configuration_error)?;
        let sender = control.sender.as_ref().ok_or(Error::Busy(
            "configuration reload is not attached to this host",
        ))?;
        sender
            .try_send(command.expected_revision.clone())
            .map_err(|_| Error::Busy("configuration reload host is unavailable or busy"))?;
        control.state = ConfigurationReloadState::Pending {
            revision: command.expected_revision,
            origin: saved.origin,
        };
        Ok(control.state.clone())
    }

    /// A new generation has its own immutable active snapshot and shares only
    /// the lifecycle gate and stable writer identity with the previous host.
    pub(super) fn reload_candidate(&self, revision: &str) -> Result<Self> {
        let (_, bytes) = self.read_file(&self.directory()?)?;
        self.require_revision(revision, &bytes)?;
        let active = self
            .proposed(Self::draft_from(summary(&Self::parse(&bytes)?)), &bytes)
            .map_err(configuration_error)?;
        let mut candidate = Self::new(&self.path, &bytes, active)?;
        if candidate.parent != self.parent {
            return Err(Error::Conflict(
                "configuration parent identity changed".into(),
            ));
        }
        candidate.reload = self.reload.clone();
        candidate.writer = self.writer.clone();
        Ok(candidate)
    }

    pub(super) fn verify_reload(&self, revision: &str) -> Result<()> {
        let (_, bytes) = self.read_file(&self.directory()?)?;
        self.require_revision(revision, &bytes)?;
        if Self::parse(&bytes)?.system_settings_read_only {
            return Err(Error::Conflict("saved configuration is read-only".into()));
        }
        Ok(())
    }

    pub(super) fn finish_reload(&self, revision: String, result: Result<()>) -> Result<()> {
        self.reload.lock().map_err(|_| Error::Internal("configuration reload lock failed"))?.state = match result {
            Ok(()) => ConfigurationReloadState::Active { revision },
            Err(_) => ConfigurationReloadState::Failed { revision, message: "API reload failed. The previous host remains active. Review server diagnostics and saved configuration before retrying.".into() },
        };
        Ok(())
    }

    fn proposed_update(
        &self,
        update: ConfigurationUpdate,
        bytes: &[u8],
    ) -> std::result::Result<Configuration, BackendConfigurationIssue> {
        let mut retained_origin = None;
        let draft = match update {
            ConfigurationUpdate::Full(draft) => *draft,
            ConfigurationUpdate::Sections(sections) => {
                if sections.is_empty() || sections.len() > 6 {
                    return Err(issue(
                        "sections",
                        "Select one through six changed sections.",
                    ));
                }
                let saved = Self::parse(bytes)
                    .map_err(|_| issue("configuration", "Saved JSON is invalid."))?;
                if !sections
                    .iter()
                    .any(|section| matches!(section, ConfigurationSectionUpdate::General { .. }))
                {
                    retained_origin = Some(saved.origin.clone());
                }
                let mut draft = Self::draft_from(summary(&saved));
                let mut seen = Vec::new();
                for update in sections {
                    let section = match update {
                        ConfigurationSectionUpdate::General {
                            environment,
                            listen,
                            origin,
                            browser_assets,
                        } => {
                            draft.environment = environment;
                            draft.listen = listen;
                            draft.origin = origin;
                            draft.browser_assets = browser_assets;
                            ConfigurationSection::General
                        }
                        ConfigurationSectionUpdate::Identity {
                            tenant_id,
                            object_id,
                            client_id,
                            client_secret_file,
                        } => {
                            draft.tenant_id = tenant_id;
                            draft.object_id = object_id;
                            draft.client_id = client_id;
                            draft.client_secret_file = client_secret_file;
                            ConfigurationSection::Identity
                        }
                        ConfigurationSectionUpdate::Tls {
                            certificate_file,
                            private_key_file,
                        } => {
                            draft.certificate_file = certificate_file;
                            draft.private_key_file = private_key_file;
                            ConfigurationSection::Tls
                        }
                        ConfigurationSectionUpdate::Storage { database, cache } => {
                            draft.database = database;
                            draft.cache = cache;
                            ConfigurationSection::Storage
                        }
                        ConfigurationSectionUpdate::MusicProviders { services } => {
                            draft.services = services;
                            ConfigurationSection::MusicProviders
                        }
                        ConfigurationSectionUpdate::MusicAcquisition { local_acquisition } => {
                            draft.local_acquisition = local_acquisition;
                            ConfigurationSection::MusicAcquisition
                        }
                    };
                    if seen.contains(&section) {
                        return Err(issue("sections", "Each section can appear only once."));
                    }
                    seen.push(section);
                }
                draft
            }
        };
        let mut proposed = self.proposed(draft, bytes)?;
        if let Some(origin) = retained_origin {
            proposed.origin = origin;
        }
        Ok(proposed)
    }

    fn proposed(
        &self,
        draft: BackendConfigurationDraft,
        bytes: &[u8],
    ) -> std::result::Result<Configuration, BackendConfigurationIssue> {
        let mut saved =
            Self::parse(bytes).map_err(|_| issue("configuration", "Saved JSON is invalid."))?;
        saved.environment = match draft.environment.as_str() {
            "production" => Environment::Production,
            "development" => Environment::Development,
            _ => return Err(issue("environment", "Select production or development.")),
        };
        saved.services = draft.services;
        saved.database = draft.database.into();
        saved.listen = draft
            .listen
            .parse()
            .map_err(|_| issue("listen", "Enter an IP address and a nonzero port."))?;
        saved.origin = draft.origin;
        saved.tenant_id = replacement(draft.tenant_id, saved.tenant_id);
        saved.object_id = replacement(draft.object_id, saved.object_id);
        saved.client_id = replacement(draft.client_id, saved.client_id);
        saved.client_secret_file = replacement(
            draft.client_secret_file,
            saved
                .client_secret_file
                .map(|v| v.to_string_lossy().into_owned()),
        )
        .map(PathBuf::from);
        saved.certificate_file = draft.certificate_file.into();
        saved.private_key_file = replacement(
            draft.private_key_file,
            Some(saved.private_key_file.to_string_lossy().into_owned()),
        )
        .map(PathBuf::from)
        .ok_or_else(|| issue("private_key_file", "A TLS private-key file is required."))?;
        saved.browser_assets = draft
            .browser_assets
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        saved.cache = draft
            .cache
            .map(|cache| crate::infrastructure::cache::CacheConfiguration {
                root: cache.root.into(),
                limits: crate::infrastructure::cache::CacheLimits {
                    item_bytes: cache.item_bytes,
                    total_bytes: cache.total_bytes,
                    entries: cache.entries,
                },
            });
        saved.local_acquisition = draft
            .local_acquisition
            .map(|local| {
                let previous = saved
                    .local_acquisition
                    .as_ref()
                    .map(|v| v.credentials_file.to_string_lossy().into_owned());
                Ok(crate::infrastructure::worker::LocalAcquisition {
                    staging_root: local.staging_root.into(),
                    credentials_file: replacement(local.credentials_file, previous)
                        .map(PathBuf::from)
                        .ok_or_else(|| {
                            issue(
                                "local_acquisition.credentials_file",
                                "Select a credential-file reference.",
                            )
                        })?,
                    server: local.server.parse().map_err(|_| {
                        issue(
                            "local_acquisition.server",
                            "Enter a public IPv4 address and port.",
                        )
                    })?,
                    acknowledge_plaintext_login: local.acknowledge_plaintext_login,
                })
            })
            .transpose()?;
        Self::normalize(&mut saved)?;
        Self::check_references(&saved)?;
        let store = Store::open_read_current(&saved.database)
            .map_err(|_| issue("database", "Use an accessible local database with the current schema. Upgrade separately if needed."))?;
        self.check_files(&saved, &store)
            .map_err(|error| issue_for_error(&error))?;
        if let Some(cache) = &saved.cache {
            cache.inspect(&store, &self.protected(&saved), true).map_err(|_| issue("cache", "Cache limits, permissions, overlap or native binding are invalid. Set up the cache separately."))?;
        }
        Ok(saved)
    }

    fn draft_from(value: BackendConfiguration) -> BackendConfigurationDraft {
        BackendConfigurationDraft {
            environment: value.environment,
            services: value.services,
            cache: value.cache,
            database: value.database,
            listen: value.listen,
            origin: value.origin,
            tenant_id: ConfigurationReplacement::Keep,
            object_id: ConfigurationReplacement::Keep,
            client_id: ConfigurationReplacement::Keep,
            client_secret_file: ConfigurationReplacement::Keep,
            certificate_file: value.certificate_file,
            private_key_file: ConfigurationReplacement::Keep,
            browser_assets: value.browser_assets,
            local_acquisition: value
                .local_acquisition
                .map(|local| BackendLocalAcquisitionDraft {
                    staging_root: local.staging_root,
                    credentials_file: ConfigurationReplacement::Keep,
                    server: local.server,
                    acknowledge_plaintext_login: local.acknowledge_plaintext_login,
                }),
        }
    }

    fn changed_sections(
        active: &Configuration,
        saved: &Configuration,
    ) -> Result<Vec<ConfigurationSection>> {
        let active = serde_json::to_value(active)
            .map_err(|_| Error::Internal("configuration comparison failed"))?;
        let saved = serde_json::to_value(saved)
            .map_err(|_| Error::Internal("configuration comparison failed"))?;
        Ok([
            (
                ConfigurationSection::General,
                &["environment", "listen", "origin", "browser_assets"][..],
            ),
            (
                ConfigurationSection::Identity,
                &["tenant_id", "object_id", "client_id", "client_secret_file"][..],
            ),
            (
                ConfigurationSection::Tls,
                &["certificate_file", "private_key_file"][..],
            ),
            (ConfigurationSection::Storage, &["database", "cache"][..]),
            (ConfigurationSection::MusicProviders, &["services"][..]),
            (
                ConfigurationSection::MusicAcquisition,
                &["local_acquisition"][..],
            ),
        ]
        .into_iter()
        .filter_map(|(section, fields)| {
            fields
                .iter()
                .any(|field| active[*field] != saved[*field])
                .then_some(section)
        })
        .collect())
    }

    fn normalize(
        configuration: &mut Configuration,
    ) -> std::result::Result<String, BackendConfigurationIssue> {
        configuration.services.validate_builtins().map_err(|_| {
            issue(
                "services",
                "Select only installed providers, without duplicates, for each service.",
            )
        })?;
        match configuration.environment {
            Environment::Production => {
                if configuration.local_acquisition.is_some() {
                    return Err(issue(
                        "local_acquisition",
                        "Local acquisition is development-only.",
                    ));
                }
                if ![
                    &configuration.tenant_id,
                    &configuration.object_id,
                    &configuration.client_id,
                ]
                .into_iter()
                .all(|v| {
                    v.as_deref()
                        .is_some_and(crate::domain::identifiers::is_canonical_uuid)
                }) || configuration.client_secret_file.is_none()
                {
                    return Err(issue(
                        "environment",
                        "Production requires canonical Entra IDs and a client secret file.",
                    ));
                }
            }
            Environment::Development => {
                if configuration.tenant_id.is_some()
                    || configuration.object_id.is_some()
                    || configuration.client_id.is_some()
                    || configuration.client_secret_file.is_some()
                {
                    return Err(issue(
                        "environment",
                        "Development mode must not contain Entra configuration. Remove each Entra reference explicitly.",
                    ));
                }
            }
        }
        if !configuration.listen.ip().is_loopback() || configuration.listen.port() == 0 {
            return Err(issue(
                "listen",
                "API preview requires a loopback address and an explicit nonzero port; public operation is not enabled.",
            ));
        }
        let origin = reqwest::Url::parse(&configuration.origin)
            .map_err(|_| issue("origin", "Service origin is invalid."))?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.port_or_known_default() != Some(configuration.listen.port())
        {
            return Err(issue(
                "origin",
                "Service origin must be HTTPS with the listener port and no path, credentials or query.",
            ));
        }
        if matches!(configuration.environment, Environment::Development)
            && !super::Api::development_origin(&origin)
        {
            return Err(issue(
                "origin",
                "Development origin must use localhost or a literal loopback IP address.",
            ));
        }
        let hostname = origin
            .host_str()
            .ok_or_else(|| issue("origin", "Service hostname is missing."))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        configuration.origin = origin.origin().ascii_serialization();
        for (field, file) in [
            ("database", &configuration.database),
            ("certificate_file", &configuration.certificate_file),
            ("private_key_file", &configuration.private_key_file),
        ]
        .into_iter()
        .chain(
            configuration
                .client_secret_file
                .iter()
                .map(|path| ("client_secret_file", path)),
        )
        .chain(
            configuration
                .browser_assets
                .iter()
                .map(|path| ("browser_assets", path)),
        )
        .chain(
            configuration
                .cache
                .iter()
                .map(|cache| ("cache.root", &cache.root)),
        )
        .chain(configuration.local_acquisition.iter().flat_map(|local| {
            [
                ("local_acquisition.staging_root", &local.staging_root),
                (
                    "local_acquisition.credentials_file",
                    &local.credentials_file,
                ),
            ]
        })) {
            valid_reference(file).map_err(|_| {
                issue(
                    field,
                    "Use an absolute local path without alternate streams or unsafe components.",
                )
            })?;
        }
        if let Some(cache) = &configuration.cache {
            cache.limits.validate().map_err(|_| issue("cache", "Use 1 through 4096 entries, items up to 32 MiB, and a total from two items with 96-byte headers through 1 GiB."))?;
        }
        Ok(hostname)
    }

    fn check_references(
        configuration: &Configuration,
    ) -> std::result::Result<(), BackendConfigurationIssue> {
        for (field, path, sensitive) in [
            ("database", &configuration.database, false),
            ("certificate_file", &configuration.certificate_file, false),
            ("private_key_file", &configuration.private_key_file, true),
        ]
        .into_iter()
        .chain(
            configuration
                .client_secret_file
                .iter()
                .map(|path| ("client_secret_file", path, true)),
        ) {
            let check = || -> Result<()> {
                valid_reference(path)?;
                let parent = LibraryDirectory::open(
                    path.parent()
                        .ok_or(Error::Internal("reference parent is missing"))?,
                )?;
                let file = parent.open_file(
                    path.file_name()
                        .ok_or(Error::Internal("reference name is missing"))?,
                )?;
                FilePermissions::current()?.check(&file, sensitive)
            };
            check().map_err(|_| issue(field, "The file must exist on local disk with trusted ownership and permissions. Links, reparse points and alternate streams are not permitted."))?;
        }
        Ok(())
    }

    pub(super) fn protected(&self, configuration: &Configuration) -> Vec<PathBuf> {
        let mut paths = vec![
            self.path.clone(),
            configuration.certificate_file.clone(),
            configuration.private_key_file.clone(),
        ];
        paths.extend(configuration.client_secret_file.iter().cloned());
        paths.extend(configuration.browser_assets.iter().cloned());
        if let Some(local) = &configuration.local_acquisition {
            paths.push(local.credentials_file.clone());
        }
        paths
    }

    fn check_files(&self, configuration: &Configuration, store: &Store) -> Result<()> {
        let protected = self.protected(configuration);
        crate::infrastructure::local_file::protect_files(
            store,
            &protected.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
        )?;
        if let Some(assets) = &configuration.browser_assets {
            crate::infrastructure::browser::BrowserAssets::load(assets)?;
        }
        if let Some(local) = &configuration.local_acquisition {
            local.validate(store, &configuration.services)?;
        }
        self.client_secret(configuration)?;
        let origin = reqwest::Url::parse(&configuration.origin)
            .map_err(|_| Error::InvalidInput("service origin is invalid".into()))?;
        load_tls(
            configuration,
            origin
                .host_str()
                .ok_or(Error::Internal("hostname is missing"))?
                .trim_matches(['[', ']']),
        )?;
        Ok(())
    }

    pub(super) fn startup_material(
        &self,
        store: &Store,
        hostname: &str,
    ) -> Result<(Option<ClientSecret>, axum_server::tls_rustls::RustlsConfig)> {
        self.check_files(&self.active, store)?;
        Ok((
            self.client_secret(&self.active)?,
            load_tls(&self.active, hostname)?,
        ))
    }

    fn client_secret(&self, configuration: &Configuration) -> Result<Option<ClientSecret>> {
        configuration
            .client_secret_file
            .as_ref()
            .map(|path| {
                let bytes = crate::infrastructure::local_file::read_local(path, 4096, true)?;
                let secret = String::from_utf8(bytes)
                    .map_err(|_| Error::InvalidInput("client secret file is not UTF-8".into()))?;
                let secret = secret.trim_end_matches(['\r', '\n']);
                if secret.trim().is_empty() || secret.chars().any(char::is_control) {
                    return Err(Error::InvalidInput(
                        "client secret file must contain one nonempty line".into(),
                    ));
                }
                Ok(ClientSecret::new(secret.into()))
            })
            .transpose()
    }

    pub(super) fn tls_material(configuration: &Configuration) -> Result<(Vec<u8>, Vec<u8>)> {
        Ok((
            crate::infrastructure::local_file::read_local(
                &configuration.certificate_file,
                262144,
                false,
            )?,
            crate::infrastructure::local_file::read_local(
                &configuration.private_key_file,
                16384,
                true,
            )?,
        ))
    }
}

struct WriterLock(File);
impl Drop for WriterLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn file_options(create: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(create)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.access_mode(0xc004_0000).share_mode(3);
    }
    options
}

fn replacement(update: ConfigurationReplacement, previous: Option<String>) -> Option<String> {
    match update {
        ConfigurationReplacement::Keep => previous,
        ConfigurationReplacement::Replace(value) if value.trim().is_empty() => previous,
        ConfigurationReplacement::Replace(value) => Some(value),
        ConfigurationReplacement::Clear => None,
    }
}

fn issue(field: &str, message: &str) -> BackendConfigurationIssue {
    BackendConfigurationIssue {
        field: field.into(),
        message: message.into(),
    }
}

fn configuration_error(error: BackendConfigurationIssue) -> Error {
    Error::InvalidInput(format!("{}: {}", error.field, error.message))
}

fn valid_reference(path: &Path) -> Result<()> {
    require_local_disk(path)?;
    if !path.is_absolute() || path.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name.to_str().is_none_or(|name| name.contains(':') || name.chars().any(char::is_control)))
    }) {
        return Err(Error::InvalidInput("service references require absolute local paths without alternate streams".into()));
    }
    Ok(())
}

fn issue_for_error(error: &Error) -> BackendConfigurationIssue {
    match error {
        Error::InvalidInput(message) => {
            let field = if message.contains("origin") {
                "origin"
            } else if message.contains("loopback address") {
                "listen"
            } else if message.contains("Entra") {
                "environment"
            } else if message.contains("development-only") {
                "local_acquisition"
            } else if message.contains("cache") {
                "cache"
            } else if message.contains("provider") {
                "services"
            } else {
                "configuration"
            };
            // InvalidInput messages from these validators contain policy only.
            issue(field, message)
        }
        _ => issue(
            "configuration",
            "A local file, TLS material, permission or root binding check failed. Check the configured references and complete any required setup separately.",
        ),
    }
}

fn changed_fields(before: &Configuration, after: &Configuration) -> Result<Vec<String>> {
    let before = serde_json::to_value(before)
        .map_err(|_| Error::Internal("configuration comparison failed"))?;
    let after = serde_json::to_value(after)
        .map_err(|_| Error::Internal("configuration comparison failed"))?;
    let (Some(before), Some(after)) = (before.as_object(), after.as_object()) else {
        return Err(Error::Internal("configuration comparison requires objects"));
    };
    Ok(after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn summary(configuration: &Configuration) -> BackendConfiguration {
    BackendConfiguration {
        environment: match configuration.environment {
            Environment::Production => "production",
            Environment::Development => "development",
        }
        .into(),
        system_settings_read_only: configuration.system_settings_read_only,
        services: configuration.services.clone(),
        cache: configuration
            .cache
            .as_ref()
            .map(|cache| BackendCacheConfiguration {
                root: cache.root.to_string_lossy().into_owned(),
                item_bytes: cache.limits.item_bytes,
                total_bytes: cache.limits.total_bytes,
                entries: cache.limits.entries,
            }),
        database: configuration.database.to_string_lossy().into_owned(),
        listen: configuration.listen.to_string(),
        origin: configuration.origin.clone(),
        tenant_id_configured: configuration.tenant_id.is_some(),
        object_id_configured: configuration.object_id.is_some(),
        client_id_configured: configuration.client_id.is_some(),
        client_secret_file_configured: configuration.client_secret_file.is_some(),
        certificate_file: configuration
            .certificate_file
            .to_string_lossy()
            .into_owned(),
        private_key_file_configured: !configuration.private_key_file.as_os_str().is_empty(),
        browser_assets: configuration
            .browser_assets
            .as_ref()
            .map(|v| v.to_string_lossy().into_owned()),
        local_acquisition: configuration.local_acquisition.as_ref().map(|local| {
            BackendLocalAcquisition {
                staging_root: local.staging_root.to_string_lossy().into_owned(),
                credentials_file_configured: !local.credentials_file.as_os_str().is_empty(),
                server: local.server.to_string(),
                acknowledge_plaintext_login: local.acknowledge_plaintext_login,
            }
        }),
    }
}

#[cfg(test)]
mod tests;
