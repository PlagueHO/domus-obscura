use super::{json_file, number};
use domus::application::{add_location, scan_location};
use domus::domain::{LibraryPath, OutputSettings, Page};
use domus::infrastructure::server::configuration::ConfigurationService;
use domus::infrastructure::store::{PlanRequest, Store, StoreOpenMode};
use domus::{Error, Result};
use std::io::Write;
use std::path::Path;

fn store(configuration: &str, mode: StoreOpenMode) -> Result<Store> {
    ConfigurationService::open(Path::new(configuration))?.open_store(mode)
}

fn editable_store(configuration: &str) -> Result<Store> {
    let configuration = ConfigurationService::open(Path::new(configuration))?;
    configuration.require_editable()?;
    configuration.open_store(StoreOpenMode::Current)
}

pub(super) fn run(args: &[String]) -> Result<bool> {
    let [command, flag, configuration, rest @ ..] = args else {
        return Ok(false);
    };
    if flag != "--host" {
        return Ok(false);
    }
    match (command.as_str(), rest) {
        ("staging-init", [root]) => {
            let mut store = editable_store(configuration)?;
            let _staging =
                domus::infrastructure::staging::StagingRoot::open(&mut store, Path::new(root))?;
            println!("Private staging initialized and native identity bound.");
        }
        ("publication-status", [job, version]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            println!(
                "{}",
                serde_json::to_string(
                    &store.publication_destinations(number(job)?, number(version)?)?
                )?
            );
        }
        ("source-candidate-prepare", [job, version, listing]) => {
            let candidate = domus::infrastructure::worker::prepare_source_candidate_file(
                Path::new(configuration),
                number(job)?,
                number(version)?,
                Path::new(listing),
            )?;
            println!("{}", serde_json::to_string(&candidate)?);
        }
        ("source-candidate", [id]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            println!(
                "{}",
                serde_json::to_string(&store.source_candidate(number(id)?)?)?
            );
        }
        ("source-candidate-approve", [id, version, hash, acknowledgement])
            if acknowledgement == "--reviewed-exact-edition-and-permitted-download" =>
        {
            let mut store = store(configuration, StoreOpenMode::Current)?;
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
        ("artwork-local-prepare", [job, version, image, background, acknowledgement])
            if acknowledgement == "--permitted-local-cover" =>
        {
            let background: Option<[u8; 3]> = serde_json::from_str(background).map_err(|_| {
                Error::InvalidInput("background must be null or an RGB byte array".into())
            })?;
            let mut store = store(configuration, StoreOpenMode::Current)?;
            let candidate = store.prepare_local_artwork_or_ask(
                number(job)?,
                number(version)?,
                Path::new(image),
                background,
                true,
            )?;
            println!("{}", serde_json::to_string(&candidate)?);
        }
        ("artwork-check", [job, version]) => {
            let mut store = store(configuration, StoreOpenMode::Current)?;
            println!(
                "{}",
                serde_json::to_string(&store.check_artwork(number(job)?, number(version)?)?)?
            );
        }
        ("artwork-question", [id]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            println!(
                "{}",
                serde_json::to_string(&store.artwork_decision(number(id)?)?)?
            );
        }
        ("artwork-resume", [id, version]) => {
            let mut store = store(configuration, StoreOpenMode::Current)?;
            println!(
                "{}",
                serde_json::to_string(&store.resume_artwork(number(id)?, number(version)?)?)?
            );
        }
        ("job-control", [job, version, action]) => {
            let action = match action.as_str() {
                "pause" => domus::infrastructure::store::JobControlAction::Pause,
                "cancel" => domus::infrastructure::store::JobControlAction::Cancel,
                _ => {
                    return Err(Error::InvalidInput(
                        "job control must be pause or cancel".into(),
                    ));
                }
            };
            let job = number(job)?;
            let version = number(version)?;
            let mut store = store(configuration, StoreOpenMode::Current)?;
            println!(
                "{}",
                serde_json::to_string(&store.request_job_control(job, version, action)?)?
            );
        }
        ("job-control-status", [id]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            println!(
                "{}",
                serde_json::to_string(&store.job_control(number(id)?)?)?
            );
        }
        ("job-history", [job, version, after, limit]) => {
            let job = number(job)?;
            let version = number(version)?;
            let page = Page::new(number(after)?, number(limit)?)?;
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            println!(
                "{}",
                serde_json::to_string(&store.plan_events(job, version, page)?)?
            );
        }
        ("job-control-continue", [id, version]) => {
            let id = number(id)?;
            let version = number(version)?;
            let mut store = store(configuration, StoreOpenMode::Current)?;
            println!(
                "{}",
                serde_json::to_string(&store.continue_job_control(id, version)?)?
            );
        }
        ("question-answer", [id, version, choice]) => {
            let mut store = store(configuration, StoreOpenMode::Current)?;
            store.answer(number(id)?, number(version)?, choice)?;
        }
        ("artwork-evidence", [job, version, candidate]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
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
        ("artwork-approve", [job, version, candidate, hash]) => {
            let mut store = store(configuration, StoreOpenMode::Current)?;
            store.approve_artwork(number(job)?, number(version)?, number(candidate)?, hash)?;
            println!("Artwork approved.");
        }
        ("artwork-preview", [job, version, candidate]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            let cover = store.artwork_jpeg(number(job)?, number(version)?, number(candidate)?)?;
            let mut output = std::io::stdout().lock();
            output.write_all(cover.bytes())?;
            output.flush()?;
        }
        ("settings", []) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            println!("{}", serde_json::to_string(&store.settings()?)?);
        }
        ("settings-save", [expected, file]) => {
            let expected: i64 = number(expected)?;
            if expected < 0 {
                return Err(Error::InvalidInput(
                    "settings version must be nonnegative".into(),
                ));
            }
            let source = ConfigurationService::open(Path::new(configuration))?;
            source.require_editable()?;
            let settings: OutputSettings = json_file(&source, file, 16384)?;
            let mut store = source.open_store(StoreOpenMode::Current)?;
            println!(
                "{}",
                store.save_settings((expected != 0).then_some(expected), &settings)?
            );
        }
        ("job-create", []) => {
            let mut store = store(configuration, StoreOpenMode::Current)?;
            println!("{}", store.enqueue(1)?);
        }
        ("plan-prepare", [file]) => {
            let source = ConfigurationService::open(Path::new(configuration))?;
            let request: PlanRequest = json_file(&source, file, 2 * 1024 * 1024)?;
            let mut store = source.open_store(StoreOpenMode::Current)?;
            println!("{}", serde_json::to_string(&store.prepare_plan(request)?)?);
        }
        ("plan", [job, version]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            println!(
                "{}",
                serde_json::to_string(&store.plan(number(job)?, number(version)?)?)?
            );
        }
        ("plan-approve", [job, version]) => {
            let mut store = store(configuration, StoreOpenMode::Current)?;
            store.approve_plan(number(job)?, number(version)?)?;
            println!("Plan approved. No transfer or file operation started.");
        }
        ("location-add", [root, kind]) => {
            let store = editable_store(configuration)?;
            println!("{}", add_location(&store, Path::new(root), kind)?);
        }
        ("scan", [location]) => {
            let mut store = store(configuration, StoreOpenMode::Current)?;
            println!("{}", scan_location(&mut store, number(location)?)?);
        }
        ("location-bind", [location, expected]) => {
            let store = editable_store(configuration)?;
            store.bind_location(number(location)?, expected)?;
            println!("Native library root identity bound. No media operation started.");
        }
        ("staging-bind", [root, expected]) => {
            let store = editable_store(configuration)?;
            store.bind_staging_root(Path::new(root), expected)?;
            println!("Native staging root identity bound. No file operation started.");
        }
        ("location", [location]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
            println!(
                "{}",
                serde_json::to_string(&store.location(number(location)?)?)?
            );
        }
        ("browse", [location, folder, after, limit]) => {
            let store = store(configuration, StoreOpenMode::Snapshot)?;
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
        _ => return Ok(false),
    }
    Ok(true)
}
