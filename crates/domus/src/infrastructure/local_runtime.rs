//! A single explicit local operation at a time. This is not a scheduler and
//! never replays an uncertain submission after failure or process restart.
use super::worker::{LocalAcquisition, ResumePublication, SourceFolderRequest, StartLocalDownload};
use crate::application::services::ServiceConfiguration;
use crate::{Error, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub struct LocalRuntime {
    database: PathBuf,
    configuration: LocalAcquisition,
    services: ServiceConfiguration,
    cache: Option<Arc<dyn crate::application::services::cache::Cache>>,
    slot: Arc<Semaphore>,
    last_error: Arc<Mutex<Option<String>>>,
    stopping: AtomicBool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> Result<Arc<LocalRuntime>> {
        Ok(Arc::new(LocalRuntime {
            database: PathBuf::from("unused-generated-state"),
            configuration: LocalAcquisition {
                staging_root: PathBuf::from("unused-generated-staging"),
                credentials_file: PathBuf::from("unused-generated-credentials"),
                server: "8.8.8.8:2242"
                    .parse()
                    .map_err(|_| Error::Internal("fixture"))?,
                acknowledge_plaintext_login: true,
            },
            services: Default::default(),
            cache: None,
            slot: Arc::new(Semaphore::new(1)),
            last_error: Arc::new(Mutex::new(None)),
            stopping: AtomicBool::new(false),
        }))
    }

    #[tokio::test]
    async fn publication_recovery_shares_the_download_slot_and_retains_failures() -> Result<()> {
        let runtime = runtime()?;
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, hold) = std::sync::mpsc::channel();
        runtime.start_operation(3, 2, "Publication recovery", move || {
            let _ = started.send(());
            hold.recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|_| Error::Internal("fixture release timed out"))?;
            Err(Error::Conflict("generated retained-file conflict".into()))
        })?;
        ready
            .await
            .map_err(|_| Error::Internal("fixture did not start"))?;
        assert!(runtime.status()?.busy);
        assert!(matches!(
            runtime.resume_publication(
                3,
                2,
                ResumePublication {
                    normalization_run: 7,
                    acknowledge_new_only_publication: true,
                }
            ),
            Err(Error::Busy(_))
        ));
        assert!(matches!(
            runtime.start(
                3,
                2,
                StartLocalDownload {
                    source_candidate: None,
                    acknowledge_new_only_publication: true,
                }
            ),
            Err(Error::Busy(_))
        ));
        assert!(runtime.status()?.last_error.is_none());
        release
            .send(())
            .map_err(|_| Error::Internal("fixture release failed"))?;
        for _ in 0..200 {
            let status = runtime.status()?;
            if !status.busy {
                let error = status
                    .last_error
                    .ok_or(Error::Internal("missing failure"))?;
                assert!(error.contains("Publication recovery 3/2"));
                assert!(error.contains("generated retained-file conflict"));
                assert!(error.contains("Saved files and receipts were kept"));
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(Error::Internal("fixture operation did not finish"))
    }

    #[tokio::test]
    async fn shutdown_rejects_new_work_and_drains_success_error_and_panicked_tasks() -> Result<()> {
        for outcome in 0..3 {
            let runtime = runtime()?;
            let (started, ready) = tokio::sync::oneshot::channel();
            let (release, hold) = std::sync::mpsc::channel();
            runtime.start_operation(3, 2, "Generated operation", move || {
                let _ = started.send(());
                hold.recv_timeout(std::time::Duration::from_secs(10))
                    .map_err(|_| Error::Internal("fixture release timed out"))?;
                match outcome {
                    0 => Ok(()),
                    1 => Err(Error::Conflict("generated stopped operation".into())),
                    _ => panic!("generated blocking-task failure"),
                }
            })?;
            ready
                .await
                .map_err(|_| Error::Internal("fixture did not start"))?;
            runtime.stop_accepting();
            assert!(!runtime.status()?.enabled);
            assert!(runtime.status()?.busy);
            assert!(matches!(runtime.permit(), Err(Error::Busy(_))));
            assert!(matches!(
                runtime.search("unused".into()).await,
                Err(Error::Busy(_))
            ));
            assert!(matches!(
                runtime.resume_publication(
                    3,
                    2,
                    ResumePublication {
                        normalization_run: 7,
                        acknowledge_new_only_publication: true,
                    }
                ),
                Err(Error::Busy(_))
            ));
            let draining = runtime.clone();
            let mut task = tokio::spawn(async move { draining.drain().await });
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(30), &mut task)
                    .await
                    .is_err()
            );
            release
                .send(())
                .map_err(|_| Error::Internal("fixture release failed"))?;
            tokio::time::timeout(std::time::Duration::from_secs(3), task)
                .await
                .map_err(|_| Error::Internal("drain timed out"))?
                .map_err(|_| Error::Internal("drain task failed"))??;
            let status = runtime.status()?;
            assert!(!status.enabled);
            assert!(!status.busy);
            if outcome == 0 {
                assert!(status.last_error.is_none());
            } else {
                assert!(
                    status
                        .last_error
                        .ok_or(Error::Internal("missing result"))?
                        .contains(if outcome == 1 {
                            "generated stopped operation"
                        } else {
                            "task failed"
                        })
                );
            }
            runtime.drain().await?;
            assert!(matches!(runtime.permit(), Err(Error::Busy(_))));
        }
        Ok(())
    }
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
            cache: None,
            slot: Arc::new(Semaphore::new(1)),
            last_error: Arc::new(Mutex::new(None)),
            stopping: AtomicBool::new(false),
        })
    }

    pub fn status(&self) -> Result<LocalStatus> {
        Ok(LocalStatus {
            enabled: !self.stopping.load(Ordering::Acquire),
            busy: self.slot.available_permits() == 0,
            last_error: self
                .last_error
                .lock()
                .map_err(|_| Error::Internal("local status lock failed"))?
                .clone(),
        })
    }

    pub(crate) fn with_cache(
        mut self,
        cache: Option<Arc<dyn crate::application::services::cache::Cache>>,
    ) -> Self {
        self.cache = cache;
        self
    }

    fn permit(&self) -> Result<OwnedSemaphorePermit> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(Error::Busy("local operations are stopping"));
        }
        let permit = self.slot.clone().try_acquire_owned().map_err(|_| {
            Error::Busy("a local source, download or publication operation is already running")
        })?;
        if self.stopping.load(Ordering::Acquire) {
            return Err(Error::Busy("local operations are stopping"));
        }
        Ok(permit)
    }

    pub(crate) fn reload_permit(&self) -> Result<OwnedSemaphorePermit> {
        self.permit()
    }

    /// Reject new local effects immediately. Never interrupt an active receive,
    /// publication copy or its durable result persistence.
    pub(crate) fn stop_accepting(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    /// Wait without a forced timeout. Supervisors must not interpret slow
    /// draining as permission to reset uncertain submissions.
    pub(crate) async fn drain(&self) -> Result<()> {
        self.stop_accepting();
        let _idle = self
            .slot
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Internal("local operation slot closed during shutdown"))?;
        Ok(())
    }

    pub async fn search(
        &self,
        release_id: String,
    ) -> Result<super::soulseek::search::SearchReport> {
        let permit = self.permit()?;
        let cache = self.cache.clone();
        let (config, database, services) = (
            self.configuration.clone(),
            self.database.clone(),
            self.services.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            config.search_with_cache(&database, services, release_id, cache)
        })
        .await
        .map_err(|_| Error::Internal("source search task failed"))?
    }

    pub async fn folder(
        &self,
        request: SourceFolderRequest,
    ) -> Result<super::worker::SourceFolderReport> {
        let permit = self.permit()?;
        let cache = self.cache.clone();
        let (config, database, services) = (
            self.configuration.clone(),
            self.database.clone(),
            self.services.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            config.folder_with_cache(&database, services, request, cache)
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
        let (config, database, services) = (
            self.configuration.clone(),
            self.database.clone(),
            self.services.clone(),
        );
        self.start_operation(job, version, "Download", move || {
            config
                .acquire_and_publish(&database, services, job, version, request)
                .map(|_| ())
        })
    }

    pub fn resume_publication(
        &self,
        job: i64,
        version: i64,
        request: ResumePublication,
    ) -> Result<()> {
        if job <= 0
            || version <= 0
            || request.normalization_run <= 0
            || !request.acknowledge_new_only_publication
        {
            return Err(Error::InvalidInput(
                "publication recovery requires positive IDs and explicit new-only acknowledgement"
                    .into(),
            ));
        }
        let (database, staging) = (
            self.database.clone(),
            self.configuration.staging_root.clone(),
        );
        self.start_operation(job, version, "Publication recovery", move || {
            super::worker::resume_publication(&database, &staging, job, version, request)
                .map(|_| ())
        })
    }

    fn start_operation(
        &self,
        job: i64,
        version: i64,
        operation: &'static str,
        run: impl FnOnce() -> Result<()> + Send + 'static,
    ) -> Result<()> {
        let permit = self.permit()?;
        *self
            .last_error
            .lock()
            .map_err(|_| Error::Internal("local status lock failed"))? = None;
        let last_error = self.last_error.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = tokio::task::spawn_blocking(run).await;
            let message = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => {
                    eprintln!("{operation} {job}/{version} stopped: {error}");
                    Some(format!(
                        "{operation} {job}/{version} stopped: {error}. Saved files and receipts were kept. Do not retry an uncertain transfer."
                    ))
                }
                Err(error) => {
                    eprintln!("{operation} task failed: {error}");
                    Some(format!(
                        "{operation} task failed. Inspect saved state before another action."
                    ))
                }
            };
            match last_error.lock() {
                Ok(mut status) => *status = message,
                Err(_) => eprintln!("{operation} status lock failed"),
            }
        });
        Ok(())
    }
}
