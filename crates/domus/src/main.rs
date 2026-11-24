use domus::domain::OutputSettings;
use domus::infrastructure::store::PlanRequest;
use domus::{Error, Result};
use std::path::Path;
use std::process::ExitCode;

mod state_cli;

const USAGE: &str = "Usage:
  domus version
  domus contracts <output-directory>
  domus init --storage-root <absolute-application-storage>
  domus api-preview <absolute-configuration-file>
  domus serve [absolute-configuration-file]
  domus serve --storage-root <absolute-application-storage>
  domus systemd-service <api|worker> <absolute-configuration-file>
  domus windows-service <api|worker> <absolute-configuration-file>
  domus windows-service-policy <api|worker> <enable|disable>
  domus configuration-check <absolute-configuration-file>
  domus media-adapter-check <absolute-adapter-configuration-file>
  domus media-adapter <absolute-adapter-configuration-file>
  domus media-client-check --host <absolute-host-configuration-file> <absolute-media-client-file>
  domus media-root-check --host <absolute-host-configuration-file> <absolute-media-client-file> <root-id>
  domus media-storage-digest --host <absolute-host-configuration-file> <absolute-media-client-file>
  domus certificate-status <absolute-configuration-file>
  domus certificate-bootstrap <absolute-configuration-file>
  domus certificate-recover <absolute-configuration-file> <resume|pending|bindings|cleanup|cleanup-owned|archive|generation|restart>
  domus certificate-recover <absolute-configuration-file> <order|write|receipt> <verified-evidence>
  domus worker <absolute-configuration-file>
  domus worker-once --host <absolute-host-configuration-file> <absolute-configuration-file>
  domus worker-check --host <absolute-host-configuration-file> <absolute-configuration-file>
  domus publish-new-once --host <absolute-host-configuration-file> <absolute-configuration-file>
  domus publication-status --host <absolute-host-configuration-file> <job-id> <plan-version>
  domus source-search --host <absolute-host-configuration-file> <absolute-configuration-file>
  domus source-folder --host <absolute-host-configuration-file> <absolute-configuration-file>
  domus source-candidate-prepare --host <absolute-host-configuration-file> <job-id> <plan-version> <absolute-listing-json>
  domus source-candidate --host <absolute-host-configuration-file> <candidate-id>
  domus source-candidate-approve --host <absolute-host-configuration-file> <candidate-id> <plan-version> <review-sha256> --reviewed-exact-edition-and-permitted-download
  domus service-call <absolute-host-configuration-file> <absolute-request-file>
  domus mcp-stdio <absolute-host-configuration-file>
  domus catalog-release --host <absolute-host-configuration-file> <release-id>
  domus catalog-plan-prepare --host <absolute-host-configuration-file> <release-id> <absolute-request-json>
  domus settings --host <absolute-host-configuration-file>
  domus settings-default
  domus staging-init --host <absolute-host-configuration-file> <absolute-root>
  domus settings-save --host <absolute-host-configuration-file> <expected-version-or-0> <settings-json>
  domus job-create --host <absolute-host-configuration-file>
  domus job-history --host <absolute-host-configuration-file> <job-id> <plan-version> <after-id> <limit>
  domus job-control --host <absolute-host-configuration-file> <job-id> <plan-version> <pause|cancel>
  domus job-control-status --host <absolute-host-configuration-file> <control-id>
  domus job-control-continue --host <absolute-host-configuration-file> <control-id> <plan-version>
  domus plan-prepare --host <absolute-host-configuration-file> <request-json>
  domus plan --host <absolute-host-configuration-file> <job-id> <plan-version>
  domus plan-approve --host <absolute-host-configuration-file> <job-id> <plan-version>
  domus artwork-local-prepare --host <absolute-host-configuration-file> <job-id> <plan-version> <absolute-image-file> <background-json> --permitted-local-cover
  domus artwork-preview --host <absolute-host-configuration-file> <job-id> <plan-version> <candidate-id>
  domus artwork-caa-prepare --host <absolute-host-configuration-file> <job-id> <plan-version> <image-id> <background-json> --permitted-caa-cover
  domus artwork-evidence --host <absolute-host-configuration-file> <job-id> <plan-version> <candidate-id>
  domus artwork-check --host <absolute-host-configuration-file> <job-id> <plan-version>
  domus artwork-question --host <absolute-host-configuration-file> <question-id>
  domus artwork-resume --host <absolute-host-configuration-file> <question-id> <plan-version>
  domus question-answer --host <absolute-host-configuration-file> <question-id> <plan-version> <offered-choice>
  domus artwork-approve --host <absolute-host-configuration-file> <job-id> <plan-version> <candidate-id> <preview-sha256>
  domus location-add --host <absolute-host-configuration-file> <absolute-root> <main|backup>
  domus location-add-paired --host <absolute-host-configuration-file> <selected-root-id> <main|backup>
  domus location-check --host <absolute-host-configuration-file> <location-id>
  domus location --host <absolute-host-configuration-file> <location-id>
  domus root-identity <absolute-root>
  domus storage-root-inspect <absolute-provider-boundary> <provider-scope> <root-id> <binding-revision> <application|state|cache|staging|certificate|media> <library-relative-path>
  domus storage-selection-digest <absolute-selection-json-file>
  domus location-bind --host <absolute-host-configuration-file> <location-id> <expected-native-identity>
  domus staging-bind --host <absolute-host-configuration-file> <absolute-root> <expected-native-identity>
  domus scan --host <absolute-host-configuration-file> <location-id>
  domus browse --host <absolute-host-configuration-file> <location-id> <relative-folder> <after-id> <limit>

