use super::publication::AlbumReplacementRestore;
use super::{ReplacementFile, Store};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Fresh native evidence for one destination. Unverified previews cannot approve
/// work; their file lists are saved evidence only, not a claim of current state.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct AlbumRestorePreview {
    pub job_id: i64,
    pub plan_version: i64,
    pub location_id: i64,
    pub destination_id: i64,
    pub review_id: i64,
    pub root: String,
    pub root_identity: String,
    pub original_folder: String,
    pub retained_folder: String,
    pub restore_folder: String,
    pub state: String,
    pub retained_album_identity: String,
    pub current_album_identity: String,
    pub retained_files: Vec<ReplacementFile>,
    pub current_files: Vec<ReplacementFile>,
    pub verified: bool,
    pub unavailable_reason: Option<String>,
    pub preview_sha256: Option<String>,
    pub status: Option<AlbumRestoreStatus>,
}

impl AlbumRestorePreview {
    pub(crate) fn digest(&self) -> Result<String> {
        let mut binding = self.clone();
        binding.preview_sha256 = None;
        binding.status = None;
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&binding)?)
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct AlbumRestoreStatus {
    pub last_error: Option<String>,
    pub question_id: Option<i64>,
}

pub(super) fn status(connection: &Connection, id: i64) -> Result<Option<AlbumRestoreStatus>> {
    Ok(connection
        .query_row(
            "SELECT last_error,question_id FROM album_restore_status WHERE destination_id=?1",
            [id],
            |row| {
                Ok(AlbumRestoreStatus {
                    last_error: row.get(0)?,
                    question_id: row.get(1)?,
                })
            },
        )
        .optional()?)
}

impl Store {
    pub(crate) fn approve_album_restore(
        &mut self,
        restore: &AlbumReplacementRestore,
        preview: &AlbumRestorePreview,
    ) -> Result<()> {
        let digest = preview
            .preview_sha256
            .as_ref()
            .filter(|_| preview.verified)
            .ok_or_else(|| Error::Conflict("restore needs a verified preview".into()))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = transaction.execute(
            "INSERT INTO album_restore_approvals
             (destination_id,preview_sha256,album_identity,current_files)
             SELECT ?1,?2,?3,?4 WHERE EXISTS(
               SELECT 1 FROM publication_replacement_journal r
               JOIN publication_destinations d ON d.id=r.destination_id
               JOIN jobs j ON j.id=d.job_id
               WHERE r.destination_id=?1 AND r.state='published'
               AND r.review_id=?5 AND j.state='complete' AND j.plan_version=d.plan_version)",
            params![
                restore.destination_id,
                digest,
                preview.current_album_identity,
                serde_json::to_string(&preview.current_files)?,
                restore.review_id,
            ],
        )?;
        if inserted != 1
            || transaction.execute(
                "UPDATE publication_replacement_journal
                 SET state='restore_prepared',restore_started_at=unixepoch()
                 WHERE destination_id=?1 AND state='published'",
                [restore.destination_id],
            )? != 1
        {
            return Err(Error::Conflict("restore approval changed".into()));
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn album_restore_result(
        &mut self,
        destination: i64,
        error: Option<&str>,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: Option<i64> = transaction
            .query_row(
                "SELECT question_id FROM album_restore_questions
                 WHERE destination_id=?1 AND resolved_at IS NULL",
                [destination],
                |row| row.get(0),
            )
            .optional()?;
        let question = if error.is_some() {
            if let Some(id) = active {
                Some(id)
            } else {
                let prompt = "Album restore needs review. Open the request to inspect this destination and its saved restore state. No files will change when you save this answer.";
                transaction.execute(
                    "INSERT INTO questions(job_id,plan_version,paused_state,prompt,choices)
                     SELECT job_id,plan_version,'complete',?2,?3
                     FROM publication_destinations WHERE id=?1",
                    params![destination, prompt, r#"["Review restore state"]"#],
                )?;
                let id = transaction.last_insert_rowid();
                transaction.execute(
                    "INSERT INTO album_restore_questions(question_id,destination_id) VALUES (?1,?2)",
                    params![id, destination],
                )?;
                Some(id)
            }
        } else {
            transaction.execute(
                "UPDATE album_restore_questions SET resolved_at=unixepoch()
                 WHERE destination_id=?1 AND resolved_at IS NULL",
                [destination],
            )?;
            None
        };
        transaction.execute(
            "INSERT INTO album_restore_status(destination_id,last_error,question_id)
             VALUES (?1,?2,?3) ON CONFLICT(destination_id)
             DO UPDATE SET last_error=excluded.last_error,question_id=excluded.question_id",
            params![
                destination,
                error.map(|text| text.chars().take(2048).collect::<String>()),
                question
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }
}
