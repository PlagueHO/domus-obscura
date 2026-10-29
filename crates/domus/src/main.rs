use domus::application::{add_location, scan_location};
use domus::domain::{LibraryPath, OutputSettings, Page};
use domus::infrastructure::store::{PlanRequest, Store};
use domus::{Error, Result};
use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "Usage:
  domus version
  domus contracts <output-directory>
  domus init <local-database>
  domus api-preview <absolute-configuration-file>
  domus worker <absolute-configuration-file>
  domus worker-once <absolute-configuration-file>
  domus worker-check <absolute-configuration-file>
  domus publish-new-once <absolute-configuration-file>
  domus publication-status <local-database> <job-id> <plan-version>
  domus source-search <absolute-configuration-file>
  domus source-folder <absolute-configuration-file>
  domus source-candidate-prepare <local-database> <job-id> <plan-version> <absolute-listing-json>
  domus source-candidate <local-database> <candidate-id>
  domus source-candidate-approve <local-database> <candidate-id> <plan-version> <review-sha256> --reviewed-exact-edition-and-permitted-download
  domus service-call <absolute-host-configuration-file> <absolute-request-file>
  domus mcp-stdio <absolute-host-configuration-file>
  domus catalog-release <local-database> <release-id>
  domus catalog-plan-prepare <local-database> <release-id> <request-json>
  domus settings <local-database>
  domus settings-default
  domus staging-init <local-database> <absolute-root>
  domus settings-save <local-database> <expected-version-or-0> <settings-json>
  domus job-create <local-database>
  domus job-history <local-database> <job-id> <plan-version> <after-id> <limit>
  domus job-control <local-database> <job-id> <plan-version> <pause|cancel>
  domus job-control-status <local-database> <control-id>
  domus job-control-continue <local-database> <control-id> <plan-version>
  domus plan-prepare <local-database> <request-json>
  domus plan <local-database> <job-id> <plan-version>
  domus plan-approve <local-database> <job-id> <plan-version>
  domus artwork-local-prepare <local-database> <job-id> <plan-version> <absolute-image-file> <background-json> --permitted-local-cover
  domus artwork-preview <local-database> <job-id> <plan-version> <candidate-id>
  domus artwork-caa-prepare <local-database> <job-id> <plan-version> <image-id> <background-json> --permitted-caa-cover
  domus artwork-evidence <local-database> <job-id> <plan-version> <candidate-id>
  domus artwork-check <local-database> <job-id> <plan-version>
  domus artwork-question <local-database> <question-id>
  domus artwork-resume <local-database> <question-id> <plan-version>
  domus question-answer <local-database> <question-id> <plan-version> <offered-choice>
  domus artwork-approve <local-database> <job-id> <plan-version> <candidate-id> <preview-sha256>
  domus location-add <local-database> <absolute-root> <main|backup>
  domus location <local-database> <location-id>
  domus root-identity <absolute-root>
  domus location-bind <local-database> <location-id> <expected-native-identity>
  domus staging-bind <local-database> <absolute-root> <expected-native-identity>
  domus scan <local-database> <location-id>
  domus browse <local-database> <location-id> <relative-folder> <after-id> <limit>

Only api-preview opens a listener. It requires HTTPS and loopback binding.
Production requires Entra. Explicit development mode uses _anonymous without sign-in.
The worker and worker-once download only an approved album from an explicitly selected source.
It requires separate Soulseek credentials and explicit plaintext-login acknowledgement.
source-search uses the same login acknowledgement for metadata-only outgoing peer search.
source-folder reads one selected peer folder and assesses names; it never approves or downloads.
catalog-release loads the exact MusicBrainz release through the durable provider budget.
service-call runs catalog discovery, metadata, artwork listing or image caching through configured services.
It cannot approve plans, download audio or publish media.
publish-new-once explicitly copies verified staging into new primary/backup album folders.
It never replaces existing albums. Partial folders remain visible after failure.
No command enables media sharing or media serving.
The database must be outside every configured media root.";

