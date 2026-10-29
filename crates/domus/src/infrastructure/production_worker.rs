//! Durable same-host handoff. The API never owns a production media operation.
use super::local_file::ProcessLock;
use super::store::{RequestClaim, Store};
use super::worker::{LocalAcquisition, StartLocalDownload};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfiguration {
    #[serde(default = "default_capacity")]
    pub queue_capacity: u16,
    pub acquisition: Option<LocalAcquisition>,
    #[serde(default)]
    pub sharing: Option<super::sharing::SharingConfiguration>,
}

fn default_capacity() -> u16 {
    64
}

impl WorkerConfiguration {
    pub(crate) fn binding(
        &self,
        services: &crate::application::services::ServiceConfiguration,
    ) -> Result<String> {
        use sha2::{Digest, Sha256};
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(services, &self.acquisition))?)
        ))
    }

    pub(crate) fn validate(
        &self,
        store: &Store,
        services: &crate::application::services::ServiceConfiguration,
    ) -> Result<()> {
        if !(1..=1024).contains(&self.queue_capacity) {
            return Err(Error::InvalidInput(
                "worker queue capacity must be 1 through 1024".into(),
            ));
        }
        if let Some(acquisition) = &self.acquisition {
            acquisition.validate(store, services)?;
        }
        if let Some(sharing) = &self.sharing {
            sharing.validate(store, self.acquisition.as_ref())?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerOperation {
    Scan {
        location_id: i64,
    },
    RefreshFolder {
        location_id: i64,
        folder: String,
    },
    Acquire {
        job_id: i64,
        plan_version: i64,
        source_candidate: Option<i64>,
        acknowledge_new_only_publication: bool,
    },
    Publish {
        job_id: i64,
        plan_version: i64,
        normalization_run: i64,
        acknowledge_new_only_publication: bool,
    },
}

impl WorkerOperation {
    pub(crate) fn validate_recovery(&self, store: &Store) -> Result<()> {
        match *self {
            Self::Acquire {
                job_id,
                plan_version,
                acknowledge_new_only_publication,
                ..
            } => match store.job_state(job_id)?.as_deref() {
                Some("publishing" | "backup_pending") => {
                    require_ack(job_id, plan_version, acknowledge_new_only_publication)?;
                    store.require_current_worker_plan(job_id, plan_version)?;
                    if store.plan(job_id, plan_version)?.approved_at.is_none() {
                        return Err(Error::Conflict(
                            "the retained acquisition requires its approved plan".into(),
                        ));
                    }
                    store.completed_worker_normalization(job_id, plan_version)?;
                    Ok(())
                }
                Some("downloading" | "validating" | "normalizing") => Self::Acquire {
                    job_id,
                    plan_version,
                    source_candidate: None,
                    acknowledge_new_only_publication,
                }
                .validate(store),
                _ => self.validate(store),
            },
            _ => self.validate(store),
        }
    }
    pub(crate) fn lane(&self) -> &'static str {
        if matches!(self, Self::Scan { .. } | Self::RefreshFolder { .. }) {
            "scan"
        } else {
            "acquisition"
        }
    }

    pub(crate) fn target(&self) -> String {
        match self {
            Self::Scan { location_id } | Self::RefreshFolder { location_id, .. } => {
                format!("scan-{location_id}")
            }
            Self::Acquire { job_id, .. } | Self::Publish { job_id, .. } => format!("job-{job_id}"),
        }
    }

    pub(crate) fn job(&self) -> Option<(i64, i64)> {
        match *self {
            Self::Scan { .. } | Self::RefreshFolder { .. } => None,
            Self::Acquire {
                job_id,
                plan_version,
                ..
            }
            | Self::Publish {
                job_id,
                plan_version,
                ..
            } => Some((job_id, plan_version)),
        }
    }

    pub(crate) fn validate(&self, store: &Store) -> Result<()> {
        if let Some((job, version)) = self.job() {
            store.require_current_worker_plan(job, version)?;
        }
        match *self {
            Self::Scan { location_id } => {
                if location_id <= 0 {
                    return Err(Error::InvalidInput("location ID must be positive".into()));
                }
                store.location(location_id)?;
            }
            Self::RefreshFolder {
                location_id,
                ref folder,
            } => {
                if location_id <= 0 || folder.len() > 3000 {
                    return Err(Error::InvalidInput(
                        "folder refresh requires a positive location and bounded relative folder"
                            .into(),
                    ));
                }
                crate::domain::LibraryPath::parse(folder)?;
                store.location(location_id)?;
            }
            Self::Acquire {
                job_id,
                plan_version,
                source_candidate,
                acknowledge_new_only_publication,
            } => {
                require_ack(job_id, plan_version, acknowledge_new_only_publication)?;
                let plan = store.plan(job_id, plan_version)?;
                if plan.approved_at.is_none()
                    || store.job_state(job_id)?.as_deref().is_none_or(|state| {
                        !matches!(
                            state,
                            "queued" | "searching" | "downloading" | "validating" | "normalizing"
                        )
                    })
                {
                    return Err(Error::Conflict(
                        "acquisition requires the current approved runnable plan".into(),
                    ));
                }
                store.require_job_running(job_id, plan_version)?;
                store.approved_artwork(job_id, plan_version)?;
                match (store.job_state(job_id)?.as_deref(), source_candidate) {
                    (Some("queued" | "searching"), Some(candidate)) => {
                        store.approved_source_candidate(job_id, plan_version, candidate)?;
                    }
                    (Some("downloading" | "validating" | "normalizing"), None) => {}
                    _ => {
                        return Err(Error::Conflict(
                            "select an approved source only for an unstarted acquisition".into(),
                        ));
                    }
                }
            }
            Self::Publish {
                job_id,
                plan_version,
                normalization_run,
                acknowledge_new_only_publication,
            } => {
                require_ack(job_id, plan_version, acknowledge_new_only_publication)?;
                let plan = store.plan(job_id, plan_version)?;
                if plan.approved_at.is_none()
                    || !matches!(
                        store.job_state(job_id)?.as_deref(),
                        Some("publishing" | "backup_pending")
                    )
                    || normalization_run
                        != store.completed_worker_normalization(job_id, plan_version)?
                {
                    return Err(Error::Conflict(
                        "publication requires its current approved completed normalization".into(),
                    ));
                }
                let targets = store.publication_destinations(job_id, plan_version)?;
                if targets.is_empty()
                    || targets
                        .iter()
                        .any(|target| target.run_id != normalization_run)
                {
                    return Err(Error::Conflict(
                        "publication recovery requires the retained exact journal".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn require_ack(job: i64, version: i64, acknowledged: bool) -> Result<()> {
    if job <= 0 || version <= 0 || !acknowledged {
        return Err(Error::InvalidInput("worker requires positive job/plan IDs and explicit new-only publication acknowledgement".into()));
    }
    Ok(())
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct WorkerRequest {
    pub id: i64,
    pub operation: WorkerOperation,
    pub state: String,
    pub last_error: Option<String>,
    pub configuration_binding: String,
}

#[derive(Serialize, ts_rs::TS)]
pub struct WorkerStatus {
    pub online: bool,
    pub stopping: bool,
    pub active_revision: Option<String>,
    pub capacity: u16,
    pub unfinished: u16,
    pub active: u16,
}

struct Running {
    claim: Arc<RequestClaim>,
    thread: JoinHandle<Result<()>>,
}

struct ActiveLanes {
    slots: [Option<Running>; 2],
    stop: Arc<AtomicBool>,
}

impl Drop for ActiveLanes {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for running in self.slots.iter_mut().filter_map(Option::take) {
            if !matches!(running.thread.join(), Ok(Ok(()))) {
                eprintln!("worker operation failed while draining the supervisor");
            }
        }
    }
}

pub struct PreparedProductionWorker {
    database: PathBuf,
    configuration: WorkerConfiguration,
    services: crate::application::services::ServiceConfiguration,
    revision: String,
    _lock: ProcessLock,
}

impl PreparedProductionWorker {
    pub fn load(path: &Path) -> Result<Self> {
        let (source, configuration, _) =
            super::server::configuration::ConfigurationService::load(path)?;
        let worker = configuration
            .production_worker
            .clone()
            .ok_or_else(|| Error::InvalidInput("production_worker is not configured".into()))?;
        let store = Store::open_read_current(&configuration.database)?;
        worker.validate(&store, &configuration.services)?;
        super::local_file::protect_files(&store, &[path])?;
        let lock = ProcessLock::acquire(store.database_path(), "worker")?;
        Ok(Self {
            database: store.database_path().to_owned(),
            configuration: worker,
            services: configuration.services,
            revision: source.active_revision().into(),
            _lock: lock,
        })
    }

    pub async fn run(self) -> Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let (sent, mut received) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let result = self.supervise(stop);
            let _ = sent.send(result);
        });
        let result = tokio::select! {
            result = &mut received => result.map_err(|_|Error::Internal("worker supervisor exited without a result"))?,
            signal = super::server::shutdown_signal() => {
                stopping.store(true,Ordering::Release);
                let drained = received.await.map_err(|_|Error::Internal("worker supervisor failed while draining"))?;
                signal?;
                drained
            }
        };
        thread
            .join()
            .map_err(|_| Error::Internal("worker supervisor panicked"))?;
        result
    }

    fn supervise(self, stop: Arc<AtomicBool>) -> Result<()> {
        let sharing = self
            .configuration
            .sharing
            .as_ref()
            .map(|configuration| {
                super::soulseek::sharing::SharingRuntime::start(
                    &self.database,
                    configuration,
                    self.configuration.acquisition.as_ref(),
                    stop.clone(),
                )
            })
            .transpose()?
            .flatten();
        let mut store = Store::open_current_existing(&self.database)?;
        let host = store.start_worker_host(
            &format!("production-{}", std::process::id()),
            &self.revision,
        )?;
        let mut active = ActiveLanes {
            slots: [None, None],
            stop: stop.clone(),
        };
        let lanes = &mut active.slots;
        let mut failure = None;
        loop {
            if let Some(sharing) = &sharing
                && let Err(error) = sharing.check()
            {
                failure.get_or_insert(error);
                stop.store(true, Ordering::Release);
            }
            let heartbeat = store.heartbeat_worker(&host, stop.load(Ordering::Acquire));
            if let Err(error) = heartbeat {
                failure.get_or_insert(error);
                stop.store(true, Ordering::Release);
            }
            for (index, lane) in ["acquisition", "scan"].iter().enumerate() {
                if let Some(running) = lanes[index].as_ref()
                    && let Err(error) = store.renew_worker_request(&running.claim)
                {
                    failure.get_or_insert(error);
                    stop.store(true, Ordering::Release);
                }
                if lanes[index]
                    .as_ref()
                    .is_some_and(|running| running.thread.is_finished())
                {
                    let running = lanes[index]
                        .take()
                        .ok_or(Error::Internal("worker lane disappeared"))?;
                    let result = running
                        .thread
                        .join()
                        .unwrap_or_else(|_| Err(Error::Internal("worker operation panicked")));
                    let state = match &result {
                        Ok(()) => "complete",
                        Err(Error::WorkerStopping) => "interrupted",
                        Err(
                            Error::ArtworkRequired(_) | Error::JobStopped(_) | Error::WorkerWaiting,
                        ) => "waiting",
                        Err(_) => "failed",
                    };
                    let message = result.as_ref().err().map(ToString::to_string);
                    if let Err(error) =
                        store.finish_worker_request(&running.claim, state, message.as_deref())
                    {
                        failure.get_or_insert(error);
                        stop.store(true, Ordering::Release);
                    }
                }
                if lanes[index].is_none() && !stop.load(Ordering::Acquire) {
                    match store.claim_worker_request(&host, lane) {
                        Ok(Some(claim)) => {
                            let database = self.database.clone();
                            let configuration = self.configuration.clone();
                            let services = self.services.clone();
                            let stopping = stop.clone();
                            let claim = Arc::new(claim);
                            let operation = claim.request.operation.clone();
                            let binding = claim.request.configuration_binding.clone();
                            let account = sharing.as_ref().map(|sharing| sharing.account.clone());
                            let spawned = std::thread::Builder::new()
                                .name(format!("domus-{lane}"))
                                .spawn(move || {
                                    execute(
                                        &database,
                                        &configuration,
                                        services,
                                        operation,
                                        stopping,
                                        &binding,
                                        account,
                                    )
                                });
                            match spawned {
                                Ok(thread) => lanes[index] = Some(Running { claim, thread }),
                                Err(error) => {
                                    if let Err(persist) = store.finish_worker_request(
                                        &claim,
                                        "failed",
                                        Some("worker operation thread could not start"),
                                    ) {
                                        failure.get_or_insert(persist);
                                    }
                                    failure.get_or_insert(Error::Io(error));
                                    stop.store(true, Ordering::Release);
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            failure.get_or_insert(error);
                            stop.store(true, Ordering::Release);
                        }
                    }
                }
            }
            if stop.load(Ordering::Acquire) && lanes.iter().all(Option::is_none) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        let stopped = store.stop_worker_host(&host);
        match failure {
            Some(error) => Err(error),
            None => stopped,
        }
    }
}

fn execute(
    database: &Path,
    configuration: &WorkerConfiguration,
    services: crate::application::services::ServiceConfiguration,
    operation: WorkerOperation,
    stop: Arc<AtomicBool>,
    binding: &str,
    shared_account: Option<Arc<super::soulseek::sharing::Account>>,
) -> Result<()> {
    let mut store = Store::open_current_existing(database)?.with_worker_stop(stop.clone());
    store.check_worker_stop()?;
    if let Some((job, version)) = operation.job() {
        store.require_current_worker_plan(job, version)?;
        match store.job_state(job)?.as_deref() {
            Some("complete" | "cancelled") => return Ok(()),
            Some("awaiting_input") => return Err(Error::WorkerWaiting),
            _ => {}
        }
    }
    if operation.job().is_some() && configuration.binding(&services)? != binding {
        return Err(Error::Conflict("worker provider or acquisition configuration differs from the accepted request; restore that configuration or dismiss and explicitly submit again".into()));
    }
    match operation {
        WorkerOperation::Scan { location_id } => {
            crate::application::scan_location(&mut store, location_id)?;
        }
        WorkerOperation::RefreshFolder {
            location_id,
            folder,
        } => {
            crate::application::refresh_library_folder(
                &mut store,
                location_id,
                &crate::domain::LibraryPath::parse(&folder)?,
            )?;
        }
        WorkerOperation::Acquire {
            job_id,
            plan_version,
            source_candidate,
            acknowledge_new_only_publication,
        } => {
            require_ack(job_id, plan_version, acknowledge_new_only_publication)?;
            let acquisition = configuration
                .acquisition
                .as_ref()
                .ok_or_else(|| Error::Conflict("worker acquisition is not configured".into()))?;
            if matches!(
                store.job_state(job_id)?.as_deref(),
                Some("publishing" | "backup_pending")
            ) {
                let run = store.completed_worker_normalization(job_id, plan_version)?;
                super::worker::publish_worker(
                    database,
                    &acquisition.staging_root,
                    job_id,
                    plan_version,
                    run,
                    stop,
                )?;
            } else {
                let source_candidate = if matches!(
                    store.job_state(job_id)?.as_deref(),
                    Some("queued" | "searching")
                ) {
                    source_candidate
                } else {
                    None
                };
                acquisition.acquire_controlled(
                    database,
                    services,
                    job_id,
                    plan_version,
                    StartLocalDownload {
                        source_candidate,
                        acknowledge_new_only_publication: true,
                    },
                    super::worker::AcquisitionExecution {
                        stop: Some(stop),
                        shared_account,
                    },
                )?;
            }
        }
        WorkerOperation::Publish {
            job_id,
            plan_version,
            normalization_run,
            acknowledge_new_only_publication,
        } => {
            require_ack(job_id, plan_version, acknowledge_new_only_publication)?;
            WorkerOperation::Publish {
                job_id,
                plan_version,
                normalization_run,
                acknowledge_new_only_publication,
            }
            .validate(&store)?;
            let acquisition = configuration
                .acquisition
                .as_ref()
                .ok_or_else(|| Error::Conflict("worker acquisition is not configured".into()))?;
            super::worker::publish_worker(
                database,
                &acquisition.staging_root,
                job_id,
                plan_version,
                normalization_run,
                stop,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
