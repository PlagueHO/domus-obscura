use super::Store;
use crate::{Error, Result};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

/// Only approval and activation metadata are retained in SQLite. Neither
/// account names, passwords nor credential hashes belong in this journal.
#[derive(Serialize, ts_rs::TS)]
pub struct SoulseekAccountChange {
    pub id: i64,
    pub state: String,
    pub configuration_revision: Option<String>,
    pub last_error: Option<String>,
}

impl Store {
    pub(crate) fn pending_soulseek_account_change(&self) -> Result<Option<i64>> {
        Ok(self.connection.query_row(
            "SELECT id FROM soulseek_account_changes WHERE state IN ('preparing','approved','activating')",
            [], |row| row.get(0),
        ).optional()?)
    }

    pub(crate) fn soulseek_account_change(&self, id: i64) -> Result<SoulseekAccountChange> {
        self.connection.query_row(
            "SELECT id,state,configuration_revision,last_error FROM soulseek_account_changes WHERE id=?1",
            [id],
            |row| Ok(SoulseekAccountChange {
                id: row.get(0)?, state: row.get(1)?,
                configuration_revision: row.get(2)?, last_error: row.get(3)?,
            }),
        ).optional()?.ok_or_else(|| Error::InvalidInput("Soulseek account change was not found.".into()))
    }

    pub(crate) fn reserve_soulseek_account(&mut self, revision: &str) -> Result<i64> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::primary_library::require_idle(&transaction)?;
        let blocked: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM primary_library_changes WHERE state IN ('approved','activating'))
             OR EXISTS(SELECT 1 FROM soulseek_account_changes WHERE state IN ('preparing','approved','activating'))
             OR (SELECT COUNT(*) FROM soulseek_account_changes)>=100",
            [], |row| row.get(0),
        )?;
        if blocked {
            return Err(Error::Conflict("Finish or cancel the pending host change before saving another account. At most 100 account changes can be retained.".into()));
        }
        transaction.execute(
            "INSERT INTO soulseek_account_changes(expected_revision) VALUES (?1)",
            [revision],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(id)
    }

    pub(crate) fn approve_soulseek_account(&self, id: i64, identity: &str) -> Result<()> {
        if self.connection.execute(
            "UPDATE soulseek_account_changes SET state='approved',file_identity=?2 WHERE id=?1 AND state='preparing'",
            params![id, identity],
        )? != 1 {
            return Err(Error::Conflict("The account save is no longer preparing.".into()));
        }
        Ok(())
    }

    pub(crate) fn soulseek_account_intent(&self, id: i64) -> Result<(String, Option<String>)> {
        Ok(self.connection.query_row(
            "SELECT expected_revision,file_identity FROM soulseek_account_changes WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    pub(crate) fn begin_soulseek_account(&mut self, id: i64, revision: &str) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::primary_library::require_idle(&transaction)?;
        if transaction.execute(
            "UPDATE soulseek_account_changes SET state='activating',candidate_revision=?2 WHERE id=?1 AND state='approved'", params![id, revision],
        )? != 1 {
            return Err(Error::Conflict("The requested account change is no longer approved.".into()));
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn fail_soulseek_account(&self, id: i64, message: &str) -> Result<()> {
        if self.connection.execute(
            "UPDATE soulseek_account_changes SET state='failed',last_error=?2,completed_at=unixepoch()
             WHERE id=?1 AND state IN ('preparing','approved','activating')",
            params![id, message],
        )? != 1 {
            return Err(Error::Conflict("The account change is no longer pending.".into()));
        }
        Ok(())
    }

    pub(crate) fn complete_soulseek_account(&self, id: i64, revision: &str) -> Result<()> {
        if self.soulseek_account_change(id)?.state == "active" {
            return Ok(());
        }
        if self.connection.execute(
            "UPDATE soulseek_account_changes SET state='active',configuration_revision=?2,completed_at=unixepoch()
             WHERE id=?1 AND state='activating' AND candidate_revision=?2", params![id, revision],
        )? != 1 {
            return Err(Error::Conflict("The account has no matching activation approval.".into()));
        }
        Ok(())
    }

    pub(crate) fn cancel_soulseek_account(&self, id: i64) -> Result<SoulseekAccountChange> {
        if self.connection.execute(
            "UPDATE soulseek_account_changes SET state='failed',last_error='Cancelled before activation.',completed_at=unixepoch()
             WHERE id=?1 AND state IN ('preparing','approved')", [id],
        )? != 1 {
            return Err(Error::Conflict("Only an account change that has not started activation can be cancelled.".into()));
        }
        self.soulseek_account_change(id)
    }
}