fn run(args: &[String]) -> Result<()> {
    match args {
        [command] if command == "settings-default" => {
            println!("{}", serde_json::to_string(&OutputSettings::default())?);
        }
        [command, database, root] if command == "staging-init" => {
            let mut store = Store::open_current_existing(Path::new(database))?;
            let _staging =
                domus::infrastructure::staging::StagingRoot::open(&mut store, Path::new(root))?;
            println!("Private staging initialized and native identity bound.");
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
                    "ConfigurationReloadCommand.ts",
                    domus::infrastructure::server::configuration::ConfigurationReloadCommand::export_to_string(&config),
                ),
                (
                    "ConfigurationReloadState.ts",
                    domus::infrastructure::server::configuration::ConfigurationReloadState::export_to_string(&config),
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
        [command, database] if command == "init" => {
            Store::open(Path::new(database))?;
            println!("Database initialized.");
        }
        [command, configuration] if command == "api-preview" => {
            let prepared =
                domus::infrastructure::server::PreparedServer::load(Path::new(configuration))?;
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(4)
                .enable_all()
                .build()?
                .block_on(prepared.run())?;
        }
        [command, configuration] if command == "worker-once" => {
            let worker =
                domus::infrastructure::worker::PreparedWorker::load(Path::new(configuration))?;
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
        [command, configuration] if command == "worker-check" => {
            let report =
                domus::infrastructure::worker::PreparedWorker::check(Path::new(configuration))?;
            println!("{}", serde_json::to_string(&report)?);
        }
        [command, configuration] if command == "publish-new-once" => {
            let outcome =
                domus::infrastructure::worker::publish_new_once(Path::new(configuration))?;
            println!("{}", serde_json::to_string(&outcome)?);
        }
        [command, database, job, version] if command == "publication-status" => {
            let store = Store::open_read_snapshot(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(
                    &store.publication_destinations(number(job)?, number(version)?)?
                )?
            );
        }
        [command, configuration] if command == "source-search" => {
            let search =
                domus::infrastructure::worker::PreparedSearch::load(Path::new(configuration))?;
            println!("{}", serde_json::to_string(&search.run()?)?);
        }
        [command, configuration] if command == "source-folder" => {
            let folder =
                domus::infrastructure::worker::PreparedFolder::load(Path::new(configuration))?;
            println!("{}", serde_json::to_string(&folder.run()?)?);
        }
        [command, database, release] if command == "catalog-release" => {
            use domus::infrastructure::catalog::CatalogQuery;
            use domus::infrastructure::services::ServiceHost;
            let query = CatalogQuery::release(release)?;
            let host = ServiceHost::new(
                Store::open_existing(Path::new(database))?,
                Default::default(),
            )?;
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
        [command, database, job, version, listing] if command == "source-candidate-prepare" => {
            let candidate = domus::infrastructure::worker::prepare_source_candidate_file(
                Path::new(database),
                number(job)?,
                number(version)?,
                Path::new(listing),
            )?;
            println!("{}", serde_json::to_string(&candidate)?);
        }
        [command, database, id] if command == "source-candidate" => {
            let store =
                domus::infrastructure::store::Store::open_read_snapshot(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.source_candidate(number(id)?)?)?
            );
        }
        [command, database, id, version, hash, acknowledgement]
            if command == "source-candidate-approve"
                && acknowledgement == "--reviewed-exact-edition-and-permitted-download" =>
        {
            let mut store =
                domus::infrastructure::store::Store::open_current_existing(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.approve_source_candidate(
                    number(id)?,
                    number(version)?,
                    hash,
                    true
                )?)?
            );
        }
        [
            command,
            database,
            job,
            version,
            image,
            background,
            acknowledgement,
        ] if command == "artwork-local-prepare" && acknowledgement == "--permitted-local-cover" => {
            let background: Option<[u8; 3]> = serde_json::from_str(background).map_err(|_| {
                Error::InvalidInput("background must be null or an RGB byte array".into())
            })?;
            let mut store = Store::open_existing(Path::new(database))?;
            let candidate = store.prepare_local_artwork_or_ask(
                number(job)?,
                number(version)?,
                Path::new(image),
                background,
                true,
            )?;
            println!("{}", serde_json::to_string(&candidate)?);
        }
        [
            command,
            database,
            job,
            version,
            image,
            background,
            acknowledgement,
        ] if command == "artwork-caa-prepare" && acknowledgement == "--permitted-caa-cover" => {
            let background: Option<[u8; 3]> = serde_json::from_str(background).map_err(|_| {
                Error::InvalidInput("background must be null or an RGB byte array".into())
            })?;
            let mut store = Store::open_existing(Path::new(database))?;
            let candidate = store.prepare_cached_caa_artwork_or_ask(
                number(job)?,
                number(version)?,
                image,
                background,
                true,
            )?;
            println!("{}", serde_json::to_string(&candidate)?);
        }
        [command, database, job, version] if command == "artwork-check" => {
            let mut store = Store::open_existing(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.check_artwork(number(job)?, number(version)?)?)?
            );
        }
        [command, database, id] if command == "artwork-question" => {
            let store = Store::open_read_snapshot(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.artwork_decision(number(id)?)?)?
            );
        }
        [command, database, id, version] if command == "artwork-resume" => {
            let mut store = Store::open_existing(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.resume_artwork(number(id)?, number(version)?)?)?
            );
        }
        [command, database, job, version, action] if command == "job-control" => {
            let action = match action.as_str() {
                "pause" => domus::infrastructure::store::JobControlAction::Pause,
                "cancel" => domus::infrastructure::store::JobControlAction::Cancel,
                _ => {
                    return Err(domus::Error::InvalidInput(
                        "job control must be pause or cancel".into(),
                    ));
                }
            };
            let job = number(job)?;
            let version = number(version)?;
            let mut store = Store::open_current_existing(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.request_job_control(job, version, action)?)?
            );
        }
        [command, database, id] if command == "job-control-status" => {
            let store = Store::open_read_snapshot(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.job_control(number(id)?)?)?
            );
        }
        [command, database, job, version, after, limit] if command == "job-history" => {
            let job = number(job)?;
            let version = number(version)?;
            let page = Page::new(number(after)?, number(limit)?)?;
            let store = Store::open_read_snapshot(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.plan_events(job, version, page)?)?
            );
        }
        [command, database, id, version] if command == "job-control-continue" => {
            let id = number(id)?;
            let version = number(version)?;
            let mut store = Store::open_current_existing(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.continue_job_control(id, version)?)?
            );
        }
        [command, database, id, version, choice] if command == "question-answer" => {
            let mut store = Store::open_existing(Path::new(database))?;
            store.answer(number(id)?, number(version)?, choice)?;
        }
        [command, database, job, version, candidate] if command == "artwork-evidence" => {
            let store = Store::open_read_snapshot(Path::new(database))?;
            // A missing candidate is an error, not a legacy-evidence result.
            store.artwork_jpeg(number(job)?, number(version)?, number(candidate)?)?;
            println!(
                "{}",
                serde_json::to_string(&store.artwork_caa_evidence(
                    number(job)?,
                    number(version)?,
                    number(candidate)?,
                )?)?
            );
        }
        [command, database, job, version, candidate, hash] if command == "artwork-approve" => {
            let mut store = Store::open_existing(Path::new(database))?;
            store.approve_artwork(number(job)?, number(version)?, number(candidate)?, hash)?;
            println!("Artwork approved.");
        }
        [command, database, job, version, candidate] if command == "artwork-preview" => {
            let store = Store::open_read_snapshot(Path::new(database))?;
            let cover = store.artwork_jpeg(number(job)?, number(version)?, number(candidate)?)?;
            let mut output = std::io::stdout().lock();
            output.write_all(cover.bytes())?;
            output.flush()?;
        }
        [command, database] if command == "settings" => {
            let store = Store::open_existing(Path::new(database))?;
            println!("{}", serde_json::to_string(&store.settings()?)?);
        }
        [command, database, expected, file] if command == "settings-save" => {
            let expected: i64 = number(expected)?;
            if expected < 0 {
                return Err(Error::InvalidInput(
                    "settings version must be nonnegative".into(),
                ));
            }
            let settings: OutputSettings = json_file(file, 16384)?;
            let mut store = Store::open_existing(Path::new(database))?;
            println!(
                "{}",
                store.save_settings((expected != 0).then_some(expected), &settings)?
            );
        }
        [command, database] if command == "job-create" => {
            let mut store = Store::open_existing(Path::new(database))?;
            println!("{}", store.enqueue(1)?);
        }
        [command, database, file] if command == "plan-prepare" => {
            let request: PlanRequest = json_file(file, 2 * 1024 * 1024)?;
            let mut store = Store::open_existing(Path::new(database))?;
            println!("{}", serde_json::to_string(&store.prepare_plan(request)?)?);
        }
        [command, database, release, file] if command == "catalog-plan-prepare" => {
            use domus::application::catalog::{CatalogPlanRequest, prepare_cached_catalog_plan};
            let request: CatalogPlanRequest = json_file(file, 16384)?;
            let mut store = Store::open_existing(Path::new(database))?;
            let plan = prepare_cached_catalog_plan(&mut store, release, request)?;
            println!("{}", serde_json::to_string(&plan)?);
        }
        [command, database, job, version] if command == "plan" => {
            let store = Store::open_existing(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.plan(number(job)?, number(version)?)?)?
            );
        }
        [command, database, job, version] if command == "plan-approve" => {
            let mut store = Store::open_existing(Path::new(database))?;
            store.approve_plan(number(job)?, number(version)?)?;
            println!("Plan approved. No transfer or file operation started.");
        }
        [command, database, root, kind] if command == "location-add" => {
            let store = Store::open_existing(Path::new(database))?;
            println!("{}", add_location(&store, Path::new(root), kind)?);
        }
        [command, database, location] if command == "scan" => {
            let mut store = Store::open_existing(Path::new(database))?;
            println!("{}", scan_location(&mut store, number(location)?)?);
        }
        [command, root] if command == "root-identity" => {
            println!(
                "{}",
                domus::infrastructure::filesystem::native_root_identity(Path::new(root))?
            );
        }
        [command, database, location, expected] if command == "location-bind" => {
            let store = Store::open_existing(Path::new(database))?;
            store.bind_location(number(location)?, expected)?;
            println!("Native library root identity bound. No media operation started.");
        }
        [command, database, root, expected] if command == "staging-bind" => {
            let store = Store::open_existing(Path::new(database))?;
            store.bind_staging_root(Path::new(root), expected)?;
            println!("Native staging root identity bound. No file operation started.");
        }
        [command, database, location] if command == "location" => {
            let store = Store::open_existing(Path::new(database))?;
            println!(
                "{}",
                serde_json::to_string(&store.location(number(location)?)?)?
            );
        }
        [command, database, location, folder, after, limit] if command == "browse" => {
            let store = Store::open_existing(Path::new(database))?;
            let page = Page::new(number(after)?, number(limit)?)?;
            println!(
                "{}",
                serde_json::to_string(&store.browse(
                    number(location)?,
                    &LibraryPath::parse(folder)?,
                    page
                )?)?
            );
        }
        _ => return Err(Error::InvalidInput(USAGE.into())),
    }
    Ok(())
}

fn json_file<T: serde::de::DeserializeOwned>(path: &str, maximum: usize) -> Result<T> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(Error::InvalidInput(format!(
            "JSON input exceeds {maximum} bytes"
        )));
    }
    Ok(serde_json::from_slice(&bytes)?)
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
