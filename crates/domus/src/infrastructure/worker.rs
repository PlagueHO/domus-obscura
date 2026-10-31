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
    pub cleanup: Option<super::store::RecoveryOperation>,
    pub cleanup_error: Option<String>,
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
    publish_controlled(configuration, protected_files, require_journal, None)
}

pub(crate) fn publish_worker(
    database: &Path,
    staging_root: &Path,
    job: i64,
    version: i64,
    run: i64,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<PublicationOutcome> {
    publish_controlled(
        PublicationConfiguration {
            database: database.to_owned(),
            staging_root: staging_root.to_owned(),
            job_id: job,
            plan_version: version,
            normalization_run: run,
            acknowledge_new_only_publication: true,
        },
        &[],
        false,
        Some(stop),
    )
}

fn publish_controlled(
    configuration: PublicationConfiguration,
    protected_files: &[&Path],
    require_journal: bool,
    stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
    if let Some(stop) = stop {
        store = store.with_worker_stop(stop);
    }
    store.check_worker_stop()?;
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
    let result = (|| {
        store.publication_source(&claimed.claim, configuration.normalization_run)?;
        let staging = StagingRoot::open(&mut store, &configuration.staging_root)?;
        crate::application::publish_new_album(
            &mut store,
            &claimed.claim,
            &staging,
            configuration.normalization_run,
        )
    })();
    store.release_job_claim(&claimed.claim)?;
    let state = result?;
    Ok(PublicationOutcome {
        state,
        cleanup: store.recovery_operation(configuration.normalization_run * 8 + 1)?,
        cleanup_error: store
            .recovery_inventory(
                crate::domain::Page::new(configuration.normalization_run * 8, 1)?,
                Some(configuration.job_id),
            )?
            .into_iter()
            .find(|e| e.id == configuration.normalization_run * 8 + 1)
            .and_then(|e| e.last_error),
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
pub(crate) struct Credentials {
    pub(crate) username: String,
    pub(crate) password: String,
}

pub(crate) fn read_credentials(path: &Path) -> Result<Credentials> {
    let mut bytes = read_local(path, 4096, true)?;
    let parsed = serde_json::from_slice::<Credentials>(&bytes);
    bytes.fill(0);
    let credentials =
        parsed.map_err(|_| Error::InvalidInput("invalid Soulseek credentials file".into()))?;
    super::soulseek::validate_credentials(&credentials.username, &credentials.password)?;
    Ok(credentials)
}

pub(crate) fn account_lock(
    database: &Path,
    credentials: &Credentials,
) -> Result<super::local_file::ProcessLock> {
    use sha2::{Digest, Sha256};
    super::local_file::ProcessLock::acquire(
        database,
        &format!(
            "account-{:x}",
            Sha256::digest(credentials.username.as_bytes())
        ),
    )
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
    #[serde(default)]
    audio_profile: Option<crate::domain::AudioProfile>,
}

impl SearchConfiguration {
    fn read(
        path: &Path,
        folder_query: bool,
    ) -> Result<(Self, crate::domain::CatalogRelease, Credentials)> {
        Self::read_selected(path, folder_query, None)
    }

    fn read_selected(
        path: &Path,
        folder_query: bool,
        host: Option<&Path>,
    ) -> Result<(Self, crate::domain::CatalogRelease, Credentials)> {
        let mut config: Self = serde_json::from_slice(&read_local(path, 16384, false)?)
            .map_err(|_| Error::InvalidInput("invalid source-read configuration".into()))?;
        config.services.validate_builtins()?;
        config
            .services
            .require(Capability::Acquisition, "soulseek")?;
        let selected = host
            .map(|host| {
                super::services::load_cli_source_store(
                    host,
                    &config.database,
                    &[path, &config.credentials_file],
                )
            })
            .transpose()?;
        let cache = selected.as_ref().and_then(Store::cache);
        if let Some(store) = &selected {
            config.database = store.database_path().to_owned();
        }
        Self::validate(config, &[path], folder_query, cache)
    }

    fn validate(
        config: Self,
        protected: &[&Path],
        folder_query: bool,
        cache: Option<std::sync::Arc<dyn crate::application::services::cache::Cache>>,
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
            if folder
                .audio_profile
                .is_some_and(|profile| profile.format() != folder.format)
            {
                return Err(Error::InvalidInput(
                    "folder audio profile differs from its format".into(),
                ));
            }
        }
        let query = CatalogQuery::release(&config.release_id)?;
        let store = Store::open_read_snapshot(&config.database)?;
        let mut files = protected.to_vec();
        files.push(&config.credentials_file);
        protect_files(&store, &files)?;
        let cached = if let Some(cache) = cache {
            drop(store);
            Store::open_read_current(&config.database)?
                .with_cache(cache)
                .peek_catalog_cache(&query)?
        } else {
            store.peek_catalog_cache(&query)?
        };
        let cache = cached.ok_or_else(|| {
            Error::Conflict(
                "load the exact MusicBrainz release through the selected catalog cache before source reads"
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

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::application::services::cache::{Cache, CacheKey, testing::MemoryCache};
    use std::sync::Arc;

    #[test]
    fn shared_cache_source_metadata_uses_the_host_cache_without_sqlite_fallback_or_network()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let state = crate::infrastructure::permissions::private_test_directory()?;
        let database = state.path().join("state.db");
        let store = Store::open(&database)?;
        let credentials = state.path().join("credentials.json");
        std::fs::write(
            &credentials,
            br#"{"username":"generated-fixture","password":"generated-only"}"#,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
        }
        let release = "20000000-0000-4000-8000-000000000001";
        let artist = "10000000-0000-4000-8000-000000000001";
        let body = serde_json::to_vec(&serde_json::json!({
            "id":release,"title":"Generated album",
            "artist-credit":[{"name":"Generated artist","artist":{"id":artist,"name":"Generated artist"}}],
            "release-group":{"id":"30000000-0000-4000-8000-000000000001"},
            "media":[{"position":1,"track-count":1,"tracks":[{
                "id":"40000000-0000-4000-8000-000000000001","position":1,"number":"1","title":"Generated track",
                "artist-credit":[{"name":"Generated artist","artist":{"id":artist,"name":"Generated artist"}}],
                "recording":{"id":"50000000-0000-4000-8000-000000000001","video":false}
            }]}]
        }))?;
        let shared = Arc::new(MemoryCache::default());
        let query = CatalogQuery::release(release)?;
        let key = CacheKey::new("musicbrainz", "responses-v1", query.source())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        shared.put(&key, &body, now, now + 86400)?;
        let configuration = LocalAcquisition {
            staging_root: state.path().join("unused-staging"),
            credentials_file: credentials.clone(),
            server: "8.8.8.8:2242".parse()?,
            acknowledge_plaintext_login: true,
        };
        let (_, exact, _) = configuration.metadata(
            &database,
            Default::default(),
            release.into(),
            None,
            Some(shared.clone()),
        )?;
        assert_eq!(exact.summary.id, release);
        assert_eq!(exact.summary.title, "Generated album");
        assert_eq!(exact.media[0].tracks.len(), 1);
        assert!(
            configuration
                .metadata(&database, Default::default(), release.into(), None, None)
                .is_err()
        );
        assert!(shared.invalidate(&key)?);
        assert!(
            configuration
                .metadata(
                    &database,
                    Default::default(),
                    release.into(),
                    None,
                    Some(shared.clone())
                )
                .is_err()
        );
        assert_eq!(store.catalog_status()?.failures, 0);
        assert_eq!(
            std::fs::read(&credentials)?,
            br#"{"username":"generated-fixture","password":"generated-only"}"#
        );
        std::fs::write(&credentials, b"invalid generated credentials")?;
        assert!(matches!(
            configuration.metadata(
                &database, Default::default(), release.into(), None, Some(shared.clone())
            ),
            Err(Error::Conflict(message)) if message.contains("selected catalog cache")
        ));
        shared.put(&key, &body, now, now + 86400)?;
        shared
            .entries
            .lock()
            .map_err(|_| "test cache lock")?
            .get_mut(key.digest())
            .ok_or("missing generated cache entry")?
            .bytes[0] ^= 1;
        assert!(matches!(
            configuration.metadata(
                &database, Default::default(), release.into(), None, Some(shared.clone())
            ),
            Err(Error::Conflict(message)) if message.contains("checksum")
        ));
        let mut disabled = ServiceConfiguration::default();
        disabled.acquisition.clear();
        assert!(
            configuration
                .metadata(&database, disabled, release.into(), None, Some(shared))
                .is_err()
        );
        Ok(())
    }
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
    _account: super::local_file::ProcessLock,
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
        Self::prepare(config, release, credentials)
    }

    /// Select only an already-bound host cache; loading never contacts a peer.
    pub fn load_with_host(path: &Path, host: &Path) -> Result<Self> {
        let (config, release, credentials) =
            SearchConfiguration::read_selected(path, true, Some(host))?;
        Self::prepare(config, release, credentials)
    }

    fn prepare(
        config: SearchConfiguration,
        release: crate::domain::CatalogRelease,
        credentials: Credentials,
    ) -> Result<Self> {
        let account = account_lock(&config.database, &credentials)?;
        let edition = release.selection(None)?;
        let selection = config.folder.ok_or_else(|| {
            Error::InvalidInput("source-folder requires one selected peer folder".into())
        })?;
        Ok(Self {
            _account: account,
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
        let profile = self
            .selection
            .audio_profile
            .unwrap_or_else(|| crate::domain::AudioProfile::legacy(self.selection.format));
        if profile.format() != self.selection.format {
            return Err(Error::InvalidInput(
                "folder audio profile differs from its format".into(),
            ));
        }
        let assessment =
            crate::domain::sources::assess_source_profile(&self.edition, profile, &listing)?;
        Ok(SourceFolderReport {
            listing,
            assessment,
        })
    }
}

/// Explicit metadata-only network operation, separate from worker claims,
/// source selection and audio download. The release must be in the real cache.
pub struct PreparedSearch {
    _account: super::local_file::ProcessLock,
    credentials: Credentials,
    server: SocketAddrV4,
    query: String,
    policy: crate::domain::AudioPolicy,
}

impl PreparedSearch {
    pub fn load(path: &Path) -> Result<Self> {
        let (config, release, credentials) = SearchConfiguration::read(path, false)?;
        Self::prepare(config, release, credentials)
    }

    /// Select only an already-bound host cache; loading never contacts a peer.
    pub fn load_with_host(path: &Path, host: &Path) -> Result<Self> {
        let (config, release, credentials) =
            SearchConfiguration::read_selected(path, false, Some(host))?;
        Self::prepare(config, release, credentials)
    }

    fn prepare(
        config: SearchConfiguration,
        release: crate::domain::CatalogRelease,
        credentials: Credentials,
    ) -> Result<Self> {
        let account = account_lock(&config.database, &credentials)?;
        let query = format!("{} {}", release.summary.artist, release.summary.title);
        if query.len() > 256 || query.chars().any(char::is_control) {
            return Err(Error::InvalidInput(
                "release search text exceeds protocol bounds".into(),
            ));
        }
        Ok(Self {
            _account: account,
            credentials,
            server: config.server,
            query,
            policy: source_policy(&config.database)?,
        })
    }

    pub fn run(self) -> Result<SourceSearchReport> {
        let token = source_token()?;
        let session = metadata_session(self.credentials, self.server)?;
        SourceSearchReport::assess(session.search(&self.query, token)?, &self.policy)
    }
}

/// One explicit run of an approved acquisition. Loading validates local state,
/// takes a worker claim and optionally journals a user-selected complete source.
/// Network access starts solely in run(). It never searches for peers, retries
/// uncertain submissions or publishes destinations.
pub struct PreparedWorker {
    _account: Option<super::local_file::ProcessLock>,
    shared_account: Option<std::sync::Arc<super::soulseek::sharing::Account>>,
    services: ServiceConfiguration,
    store: Store,
    claim: JobClaim,
    staging: StagingRoot,
    credentials: Credentials,
    credentials_file: PathBuf,
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
    pub cleanup: Option<super::store::RecoveryOperation>,
    pub cleanup_error: Option<String>,
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
        Self::from_validated_account(validated, None)
    }

    fn from_validated_account(
        validated: ValidatedWorker,
        shared_account: Option<std::sync::Arc<super::soulseek::sharing::Account>>,
    ) -> Result<Self> {
        let ValidatedWorker {
            configuration,
            mut store,
            state,
            source,
            credentials,
        } = validated;
        let account = if shared_account.is_some() {
            None
        } else {
            Some(account_lock(&configuration.database, &credentials)?)
        };
        if let Some(decision) =
            store.check_artwork(configuration.job_id, configuration.plan_version)?
        {
            return Err(Error::ArtworkRequired(decision.question_id));
        }
        let claimed = store
            .claim_acquisition_recovery(
                configuration.job_id,
                configuration.plan_version,
                &format!("worker-once-{}", std::process::id()),
                300,
            )?
            .ok_or_else(|| Error::Conflict("job is already owned by a live worker".into()))?;
        let normalize_only = matches!(state.as_str(), "validating" | "normalizing");
        let prepared = (|| {
            if source.is_some() || normalize_only {
                store.job_work_boundary(&claimed.claim)?;
            }
            if source.is_none() && !normalize_only {
                store.reconciliation_files(&claimed.claim)?;
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
                store.reconciliation_files(&claimed.claim)?
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
            Ok::<_, Error>((staging, peer))
        })();
        let (staging, peer) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                store.release_job_claim(&claimed.claim)?;
                return Err(error);
            }
        };
        Ok(Self {
            _account: account,
            shared_account,
            services: configuration.services,
            store,
            claim: claimed.claim,
            staging,
            credentials,
            credentials_file: configuration.credentials_file,
            server: configuration.server,
            peer,
            export_for_playback: configuration.export_for_playback,
            normalize_only,
        })
    }

    pub fn run(self) -> Result<WorkerOutcome> {
        self.execute(false)
    }

    fn execute(self, publish: bool) -> Result<WorkerOutcome> {
        let shared = self.shared_account.clone();
        let peer = self.peer.clone();
        self.execute_with_login(publish, &mut |server, username, password| {
            if let Some(account) = &shared {
                return account.session(&peer, server, username);
            }
            let socket = TcpStream::connect_timeout(&server.into(), Duration::from_secs(10))
                .map_err(Error::SoulseekTransport)?;
            SoulseekSession::login(socket, username, password)
        })
    }

    #[cfg(test)]
    pub(crate) fn run_with_test_login(
        self,
        login: &mut impl FnMut(SocketAddrV4, &str, &str) -> Result<SoulseekSession>,
    ) -> Result<WorkerOutcome> {
        self.execute_with_login(false, login)
    }

    fn execute_with_login(
        mut self,
        publish: bool,
        login: &mut impl FnMut(SocketAddrV4, &str, &str) -> Result<SoulseekSession>,
    ) -> Result<WorkerOutcome> {
        let result = self.execute_inner(publish, login);
        self.store.release_job_claim(&self.claim)?;
        result
    }

    fn execute_inner(
        &mut self,
        publish: bool,
        login: &mut impl FnMut(SocketAddrV4, &str, &str) -> Result<SoulseekSession>,
    ) -> Result<WorkerOutcome> {
        self.store.renew_claim(&self.claim, 300)?;
        if !self.normalize_only {
            crate::application::reconcile_local_inputs(
                &mut self.store,
                &self.claim,
                &self.staging,
            )?;
        }
        self.store.job_work_boundary(&self.claim)?;
        if !self.normalize_only
            && self
                .store
                .reconciliation_files(&self.claim)?
                .iter()
                .all(|file| file.state == "received")
        {
            self.store
                .advance(&self.claim, JobState::Downloading, JobState::Validating)?;
            self.normalize_only = true;
        }
        let run = if self.normalize_only {
            self.credentials.password.clear();
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
            self.store.reconciliation_files(&self.claim)?;
            self.store.job_work_boundary(&self.claim)?;
            let account = self.credentials.username.clone();
            self.credentials.password.clear();
            let mut budget = super::soulseek::ReconnectBudget::default();
            loop {
                self.store.renew_claim(&self.claim, 300)?;
                crate::application::reconcile_local_inputs(
                    &mut self.store,
                    &self.claim,
                    &self.staging,
                )?;
                self.store.job_work_boundary(&self.claim)?;
                self.store.submission_approval(&self.claim)?;
                let result = (|| {
                    protect_files(&self.store, &[&self.credentials_file])?;
                    let mut credentials = read_credentials(&self.credentials_file)?;
                    if credentials.username != account {
                        return Err(Error::Conflict(
                            "Soulseek account changed during worker reconnect".into(),
                        ));
                    }
                    let connected =
                        login(self.server, &credentials.username, &credentials.password);
                    credentials.password.clear();
                    drop(credentials);
                    let session = connected?;
                    let mut provider = super::services::SoulseekAcquisition {
                        session,
                        peer: self.peer.clone(),
                    };
                    crate::application::services::acquisition::acquire(
                        &self.services,
                        &mut provider,
                        &mut self.store,
                        &self.claim,
                        &self.staging,
                    )
                })();
                match result {
                    Ok(run) => break run,
                    Err(error) => match budget.next(&error) {
                        Some(delay) => std::thread::sleep(delay),
                        None if budget.exhausted()
                            && matches!(error, Error::SoulseekTransport(_)) =>
                        {
                            return Err(Error::Remote(
                                "Soulseek reconnect budget exhausted; reconcile retained transfers before further work",
                            ));
                        }
                        None => return Err(error),
                    },
                }
            }
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
            cleanup: self.store.recovery_operation(run * 8 + 1)?,
            cleanup_error: self
                .store
                .recovery_inventory(
                    crate::domain::Page::new(run * 8, 1)?,
                    Some(self.claim.job()),
                )?
                .into_iter()
                .find(|e| e.id == run * 8 + 1)
                .and_then(|e| e.last_error),
        })
    }
}

/// Operator-selected local acquisition settings. Runtime operation requests
/// cannot choose native destinations or credentials. Backend configuration
/// replacements are validated separately and require a restart.
#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalAcquisition {
    pub staging_root: PathBuf,
    pub credentials_file: PathBuf,
    pub server: SocketAddrV4,
    pub acknowledge_plaintext_login: bool,
}

#[derive(Default)]
pub(crate) struct AcquisitionExecution {
    pub(crate) stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) shared_account: Option<std::sync::Arc<super::soulseek::sharing::Account>>,
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
        super::permissions::FilePermissions::current()?
            .check(&root.into_directory().into_std_file(), true)?;
        read_credentials(&self.credentials_file)?;
        Ok(())
    }

    fn metadata(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        release_id: String,
        folder: Option<FolderSelection>,
        cache: Option<std::sync::Arc<dyn crate::application::services::cache::Cache>>,
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
            cache,
        )
    }

    pub fn search(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        release_id: String,
    ) -> Result<SourceSearchReport> {
        self.search_with_cache(database, services, release_id, None)
    }

    pub(crate) fn search_with_cache(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        release_id: String,
        cache: Option<std::sync::Arc<dyn crate::application::services::cache::Cache>>,
    ) -> Result<SourceSearchReport> {
        let (config, release, credentials) =
            self.metadata(database, services, release_id, None, cache)?;
        let query = format!("{} {}", release.summary.artist, release.summary.title);
        if query.len() > 256 || query.chars().any(char::is_control) {
            return Err(Error::InvalidInput(
                "release search text exceeds protocol bounds".into(),
            ));
        }
        PreparedSearch {
            _account: account_lock(database, &credentials)?,
            credentials,
            server: config.server,
            query,
            policy: source_policy(database)?,
        }
        .run()
    }

    pub fn folder(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        request: SourceFolderRequest,
    ) -> Result<SourceFolderReport> {
        self.folder_with_cache(database, services, request, None)
    }

    pub(crate) fn folder_with_cache(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        request: SourceFolderRequest,
        cache: Option<std::sync::Arc<dyn crate::application::services::cache::Cache>>,
    ) -> Result<SourceFolderReport> {
        let (config, release, credentials) = self.metadata(
            database,
            services,
            request.release_id,
            Some(FolderSelection {
                peer: request.peer,
                remote_folder: request.remote_folder,
                format: request.format,
                audio_profile: request.audio_profile,
            }),
            cache,
        )?;
        PreparedFolder {
            _account: account_lock(database, &credentials)?,
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
        self.acquire_controlled(
            database,
            services,
            job_id,
            plan_version,
            request,
            AcquisitionExecution::default(),
        )
    }

    pub(crate) fn acquire_controlled(
        &self,
        database: &Path,
        services: ServiceConfiguration,
        job_id: i64,
        plan_version: i64,
        request: StartLocalDownload,
        execution: AcquisitionExecution,
    ) -> Result<WorkerOutcome> {
        if !request.acknowledge_new_only_publication {
            return Err(Error::InvalidInput(
                "new-only publication needs explicit acknowledgement".into(),
            ));
        }
        let mut validated = ValidatedWorker::validate(
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
        if let Some(stop) = execution.stop {
            validated.store = validated.store.with_worker_stop(stop);
        }
        PreparedWorker::from_validated_account(validated, execution.shared_account)?.execute(true)
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
    #[serde(default)]
    pub audio_profile: Option<crate::domain::AudioProfile>,
}

fn source_policy(database: &Path) -> Result<crate::domain::AudioPolicy> {
    Ok(Store::open_read_current(database)?
        .settings()?
        .ok_or_else(|| Error::Conflict("save output settings before source discovery".into()))?
        .policy)
}

#[derive(Serialize, ts_rs::TS)]
pub struct SourceAdvertisedProfile {
    pub profile: crate::domain::AudioProfile,
    pub preferred: bool,
    pub quality: crate::domain::AdvertisedQuality,
}

#[derive(Serialize, ts_rs::TS)]
pub struct SourceSearchFile {
    pub file: crate::domain::sources::SourceFileMetadata,
    pub matches: Vec<SourceAdvertisedProfile>,
}

#[derive(Serialize, ts_rs::TS)]
pub struct SourceSearchGroup {
    pub peer: String,
    pub folder: String,
    pub files: Vec<SourceSearchFile>,
}

#[derive(Serialize, ts_rs::TS)]
pub struct SourceSearchReport {
    #[serde(flatten)]
    pub report: super::soulseek::search::SearchReport,
    pub groups: Vec<SourceSearchGroup>,
}

impl SourceSearchReport {
    pub fn assess(
        report: super::soulseek::search::SearchReport,
        policy: &crate::domain::AudioPolicy,
    ) -> Result<Self> {
        use crate::domain::{AdvertisedQuality, sources::advertised_file};
        policy.validate()?;
        let mut groups = std::collections::BTreeMap::new();
        for peer in &report.peers {
            for file in &peer.files {
                let path = crate::domain::sources::remote_identifier(&file.remote_path)?;
                let Some((folder, _)) = path.rsplit_once('\\') else {
                    continue;
                };
                let matches: Vec<_> = policy
                    .accepted
                    .iter()
                    .map(|profile| SourceAdvertisedProfile {
                        profile: *profile,
                        preferred: *profile == policy.preferred,
                        quality: advertised_file(*profile, file),
                    })
                    .collect();
                if matches
                    .iter()
                    .all(|item| item.quality == AdvertisedQuality::Mismatch)
                {
                    continue;
                }
                groups
                    .entry((peer.peer.clone(), folder.to_string()))
                    .or_insert_with(Vec::new)
                    .push(SourceSearchFile {
                        file: file.clone(),
                        matches,
                    });
            }
        }
        Ok(Self {
            report,
            groups: groups
                .into_iter()
                .map(|((peer, folder), files)| SourceSearchGroup {
                    peer,
                    folder,
                    files,
                })
                .collect(),
        })
    }
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct StartLocalDownload {
    pub source_candidate: Option<i64>,
    pub acknowledge_new_only_publication: bool,
}
