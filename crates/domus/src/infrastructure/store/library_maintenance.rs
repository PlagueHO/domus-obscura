use super::Store;
use crate::domain::library::{
    LibraryComparison, LibraryDifference, LibraryFolderOption, LibraryScanPolicy, LibrarySetup,
    SaveLibraryScanPolicy,
};
use crate::domain::{LibraryPath, Page};
use crate::infrastructure::filesystem::StorageRole;
use crate::infrastructure::production_worker::{WorkerOperation, WorkerRequest};
use crate::{Error, Result};
use rusqlite::{TransactionBehavior, params};

impl Store {
    pub(super) fn describe_location(
        &self,
        mut location: super::Location,
    ) -> Result<super::Location> {
        if let Some(id) = location.root.strip_prefix("storage-root:") {
            if let Some(filesystem) = &self.filesystem {
                location.scan_only = filesystem.media_scan_only(id)?.then_some(true);
                location.display_path = Some(
                    filesystem
                        .root_path(id, StorageRole::Media)?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        } else if let Some(id) = location.root.strip_prefix("paired-media-root:") {
            location.display_path = Some(format!("Paired media folder: {id}"));
        } else {
            location.display_path = Some(location.root.clone());
        }
        Ok(location)
    }

    /// Describe only operator-selected folders. No candidate folder is opened.
    pub fn library_folder_options(&self) -> Result<Vec<LibraryFolderOption>> {
        let locations = self.locations(Page::new(0, 100)?)?;
        let mut folders = Vec::new();
        if let Some(filesystem) = &self.filesystem {
            for id in filesystem.media_root_ids() {
                let path = filesystem.root_path(id, StorageRole::Media)?;
                folders.push(LibraryFolderOption {
                    root_id: id.into(),
                    display_path: path.to_string_lossy().into_owned(),
                    paired: false,
                    location_id: self.primary_library_target(&format!("storage-root:{id}"))?,
                });
            }
        }
        if let Some(media) = &self.paired_media {
            for id in media.root_ids() {
                folders.push(LibraryFolderOption {
                    root_id: id.into(),
                    display_path: format!("Paired media folder: {id}"),
                    paired: true,
                    location_id: locations
                        .iter()
                        .find(|location| location.root == format!("paired-media-root:{id}"))
                        .map(|location| location.id),
                });
            }
        }
        Ok(folders)
    }

    pub fn library_setup(&self) -> Result<LibrarySetup> {
        let primary = self
            .locations(Page::new(0, 100)?)?
            .into_iter()
            .find(|location| location.kind == "main");
        let primary_error = primary.as_ref().and_then(|location| {
            crate::application::check_location_availability(self, location.id)
                .err()
                .map(|error| error.to_string())
        });
        Ok(LibrarySetup {
            primary_id: primary.map(|location| location.id),
            primary_error,
            output_configured: self.settings()?.is_some(),
            folders: self.library_folder_options()?,
        })
    }

    pub fn register_selected_library_folder(
        &mut self,
        root_id: &str,
        paired: bool,
        kind: &str,
    ) -> Result<super::Location> {
        let selected = self
            .library_folder_options()?
            .into_iter()
            .find(|folder| folder.root_id == root_id && folder.paired == paired)
            .ok_or_else(|| Error::Conflict(
                "This folder is not selected in the host storage configuration. An operator must select and validate it with the API and workers stopped, then restart them before registration.".into(),
            ))?;
        if selected.location_id.is_some() {
            return Err(Error::Conflict(
                "This library folder is already registered.".into(),
            ));
        }
        let id = if paired {
            self.add_paired_location(root_id, kind)?
        } else {
            let path = self
                .filesystem
                .as_ref()
                .ok_or(Error::Internal("selected filesystem is missing"))?
                .root_path(root_id, StorageRole::Media)?;
            self.add_location(
                path.to_str().ok_or_else(|| {
                    Error::InvalidInput("library folder path is not valid Unicode".into())
                })?,
                kind,
            )?
        };
        self.location(id)
    }

    pub fn primary_library_id(&self) -> Result<i64> {
        self.locations(Page::new(0, 100)?)?
            .into_iter()
            .find(|location| location.kind == "main")
            .map(|location| location.id)
            .ok_or_else(|| Error::Conflict("Configure a primary library before scanning.".into()))
    }

    pub fn library_scan_policy(&self) -> Result<LibraryScanPolicy> {
        Ok(self.connection.query_row(
            "SELECT p.version,p.interval_minutes,s.next_due,s.last_request,s.last_error
             FROM library_scan_settings p CROSS JOIN library_scan_schedule s
             WHERE s.id=1 ORDER BY p.version DESC LIMIT 1",
            [],
            |row| {
                Ok(LibraryScanPolicy {
                    version: row.get(0)?,
                    interval_minutes: row.get(1)?,
                    next_due: row.get(2)?,
                    last_request: row.get(3)?,
                    last_error: row.get(4)?,
                })
            },
        )?)
    }

    pub fn save_library_scan_policy(
        &mut self,
        policy: &SaveLibraryScanPolicy,
    ) -> Result<LibraryScanPolicy> {
        if policy
            .interval_minutes
            .is_some_and(|minutes| !(60..=10080).contains(&minutes))
        {
            return Err(Error::InvalidInput(
                "Scan intervals must be 60 through 10080 minutes.".into(),
            ));
        }
        self.primary_library_id()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = transaction.query_row(
            "SELECT MAX(version) FROM library_scan_settings",
            [],
            |row| row.get(0),
        )?;
        if policy.expected_version != version {
            return Err(Error::Conflict(
                "The scan schedule changed. Reload it before saving.".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO library_scan_settings(version,interval_minutes) VALUES (?1,?2)",
            params![version + 1, policy.interval_minutes],
        )?;
        transaction.execute(
            "UPDATE library_scan_schedule SET next_due=CASE WHEN ?1 IS NULL THEN NULL
             ELSE unixepoch()+?1*60 END,last_error=NULL WHERE id=1",
            [policy.interval_minutes],
        )?;
        transaction.commit()?;
        self.library_scan_policy()
    }

    pub fn enqueue_library_scan(&mut self, capacity: u16, binding: &str) -> Result<WorkerRequest> {
        let location_id = self.primary_library_id()?;
        self.enqueue_worker(
            WorkerOperation::ScanLibrary { location_id },
            capacity,
            binding,
        )
    }

    pub(crate) fn library_scan_due(&self) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT next_due IS NOT NULL AND next_due<=unixepoch() FROM library_scan_schedule WHERE id=1",
            [], |row| row.get(0),
        )?)
    }

    /// The supervisor holds the host lock. A retry delay is committed before
    /// queue submission, whose target is idempotent after interruption.
    pub(crate) fn schedule_library_scan(
        &mut self,
        capacity: u16,
        binding: &str,
        editable: Result<()>,
    ) -> Result<()> {
        let claimed = self.connection.execute(
            "UPDATE library_scan_schedule SET next_due=unixepoch()+60
             WHERE id=1 AND next_due<=unixepoch()",
            [],
        )?;
        if claimed == 0 {
            return Ok(());
        }
        let result = editable.and_then(|()| self.enqueue_library_scan(capacity, binding));
        match result {
            Ok(request)
                if matches!(request.state.as_str(), "queued" | "running" | "interrupted") =>
            {
                self.connection.execute(
                    "UPDATE library_scan_schedule SET next_due=unixepoch()+
                     (SELECT interval_minutes*60 FROM library_scan_settings ORDER BY version DESC LIMIT 1),
                     last_request=?1,last_error=NULL WHERE id=1",
                    [request.id],
                )?;
                Ok(())
            }
            result => {
                let error = match result {
                    Ok(_) => Error::Conflict(
                        "A previous library scan needs an explicit retry or dismissal.".into(),
                    ),
                    Err(error) => error,
                };
                self.connection.execute(
                    "UPDATE library_scan_schedule SET last_error=?1 WHERE id=1",
                    [error.to_string()],
                )?;
                Err(error)
            }
        }
    }

    pub fn compare_library_backup(
        &self,
        backup_id: i64,
        after: &str,
        limit: u16,
    ) -> Result<LibraryComparison> {
        LibraryPath::parse(after)?;
        Page::new(0, limit)?;
        let snapshot = self.connection.unchecked_transaction()?;
        let primary_id = self.primary_library_id()?;
        if self.location(backup_id)?.kind != "backup" {
            return Err(Error::InvalidInput(
                "Select a backup library for comparison.".into(),
            ));
        }
        let root = LibraryPath::parse("")?;
        let primary = self.library_coverage(primary_id, &root)?;
        let backup = self.library_coverage(backup_id, &root)?;
        let mut differences = Vec::new();
        if primary.coverage == "complete" && backup.coverage == "complete" {
            let mut statement = self.connection.prepare_cached(
                "SELECT p.relative_path,CASE WHEN b.id IS NULL THEN 'missing_in_backup'
                 WHEN p.kind<>b.kind THEN 'entry_type_differs' ELSE 'file_size_differs' END
                 FROM entries p LEFT JOIN entries b ON b.location_id=?2
                   AND b.relative_path=p.relative_path AND b.present=1
                 WHERE p.location_id=?1 AND p.present=1 AND p.relative_path>?3
                   AND (b.id IS NULL OR p.kind<>b.kind OR p.size IS NOT b.size)
                 UNION ALL
                 SELECT b.relative_path,'only_in_backup' FROM entries b
                 WHERE b.location_id=?2 AND b.present=1 AND b.relative_path>?3
                   AND NOT EXISTS(SELECT 1 FROM entries p WHERE p.location_id=?1
                     AND p.relative_path=b.relative_path AND p.present=1)
                 ORDER BY 1 LIMIT ?4",
            )?;
            differences = statement
                .query_map(params![primary_id, backup_id, after, limit], |row| {
                    Ok(LibraryDifference {
                        path: row.get(0)?,
                        difference: row.get(1)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
        }
        snapshot.commit()?;
        Ok(LibraryComparison {
            primary,
            backup,
            differences,
        })
    }
}

#[cfg(test)]
#[path = "library_maintenance_tests.rs"]
mod tests;
