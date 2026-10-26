use super::catalog::CatalogQuery;
use super::filesystem::require_local_disk;
use super::local_file::{protect_files, read_local};
use super::soulseek::{SoulseekSession, public_peer_ip};
use super::staging::StagingRoot;
use super::store::{JobClaim, SourceFile, Store};
use crate::application::export_playback_album;
use crate::application::services::{Capability, ServiceConfiguration};
use crate::domain::JobState;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::net::{SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Import only bounded local folder metadata for a separate unapproved review.
/// No credentials, live providers, staging or worker claims are used.
pub fn prepare_source_candidate_file(
    database: &Path,
    job: i64,
    version: i64,
    path: &Path,
) -> Result<super::store::SourceCandidate> {
    let snapshot = Store::open_read_snapshot(database)?;
    protect_files(&snapshot, &[path])?;
    let listing = serde_json::from_slice(&read_local(path, 1_048_576, false)?)
        .map_err(|_| Error::InvalidInput("invalid source folder listing".into()))?;
    drop(snapshot);
    Store::open_current_existing(database)?.prepare_source_candidate(job, version, listing)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationConfiguration {
    database: PathBuf,
    staging_root: PathBuf,
    job_id: i64,
    plan_version: i64,
    normalization_run: i64,
    acknowledge_new_only_publication: bool,
}

#[derive(Serialize)]
pub struct PublicationOutcome {
    pub state: String,
    pub destinations: Vec<super::store::PublicationDestination>,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ResumePublication {
    pub normalization_run: i64,
    pub acknowledge_new_only_publication: bool,
}

/// Resume only an existing publication journal. Browser input selects no paths
/// or credentials. All plan, claim, staging and exact-file checks still apply.
pub fn resume_publication(
    database: &Path,
    staging_root: &Path,
    job: i64,
    version: i64,
    request: ResumePublication,
) -> Result<PublicationOutcome> {
    publish_configuration(
        PublicationConfiguration {
            database: database.to_owned(),
            staging_root: staging_root.to_owned(),
            job_id: job,
            plan_version: version,
            normalization_run: request.normalization_run,
            acknowledge_new_only_publication: request.acknowledge_new_only_publication,
        },
        &[],
        true,
    )
}

/// Explicit local publication, never an acquisition retry or replacement.
/// No account credentials or provider connection is used by this command.
pub fn publish_new_once(path: &Path) -> Result<PublicationOutcome> {
    let configuration: PublicationConfiguration =
        serde_json::from_slice(&read_local(path, 16384, false)?)
            .map_err(|_| Error::InvalidInput("invalid publication configuration".into()))?;
    publish_configuration(configuration, &[path], false)
}

fn publish_configuration(
    configuration: PublicationConfiguration,
    protected_files: &[&Path],
    require_journal: bool,
) -> Result<PublicationOutcome> {
    if !configuration.acknowledge_new_only_publication
        || !configuration.database.is_absolute()
        || !configuration.staging_root.is_absolute()
        || configuration.job_id <= 0
        || configuration.plan_version <= 0
        || configuration.normalization_run <= 0
    {
        return Err(Error::InvalidInput("publication requires absolute local state/staging paths, positive IDs and explicit new-only acknowledgement".into()));
    }
    let mut store = Store::open_current_existing(&configuration.database)?;
    protect_files(&store, protected_files)?;
    if !matches!(
        store.job_state(configuration.job_id)?.as_deref(),
        Some("publishing" | "backup_pending")
    ) {
        return Err(Error::Conflict(
            "publication needs a publishing or backup_pending job".into(),
        ));
    }
    if require_journal {
        let targets =
            store.publication_destinations(configuration.job_id, configuration.plan_version)?;
        if targets.is_empty()
            || targets
                .iter()
                .any(|target| target.run_id != configuration.normalization_run)
        {
            return Err(Error::Conflict(
                "publication recovery requires the exact retained publication journal".into(),
            ));
        }
    }
    let claimed = store
        .claim_approved_plan(
            configuration.job_id,
            configuration.plan_version,
            &format!("publication-{}", std::process::id()),
            300,
        )?
        .ok_or_else(|| Error::Conflict("publication job already has a live worker".into()))?;
    store.publication_source(&claimed.claim, configuration.normalization_run)?;
    let staging = StagingRoot::open(&mut store, &configuration.staging_root)?;
    let state = crate::application::publish_new_album(
        &mut store,
        &claimed.claim,
        &staging,
        configuration.normalization_run,
    )?;
    Ok(PublicationOutcome {
        state,
        destinations: store
            .publication_destinations(configuration.job_id, configuration.plan_version)?,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    #[serde(default)]
    services: ServiceConfiguration,
    database: PathBuf,
    staging_root: PathBuf,
    credentials_file: PathBuf,
    server: SocketAddrV4,
    job_id: i64,
    plan_version: i64,
    acknowledge_plaintext_login: bool,
    #[serde(default)]
    export_for_playback: bool,
    source: Option<SourceSelection>,
    source_candidate: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceSelection {
    release_id: String,
    peer: String,
    acknowledge_exact_edition_and_permitted_download: bool,
    files: Vec<SelectedFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedFile {
    track_number: u16,
    remote_path: String,
    expected_bytes: i64,
}

impl SourceSelection {
    fn checked(self, store: &Store, job: i64, version: i64) -> Result<(String, Vec<SourceFile>)> {
        let plan = store.plan(job, version)?;
        let release = plan.plan.catalog.as_ref().ok_or_else(|| {
            Error::Conflict("source selection requires a provider-backed exact-release plan".into())
        })?;
        if !self.acknowledge_exact_edition_and_permitted_download
            || self.release_id != release.data.summary.id
            || self.files.is_empty()
            || self.files.len() != plan.plan.edition.tracks.len()
            || self.files.len() > 1000
            || self
                .files
                .iter()
                .enumerate()
                .any(|(index, file)| usize::from(file.track_number) != index + 1)
        {
            return Err(Error::InvalidInput(
                "confirm permitted use of the exact release and map every track in frozen release order".into(),
            ));
        }
        super::soulseek::username(&self.peer)?;
        let files = self
            .files
            .into_iter()
            .map(|file| SourceFile {
                remote_path: file.remote_path,
                expected_bytes: file.expected_bytes,
            })
            .collect::<Vec<_>>();
        store.validate_acquisition_source(job, version, &self.peer, &files)?;
        Ok((self.peer, files))
    }
}

// Deliberately no Debug, Serialize, Clone or public credential fields.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    username: String,
    password: String,
}

fn read_credentials(path: &Path) -> Result<Credentials> {
    let mut bytes = read_local(path, 4096, true)?;
    let parsed = serde_json::from_slice::<Credentials>(&bytes);
    bytes.fill(0);
    let credentials =
        parsed.map_err(|_| Error::InvalidInput("invalid Soulseek credentials file".into()))?;
    super::soulseek::validate_credentials(&credentials.username, &credentials.password)?;
    Ok(credentials)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchConfiguration {
    #[serde(default)]
    services: ServiceConfiguration,
    database: PathBuf,
    credentials_file: PathBuf,
    server: SocketAddrV4,
    release_id: String,
    acknowledge_plaintext_login: bool,
    folder: Option<FolderSelection>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderSelection {
    peer: String,
    remote_folder: String,
    format: crate::domain::AudioFormat,
}

impl SearchConfiguration {
    fn read(
        path: &Path,
        folder_query: bool,
    ) -> Result<(Self, crate::domain::CatalogRelease, Credentials)> {
        let config: Self = serde_json::from_slice(&read_local(path, 16384, false)?)
            .map_err(|_| Error::InvalidInput("invalid source-read configuration".into()))?;
        Self::validate(config, &[path], folder_query)
    }

    fn validate(
        config: Self,
        protected: &[&Path],
        folder_query: bool,
    ) -> Result<(Self, crate::domain::CatalogRelease, Credentials)> {
        config.services.validate_builtins()?;
        config
            .services
            .require(Capability::Acquisition, "soulseek")?;
        if !config.acknowledge_plaintext_login
            || config.server.port() == 0
            || !public_peer_ip(*config.server.ip())
            || !config.database.is_absolute()
            || !config.credentials_file.is_absolute()
            || config.folder.is_some() != folder_query
        {
            return Err(Error::InvalidInput("source reads need absolute local files, a public server, plaintext-login acknowledgement and the correct query selection".into()));
        }
        if let Some(folder) = &config.folder {
            super::soulseek::username(&folder.peer)?;
            crate::domain::sources::remote_identifier(&folder.remote_folder)?;
        }
        let query = CatalogQuery::release(&config.release_id)?;
        let store = Store::open_read_snapshot(&config.database)?;
        let mut files = protected.to_vec();
        files.push(&config.credentials_file);
        protect_files(&store, &files)?;
        let cache = store.peek_catalog_cache(&query)?.ok_or_else(|| {
            Error::Conflict(
                "load the exact MusicBrainz release with catalog-release before source reads"
                    .into(),
            )
        })?;
        let edition = query.parse_release(&cache.body)?;
        let credentials = read_credentials(&config.credentials_file)?;
        Ok((config, edition, credentials))
    }
}

fn metadata_session(credentials: Credentials, server: SocketAddrV4) -> Result<SoulseekSession> {
    let socket = TcpStream::connect_timeout(&server.into(), Duration::from_secs(10))?;
    SoulseekSession::login(socket, &credentials.username, &credentials.password)
}

fn source_token() -> Result<u32> {
    let mut bytes = [0; 4];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| Error::Internal("source request token randomness failed"))?;
    Ok(u32::from_le_bytes(bytes).max(1))
}

/// A selected metadata-only query. Loading never opens a socket or mutates
/// state. Running does not claim jobs, approve editions or queue audio.
pub struct PreparedFolder {
    credentials: Credentials,
    server: SocketAddrV4,
    edition: crate::domain::EditionSelection,
    selection: FolderSelection,
}

#[derive(Serialize, ts_rs::TS)]
pub struct SourceFolderReport {
    pub listing: crate::domain::sources::SourceFolder,
    pub assessment: crate::domain::sources::SourceAssessment,
}

impl PreparedFolder {
    pub fn load(path: &Path) -> Result<Self> {
        let (config, release, credentials) = SearchConfiguration::read(path, true)?;
        let edition = release.selection(None)?;
        let selection = config.folder.ok_or_else(|| {
            Error::InvalidInput("source-folder requires one selected peer folder".into())
        })?;
        Ok(Self {
            credentials,
            server: config.server,
            edition,
            selection,
        })
    }

    pub fn run(self) -> Result<SourceFolderReport> {
        let token = source_token()?;
        let mut session = metadata_session(self.credentials, self.server)?;
        let peer = session.connect_peer(&self.selection.peer)?;
        let listing = peer.folder_contents(&self.selection.remote_folder, token)?;
        let assessment =
            crate::domain::sources::assess_source(&self.edition, self.selection.format, &listing)?;
        Ok(SourceFolderReport {
            listing,
            assessment,
        })
    }
}

/// Explicit metadata-only network operation, separate from worker claims,
/// source selection and audio download. The release must be in the real cache.
pub struct PreparedSearch {
    credentials: Credentials,
    server: SocketAddrV4,
    query: String,
}

impl PreparedSearch {
    pub fn load(path: &Path) -> Result<Self> {
        let (config, release, credentials) = SearchConfiguration::read(path, false)?;
        let query = format!("{} {}", release.summary.artist, release.summary.title);
        if query.len() > 256 || query.chars().any(char::is_control) {
            return Err(Error::InvalidInput(
                "release search text exceeds protocol bounds".into(),
            ));
        }
        Ok(Self {
            credentials,
            server: config.server,
            query,
        })
    }

    pub fn run(self) -> Result<super::soulseek::search::SearchReport> {
        let token = source_token()?;
        let session = metadata_session(self.credentials, self.server)?;
        session.search(&self.query, token)
    }
}

/// One explicit run of an approved acquisition. Loading validates local state,
/// takes a worker claim and optionally journals a user-selected complete source.
/// Network access starts solely in run(). It never searches for peers, retries
/// uncertain submissions or publishes destinations.
pub struct PreparedWorker {
    services: ServiceConfiguration,
    store: Store,
    claim: JobClaim,
    staging: StagingRoot,
    credentials: Credentials,
    server: SocketAddrV4,
    peer: String,
    export_for_playback: bool,
    normalize_only: bool,
}

#[derive(Serialize)]
pub struct WorkerOutcome {
    pub normalization_run: i64,
    pub playback_folder: Option<PathBuf>,
    pub publication_pending: bool,
}

/// A successful check is local configuration evidence only. It does not prove
/// network/account reachability, staged bytes, free disk space or a future claim.
#[derive(Serialize)]
pub struct WorkerCheck {
    pub job_id: i64,
    pub plan_version: i64,
    pub job_state: String,
    pub tracks: usize,
    pub received_tracks: usize,
    pub source_will_be_recorded: bool,
    pub export_for_playback: bool,
    pub network_checked: bool,
    pub staging_checked: bool,
    pub claim_acquired: bool,
}

struct ValidatedWorker {
    configuration: Configuration,
    store: Store,
    state: String,
    source: Option<(String, Vec<SourceFile>)>,
    credentials: Credentials,
}

impl ValidatedWorker {
    fn read(path: &Path, snapshot: bool) -> Result<Self> {
        let configuration: Configuration = serde_json::from_slice(&read_local(path, 16384, false)?)
            .map_err(|_| Error::InvalidInput("invalid worker configuration".into()))?;
        Self::validate(configuration, &[path], snapshot)
    }

    fn validate(
        mut configuration: Configuration,
        protected: &[&Path],
        snapshot: bool,
    ) -> Result<Self> {
        configuration.services.validate_builtins()?;
        configuration
            .services
            .require(Capability::Acquisition, "soulseek")?;
        if configuration.job_id <= 0
            || configuration.plan_version <= 0
            || !configuration.acknowledge_plaintext_login
            || configuration.server.port() == 0
            || !public_peer_ip(*configuration.server.ip())
        {
            return Err(Error::InvalidInput(
                "worker needs positive job/plan IDs, a public server address and explicit plaintext-login acknowledgement".into(),
            ));
        }
        for file in [&configuration.database, &configuration.credentials_file] {
            if !file.is_absolute() {
                return Err(Error::InvalidInput(
                    "worker file references must be absolute".into(),
                ));
            }
            require_local_disk(&std::fs::canonicalize(file)?)?;
        }
        let store = if snapshot {
            Store::open_read_snapshot(&configuration.database)?
        } else {
            Store::open_current_existing(&configuration.database)?
        };
        let mut files = protected.to_vec();
        files.push(&configuration.credentials_file);
        protect_files(&store, &files)?;
        let state = store.job_state(configuration.job_id)?;
        if snapshot {
            store.require_job_running(configuration.job_id, configuration.plan_version)?;
        }
        let source = match (state.as_deref(), configuration.source.take(),configuration.source_candidate) {
            (Some("queued" | "searching"), Some(source),None) => Some(source.checked(
                &store, configuration.job_id, configuration.plan_version,
            )?),
            (Some("queued" | "searching"),None,Some(candidate))=>Some(store.approved_source_candidate(
                configuration.job_id,configuration.plan_version,candidate)?),
            (Some("downloading" | "validating" | "normalizing"), None,None) => None,
            _ => return Err(Error::Conflict(
                "provide exactly one manual source or approved source_candidate for an unselected queued/searching job; omit both for an existing downloading/validating/normalizing source".into(),
            )),
        };
        let credentials = read_credentials(&configuration.credentials_file)?;
        if snapshot {
            store.approved_artwork(configuration.job_id, configuration.plan_version)?;
        }
        if matches!(state.as_deref(), Some("validating" | "normalizing"))
            && store
                .acquisition_preview(configuration.job_id, configuration.plan_version)?
                .iter()
                .any(|file| file.state != "received")
        {
            return Err(Error::Conflict(
                "local normalization requires all received input receipts".into(),
            ));
        }
        Ok(Self {
            configuration,
            store,
            state: state.ok_or_else(|| Error::Conflict("worker job does not exist".into()))?,
            source,
            credentials,
        })
    }
}

impl PreparedWorker {
    /// Check local inputs without claiming the job, changing source state,
    /// opening staging or making network requests. Run reloads all inputs.
    pub fn check(path: &Path) -> Result<WorkerCheck> {
        let validated = ValidatedWorker::read(path, true)?;
        let (tracks, received_tracks) = if let Some((_, files)) = &validated.source {
            (files.len(), 0)
        } else {
            let files = validated.store.acquisition_preview(
                validated.configuration.job_id,
                validated.configuration.plan_version,
            )?;
            (
                files.len(),
                files.iter().filter(|file| file.state == "received").count(),
            )
        };
        Ok(WorkerCheck {
            job_id: validated.configuration.job_id,
            plan_version: validated.configuration.plan_version,
            job_state: validated.state,
            tracks,
            received_tracks,
            source_will_be_recorded: validated.source.is_some(),
            export_for_playback: validated.configuration.export_for_playback,
            network_checked: false,
            staging_checked: false,
            claim_acquired: false,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::from_validated(ValidatedWorker::read(path, false)?)
    }

    fn from_validated(validated: ValidatedWorker) -> Result<Self> {
        let ValidatedWorker {
            configuration,
            mut store,
            state,
            source,
            credentials,
        } = validated;
        if let Some(decision) =
            store.check_artwork(configuration.job_id, configuration.plan_version)?
        {
            return Err(Error::ArtworkRequired(decision.question_id));
        }
        let claimed = store
            .claim_approved_plan(
                configuration.job_id,
                configuration.plan_version,
                &format!("worker-once-{}", std::process::id()),
                300,
            )?
            .ok_or_else(|| Error::Conflict("job is already owned by a live worker".into()))?;
        store.job_work_boundary(&claimed.claim)?;
        let normalize_only = matches!(state.as_str(), "validating" | "normalizing");
        if source.is_none() && !normalize_only {
            store.acquisition_files(&claimed.claim)?;
        }
        let staging = StagingRoot::open(&mut store, &configuration.staging_root)?;
        if let Some((peer, files)) = source {
            if state == "queued" {
                store.advance(&claimed.claim, JobState::Queued, JobState::Searching)?;
            }
            if let Some(candidate) = configuration.source_candidate {
                store.begin_reviewed_acquisition(&claimed.claim, candidate)?;
            } else {
                store.begin_acquisition(&claimed.claim, &peer, &files)?;
            }
        }
        let files = if normalize_only {
            store.acquisition_preview(configuration.job_id, configuration.plan_version)?
        } else {
            store.acquisition_files(&claimed.claim)?
        };
        let peer = files
            .first()
            .ok_or(Error::Internal("selected source is empty"))?
            .peer
            .clone();
        for file in &files {
            if file.state == "received" {
                staging.received(file)?;
            }
        }
        Ok(Self {
            services: configuration.services,
            store,
            claim: claimed.claim,
            staging,
            credentials,
            server: configuration.server,
            peer,
            export_for_playback: configuration.export_for_playback,
            normalize_only,
        })
    }

    pub fn run(self) -> Result<WorkerOutcome> {
        self.execute(false)
    }

    fn execute(mut self, publish: bool) -> Result<WorkerOutcome> {
        self.store.renew_claim(&self.claim, 300)?;
        self.store.job_work_boundary(&self.claim)?;
        if !self.normalize_only
            && self
                .store
                .acquisition_files(&self.claim)?
                .iter()
                .all(|file| file.state == "received")
        {
            self.store
                .advance(&self.claim, JobState::Downloading, JobState::Validating)?;
            self.normalize_only = true;
        }
        let run = if self.normalize_only {
            self.credentials.password.clear();
            drop(self.credentials);
            let cover = self
                .store
                .approved_artwork(self.claim.job(), self.claim.plan_version())?;
            crate::application::normalize_album(
                &mut self.store,
                &self.claim,
                &self.staging,
                &cover,
            )?
        } else {
            self.store.acquisition_files(&self.claim)?;
            self.store.job_work_boundary(&self.claim)?;
            let socket = TcpStream::connect_timeout(&self.server.into(), Duration::from_secs(10))?;
            let session = SoulseekSession::login(
                socket,
                &self.credentials.username,
                &self.credentials.password,
            )?;
            // Do not keep the password through the album receive/normalization.
            self.credentials.password.clear();
            drop(self.credentials);
            self.store.renew_claim(&self.claim, 300)?;
            let mut provider = super::services::SoulseekAcquisition {
                session,
                peer: self.peer,
            };
            crate::application::services::acquisition::acquire(
                &self.services,
                &mut provider,
                &mut self.store,
                &self.claim,
                &self.staging,
            )?
        };
        let playback_folder = if self.export_for_playback {
            Some(export_playback_album(
                &self.store,
                &self.claim,
                &self.staging,
                run,
            )?)
        } else {
            None
        };
        let publication_pending = if publish {
            crate::application::publish_new_album(&mut self.store, &self.claim, &self.staging, run)?
                != "complete"
        } else {
            true
        };
        Ok(WorkerOutcome {
            normalization_run: run,
            playback_folder,
            publication_pending,
        })
    }
}

/// Operator-selected local acquisition settings. Browser input cannot select a
/// database, credential file, server address or native destination.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalAcquisition {
    pub staging_root: PathBuf,
    pub credentials_file: PathBuf,
    pub server: SocketAddrV4,
    pub acknowledge_plaintext_login: bool,
}

impl LocalAcquisition {
    pub fn validate(&self, store: &Store, services: &ServiceConfiguration) -> Result<()> {
        services.validate_builtins()?;
        services.require(Capability::Acquisition, "soulseek")?;
        if !self.staging_root.is_absolute()
            || !self.credentials_file.is_absolute()
            || !self.acknowledge_plaintext_login
            || self.server.port() == 0
            || !public_peer_ip(*self.server.ip())
        {
            return Err(Error::InvalidInput(
                "local acquisition needs absolute staging/credential paths, a public server and explicit plaintext-login acknowledgement".into(),
            ));
        }
        protect_files(store, &[&self.credentials_file])?;
        let root = super::filesystem::LibraryDirectory::open(&self.staging_root)?;
        let canonical = std::fs::canonicalize(&self.staging_root)?;
        require_local_disk(&canonical)?;
        store.check_native_root(
            canonical
                .to_str()
                .ok_or_else(|| Error::InvalidInput("staging path is not UTF-8".into()))?,
            "staging",
            root.native_root(),
        )?;
        Ok(())
    }

    fn metadata(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        release_id: String,
        folder: Option<FolderSelection>,
    ) -> Result<(
        SearchConfiguration,
        crate::domain::CatalogRelease,
        Credentials,
    )> {
        let is_folder = folder.is_some();
        SearchConfiguration::validate(
            SearchConfiguration {
                services,
                database: database.to_owned(),
                credentials_file: self.credentials_file.clone(),
                server: self.server,
                release_id,
                acknowledge_plaintext_login: self.acknowledge_plaintext_login,
                folder,
            },
            &[],
            is_folder,
        )
    }

    pub fn search(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        release_id: String,
    ) -> Result<super::soulseek::search::SearchReport> {
        let (config, release, credentials) = self.metadata(database, services, release_id, None)?;
        let query = format!("{} {}", release.summary.artist, release.summary.title);
        if query.len() > 256 || query.chars().any(char::is_control) {
            return Err(Error::InvalidInput(
                "release search text exceeds protocol bounds".into(),
            ));
        }
        PreparedSearch {
            credentials,
            server: config.server,
            query,
        }
        .run()
    }

    pub fn folder(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        request: SourceFolderRequest,
    ) -> Result<SourceFolderReport> {
        let (config, release, credentials) = self.metadata(
            database,
            services,
            request.release_id,
            Some(FolderSelection {
                peer: request.peer,
                remote_folder: request.remote_folder,
                format: request.format,
            }),
        )?;
        PreparedFolder {
            credentials,
            server: config.server,
            edition: release.selection(None)?,
            selection: config
                .folder
                .ok_or(Error::Internal("folder selection missing"))?,
        }
        .run()
    }

    pub fn acquire_and_publish(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        job_id: i64,
        plan_version: i64,
        request: StartLocalDownload,
    ) -> Result<WorkerOutcome> {
        if !request.acknowledge_new_only_publication {
            return Err(Error::InvalidInput(
                "new-only publication needs explicit acknowledgement".into(),
            ));
        }
        let validated = ValidatedWorker::validate(
            Configuration {
                services,
                database: database.to_owned(),
                staging_root: self.staging_root.clone(),
                credentials_file: self.credentials_file.clone(),
                server: self.server,
                job_id,
                plan_version,
                acknowledge_plaintext_login: self.acknowledge_plaintext_login,
                export_for_playback: false,
                source: None,
                source_candidate: request.source_candidate,
            },
            &[],
            false,
        )?;
        PreparedWorker::from_validated(validated)?.execute(true)
    }
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct SourceSearchRequest {
    pub release_id: String,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct SourceFolderRequest {
    pub release_id: String,
    pub peer: String,
    pub remote_folder: String,
    pub format: crate::domain::AudioFormat,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct StartLocalDownload {
    pub source_candidate: Option<i64>,
    pub acknowledge_new_only_publication: bool,
}
