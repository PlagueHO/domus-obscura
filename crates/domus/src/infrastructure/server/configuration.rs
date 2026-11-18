//! The sole access boundary for backend JSON. Validation never activates settings.
use super::load_tls;
use crate::application::services::ServiceConfiguration;
use crate::domain::certificates::{AcmeEnvironment, TlsMode};
use crate::infrastructure::certificates::ManagedTlsConfiguration;
use crate::infrastructure::filesystem::{
    FilesystemClient, LibraryDirectory, NativeRoot, StorageConfiguration, StorageLease,
};
use crate::infrastructure::permissions::FilePermissions;
use crate::infrastructure::store::{Store, StoreOpenMode};
use crate::{Error, Result};
use openidconnect::ClientSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const MAXIMUM: u64 = 16384;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Environment {
    #[default]
    Production,
    Development,
    SingleUser,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Configuration {
    #[serde(default)]
    pub(super) environment: Environment,
    #[serde(default = "system_settings_read_only_by_default")]
    pub(super) system_settings_read_only: bool,
    #[serde(default)]
    pub(crate) services: ServiceConfiguration,
    pub(crate) cache: Option<crate::infrastructure::cache::CacheConfiguration>,
    #[serde(default = "default_database")]
    pub(crate) database: PathBuf,
    #[serde(default)]
    pub(crate) storage: Option<StorageConfiguration>,
    pub(super) listen: std::net::SocketAddr,
    pub(super) origin: String,
    pub(super) tenant_id: Option<String>,
    pub(super) object_id: Option<String>,
    pub(super) client_id: Option<String>,
    pub(super) client_secret_file: Option<PathBuf>,
    pub(super) certificate_file: Option<PathBuf>,
    pub(super) private_key_file: Option<PathBuf>,
    #[serde(default)]
    pub(super) tls_mode: TlsMode,
    #[serde(default)]
    pub(super) managed_tls: Option<ManagedTlsConfiguration>,
    pub(super) browser_assets: Option<PathBuf>,
    pub(super) local_acquisition: Option<crate::infrastructure::worker::LocalAcquisition>,
    pub(crate) production_worker:
        Option<crate::infrastructure::production_worker::WorkerConfiguration>,
}

fn system_settings_read_only_by_default() -> bool {
    true
}

pub(super) fn default_database() -> PathBuf {
    Path::new("state").join("state.db")
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

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendWorker {
    pub queue_capacity: u16,
    pub acquisition: Option<BackendLocalAcquisition>,
    pub sharing: Option<BackendSharing>,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendWorkerDraft {
    pub queue_capacity: u16,
    pub acquisition: Option<BackendLocalAcquisitionDraft>,
    pub sharing: Option<BackendSharingDraft>,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendAcquisitionWorkerDraft {
    pub queue_capacity: u16,
    pub acquisition: Option<BackendLocalAcquisitionDraft>,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendSharing {
    pub policy: crate::domain::sharing::SharePolicy,
    pub account: Option<BackendSharingAccount>,
    pub listen: Option<String>,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendSharingAccount {
    pub credentials_file_configured: bool,
    pub server: String,
    pub acknowledge_plaintext_login: bool,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendSharingDraft {
    pub policy: crate::domain::sharing::SharePolicy,
    pub account: Option<BackendSharingAccountDraft>,
    pub listen: Option<String>,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendSharingAccountDraft {
    pub credentials_file: ConfigurationReplacement,
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
    pub certificate_file: Option<String>,
    pub private_key_file_configured: bool,
    pub tls_mode: TlsMode,
    pub managed_tls: Option<BackendManagedTls>,
    pub browser_assets: Option<String>,
    pub local_acquisition: Option<BackendLocalAcquisition>,
    pub production_worker: Option<BackendWorker>,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendManagedTls {
    pub environment: AcmeEnvironment,
    pub state_directory: String,
    pub tenant_id_configured: bool,
    pub client_id_configured: bool,
    pub client_secret_file_configured: bool,
    pub subscription_id: String,
    pub resource_group: String,
    pub zone: String,
    pub terms_of_service_agreed: bool,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BackendManagedTlsDraft {
    pub environment: AcmeEnvironment,
    pub state_directory: String,
    pub tenant_id: ConfigurationReplacement,
    pub client_id: ConfigurationReplacement,
    pub client_secret_file: ConfigurationReplacement,
    pub subscription_id: String,
    pub resource_group: String,
    pub zone: String,
    pub terms_of_service_agreed: bool,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendCertificateStatus {
    pub configuration_revision: String,
    pub certificate: Option<crate::infrastructure::certificates::state::CertificateStatus>,
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
    MusicSharing,
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
        certificate_file: Option<String>,
        private_key_file: ConfigurationReplacement,
        #[serde(default)]
        tls_mode: TlsMode,
        managed_tls: Option<BackendManagedTlsDraft>,
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
        production_worker: Option<BackendAcquisitionWorkerDraft>,
    },
    MusicSharing {
        sharing: Option<BackendSharingDraft>,
    },
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSectionCommand {
    pub expected_revision: String,
    pub sections: Vec<ConfigurationSectionUpdate>,
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
    pub certificate_file: Option<String>,
    pub private_key_file: ConfigurationReplacement,
    #[serde(default)]
    pub tls_mode: TlsMode,
    pub managed_tls: Option<BackendManagedTlsDraft>,
    pub browser_assets: Option<String>,
    pub local_acquisition: Option<BackendLocalAcquisitionDraft>,
    pub production_worker: Option<BackendWorkerDraft>,
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
    /// Passive validation never returns a replacement saved state.
    pub state: Option<BackendConfigurationState>,
}

/// Startup and HTTP share passive inspection and revision-bound validation.
/// Only an operator can edit host JSON, with every state user stopped.
///
/// Callers must authorize access before exposing this service to a transport.
/// Opening it does not start a server, migrate SQLite, or activate saved changes.
pub struct ConfigurationService {
    path: PathBuf,
    parent: NativeRoot,
    _directory: LibraryDirectory,
    pub(super) active: Configuration,
    active_document: Configuration,
    active_revision: String,
    certificate_selection:
        Arc<Mutex<Option<crate::infrastructure::certificates::state::CertificateSelection>>>,
    certificate_recovery: Arc<std::sync::atomic::AtomicBool>,
    state_storage: StorageLease,
    _configuration_storage: StorageLease,
}

impl ConfigurationService {
    /// Read and bind a startup configuration without activating its services.
    /// The caller owns the containing directory and must keep it private.
    pub fn open(path: &Path) -> Result<Self> {
        Self::load(path).map(|(service, _, _)| service)
    }

    /// Open only the state selected by this frozen host configuration.
    /// The caller chooses current-schema or snapshot behavior explicitly.
    pub fn open_store(&self, mode: StoreOpenMode) -> Result<Store> {
        Store::open_scoped(&self.active.database, self.state_storage.client(), mode)
    }

    /// Read bounded local input through an admitted private directory handle.
    /// This performs blocking file I/O, without opening a SQL transaction.
    pub fn read_input(&self, path: &Path, maximum: u64, sensitive: bool) -> Result<Vec<u8>> {
        crate::infrastructure::filesystem::read_local_file_for(
            Some(self.state_storage.client()),
            path,
            maximum,
            sensitive,
        )
    }

    pub(crate) fn state_lock(
        &self,
        purpose: &str,
    ) -> Result<super::super::local_file::ProcessLock> {
        let (context, _, directory) = self
            .state_storage
            .client()
            .sqlite_context(&self.active.database, false)?;
        Ok(
            super::super::local_file::ProcessLock::acquire_in(&directory, purpose)?
                .with_context(context),
        )
    }

    pub(crate) fn certificate_selection(
        &self,
    ) -> Arc<Mutex<Option<crate::infrastructure::certificates::state::CertificateSelection>>> {
        self.certificate_selection.clone()
    }

    pub(crate) fn certificate_recovery(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.certificate_recovery.clone()
    }

    pub(crate) fn certificate_status(&self) -> Result<BackendCertificateStatus> {
        let mut certificate = if self.active.tls_mode == TlsMode::AzureDns
            && !matches!(self.active.environment, Environment::SingleUser)
        {
            let managed =
                self.active.managed_tls.as_ref().ok_or_else(|| {
                    Error::InvalidInput("managed TLS settings are missing".into())
                })?;
            let origin = reqwest::Url::parse(&self.active.origin)
                .map_err(|_| Error::InvalidInput("service origin is invalid".into()))?;
            let hostname = origin
                .host_str()
                .ok_or(Error::Internal("service hostname is missing"))?;
            let selected = self
                .certificate_selection
                .lock()
                .map_err(|_| Error::Internal("live certificate selection lock failed"))?
                .clone();
            let store = self.open_store(StoreOpenMode::ReadCurrent)?;
            Some(
                crate::infrastructure::certificates::state::StateStore::status_bound_for(
                    &store,
                    &managed.state_directory,
                    &managed.binding_for(hostname, &store.source())?,
                    managed.environment,
                    selected.as_ref(),
                )?,
            )
        } else {
            None
        };
        if let Some(status) = certificate.as_mut() {
            status.recovery_required |= self
                .certificate_recovery
                .load(std::sync::atomic::Ordering::Acquire);
        }
        Ok(BackendCertificateStatus {
            configuration_revision: self.active_revision.clone(),
            certificate,
        })
    }

    pub(crate) fn active_revision(&self) -> &str {
        &self.active_revision
    }
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

    /// Recheck both the startup policy and the current saved editing policy.
    pub fn require_editable(&self) -> Result<()> {
        let (_, bytes) = self.read_file(&self.directory()?)?;
        if self.active.system_settings_read_only || Self::parse(&bytes)?.system_settings_read_only {
            return Err(Error::Conflict("system settings are read-only".into()));
        }
        Ok(())
    }

    pub(crate) fn load(path: &Path) -> Result<(Self, Configuration, String)> {
        valid_reference(path)?;
        let bytes = crate::infrastructure::local_file::read_local(path, MAXIMUM, false)?;
        let mut configuration = Self::parse(&bytes)?;
        Self::resolve_paths(
            &mut configuration,
            path.parent().ok_or(Error::InvalidInput(
                "configuration parent is missing".into(),
            ))?,
        );
        let hostname = Self::normalize(&mut configuration).map_err(configuration_error)?;
        Self::check_references(&configuration).map_err(configuration_error)?;
        let service = Self::new(path, &bytes, configuration.clone())?;
        Ok((service, configuration, hostname))
    }

    fn resolve_paths(configuration: &mut Configuration, root: &Path) {
        let resolve = |path: &mut PathBuf| {
            if path.is_relative() && !path.has_root() && !path.as_os_str().is_empty() {
                *path = root.join(&*path);
            }
        };
        for path in [
            Some(&mut configuration.database),
            configuration.client_secret_file.as_mut(),
            configuration.certificate_file.as_mut(),
            configuration.private_key_file.as_mut(),
            configuration.browser_assets.as_mut(),
            configuration.cache.as_mut().map(|cache| &mut cache.root),
        ]
        .into_iter()
        .flatten()
        {
            resolve(path);
        }
        if let Some(storage) = configuration.storage.as_mut() {
            storage.resolve_paths(root);
        }
        if let Some(managed) = configuration.managed_tls.as_mut() {
            resolve(&mut managed.state_directory);
            resolve(&mut managed.dns.client_secret_file);
        }
        for acquisition in [
            configuration.local_acquisition.as_mut(),
            configuration
                .production_worker
                .as_mut()
                .and_then(|worker| worker.acquisition.as_mut()),
        ]
        .into_iter()
        .flatten()
        {
            resolve(&mut acquisition.staging_root);
            resolve(&mut acquisition.credentials_file);
        }
        if let Some(account) = configuration
            .production_worker
            .as_mut()
            .and_then(|worker| worker.sharing.as_mut())
            .and_then(|sharing| sharing.account.as_mut())
        {
            resolve(&mut account.credentials_file);
        }
    }

    fn new(path: &Path, bytes: &[u8], active: Configuration) -> Result<Self> {
        let name = path.file_name().and_then(|name| name.to_str());
        if name.is_none_or(|name| name.contains([':', '\\', '/']) || name.is_empty()) {
            return Err(Error::InvalidInput(
                "configuration file name is invalid".into(),
            ));
        }
        let directory = LibraryDirectory::open(
            path.parent()
                .ok_or(Error::Internal("configuration parent is missing"))?,
        )?;
        let parent = directory.native_root().clone();
        let (state_storage, configuration_storage) = Self::storage(&active, &digest(bytes), path)?
            .ok_or_else(|| {
                Error::Conflict(
                    "storage selection is missing; explicit fresh setup is required".into(),
                )
            })?;
        Ok(Self {
            path: path.to_owned(),
            parent,
            _directory: directory,
            active,
            active_document: Self::parse(bytes)?,
            active_revision: digest(bytes),
            certificate_selection: Arc::new(Mutex::new(None)),
            certificate_recovery: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            state_storage,
            _configuration_storage: configuration_storage,
        })
    }

    fn parse(bytes: &[u8]) -> Result<Configuration> {
        serde_json::from_slice(bytes)
            .map_err(|_| Error::InvalidInput("saved backend configuration JSON is invalid".into()))
    }

    fn storage(
        configuration: &Configuration,
        revision: &str,
        path: &Path,
    ) -> Result<Option<(StorageLease, StorageLease)>> {
        configuration
            .storage
            .as_ref()
            .map(|storage| {
                let client = FilesystemClient::new(storage.clone(), revision.to_owned())?;
                let application = client.open_application(
                    path.parent()
                        .ok_or(Error::Internal("configuration parent is missing"))?,
                )?;
                Ok((client.open_state(&configuration.database)?, application))
            })
            .transpose()
    }

    fn directory(&self) -> Result<LibraryDirectory> {
        let directory = self
            ._configuration_storage
            .client()
            .open_application(
                self.path
                    .parent()
                    .ok_or(Error::Internal("configuration parent is missing"))?,
            )?
            .into_directory()?;
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

    /// Read redacted saved and startup state without activating either document.
    pub fn read(&self) -> Result<BackendConfigurationState> {
        let (_, bytes) = self.read_file(&self.directory()?)?;
        self.state(&bytes)
    }

    /// Validate the exact saved document without startup effects or a listener.
    /// This does not reserve references or replace checks at later startup.
    pub fn validate_saved(&self) -> Result<BackendConfigurationResult> {
        let (_, bytes) = self.read_file(&self.directory()?)?;
        self.validate_update(
            digest(&bytes),
            ConfigurationUpdate::Full(Box::new(Self::draft_from(summary(&Self::parse(&bytes)?)))),
        )
    }

    fn state(&self, bytes: &[u8]) -> Result<BackendConfigurationState> {
        let saved = Self::parse(bytes)?;
        let revision = digest(bytes);
        Ok(BackendConfigurationState {
            active: summary(&self.active),
            saved: summary(&saved),
            active_revision: self.active_revision.clone(),
            restart_required: revision != self.active_revision,
            saved_requires_validation: revision != self.active_revision,
            saved_revision: revision,
            changed_sections: Self::changed_sections(&self.active_document, &saved)?,
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

    /// Validate a revision-bound draft without saving or activating it.
    pub fn validate_sections(
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

    fn require_revision(&self, expected: &str, bytes: &[u8]) -> Result<()> {
        if expected != digest(bytes) {
            return Err(Error::Conflict(
                "saved configuration changed; reload and review the draft before retrying".into(),
            ));
        }
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
                if sections.is_empty() || sections.len() > 7 {
                    return Err(issue(
                        "sections",
                        "Select one through seven changed sections.",
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
                            tls_mode,
                            managed_tls,
                        } => {
                            draft.certificate_file = certificate_file;
                            draft.private_key_file = private_key_file;
                            draft.tls_mode = tls_mode;
                            draft.managed_tls = managed_tls;
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
                        ConfigurationSectionUpdate::MusicAcquisition {
                            local_acquisition,
                            production_worker,
                        } => {
                            draft.local_acquisition = local_acquisition;
                            let sharing = draft
                                .production_worker
                                .as_mut()
                                .and_then(|worker| worker.sharing.take());
                            draft.production_worker = match production_worker {
                                Some(worker) => Some(BackendWorkerDraft {
                                    queue_capacity: worker.queue_capacity,
                                    acquisition: worker.acquisition,
                                    sharing,
                                }),
                                None => sharing.map(|sharing| BackendWorkerDraft {
                                    queue_capacity: draft
                                        .production_worker
                                        .as_ref()
                                        .map_or(64, |worker| worker.queue_capacity),
                                    acquisition: None,
                                    sharing: Some(sharing),
                                }),
                            };
                            ConfigurationSection::MusicAcquisition
                        }
                        ConfigurationSectionUpdate::MusicSharing { sharing } => {
                            if let Some(worker) = draft.production_worker.as_mut() {
                                worker.sharing = sharing;
                            } else if sharing.is_some() {
                                draft.production_worker = Some(BackendWorkerDraft {
                                    queue_capacity: 64,
                                    acquisition: None,
                                    sharing,
                                });
                            }
                            ConfigurationSection::MusicSharing
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
            "single_user" => Environment::SingleUser,
            _ => {
                return Err(issue(
                    "environment",
                    "Select production, development, or single user.",
                ));
            }
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
        saved.certificate_file = draft
            .certificate_file
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        saved.private_key_file = replacement(
            draft.private_key_file,
            saved
                .private_key_file
                .map(|v| v.to_string_lossy().into_owned()),
        )
        .map(PathBuf::from);
        saved.tls_mode = draft.tls_mode;
        if let Some(managed) = draft.managed_tls {
            let previous = saved.managed_tls.take();
            let previous_dns = previous.as_ref().map(|value| &value.dns);
            saved.managed_tls = Some(ManagedTlsConfiguration {
                environment: managed.environment,
                state_directory: managed.state_directory.into(),
                terms_of_service_agreed: managed.terms_of_service_agreed,
                dns: crate::infrastructure::certificates::azure::AzureDnsConfiguration {
                    tenant_id: replacement(
                        managed.tenant_id,
                        previous_dns.map(|dns| dns.tenant_id.clone()),
                    )
                    .unwrap_or_default(),
                    client_id: replacement(
                        managed.client_id,
                        previous_dns.map(|dns| dns.client_id.clone()),
                    )
                    .unwrap_or_default(),
                    client_secret_file: replacement(
                        managed.client_secret_file,
                        previous_dns
                            .map(|dns| dns.client_secret_file.to_string_lossy().into_owned()),
                    )
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                    subscription_id: managed.subscription_id,
                    resource_group: managed.resource_group,
                    zone: managed.zone,
                },
            });
        }
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
        saved.production_worker = draft
            .production_worker
            .map(|worker| {
                let previous = saved
                    .production_worker
                    .as_ref()
                    .and_then(|worker| worker.acquisition.as_ref());
                let acquisition = worker
                    .acquisition
                    .map(|local| {
                        Ok(crate::infrastructure::worker::LocalAcquisition {
                            staging_root: local.staging_root.into(),
                            credentials_file: replacement(
                                local.credentials_file,
                                previous.map(|local| {
                                    local.credentials_file.to_string_lossy().into_owned()
                                }),
                            )
                            .map(PathBuf::from)
                            .ok_or_else(|| {
                                issue(
                                    "production_worker.acquisition.credentials_file",
                                    "Select a credential-file reference.",
                                )
                            })?,
                            server: local.server.parse().map_err(|_| {
                                issue(
                                    "production_worker.acquisition.server",
                                    "Enter a public IPv4 address and port.",
                                )
                            })?,
                            acknowledge_plaintext_login: local.acknowledge_plaintext_login,
                        })
                    })
                    .transpose()?;
                let sharing = worker.sharing.map(|sharing| {
                    let previous = saved.production_worker.as_ref()
                        .and_then(|worker| worker.sharing.as_ref())
                        .and_then(|sharing| sharing.account.as_ref());
                    let account = sharing.account.map(|account| {
                        Ok::<_, BackendConfigurationIssue>(crate::infrastructure::sharing::SharingAccount {
                            credentials_file: replacement(account.credentials_file, previous.map(|account| account.credentials_file.to_string_lossy().into_owned()))
                                .map(PathBuf::from)
                                .ok_or_else(|| issue("production_worker.sharing.account.credentials_file", "Select a credential-file reference."))?,
                            server: account.server.parse().map_err(|_| issue("production_worker.sharing.account.server", "Enter a public IPv4 address and port."))?,
                            acknowledge_plaintext_login: account.acknowledge_plaintext_login,
                        })
                    }).transpose()?;
                    Ok::<_, BackendConfigurationIssue>(crate::infrastructure::sharing::SharingConfiguration {
                        policy: sharing.policy,
                        account,
                        listen: sharing.listen.filter(|value| !value.trim().is_empty())
                            .map(|value| value.parse().map_err(|_| issue("production_worker.sharing.listen", "Enter an IPv4 address and nonzero port.")))
                            .transpose()?,
                    })
                }).transpose()?;
                Ok::<_, BackendConfigurationIssue>(
                    crate::infrastructure::production_worker::WorkerConfiguration {
                        queue_capacity: worker.queue_capacity,
                        acquisition,
                        sharing,
                    },
                )
            })
            .transpose()?;
        Self::resolve_paths(
            &mut saved,
            self.path
                .parent()
                .ok_or_else(|| issue("configuration", "Configuration parent is missing."))?,
        );
        Self::normalize(&mut saved)?;
        Self::check_references(&saved)?;
        let (storage, _application) = Self::storage(&saved, &digest(bytes), &self.path)
            .map_err(|error| issue_for_error(&error))?
            .ok_or_else(|| {
                issue(
                    "storage",
                    "Storage selection is missing. Use explicit fresh setup.",
                )
            })?;
        let store = Store::open_scoped(&saved.database, storage.client(), StoreOpenMode::ReadCurrent)
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
            tls_mode: value.tls_mode,
            managed_tls: value.managed_tls.map(|managed| BackendManagedTlsDraft {
                environment: managed.environment,
                state_directory: managed.state_directory,
                tenant_id: ConfigurationReplacement::Keep,
                client_id: ConfigurationReplacement::Keep,
                client_secret_file: ConfigurationReplacement::Keep,
                subscription_id: managed.subscription_id,
                resource_group: managed.resource_group,
                zone: managed.zone,
                terms_of_service_agreed: managed.terms_of_service_agreed,
            }),
            browser_assets: value.browser_assets,
            local_acquisition: value
                .local_acquisition
                .map(|local| BackendLocalAcquisitionDraft {
                    staging_root: local.staging_root,
                    credentials_file: ConfigurationReplacement::Keep,
                    server: local.server,
                    acknowledge_plaintext_login: local.acknowledge_plaintext_login,
                }),
            production_worker: value.production_worker.map(|worker| BackendWorkerDraft {
                queue_capacity: worker.queue_capacity,
                acquisition: worker
                    .acquisition
                    .map(|local| BackendLocalAcquisitionDraft {
                        staging_root: local.staging_root,
                        credentials_file: ConfigurationReplacement::Keep,
                        server: local.server,
                        acknowledge_plaintext_login: local.acknowledge_plaintext_login,
                    }),
                sharing: worker.sharing.map(|sharing| BackendSharingDraft {
                    policy: sharing.policy,
                    account: sharing.account.map(|account| BackendSharingAccountDraft {
                        credentials_file: ConfigurationReplacement::Keep,
                        server: account.server,
                        acknowledge_plaintext_login: account.acknowledge_plaintext_login,
                    }),
                    listen: sharing.listen,
                }),
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
                &[
                    "certificate_file",
                    "private_key_file",
                    "tls_mode",
                    "managed_tls",
                ][..],
            ),
            (
                ConfigurationSection::Storage,
                &["database", "cache", "storage"][..],
            ),
            (ConfigurationSection::MusicProviders, &["services"][..]),
            (
                ConfigurationSection::MusicAcquisition,
                &["local_acquisition"][..],
            ),
        ]
        .into_iter()
        .filter_map(|(section, fields)| {
            (fields.iter().any(|field| active[*field] != saved[*field])
                || (section == ConfigurationSection::MusicAcquisition
                    && (active["production_worker"]["queue_capacity"]
                        .as_u64()
                        .unwrap_or(64)
                        != saved["production_worker"]["queue_capacity"]
                            .as_u64()
                            .unwrap_or(64)
                        || active["production_worker"]["acquisition"]
                            != saved["production_worker"]["acquisition"]
                        || (active["production_worker"].is_null()
                            != saved["production_worker"].is_null()
                            && active["production_worker"]["sharing"].is_null()
                            && saved["production_worker"]["sharing"].is_null()))))
            .then_some(section)
        })
        .chain(
            (active["production_worker"]["sharing"] != saved["production_worker"]["sharing"])
                .then_some(ConfigurationSection::MusicSharing),
        )
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
        if !configuration.services.artist_images.is_empty() && configuration.cache.is_none() {
            return Err(issue(
                "services.artist_images",
                "Artist photos require the shared file cache.",
            ));
        }
        if configuration
            .production_worker
            .as_ref()
            .is_some_and(|worker| !(1..=1024).contains(&worker.queue_capacity))
        {
            return Err(issue(
                "production_worker.queue_capacity",
                "Use 1 through 1024 unfinished requests.",
            ));
        }
        if configuration.production_worker.is_some() && configuration.local_acquisition.is_some() {
            return Err(issue(
                "production_worker",
                "Select the production worker or local acquisition, not both.",
            ));
        }
        if let Some(worker) = &configuration.production_worker
            && let Some(sharing) = &worker.sharing
        {
            sharing
                .policy
                .validate()
                .map_err(|error| issue("production_worker.sharing.policy", &error.to_string()))?;
            sharing
                .resolved_account(worker.acquisition.as_ref())
                .map_err(|_| {
                    issue(
                        "production_worker.sharing.account",
                        "Sharing and acquisition must use the same account settings.",
                    )
                })?;
        }
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
            Environment::SingleUser => {
                if configuration.local_acquisition.is_some() {
                    return Err(issue(
                        "local_acquisition",
                        "The development harness is not single-user application setup.",
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
        let plain_http = matches!(configuration.environment, Environment::SingleUser);
        if origin.scheme() != if plain_http { "http" } else { "https" }
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
                "Use the mode's HTTP or HTTPS scheme with the listener port and no path, credentials or query.",
            ));
        }
        if matches!(
            configuration.environment,
            Environment::Development | Environment::SingleUser
        ) && !super::Api::development_origin(&origin)
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
        if !plain_http
            && configuration.tls_mode == TlsMode::Manual
            && (configuration.certificate_file.is_none()
                || configuration.private_key_file.is_none())
        {
            return Err(issue(
                "private_key_file",
                "HTTPS requires a certificate and private-key file.",
            ));
        }
        if !plain_http && configuration.tls_mode == TlsMode::AzureDns {
            configuration
                .managed_tls
                .as_ref()
                .ok_or_else(|| {
                    issue(
                        "managed_tls",
                        "Azure DNS mode requires its managed TLS settings.",
                    )
                })?
                .validate(&hostname)
                .map_err(|error| issue("managed_tls", &error.to_string()))?;
        }
        for (field, file) in [("database", &configuration.database)]
            .into_iter()
            .chain(
                configuration
                    .certificate_file
                    .iter()
                    .map(|path| ("certificate_file", path)),
            )
            .chain(
                configuration
                    .private_key_file
                    .iter()
                    .map(|path| ("private_key_file", path)),
            )
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
            }))
            .chain(
                configuration
                    .production_worker
                    .iter()
                    .filter_map(|worker| worker.acquisition.as_ref())
                    .flat_map(|local| {
                        [
                            (
                                "production_worker.acquisition.staging_root",
                                &local.staging_root,
                            ),
                            (
                                "production_worker.acquisition.credentials_file",
                                &local.credentials_file,
                            ),
                        ]
                    }),
            )
            .chain(
                configuration
                    .production_worker
                    .iter()
                    .filter_map(|worker| worker.sharing.as_ref())
                    .filter_map(|sharing| sharing.account.as_ref())
                    .map(|account| {
                        (
                            "production_worker.sharing.account.credentials_file",
                            &account.credentials_file,
                        )
                    }),
            )
        {
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
        let secure = !matches!(configuration.environment, Environment::SingleUser);
        for (field, path, sensitive) in [("database", &configuration.database, false)]
            .into_iter()
            .chain(
                configuration
                    .certificate_file
                    .iter()
                    .filter(|_| secure && configuration.tls_mode == TlsMode::Manual)
                    .map(|path| ("certificate_file", path, false)),
            )
            .chain(
                configuration
                    .private_key_file
                    .iter()
                    .filter(|_| secure && configuration.tls_mode == TlsMode::Manual)
                    .map(|path| ("private_key_file", path, true)),
            )
            .chain(
                configuration
                    .client_secret_file
                    .iter()
                    .filter(|_| secure)
                    .map(|path| ("client_secret_file", path, true)),
            )
        {
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
        let mut paths = vec![self.path.clone()];
        if !matches!(configuration.environment, Environment::SingleUser) {
            match configuration.tls_mode {
                TlsMode::Manual => {
                    paths.extend(configuration.certificate_file.iter().cloned());
                    paths.extend(configuration.private_key_file.iter().cloned());
                }
                TlsMode::AzureDns => {
                    paths.extend(
                        configuration
                            .managed_tls
                            .iter()
                            .map(|managed| managed.dns.client_secret_file.clone()),
                    );
                }
            }
            paths.extend(configuration.client_secret_file.iter().cloned());
        }
        paths.extend(configuration.browser_assets.iter().cloned());
        paths.extend(
            configuration
                .production_worker
                .iter()
                .filter_map(|worker| worker.sharing.as_ref())
                .filter_map(|sharing| sharing.account.as_ref())
                .map(|account| account.credentials_file.clone()),
        );
        if let Some(local) = &configuration.local_acquisition {
            paths.push(local.credentials_file.clone());
        }
        if let Some(local) = configuration
            .production_worker
            .as_ref()
            .and_then(|worker| worker.acquisition.as_ref())
        {
            paths.push(local.credentials_file.clone());
        }
        paths
    }

    fn check_files(&self, configuration: &Configuration, store: &Store) -> Result<()> {
        let protected = self.protected(configuration);
        crate::infrastructure::local_file::protect_files(
            store,
            &protected
                .iter()
                .filter(|path| configuration.browser_assets.as_ref() != Some(*path))
                .map(PathBuf::as_path)
                .collect::<Vec<_>>(),
        )?;
        if let Some(assets) = &configuration.browser_assets {
            if let Some(client) = store.selected_filesystem() {
                let directory = crate::infrastructure::filesystem::LibraryDirectory::open(assets)?;
                client.require_non_media_input(&directory)?;
            } else {
                crate::infrastructure::local_file::protect_files(store, &[assets])?;
            }
            crate::infrastructure::browser::BrowserAssets::load_for(
                assets,
                store.selected_filesystem(),
            )?;
        }
        if let Some(local) = &configuration.local_acquisition {
            local.validate(store, &configuration.services)?;
        }
        if let Some(worker) = &configuration.production_worker {
            worker.validate(store, &configuration.services)?;
        }
        if matches!(configuration.environment, Environment::SingleUser) {
            return Ok(());
        }
        self.client_secret(configuration)?;
        let origin = reqwest::Url::parse(&configuration.origin)
            .map_err(|_| Error::InvalidInput("service origin is invalid".into()))?;
        let hostname = origin
            .host_str()
            .ok_or(Error::Internal("hostname is missing"))?
            .trim_matches(['[', ']']);
        match configuration.tls_mode {
            TlsMode::Manual => {
                load_tls(self, configuration, hostname)?;
            }
            TlsMode::AzureDns => {
                let managed = configuration.managed_tls.as_ref().ok_or_else(|| {
                    Error::InvalidInput("managed TLS configuration is missing".into())
                })?;
                managed.binding_for(hostname, &store.source())?;
                self.check_certificate_layout(configuration, managed, store)?;
            }
        }
        Ok(())
    }

    fn check_certificate_layout(
        &self,
        configuration: &Configuration,
        managed: &ManagedTlsConfiguration,
        store: &Store,
    ) -> Result<()> {
        use crate::infrastructure::certificates::StateDirectory;
        store.inspect_certificate_root(&managed.state_directory)?;
        let candidate = StateDirectory::inspect(&managed.state_directory)?;
        for file in self
            .protected(configuration)
            .into_iter()
            .chain(std::iter::once(configuration.database.clone()))
        {
            if configuration.browser_assets.as_ref() == Some(&file) {
                let (path, directory) = LibraryDirectory::canonical(&file)?;
                if candidate.overlaps(&path, directory.native_root()) {
                    return Err(Error::InvalidInput(
                        "certificate state overlaps browser assets".into(),
                    ));
                }
            } else {
                let (parent, directory) = LibraryDirectory::canonical(
                    file.parent()
                        .ok_or(Error::Internal("protected file has no parent"))?,
                )?;
                let path = parent.join(
                    file.file_name()
                        .ok_or(Error::Internal("protected file has no name"))?,
                );
                if path.starts_with(&candidate.path)
                    || (candidate.exists
                        && directory
                            .native_root()
                            .ancestors
                            .contains(&candidate.native.identity))
                {
                    return Err(Error::InvalidInput(
                        "certificate state contains configuration, database or credentials".into(),
                    ));
                }
            }
        }
        for path in configuration
            .cache
            .iter()
            .map(|cache| &cache.root)
            .chain(
                configuration
                    .local_acquisition
                    .iter()
                    .map(|local| &local.staging_root),
            )
            .chain(
                configuration
                    .production_worker
                    .iter()
                    .filter_map(|worker| worker.acquisition.as_ref())
                    .map(|local| &local.staging_root),
            )
        {
            let (path, directory) = LibraryDirectory::canonical(path)?;
            if candidate.overlaps(&path, directory.native_root()) {
                return Err(Error::InvalidInput(
                    "certificate state overlaps cache or staging".into(),
                ));
            }
        }
        Ok(())
    }

    /// Local certificate commands use the same passive JSON and file boundary,
    /// without constructing an API host or requiring an active certificate.
    pub(crate) fn certificate_configuration(
        path: &Path,
    ) -> Result<(ManagedTlsConfiguration, String, Store)> {
        Self::certificate_configuration_mode(path, false)
    }

    pub(crate) fn certificate_operation_configuration(
        path: &Path,
    ) -> Result<(ManagedTlsConfiguration, String, Store)> {
        Self::certificate_configuration_mode(path, true)
    }

    fn certificate_configuration_mode(
        path: &Path,
        write: bool,
    ) -> Result<(ManagedTlsConfiguration, String, Store)> {
        let (service, configuration, hostname) = Self::load(path)?;
        if configuration.tls_mode != TlsMode::AzureDns
            || matches!(configuration.environment, Environment::SingleUser)
        {
            return Err(Error::InvalidInput(
                "certificate commands require explicit managed HTTPS configuration".into(),
            ));
        }
        let store = service.open_store(if write {
            StoreOpenMode::Current
        } else {
            StoreOpenMode::ReadCurrent
        })?;
        service.check_files(&configuration, &store)?;
        Ok((
            configuration.managed_tls.ok_or_else(|| {
                Error::InvalidInput("managed TLS configuration is missing".into())
            })?,
            hostname,
            store,
        ))
    }

    pub(super) fn startup_material(
        &self,
        store: &Store,
        hostname: &str,
    ) -> Result<(
        Option<ClientSecret>,
        Option<axum_server::tls_rustls::RustlsConfig>,
    )> {
        self.check_files(&self.active, store)?;
        if matches!(self.active.environment, Environment::SingleUser) {
            return Ok((None, None));
        }
        if self.active.tls_mode == TlsMode::AzureDns {
            let managed = self.active.managed_tls.as_ref().ok_or_else(|| {
                Error::InvalidInput("managed TLS configuration is missing".into())
            })?;
            let directory = crate::infrastructure::certificates::StateDirectory::inspect(
                &managed.state_directory,
            )?;
            if !directory.exists {
                return Err(Error::InvalidInput(
                    "managed TLS state is missing; run local bootstrap".into(),
                ));
            }
            store.check_native_root(
                directory.path.to_str().ok_or_else(|| {
                    Error::InvalidInput("certificate state path must be Unicode".into())
                })?,
                "certificate",
                &directory.native,
            )?;
            let (selected, _, tls) =
                crate::infrastructure::certificates::state::StateStore::load_active_for(
                    store,
                    &managed.state_directory,
                    &managed.binding_for(hostname, &store.source())?,
                    hostname,
                )?;
            store.check_native_root(
                directory.path.to_str().ok_or_else(|| {
                    Error::InvalidInput("certificate state path must be Unicode".into())
                })?,
                "certificate",
                &selected.root,
            )?;
            *self
                .certificate_selection
                .lock()
                .map_err(|_| Error::Internal("live certificate selection lock failed"))? =
                Some(selected);
            return Ok((self.client_secret(&self.active)?, Some(tls)));
        }
        Ok((
            self.client_secret(&self.active)?,
            Some(load_tls(self, &self.active, hostname)?),
        ))
    }

    fn client_secret(&self, configuration: &Configuration) -> Result<Option<ClientSecret>> {
        configuration
            .client_secret_file
            .as_ref()
            .map(|path| {
                let bytes = self.read_input(path, 4096, true)?;
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

    pub(super) fn tls_material(&self, configuration: &Configuration) -> Result<(Vec<u8>, Vec<u8>)> {
        Ok((
            self.read_input(
                configuration
                    .certificate_file
                    .as_deref()
                    .ok_or_else(|| Error::InvalidInput("TLS certificate file is missing".into()))?,
                262144,
                false,
            )?,
            self.read_input(
                configuration
                    .private_key_file
                    .as_deref()
                    .ok_or_else(|| Error::InvalidInput("TLS private-key file is missing".into()))?,
                16384,
                true,
            )?,
        ))
    }
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
    crate::infrastructure::local_file::validate_reference(path)
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
            } else if message.contains("sharing") {
                "production_worker.sharing"
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

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn summary(configuration: &Configuration) -> BackendConfiguration {
    BackendConfiguration {
        environment: match configuration.environment {
            Environment::Production => "production",
            Environment::Development => "development",
            Environment::SingleUser => "single_user",
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
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        private_key_file_configured: configuration.private_key_file.is_some(),
        tls_mode: configuration.tls_mode,
        managed_tls: configuration
            .managed_tls
            .as_ref()
            .map(|managed| BackendManagedTls {
                environment: managed.environment,
                state_directory: managed.state_directory.to_string_lossy().into_owned(),
                tenant_id_configured: !managed.dns.tenant_id.is_empty(),
                client_id_configured: !managed.dns.client_id.is_empty(),
                client_secret_file_configured: !managed
                    .dns
                    .client_secret_file
                    .as_os_str()
                    .is_empty(),
                subscription_id: managed.dns.subscription_id.clone(),
                resource_group: managed.dns.resource_group.clone(),
                zone: managed.dns.zone.clone(),
                terms_of_service_agreed: managed.terms_of_service_agreed,
            }),
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
        production_worker: configuration
            .production_worker
            .as_ref()
            .map(|worker| BackendWorker {
                queue_capacity: worker.queue_capacity,
                acquisition: worker
                    .acquisition
                    .as_ref()
                    .map(|local| BackendLocalAcquisition {
                        staging_root: local.staging_root.to_string_lossy().into_owned(),
                        credentials_file_configured: !local.credentials_file.as_os_str().is_empty(),
                        server: local.server.to_string(),
                        acknowledge_plaintext_login: local.acknowledge_plaintext_login,
                    }),
                sharing: worker.sharing.as_ref().map(|sharing| BackendSharing {
                    policy: sharing.policy.clone(),
                    account: sharing
                        .account
                        .as_ref()
                        .map(|account| BackendSharingAccount {
                            credentials_file_configured: !account
                                .credentials_file
                                .as_os_str()
                                .is_empty(),
                            server: account.server.to_string(),
                            acknowledge_plaintext_login: account.acknowledge_plaintext_login,
                        }),
                    listen: sharing.listen.map(|listen| listen.to_string()),
                }),
            }),
    }
}

#[cfg(test)]
mod tests;
