use super::{Store, native_roots};
use crate::domain::library::PrimaryLibraryChange;
use crate::infrastructure::filesystem::StorageRole;
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

fn current_primary(connection: &Connection) -> Result<Option<i64>> {
    Ok(connection
        .query_row(
            "SELECT id FROM locations WHERE kind='main' AND retired=0",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

pub(super) fn require_idle(connection: &Connection) -> Result<()> {
    let unfinished: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE state NOT IN ('complete','cancelled'))
         OR EXISTS(SELECT 1 FROM worker_requests WHERE state!='complete')
         OR EXISTS(SELECT 1 FROM scans WHERE status='running')
         OR EXISTS(SELECT 1 FROM recovery_operations WHERE state!='complete')
         OR EXISTS(SELECT 1 FROM scratch_cleanup_admissions s WHERE NOT EXISTS(
            SELECT 1 FROM recovery_operations o WHERE o.entry_id=s.run_id*8+1 AND o.state='complete'))
         OR EXISTS(SELECT 1 FROM publication_replacement_journal
            WHERE state NOT IN ('published','restored','planned'))
         OR EXISTS(SELECT 1 FROM transfer_files WHERE state IN ('submitting','active'))",
        [],
        |row| row.get(0),
    )?;
    if unfinished {
        return Err(Error::Conflict(
            "Finish, cancel, or recover pending jobs, scans, and recovery work before changing host settings.".into(),
        ));
    }
    Ok(())
}

impl Store {
    pub(crate) fn primary_library_target(&self, root: &str) -> Result<Option<i64>> {
        let id = self
            .connection
            .query_row("SELECT id FROM locations WHERE root=?1", [root], |row| {
                row.get(0)
            })
            .optional()?;
        if id.is_none() {
            let count: i64 =
                self.connection
                    .query_row("SELECT COUNT(*) FROM locations", [], |row| row.get(0))?;
            if count >= 100 {
                return Err(Error::Conflict(
                    "At most 100 current and previous library folders can be retained.".into(),
                ));
            }
        }
        Ok(id)
    }

    pub(crate) fn primary_library_selection(&self) -> Result<Option<i64>> {
        current_primary(&self.connection)
    }

    pub(crate) fn require_primary_library_idle(&self) -> Result<()> {
        require_idle(&self.connection)
    }

    pub(crate) fn approve_primary_library_change(
        &mut self,
        body: &str,
        revision: &str,
        previous: Option<i64>,
    ) -> Result<PrimaryLibraryChange> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_idle(&transaction)?;
        let account_pending: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM soulseek_account_changes WHERE state IN ('preparing','approved','activating'))",
            [], |row| row.get(0),
        )?;
        if account_pending {
            return Err(Error::Conflict("Finish or cancel the pending Soulseek account change before changing library folders.".into()));
        }
        if current_primary(&transaction)? != previous {
            return Err(Error::Conflict(
                "The primary library changed. Review the location again.".into(),
            ));
        }
        let pending: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM primary_library_changes
             WHERE state IN ('approved','activating'))",
            [],
            |row| row.get(0),
        )?;
        if pending {
            return Err(Error::Conflict(
                "A library folder change is already pending. Check its result before retrying."
                    .into(),
            ));
        }
        transaction.execute(
            "INSERT INTO primary_library_changes(body,expected_revision,previous_location_id)
             VALUES (?1,?2,?3)",
            params![body, revision, previous],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.commit()?;
        self.primary_library_change(id)
    }

    pub fn primary_library_change(&self, id: i64) -> Result<PrimaryLibraryChange> {
        Ok(self.connection.query_row(
            "SELECT id,state,location_id,configuration_revision,last_error,json_extract(body,'$.path')
             FROM primary_library_changes WHERE id=?1",
            [id],
            |row| {
                Ok(PrimaryLibraryChange {
                    id: row.get(0)?,
                    path: row.get(5)?,
                    state: row.get(1)?,
                    location_id: row.get(2)?,
                    configuration_revision: row.get(3)?,
                    last_error: row.get(4)?,
                })
            },
        )?)
    }

    pub(crate) fn pending_primary_library_change(&self) -> Result<Option<i64>> {
        Ok(self.pending_library_change()?.map(|(id, _)| id))
    }

    pub(crate) fn pending_library_change(&self) -> Result<Option<(i64, bool)>> {
        Ok(self
            .connection
            .query_row(
                "SELECT id, COALESCE(json_extract(body,'$.backup'),0)
                 FROM primary_library_changes WHERE state IN ('approved','activating')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub(crate) fn primary_library_change_intent(
        &self,
        id: i64,
    ) -> Result<(String, String, Option<i64>)> {
        Ok(self.connection.query_row(
            "SELECT body,expected_revision,previous_location_id FROM primary_library_changes WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?)
    }

    pub(crate) fn begin_primary_library_change(&mut self, id: i64) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_idle(&transaction)?;
        let previous: Option<i64> = transaction.query_row(
            "SELECT previous_location_id FROM primary_library_changes
             WHERE id=?1 AND state='approved'",
            [id],
            |row| row.get(0),
        )?;
        if current_primary(&transaction)? != previous {
            return Err(Error::Conflict("The primary library changed.".into()));
        }
        transaction.execute(
            "UPDATE primary_library_changes SET state='activating' WHERE id=?1",
            [id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn fail_primary_library_change(&self, id: i64, error: &str) -> Result<()> {
        if self.connection.execute(
            "UPDATE primary_library_changes SET state='failed',last_error=?2,completed_at=unixepoch()
             WHERE id=?1 AND state IN ('approved','activating')",
            params![id, error],
        )? != 1
        {
            return Err(Error::Conflict(
                "The library folder change is no longer pending.".into(),
            ));
        }

        Ok(())
    }

    pub(crate) fn cancel_primary_library_change(&self, id: i64) -> Result<PrimaryLibraryChange> {
        if self.connection.execute(
            "UPDATE primary_library_changes SET state='failed',last_error='Cancelled before activation.',
             completed_at=unixepoch() WHERE id=?1 AND state='approved'",
            [id],
        )? != 1 {
            return Err(Error::Conflict("Only a saved change that has not started can be cancelled.".into()));
        }
        self.primary_library_change(id)
    }

    pub(crate) fn library_activation_matches(
        &self,
        root_id: &str,
        location_id: Option<i64>,
        previous: Option<i64>,
        backup: bool,
    ) -> Result<bool> {
        let Some(id) = location_id else {
            return Ok(false);
        };
        let primary = if backup { previous } else { location_id };
        if self.primary_library_selection()? != primary || primary.is_none() {
            return Ok(false);
        }
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM locations WHERE id=?1 AND root=?2 AND kind=?3 AND retired=0)",
            params![id, format!("storage-root:{root_id}"), if backup { "backup" } else { "main" }],
            |row| row.get(0),
        )?)
    }

    /// Registration authority and native I/O are prepared before the transaction.
    /// Old locations and bindings remain available to history and recovery.
    pub(crate) fn activate_primary_library(
        &mut self,
        change_id: i64,
        root_id: &str,
        revision: &str,
        backup: bool,
    ) -> Result<()> {
        let saved = self.primary_library_change(change_id)?;
        let (_, _, previous) = self.primary_library_change_intent(change_id)?;
        if saved.state == "active" {
            if !self.library_activation_matches(root_id, saved.location_id, previous, backup)? {
                return Err(Error::Conflict(
                    "The saved library folder activation differs from this host.".into(),
                ));
            }
            return Ok(());
        }
        if saved.state != "activating" {
            return Err(Error::Conflict(
                "The library folder has no matching activation approval.".into(),
            ));
        }
        let filesystem = self
            .filesystem
            .as_ref()
            .ok_or(Error::Internal("selected filesystem is missing"))?;
        let lease = filesystem.open(root_id, StorageRole::Media)?;
        let binding =
            native_roots::SavedRoot::Logical(filesystem.binding(root_id, StorageRole::Media)?);
        let root = binding.key("");
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_idle(&transaction)?;
        if current_primary(&transaction)? != previous || (backup && previous.is_none()) {
            return Err(Error::Conflict("The primary library changed.".into()));
        }
        let existing: Option<i64> = transaction
            .query_row("SELECT id FROM locations WHERE root=?1", [&root], |row| {
                row.get(0)
            })
            .optional()?;
        if backup && existing.is_some() {
            return Err(Error::Conflict(
                "This folder already has a library location.".into(),
            ));
        }
        native_roots::register(
            &transaction,
            &root,
            "media",
            &binding,
            self.filesystem.as_deref(),
            self.paired_media.as_deref(),
        )?;
        if !backup {
            transaction.execute(
                "UPDATE locations SET retired=1 WHERE kind='main' AND retired=0",
                [],
            )?;
        }
        let location_id = if let Some(id) = existing {
            transaction.execute(
                "UPDATE locations SET kind='main',retired=0 WHERE id=?1",
                [id],
            )?;
            id
        } else {
            let count: i64 =
                transaction.query_row("SELECT COUNT(*) FROM locations", [], |row| row.get(0))?;
            if count >= 100 {
                return Err(Error::Conflict(
                    "At most 100 current and previous library folders can be retained.".into(),
                ));
            }
            transaction.execute(
                "INSERT INTO locations(root,kind) VALUES (?1,?2)",
                params![root, if backup { "backup" } else { "main" }],
            )?;
            transaction.last_insert_rowid()
        };
        if !backup {
            transaction.execute(
                "UPDATE library_scan_schedule SET last_request=NULL,last_error=NULL,
             next_due=(SELECT CASE WHEN interval_minutes IS NULL THEN NULL
                 ELSE unixepoch()+interval_minutes*60 END
                 FROM library_scan_settings ORDER BY version DESC LIMIT 1) WHERE id=1",
                [],
            )?;
        }
        if transaction.execute(
            "UPDATE primary_library_changes SET state='active',location_id=?2,
             configuration_revision=?3,completed_at=unixepoch()
             WHERE id=?1 AND state='activating'",
            params![change_id, location_id, revision],
        )? != 1
        {
            return Err(Error::Conflict(
                "The library folder activation changed.".into(),
            ));
        }
        transaction.commit()?;
        drop(lease);
        Ok(())
    }
}
