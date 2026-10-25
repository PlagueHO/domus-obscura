//! A single explicit local operation at a time. This is not a scheduler and
//! never replays an uncertain submission after failure or process restart.
use super::worker::{LocalAcquisition, SourceFolderRequest, StartLocalDownload};
use crate::application::services::ServiceConfiguration;
use crate::{Error, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub struct LocalRuntime {
    database: PathBuf,
    configuration: LocalAcquisition,
    services: ServiceConfiguration,
    slot: Arc<Semaphore>,
    last_error: Arc<Mutex<Option<String>>>,
}

#[derive(Serialize, ts_rs::TS)]
pub struct LocalStatus {
    pub enabled: bool,
    pub busy: bool,
    pub last_error: Option<String>,
}

impl LocalRuntime {
    pub fn new(
        database: PathBuf,
        configuration: LocalAcquisition,
        services: ServiceConfiguration,
    ) -> Result<Self> {
        let store = super::store::Store::open_read_snapshot(&database)?;
        configuration.validate(&store, &services)?;
        Ok(Self {
            database,
            configuration,
            services,
            slot: Arc::new(Semaphore::new(1)),
            last_error: Arc::new(Mutex::new(None)),
        })
    }

    pub fn status(&self) -> Result<LocalStatus> {
        Ok(LocalStatus {
            enabled: true,
            busy: self.slot.available_permits() == 0,
            last_error: self
                .last_error
                .lock()
                .map_err(|_| Error::Internal("local status lock failed"))?
                .clone(),
        })
    }

    fn permit(&self) -> Result<OwnedSemaphorePermit> {
        self.slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy("a local source or download operation is already running"))
    }

    pub async fn search(
        &self,
        release_id: String,
    ) -> Result<super::soulseek::search::SearchReport> {
        let permit = self.permit()?;
        let (config, database, services) = (
            self.configuration.clone(),
            self.database.clone(),
            self.services.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            config.search(&database, services, release_id)
        })
        .await
        .map_err(|_| Error::Internal("source search task failed"))?
    }

    pub async fn folder(
        &self,
        request: SourceFolderRequest,
    ) -> Result<super::worker::SourceFolderReport> {
        let permit = self.permit()?;
        let (config, database, services) = (
            self.configuration.clone(),
            self.database.clone(),
            self.services.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            config.folder(&database, services, request)
        })
        .await
        .map_err(|_| Error::Internal("source folder task failed"))?
    }

    pub fn start(&self, job: i64, version: i64, request: StartLocalDownload) -> Result<()> {
        if job <= 0 || version <= 0 || !request.acknowledge_new_only_publication {
            return Err(Error::InvalidInput(
                "positive job/plan IDs and new-only publication acknowledgement are required"
                    .into(),
            ));
        }
        let permit = self.permit()?;
        *self
            .last_error
            .lock()
            .map_err(|_| Error::Internal("local status lock failed"))? = None;
        let (config, database, services, last_error) = (
            self.configuration.clone(),
            self.database.clone(),
            self.services.clone(),
            self.last_error.clone(),
        );
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let result = config.acquire_and_publish(&database, services, job, version, request);
                // Hold the slot through result recording, including validation failures.
                (result, permit)
            })
            .await;
            let (message, _permit) = match result {
                Ok((Ok(_), permit)) => (None, Some(permit)),
                Ok((Err(error), permit)) => {
                    eprintln!("local download {job}/{version} stopped: {error}");
                    (
                        Some(format!(
                            "Download {job}/{version} stopped: {error}. Saved files and receipts were kept. Do not retry an uncertain transfer."
                        )),
                        Some(permit),
                    )
                }
                Err(error) => {
                    eprintln!("local download task failed: {error}");
                    (Some("Local download task failed. Inspect saved state before another action.".into()), None)
                }
            };
            match last_error.lock() {
                Ok(mut status) => *status = message,
                Err(_) => eprintln!("local download status lock failed"),
            }
        });
        Ok(())
    }
}