serve without configuration uses single-user anonymous HTTP on loopback.
serve with configuration and api-preview never downgrade failed secure configuration.
Production requires Entra. Explicit development mode uses _anonymous without sign-in.
The worker and worker-once download only an approved album from an explicitly selected source.
It requires separate Soulseek credentials and explicit plaintext-login acknowledgement.
source-search uses the same login acknowledgement for metadata-only outgoing peer search.
source-folder reads one selected peer folder and assesses names; it never approves or downloads.
catalog-release loads the exact MusicBrainz release through the durable provider budget.
--host requires the main host configuration and its external storage selection.
State commands never accept a bare database path or migrate existing state.
Provider commands use an already-bound selected cache without fallback.
init creates fresh private storage without loading services or opening a listener.
storage-selection-digest computes a candidate digest without approving or activating storage.
service-call runs catalog discovery, metadata, artwork listing or image caching through configured services.
It cannot approve plans, download audio or publish media.
publish-new-once explicitly copies verified staging into new primary/backup album folders.
It never replaces existing albums. Partial folders remain visible after failure.
media-adapter explicitly serves read-only selected media to its paired native client.
It does not enable public media sharing or connect production library operations.
The database must be outside every configured media root.";

fn run(args: &[String]) -> Result<()> {
    if state_cli::run(args)? {
        return Ok(());
    }
    match args {
        [command, flag, host, configuration]
            if command == "media-storage-digest" && flag == "--host" =>
        {
            let source = domus::infrastructure::server::configuration::ConfigurationService::open(
                Path::new(host),
            )?;
            println!("{}", source.media_storage_digest(Path::new(configuration))?);
        }
        [command, flag, host, configuration]
            if command == "media-client-check" && flag == "--host" =>
        {
            let source = domus::infrastructure::server::configuration::ConfigurationService::open(
                Path::new(host),
            )?;
            source.prepare_media_client(Path::new(configuration))?;
            println!("Media client configuration is valid; no peer connection opened.");
        }
        [command, flag, host, configuration, root]
            if command == "media-root-check" && flag == "--host" =>
        {
            let source = domus::infrastructure::server::configuration::ConfigurationService::open(
                Path::new(host),
            )?;
            let mut client = source.prepare_media_client(Path::new(configuration))?;
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(async {
                    let directory = client.open_root(root).await?;
                    client.close_directory(directory).await
                })?;
            println!("Selected media root is available; no scan or registration performed.");
        }
        [command, configuration]
            if matches!(command.as_str(), "media-adapter" | "media-adapter-check") =>
        {
            let prepared = domus::infrastructure::server::configuration::ConfigurationService::prepare_media_adapter(
                Path::new(configuration),
            )?;
            if command == "media-adapter-check" {
                println!("Native media adapter configuration is valid; no listener started.");
            } else {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .max_blocking_threads(8)
                    .enable_all()
                    .build()?
                    .block_on(prepared.run())?;
            }
        }
        [command, configuration] if command == "configuration-check" => {
            let source = domus::infrastructure::server::configuration::ConfigurationService::open(
                Path::new(configuration),
            )?;
            let result = source.validate_saved()?;
            println!("{}", serde_json::to_string(&result)?);
            if !result.errors.is_empty() {
                return Err(Error::InvalidInput(
                    "host configuration validation failed".into(),
                ));
            }
        }
        #[cfg(target_os = "linux")]
        [command, role, configuration] if command == "systemd-service" => {
            domus::infrastructure::systemd::run(role, Path::new(configuration))?;
        }
        #[cfg(windows)]
        [command, role, policy] if command == "windows-service-policy" => {
            domus::infrastructure::windows_service::configure_policy(role, policy)?;
            println!("Windows service supervision policy updated.");
        }
        #[cfg(windows)]
        [command, role, configuration] if command == "windows-service" => {
            domus::infrastructure::windows_service::dispatch(role, Path::new(configuration))?;
        }
        [command, configuration, action] if command == "certificate-recover" => {
            println!(
                "{}",
                serde_json::to_string(&domus::infrastructure::certificates::recover(
                    Path::new(configuration),
                    action,
                    None
                )?)?
            );
        }
        [command, configuration, action, evidence] if command == "certificate-recover" => {
            println!(
                "{}",
                serde_json::to_string(&domus::infrastructure::certificates::recover(
                    Path::new(configuration),
                    action,
                    Some(evidence)
                )?)?
            );
        }
        [command, configuration] if command == "certificate-bootstrap" => {
            println!(
                "{}",
                serde_json::to_string(&domus::infrastructure::certificates::bootstrap(
                    Path::new(configuration)
                )?)?
            );
        }
        [command, configuration] if command == "certificate-status" => {
            println!(
                "{}",
                serde_json::to_string(&domus::infrastructure::certificates::status(Path::new(
                    configuration
                ))?)?
            );
        }
        [command] if command == "settings-default" => {
            println!("{}", serde_json::to_string(&OutputSettings::default())?);
        }
        [command] if command == "version" => {
            println!("Domus Obscura {}", env!("CARGO_PKG_VERSION"))
        }
        [command, directory] if matches!(command.as_str(), "contracts" | "contracts-check") => {
            use domus::domain::{
                ArtworkIssue, ArtworkSize, AudioFormat, CatalogArtist, CatalogCredit,
                CatalogMedium, CatalogPage, CatalogRelease, CatalogReleaseFilters,
                CatalogReleaseGroup, CatalogReleasePage, CatalogReleaseSummary, CatalogTrack,
                EditionSelection, JobState, OutputPreview, OutputSettings, OutputTemplate,
                PlannedTrack,
            };
            use domus::infrastructure::api::{
                Answer, ApiMessage, ApproveArtwork, ArtistSearch, BrowseQuery, CaaCoverOption,
                CaaCoverSearch, CatalogBatchPlanRequest, CatalogPageQuery, CatalogPlanRequest,
                CatalogPlanSelection, CatalogResumeRequest, CreateJob, CreatedJob, EventCursor,
                PageQuery, PrepareCaaArtwork, RegisterLocation, ResumeArtwork, SaveSettings,
                ScanLocation, SystemSettingsPolicy,
            };
            use domus::infrastructure::caa::{CaaImage, CaaListing};
            use domus::infrastructure::catalog::CatalogResult;
            use domus::infrastructure::store::{
                ArtworkCandidate, ArtworkDecision, CaaArtworkEvidence, CatalogProviderState,
                CatalogStatus, Entry, JobEvent, JobSummary, Location, PlanDestination,
                PreparedPlan, QuestionDetails, QuestionRecovery, SessionIdentity, SettingsVersion,
                StoredPlan, StoredQuestion, TransferFile,
            };
            use ts_rs::TS;
            // serde_json emits integer JSON numbers, not JavaScript bigint values.
            let config = ts_rs::Config::default().with_large_int("number");
            let contracts = [
                ("AudioProfile.ts", domus::domain::AudioProfile::export_to_string(&config)),
                ("EntityRef.ts", domus::application::services::EntityRef::export_to_string(&config)),
                ("ImageRights.ts", domus::application::services::artist_images::ImageRights::export_to_string(&config)),
                ("ArtistImageSource.ts", domus::application::services::artist_images::ArtistImageSource::export_to_string(&config)),
                ("ArtistPhoto.ts", domus::application::services::artist_images::ArtistPhoto::export_to_string(&config)),
                ("ArtistPhotoBinding.ts", domus::application::services::artist_images::ArtistPhotoBinding::export_to_string(&config)),
                ("CheckArtistPhoto.ts", domus::application::services::artist_images::CheckArtistPhoto::export_to_string(&config)),
                ("SaveArtistPhoto.ts", domus::application::services::artist_images::SaveArtistPhoto::export_to_string(&config)),
                ("RemoveArtistPhoto.ts", domus::infrastructure::api::artist_images::RemoveArtistPhoto::export_to_string(&config)),
                ("ArtistPhotoCheck.ts", domus::infrastructure::api::artist_images::ArtistPhotoCheck::export_to_string(&config)),
                ("ArtistPhotoState.ts", domus::infrastructure::api::artist_images::ArtistPhotoState::export_to_string(&config)),
                ("AudioPolicy.ts", domus::domain::AudioPolicy::export_to_string(&config)),
                ("AudioSelection.ts", domus::domain::AudioSelection::export_to_string(&config)),
                ("AudioProfileDescriptor.ts", domus::domain::AudioProfileDescriptor::export_to_string(&config)),
                ("AdvertisedQuality.ts", domus::domain::AdvertisedQuality::export_to_string(&config)),
                ("SourceProfileAlternative.ts", domus::domain::sources::SourceProfileAlternative::export_to_string(&config)),
                ("SourceSearchReport.ts", domus::infrastructure::worker::SourceSearchReport::export_to_string(&config)),
                ("SourceSearchGroup.ts", domus::infrastructure::worker::SourceSearchGroup::export_to_string(&config)),
                ("SourceSearchFile.ts", domus::infrastructure::worker::SourceSearchFile::export_to_string(&config)),
                ("SourceAdvertisedProfile.ts", domus::infrastructure::worker::SourceAdvertisedProfile::export_to_string(&config)),
                ("LibraryRefresh.ts", domus::infrastructure::api::LibraryRefresh::export_to_string(&config)),
                ("RefreshLibraryFolder.ts", domus::infrastructure::api::RefreshLibraryFolder::export_to_string(&config)),
                ("LibraryTags.ts", domus::domain::library::LibraryTags::export_to_string(&config)),
                ("LibraryAudio.ts", domus::domain::library::LibraryAudio::export_to_string(&config)),
                ("LibraryMetadata.ts", domus::domain::library::LibraryMetadata::export_to_string(&config)),
                ("LibraryArtist.ts", domus::domain::library::LibraryArtist::export_to_string(&config)),
                ("LibraryAlbum.ts", domus::domain::library::LibraryAlbum::export_to_string(&config)),
                ("LibraryFile.ts", domus::domain::library::LibraryFile::export_to_string(&config)),
                ("LibraryCoverage.ts", domus::domain::library::LibraryCoverage::export_to_string(&config)),
                ("LibraryMatchQuery.ts", domus::domain::library::LibraryMatchQuery::export_to_string(&config)),
                ("LibraryMatch.ts", domus::domain::library::LibraryMatch::export_to_string(&config)),
                ("LibraryMatches.ts", domus::domain::library::LibraryMatches::export_to_string(&config)),
                ("WorkerOperation.ts", domus::infrastructure::production_worker::WorkerOperation::export_to_string(&config)),
                ("WorkerRequest.ts", domus::infrastructure::production_worker::WorkerRequest::export_to_string(&config)),
                ("WorkerStatus.ts", domus::infrastructure::production_worker::WorkerStatus::export_to_string(&config)),
                ("BackendWorker.ts", domus::infrastructure::server::configuration::BackendWorker::export_to_string(&config)),
                ("BackendWorkerDraft.ts", domus::infrastructure::server::configuration::BackendWorkerDraft::export_to_string(&config)),
                ("BackendAcquisitionWorkerDraft.ts", domus::infrastructure::server::configuration::BackendAcquisitionWorkerDraft::export_to_string(&config)),
                ("BackendSharing.ts", domus::infrastructure::server::configuration::BackendSharing::export_to_string(&config)),
                ("BackendSharingAccount.ts", domus::infrastructure::server::configuration::BackendSharingAccount::export_to_string(&config)),
                ("BackendSharingDraft.ts", domus::infrastructure::server::configuration::BackendSharingDraft::export_to_string(&config)),
                ("BackendSharingAccountDraft.ts", domus::infrastructure::server::configuration::BackendSharingAccountDraft::export_to_string(&config)),
                ("ShareAccess.ts", domus::domain::sharing::ShareAccess::export_to_string(&config)),
                ("ShareRoot.ts", domus::domain::sharing::ShareRoot::export_to_string(&config)),
                ("SharePolicy.ts", domus::domain::sharing::SharePolicy::export_to_string(&config)),
                ("ShareLimits.ts", domus::domain::sharing::ShareLimits::export_to_string(&config)),
                (
                    "ConfigurationSection.ts",
                    domus::infrastructure::server::configuration::ConfigurationSection::export_to_string(&config),
                ),
                (
                    "ConfigurationSectionUpdate.ts",
                    domus::infrastructure::server::configuration::ConfigurationSectionUpdate::export_to_string(&config),
                ),
                (
                    "ConfigurationSectionCommand.ts",
                    domus::infrastructure::server::configuration::ConfigurationSectionCommand::export_to_string(&config),
                ),
                (
                    "ServiceConfiguration.ts",
                    domus::application::services::ServiceConfiguration::export_to_string(&config),
                ),
                (
                    "ConfigurationReplacement.ts",
                    domus::infrastructure::server::configuration::ConfigurationReplacement::export_to_string(&config),
                ),
                (
                    "BackendLocalAcquisitionDraft.ts",
                    domus::infrastructure::server::configuration::BackendLocalAcquisitionDraft::export_to_string(&config),
                ),
                (
                    "BackendConfigurationDraft.ts",
                    domus::infrastructure::server::configuration::BackendConfigurationDraft::export_to_string(&config),
                ),
                (
                    "BackendConfigurationCommand.ts",
                    domus::infrastructure::server::configuration::BackendConfigurationCommand::export_to_string(&config),
                ),
                (
                    "BackendConfigurationIssue.ts",
                    domus::infrastructure::server::configuration::BackendConfigurationIssue::export_to_string(&config),
                ),
                (
                    "BackendConfigurationResult.ts",
                    domus::infrastructure::server::configuration::BackendConfigurationResult::export_to_string(&config),
                ),
                (
                    "BackendConfigurationState.ts",
                    domus::infrastructure::server::configuration::BackendConfigurationState::export_to_string(&config),
                ),
                (
                    "BackendConfiguration.ts",
                    domus::infrastructure::server::configuration::BackendConfiguration::export_to_string(&config),
                ),
                (
                    "BackendCacheConfiguration.ts",
                    domus::infrastructure::server::configuration::BackendCacheConfiguration::export_to_string(&config),
                ),
                (
                    "TlsMode.ts",
                    domus::domain::certificates::TlsMode::export_to_string(&config),
                ),
                (
                    "AcmeEnvironment.ts",
                    domus::domain::certificates::AcmeEnvironment::export_to_string(&config),
                ),
                (
                    "BackendManagedTls.ts",
                    domus::infrastructure::server::configuration::BackendManagedTls::export_to_string(&config),
                ),
                (
                    "BackendManagedTlsDraft.ts",
                    domus::infrastructure::server::configuration::BackendManagedTlsDraft::export_to_string(&config),
                ),
                (
                    "CertificateStatus.ts",
                    domus::infrastructure::certificates::state::CertificateStatus::export_to_string(&config),
                ),
                (
                    "BackendCertificateStatus.ts",
                    domus::infrastructure::server::configuration::BackendCertificateStatus::export_to_string(&config),
                ),
                (
                    "CertificatePhase.ts",
                    domus::domain::certificates::CertificatePhase::export_to_string(&config),
                ),
                (
                    "BackendLocalAcquisition.ts",
                    domus::infrastructure::server::configuration::BackendLocalAcquisition::export_to_string(&config),
                ),
                (
                    "LocalStatus.ts",
                    domus::infrastructure::local_runtime::LocalStatus::export_to_string(&config),
                ),
                (
                    "SourceSearchRequest.ts",
                    domus::infrastructure::worker::SourceSearchRequest::export_to_string(&config),
                ),
                (
                    "SourceFolderRequest.ts",
                    domus::infrastructure::worker::SourceFolderRequest::export_to_string(&config),
                ),
                (
                    "StartLocalDownload.ts",
                    domus::infrastructure::worker::StartLocalDownload::export_to_string(&config),
                ),
                (
                    "ResumePublication.ts",
                    domus::infrastructure::worker::ResumePublication::export_to_string(&config),
                ),
                (
                    "SearchReport.ts",
                    domus::infrastructure::soulseek::search::SearchReport::export_to_string(
                        &config,
                    ),
                ),
                (
                    "SearchPeer.ts",
                    domus::infrastructure::soulseek::search::SearchPeer::export_to_string(&config),
                ),
                (
                    "SearchIssue.ts",
                    domus::infrastructure::soulseek::search::SearchIssue::export_to_string(&config),
                ),
                (
                    "SourceFileMetadata.ts",
                    domus::domain::sources::SourceFileMetadata::export_to_string(&config),
                ),
                (
                    "SourceFolder.ts",
                    domus::domain::sources::SourceFolder::export_to_string(&config),
                ),
                (
                    "SourceMappingIssue.ts",
                    domus::domain::sources::SourceMappingIssue::export_to_string(&config),
                ),
                (
                    "SourceTrackMatch.ts",
                    domus::domain::sources::SourceTrackMatch::export_to_string(&config),
                ),
                (
                    "SourceMappingProblem.ts",
                    domus::domain::sources::SourceMappingProblem::export_to_string(&config),
                ),
                (
                    "SourceAssessment.ts",
                    domus::domain::sources::SourceAssessment::export_to_string(&config),
                ),
                (
                    "SourceFolderReport.ts",
                    domus::infrastructure::worker::SourceFolderReport::export_to_string(&config),
                ),
                (
                    "SourceCandidate.ts",
                    domus::infrastructure::store::SourceCandidate::export_to_string(&config),
                ),
                (
                    "ApproveSourceCandidate.ts",
                    domus::infrastructure::api::ApproveSourceCandidate::export_to_string(&config),
                ),
                (
                    "PublicationDestination.ts",
                    domus::infrastructure::store::PublicationDestination::export_to_string(&config),
                ),
                (
                    "PublicationDestinationReplacement.ts",
                    domus::infrastructure::store::PublicationDestinationReplacement::export_to_string(&config),
                ),
                (
                    "ReplacementAction.ts",
                    domus::domain::ReplacementAction::export_to_string(&config),
                ),
                (
                    "ReplacementFile.ts",
                    domus::infrastructure::store::ReplacementFile::export_to_string(&config),
                ),
                (
                    "AlbumReplacementPreview.ts",
                    domus::infrastructure::store::AlbumReplacementPreview::export_to_string(&config),
                ),
                (
                    "AlbumReplacementDecision.ts",
                    domus::infrastructure::store::AlbumReplacementDecision::export_to_string(&config),
                ),
                (
                    "AlbumReplacementReview.ts",
                    domus::infrastructure::store::AlbumReplacementReview::export_to_string(&config),
                ),
                (
                    "AlbumReplacementRestoreDecision.ts",
                    domus::infrastructure::store::AlbumReplacementRestoreDecision::export_to_string(&config),
                ),
                (
                    "AlbumRestorePreview.ts",
                    domus::infrastructure::store::AlbumRestorePreview::export_to_string(&config),
                ),
                (
                    "AlbumRestoreStatus.ts",
                    domus::infrastructure::store::AlbumRestoreStatus::export_to_string(&config),
                ),
                ("RecoveryEntry.ts",domus::infrastructure::store::RecoveryEntry::export_to_string(&config)),
                ("RecoveryFile.ts",domus::infrastructure::store::RecoveryFile::export_to_string(&config)),
                ("RecoveryPreview.ts",domus::infrastructure::store::RecoveryPreview::export_to_string(&config)),
                ("RecoveryOperation.ts",domus::infrastructure::store::RecoveryOperation::export_to_string(&config)),
                ("RecoveryDecision.ts",domus::infrastructure::store::RecoveryDecision::export_to_string(&config)),
                ("RecoveryContinue.ts",domus::infrastructure::store::RecoveryContinue::export_to_string(&config)),
                ("ArtworkIssue.ts", ArtworkIssue::export_to_string(&config)),
                (
                    "JobControl.ts",
                    domus::infrastructure::store::JobControl::export_to_string(&config),
                ),
                (
                    "JobControlAction.ts",
                    domus::infrastructure::store::JobControlAction::export_to_string(&config),
                ),
                (
                    "RequestJobControl.ts",
                    domus::infrastructure::api::RequestJobControl::export_to_string(&config),
                ),
                (
                    "ContinueJobControl.ts",
                    domus::infrastructure::api::ContinueJobControl::export_to_string(&config),
                ),
                (
                    "ArtworkDecision.ts",
                    ArtworkDecision::export_to_string(&config),
                ),
                ("ResumeArtwork.ts", ResumeArtwork::export_to_string(&config)),
                ("CaaImage.ts", CaaImage::export_to_string(&config)),
                ("CaaListing.ts", CaaListing::export_to_string(&config)),
                (
                    "CaaArtworkEvidence.ts",
                    CaaArtworkEvidence::export_to_string(&config),
                ),
                (
                    "PrepareCaaArtwork.ts",
                    PrepareCaaArtwork::export_to_string(&config),
                ),
                (
                    "ArtworkCandidate.ts",
                    ArtworkCandidate::export_to_string(&config),
                ),
                (
                    "ApproveArtwork.ts",
                    ApproveArtwork::export_to_string(&config),
                ),
                (
                    "CatalogBatchPlanRequest.ts",
                    CatalogBatchPlanRequest::export_to_string(&config),
                ),
                (
                    "CatalogPlanSelection.ts",
                    CatalogPlanSelection::export_to_string(&config),
                ),
                (
                    "CatalogReleaseFilters.ts",
                    CatalogReleaseFilters::export_to_string(&config),
                ),
                (
                    "CatalogReleaseGroup.ts",
                    CatalogReleaseGroup::export_to_string(&config),
                ),
                (
                    "CatalogReleasePage.ts",
                    CatalogReleasePage::export_to_string(&config),
                ),
                (
                    "CatalogProviderState.ts",
                    CatalogProviderState::export_to_string(&config),
                ),
                ("CatalogStatus.ts", CatalogStatus::export_to_string(&config)),
                (
                    "CatalogResumeRequest.ts",
                    CatalogResumeRequest::export_to_string(&config),
                ),
                ("CatalogArtist.ts", CatalogArtist::export_to_string(&config)),
                ("CatalogCredit.ts", CatalogCredit::export_to_string(&config)),
                ("CatalogMedium.ts", CatalogMedium::export_to_string(&config)),
                (
                    "CatalogPage.ts",
                    CatalogPage::<CatalogArtist>::export_to_string(&config),
                ),
                (
                    "CatalogRelease.ts",
                    CatalogRelease::export_to_string(&config),
                ),
                (
                    "CatalogReleaseSummary.ts",
                    CatalogReleaseSummary::export_to_string(&config),
                ),
                ("CatalogTrack.ts", CatalogTrack::export_to_string(&config)),
                (
                    "CatalogResult.ts",
                    CatalogResult::<CatalogArtist>::export_to_string(&config),
                ),
                ("ArtistSearch.ts", ArtistSearch::export_to_string(&config)),
                (
                    "CatalogPageQuery.ts",
                    CatalogPageQuery::export_to_string(&config),
                ),
                (
                    "CatalogPlanRequest.ts",
                    CatalogPlanRequest::export_to_string(&config),
                ),
                ("AudioFormat.ts", AudioFormat::export_to_string(&config)),
                ("ArtworkSize.ts", ArtworkSize::export_to_string(&config)),
                ("JobState.ts", JobState::export_to_string(&config)),
                ("OutputPreview.ts", OutputPreview::export_to_string(&config)),
                (
                    "OutputTemplate.ts",
                    OutputTemplate::export_to_string(&config),
                ),
                (
                    "OutputSettings.ts",
                    OutputSettings::export_to_string(&config),
                ),
                ("PlannedTrack.ts", PlannedTrack::export_to_string(&config)),
                (
                    "EditionSelection.ts",
                    EditionSelection::export_to_string(&config),
                ),
                (
                    "SettingsVersion.ts",
                    SettingsVersion::export_to_string(&config),
                ),
                (
                    "PlanDestination.ts",
                    PlanDestination::export_to_string(&config),
                ),
                ("PreparedPlan.ts", PreparedPlan::export_to_string(&config)),
                ("StoredPlan.ts", StoredPlan::export_to_string(&config)),
                ("PlanRequest.ts", PlanRequest::export_to_string(&config)),
                ("Location.ts", Location::export_to_string(&config)),
                ("Entry.ts", Entry::export_to_string(&config)),
                (
                    "StoredQuestion.ts",
                    StoredQuestion::export_to_string(&config),
                ),
                (
                    "QuestionDetails.ts",
                    QuestionDetails::export_to_string(&config),
                ),
                (
                    "QuestionRecovery.ts",
                    QuestionRecovery::export_to_string(&config),
                ),
                ("JobSummary.ts", JobSummary::export_to_string(&config)),
                (
                    "QueueSelection.ts",
                    domus::infrastructure::store::QueueSelection::export_to_string(&config),
                ),
                ("JobEvent.ts", JobEvent::export_to_string(&config)),
                (
                    "SessionIdentity.ts",
                    SessionIdentity::export_to_string(&config),
                ),
                ("ApiMessage.ts", ApiMessage::export_to_string(&config)),
                (
                    "CaaCoverOption.ts",
                    CaaCoverOption::export_to_string(&config),
                ),
                (
                    "CaaCoverSearch.ts",
                    CaaCoverSearch::export_to_string(&config),
                ),
                ("PageQuery.ts", PageQuery::export_to_string(&config)),
                ("BrowseQuery.ts", BrowseQuery::export_to_string(&config)),
                ("SaveSettings.ts", SaveSettings::export_to_string(&config)),
                (
                    "RegisterLocation.ts",
                    RegisterLocation::export_to_string(&config),
                ),
                ("ScanLocation.ts", ScanLocation::export_to_string(&config)),
                ("ShareFolderInspection.ts", domus::application::sharing::ShareFolderInspection::export_to_string(&config)),
                ("InspectShareFolder.ts", domus::application::sharing::InspectShareFolder::export_to_string(&config)),
                ("BindShareFolder.ts", domus::application::sharing::BindShareFolder::export_to_string(&config)),
                ("CreateJob.ts", CreateJob::export_to_string(&config)),
                ("CreatedJob.ts", CreatedJob::export_to_string(&config)),
                ("Answer.ts", Answer::export_to_string(&config)),
                ("TransferFile.ts", TransferFile::export_to_string(&config)),
                (
                    "TransferSummary.ts",
                    domus::infrastructure::store::TransferSummary::export_to_string(&config),
                ),
                ("EventCursor.ts", EventCursor::export_to_string(&config)),
                (
                    "SystemSettingsPolicy.ts",
                    SystemSettingsPolicy::export_to_string(&config),
                ),
            ];
            if command == "contracts" {
                std::fs::create_dir_all(directory)?;
            }
            for (name, content) in contracts {
                let content = content.map_err(|error| {
                    Error::InvalidInput(format!("contract export failed: {error}"))
                })?;
                let path = Path::new(directory).join(name);
                if command == "contracts-check" {
                    if std::fs::read_to_string(&path)? != content {
                        return Err(Error::Conflict(format!(
                            "generated contract is stale: {}",
                            path.display()
                        )));
                    }
                } else {
                    std::fs::write(path, content)?;
                }
            }
        }
        [command, flag, root] if command == "init" && flag == "--storage-root" => {
            let configuration = domus::infrastructure::server::PreparedServer::initialize_local_at(
                Path::new(root),
            )?;
            println!(
                "Fresh application storage initialized: {}",
                configuration.display()
            );
        }
        [command] if command == "serve" => {
            let prepared = domus::infrastructure::server::PreparedServer::local()?;
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(4)
                .enable_all()
                .build()?
                .block_on(prepared.run())?;
        }
        [command, option, root] if command == "serve" && option == "--storage-root" => {
            let prepared =
                domus::infrastructure::server::PreparedServer::local_at(Path::new(root))?;
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(4)
                .enable_all()
                .build()?
                .block_on(prepared.run())?;
        }
        [command, configuration] if matches!(command.as_str(), "api-preview" | "serve") => {
            let prepared =
                domus::infrastructure::server::PreparedServer::load(Path::new(configuration))?;
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(4)
                .enable_all()
                .build()?
                .block_on(prepared.run())?;
        }
        [command, flag, host, configuration] if command == "worker-once" && flag == "--host" => {
            let worker = domus::infrastructure::worker::PreparedWorker::load(
                Path::new(configuration),
                Path::new(host),
            )?;
            let outcome = worker.run()?;
            println!("{}", serde_json::to_string(&outcome)?);
        }
        [command, configuration] if command == "worker" => {
            let worker = domus::infrastructure::production_worker::PreparedProductionWorker::load(
                Path::new(configuration),
            )?;
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(worker.run())?;
        }
        [command, flag, host, configuration] if command == "worker-check" && flag == "--host" => {
            let report = domus::infrastructure::worker::PreparedWorker::check(
                Path::new(configuration),
                Path::new(host),
            )?;
            println!("{}", serde_json::to_string(&report)?);
        }
        [command, flag, host, configuration]
            if command == "publish-new-once" && flag == "--host" =>
        {
            let outcome = domus::infrastructure::worker::publish_new_once(
                Path::new(configuration),
                Path::new(host),
            )?;
            println!("{}", serde_json::to_string(&outcome)?);
        }
        [command, flag, host, configuration] if command == "source-search" && flag == "--host" => {
            let search = domus::infrastructure::worker::PreparedSearch::load_with_host(
                Path::new(configuration),
                Path::new(host),
            )?;
            println!("{}", serde_json::to_string(&search.run()?)?);
        }
        [command, flag, host, configuration] if command == "source-folder" && flag == "--host" => {
            let folder = domus::infrastructure::worker::PreparedFolder::load_with_host(
                Path::new(configuration),
                Path::new(host),
            )?;
            println!("{}", serde_json::to_string(&folder.run()?)?);
        }
        [command, flag, configuration, release]
            if command == "catalog-release" && flag == "--host" =>
        {
            use domus::infrastructure::catalog::CatalogQuery;
            use domus::infrastructure::services::{ServiceHost, load_cli_store};
            let query = CatalogQuery::release(release)?;
            let (store, services) = load_cli_store(Path::new(configuration), &[])?;
            let host = ServiceHost::new(store, services)?;
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(host.musicbrainz.query(query, CatalogQuery::parse_release))?;
            println!("{}", serde_json::to_string(&result)?);
        }
        [command, configuration, request] if command == "service-call" => {
            let (host, request) = domus::infrastructure::services::load_call(
                Path::new(configuration),
                Path::new(request),
            )?;
            let response = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(host.call(request))?;
            println!("{}", serde_json::to_string(&response)?);
        }
        [command, configuration] if command == "mcp-stdio" => {
            domus::infrastructure::mcp::run_stdio(Path::new(configuration))?;
        }
        [
            command,
            flag,
            configuration,
            job,
            version,
            image,
            background,
            acknowledgement,
        ] if command == "artwork-caa-prepare"
            && flag == "--host"
            && acknowledgement == "--permitted-caa-cover" =>
        {
            let background: Option<[u8; 3]> = serde_json::from_str(background).map_err(|_| {
                Error::InvalidInput("background must be null or an RGB byte array".into())
            })?;
            let job = number(job)?;
            let version = number(version)?;
            let (mut store, services) =
                domus::infrastructure::services::load_cli_store(Path::new(configuration), &[])?;
            services.require(
                domus::application::services::Capability::Artwork,
                "cover-art-archive",
            )?;
            let candidate =
                store.prepare_cached_caa_artwork_or_ask(job, version, image, background, true)?;
            println!("{}", serde_json::to_string(&candidate)?);
        }
        [command, flag, configuration, release, file]
            if command == "catalog-plan-prepare" && flag == "--host" =>
        {
            let plan = domus::infrastructure::services::prepare_cli_catalog_plan(
                Path::new(configuration),
                release,
                Path::new(file),
            )?;
            println!("{}", serde_json::to_string(&plan)?);
        }
        [command, boundary, scope, root_id, revision, role, relative]
            if command == "storage-root-inspect" =>
        {
            use domus::infrastructure::filesystem::{StorageRole, StorageRoot};
            let role: StorageRole = serde_json::from_value(serde_json::Value::String(role.clone()))
                .map_err(|_| Error::InvalidInput("invalid storage role".into()))?;
            let root = StorageRoot::inspect(
                root_id,
                number(revision)?,
                role,
                scope,
                Path::new(boundary),
                &domus::domain::LibraryPath::parse(relative)?,
            )?;
            println!("{}", serde_json::to_string_pretty(&root)?);
        }
        [command, path] if command == "storage-selection-digest" => {
            let bytes = domus::infrastructure::filesystem::read_local_file(
                Path::new(path),
                16 * 1024,
                true,
            )?;
            let selection: domus::infrastructure::filesystem::StorageConfiguration =
                serde_json::from_slice(&bytes)
                    .map_err(|_| Error::InvalidInput("invalid storage selection JSON".into()))?;
            println!("{}", selection.selection_digest()?);
        }
        [command, root] if command == "root-identity" => {
            println!(
                "{}",
                domus::infrastructure::filesystem::native_root_identity(Path::new(root))?
            );
        }
        _ => return Err(Error::InvalidInput(USAGE.into())),
    }
    Ok(())
}

fn json_file<T: serde::de::DeserializeOwned>(
    source: &domus::infrastructure::server::configuration::ConfigurationService,
    path: &str,
    maximum: usize,
) -> Result<T> {
    let bytes = source.read_input(Path::new(path), maximum as u64, false)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::InvalidInput("invalid JSON input".into()))
}

fn number<T: std::str::FromStr>(value: &str) -> Result<T> {
    value
        .parse()
        .map_err(|_| Error::InvalidInput("invalid numeric argument".into()))
}

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
