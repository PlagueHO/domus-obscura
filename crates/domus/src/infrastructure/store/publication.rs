use super::{
    JobClaim, NormalizationRun, NormalizationWork, Store, StoredPlan, artwork, check_job_owner,
    normalization, planning,
};
use crate::domain::{JobState, LibraryPath, Page};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct PublicationDestination {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub run_id: i64,
    pub location_id: i64,
    pub root: String,
    pub root_identity: String,
    pub kind: String,
    pub relative_folder: String,
    pub manifest_sha256: String,
    pub album_identity: Option<String>,
    pub verified_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct PublicationFile {
    pub ordinal: u16,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub verified_at: Option<i64>,
}

fn conflict() -> Error {
    Error::Conflict("publication requires its current approved plan, completed normalization and exact destination receipts".into())
}

fn owner(connection: &Connection, claim: &JobClaim) -> Result<()> {
    let state: String =
        connection.query_row("SELECT state FROM jobs WHERE id=?1", [claim.job], |row| {
            row.get(0)
        })?;
    check_job_owner(
        connection,
        claim,
        match state.as_str() {
            "publishing" => JobState::Publishing,
            "backup_pending" => JobState::BackupPending,
            _ => return Err(conflict()),
        },
    )
}

fn snapshot(
    connection: &Connection,
    claim: &JobClaim,
    run: i64,
) -> Result<(NormalizationRun, StoredPlan)> {
    owner(connection, claim)?;
    let plan = planning::acquisition_plan(connection, claim.job, claim.plan_version)?;
    let run = normalization::read_run(connection, run)?;
    if run.job_id != claim.job
        || run.plan_version != claim.plan_version
        || run.completed_at.is_none()
        || run.cover_verified_at.is_none()
        || run.files == 0
        || run.files != run.verified_files
        || usize::from(run.files) != plan.plan.outputs.len()
    {
        return Err(conflict());
    }
    artwork::check_cover(
        connection,
        claim.job,
        claim.plan_version,
        &run.cover_sha256,
        run.cover_bytes,
        (run.cover_width, run.cover_height),
    )?;
    artwork::load_jpeg(
        connection,
        claim.job,
        claim.plan_version,
        artwork::approved(connection, claim.job, claim.plan_version)?.id,
    )?;
    Ok((run, plan))
}

fn destination(connection: &Connection, id: i64) -> Result<PublicationDestination> {
    Ok(connection.query_row(
        "SELECT job_id,plan_version,run_id,location_id,root,root_identity,kind,relative_folder,
          manifest_sha256,album_identity,verified_at,last_error FROM publication_destinations WHERE id=?1",
        [id],|row| Ok(PublicationDestination {
            id,job_id:row.get(0)?,plan_version:row.get(1)?,run_id:row.get(2)?,location_id:row.get(3)?,
            root:row.get(4)?,root_identity:row.get(5)?,kind:row.get(6)?,relative_folder:row.get(7)?,
            manifest_sha256:row.get(8)?,album_identity:row.get(9)?,verified_at:row.get(10)?,last_error:row.get(11)?,
        }),
    )?)
}

fn bound(connection: &Connection, claim: &JobClaim, id: i64) -> Result<PublicationDestination> {
    let saved = destination(connection, id)?;
    let (_, plan) = snapshot(connection, claim, saved.run_id)?;
    if saved.job_id != claim.job
        || saved.plan_version != claim.plan_version
        || !plan.plan.destinations.iter().any(|d| {
            d.location_id == saved.location_id && d.root == saved.root && d.kind == saved.kind
        })
        || plan
            .plan
            .outputs
            .iter()
            .any(|output| output.relative_folder != saved.relative_folder)
    {
        return Err(conflict());
    }
    let identity: String = connection.query_row(
        "SELECT identity FROM filesystem_roots WHERE root=?1 AND kind='media'",
        [&saved.root],
        |row| row.get(0),
    )?;
    if identity != saved.root_identity {
        return Err(conflict());
    }
    Ok(saved)
}

impl Store {
    /// Publication may reclaim a completed normalization from an older worker.
    /// Its immutable run/receipts are retained; no claim authorizes rewriting them.
    pub(crate) fn publication_source(
        &self,
        claim: &JobClaim,
        run: i64,
    ) -> Result<(NormalizationRun, StoredPlan, Vec<NormalizationWork>)> {
        let (saved, plan) = snapshot(&self.connection, claim, run)?;
        let mut work = Vec::new();
        let mut after = 0;
        loop {
            let page = self.read_normalization_work(claim, run, Page::new(after, 200)?)?;
            if page.is_empty() {
                break;
            }
            after = i64::from(page.last().ok_or_else(conflict)?.track_number);
            work.extend(page);
            if work.len() > 1000 {
                return Err(conflict());
            }
        }
        if work.len() != usize::from(saved.files) {
            return Err(conflict());
        }
        Ok((saved, plan, work))
    }

    /// Persist all create-new intents before destination I/O. No destination
    /// absence is inferred from the index and this never authorizes replacement.
    pub(crate) fn begin_publication(
        &mut self,
        claim: &JobClaim,
        run: i64,
    ) -> Result<Vec<PublicationDestination>> {
        let (saved, plan, work) = self.publication_source(claim, run)?;
        let folder = &plan
            .plan
            .outputs
            .first()
            .ok_or_else(conflict)?
            .relative_folder;
        LibraryPath::parse(folder)?;
        let mut names = HashSet::new();
        let mut files = vec![PublicationFile {
            ordinal: 0,
            name: "folder.jpg".into(),
            bytes: saved.cover_bytes,
            sha256: saved.cover_sha256.clone(),
            verified_at: None,
        }];
        names.insert("folder.jpg".to_uppercase());
        for (index, (item, output)) in work.iter().zip(&plan.plan.outputs).enumerate() {
            LibraryPath::parse(&output.filename)?;
            let receipt = item.receipt.as_ref().ok_or_else(conflict)?;
            if output.relative_folder != *folder
                || output.filename.contains('/')
                || output.filename.is_empty()
                || !names.insert(output.filename.to_uppercase())
                || item.track_number as usize != index + 1
                || receipt.bytes == 0
                || receipt.bytes > i64::MAX as u64
            {
                return Err(conflict());
            }
            files.push(PublicationFile {
                ordinal: item.track_number,
                name: output.filename.clone(),
                bytes: receipt.bytes,
                sha256: receipt.sha256.clone(),
                verified_at: None,
            });
        }
        let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&files)?));
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        snapshot(&transaction, claim, run)?;
        super::native_roots::require_bound_roots(&transaction)?;
        let mut ids = Vec::new();
        for target in &plan.plan.destinations {
            let root_identity: String = transaction.query_row(
                "SELECT identity FROM filesystem_roots WHERE root=?1 AND kind='media'",
                [&target.root],
                |row| row.get(0),
            )?;
            let existing: Option<i64> = transaction.query_row(
                "SELECT id FROM publication_destinations WHERE job_id=?1 AND plan_version=?2 AND location_id=?3",
                params![claim.job,claim.plan_version,target.location_id],|row| row.get(0),
            ).optional()?;
            let id = if let Some(id) = existing {
                let prior = destination(&transaction, id)?;
                if prior.run_id != run
                    || prior.manifest_sha256 != hash
                    || prior.relative_folder != *folder
                    || prior.root != target.root
                    || prior.root_identity != root_identity
                    || prior.kind != target.kind
                {
                    return Err(conflict());
                }
                id
            } else {
                transaction.execute(
                    "INSERT INTO publication_destinations(job_id,plan_version,run_id,location_id,root,root_identity,kind,relative_folder,folder_key,manifest_sha256)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![claim.job,claim.plan_version,run,target.location_id,target.root,root_identity,target.kind,folder,folder.to_uppercase(),hash],
                )?;
                let id = transaction.last_insert_rowid();
                for file in &files {
                    transaction.execute("INSERT INTO publication_files(destination_id,ordinal,name,bytes,sha256) VALUES (?1,?2,?3,?4,?5)",
                        params![id,file.ordinal,file.name,file.bytes as i64,file.sha256])?;
                }
                transaction.execute("INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'publication_planned')",params![claim.job,claim.plan_version])?;
                id
            };
            ids.push(id);
        }
        transaction.commit()?;
        ids.into_iter()
            .map(|id| destination(&self.connection, id))
            .collect()
    }

    pub fn publication_destinations(
        &self,
        job: i64,
        version: i64,
    ) -> Result<Vec<PublicationDestination>> {
        planning::read_plan(&self.connection, job, version)?;
        let ids = self.connection.prepare("SELECT id FROM publication_destinations WHERE job_id=?1 AND plan_version=?2 ORDER BY kind DESC,id LIMIT 16")?
            .query_map(params![job,version],|row| row.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
        ids.into_iter()
            .map(|id| destination(&self.connection, id))
            .collect()
    }

    pub(crate) fn check_publication(
        &self,
        claim: &JobClaim,
        id: i64,
    ) -> Result<PublicationDestination> {
        bound(&self.connection, claim, id)
    }

    pub(crate) fn publication_files(
        &self,
        claim: &JobClaim,
        id: i64,
    ) -> Result<Vec<PublicationFile>> {
        let saved = bound(&self.connection, claim, id)?;
        let files = self.connection.prepare("SELECT ordinal,name,bytes,sha256,verified_at FROM publication_files WHERE destination_id=?1 ORDER BY ordinal LIMIT 1002")?
            .query_map([id],|row| Ok(PublicationFile {ordinal:row.get(0)?,name:row.get(1)?,
                bytes:u64::try_from(row.get::<_,i64>(2)?).map_err(|error| rusqlite::Error::FromSqlConversionFailure(2,rusqlite::types::Type::Integer,Box::new(error)))?,
                sha256:row.get(3)?,verified_at:row.get(4)?}))?
            .collect::<std::result::Result<Vec<_>,_>>()?;
        let mut intent = files.clone();
        for file in &mut intent {
            file.verified_at = None;
        }
        if files.is_empty()
            || files.len() > 1001
            || format!("{:x}", Sha256::digest(serde_json::to_vec(&intent)?))
                != saved.manifest_sha256
        {
            return Err(conflict());
        }
        Ok(files)
    }

    pub(crate) fn bind_publication_album(
        &mut self,
        claim: &JobClaim,
        id: i64,
        identity: &str,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        bound(&transaction, claim, id)?;
        if identity.is_empty() || identity.len() > 256 {
            return Err(conflict());
        }
        if transaction.execute("UPDATE publication_destinations SET album_identity=?2 WHERE id=?1 AND album_identity IS NULL",params![id,identity])?!=1 {
            return Err(conflict());
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn record_publication_file(
        &mut self,
        claim: &JobClaim,
        id: i64,
        file: &PublicationFile,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if bound(&transaction, claim, id)?.album_identity.is_none() {
            return Err(conflict());
        }
        if transaction.execute(
            "UPDATE publication_files SET verified_at=COALESCE(verified_at,unixepoch()) WHERE destination_id=?1
             AND ordinal=?2 AND name=?3 AND bytes=?4 AND sha256=?5",
            params![id,file.ordinal,file.name,file.bytes as i64,file.sha256],
        )?!=1 { return Err(conflict()); }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn finish_publication_destination(
        &mut self,
        claim: &JobClaim,
        id: i64,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved = bound(&transaction, claim, id)?;
        let missing: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM publication_files WHERE destination_id=?1 AND verified_at IS NULL)",[id],|row| row.get(0))?;
        let count: i64 = transaction.query_row(
            "SELECT count(*) FROM publication_files WHERE destination_id=?1",
            [id],
            |row| row.get(0),
        )?;
        let run = normalization::read_run(&transaction, saved.run_id)?;
        if missing || count != i64::from(run.files) + 1 || saved.album_identity.is_none() {
            return Err(conflict());
        }
        transaction.execute("UPDATE publication_destinations SET verified_at=COALESCE(verified_at,unixepoch()),last_error=NULL WHERE id=?1",[id])?;
        if saved.verified_at.is_none() {
            transaction.execute("INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'destination_verified')",params![claim.job,claim.plan_version])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn publication_error(
        &mut self,
        claim: &JobClaim,
        id: i64,
        message: &str,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owner(&transaction, claim)?;
        let saved = destination(&transaction, id)?;
        if saved.job_id != claim.job || saved.plan_version != claim.plan_version {
            return Err(conflict());
        }
        let mut text = message.chars().take(512).collect::<String>();
        if text.is_empty() {
            text = "publication failed".into();
        }
        transaction.execute(
            "UPDATE publication_destinations SET last_error=?2 WHERE id=?1",
            params![id, text],
        )?;
        transaction.execute(
            "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'publication_error')",
            params![claim.job, claim.plan_version],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn finish_publication(&mut self, claim: &JobClaim, run: i64) -> Result<String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (_, plan) = snapshot(&transaction, claim, run)?;
        let (count,primary,verified): (i64,i64,i64) = transaction.query_row(
            "SELECT count(*),COALESCE(sum(kind='main' AND verified_at IS NOT NULL AND last_error IS NULL),0),
             COALESCE(sum(verified_at IS NOT NULL AND last_error IS NULL),0)
             FROM publication_destinations WHERE job_id=?1 AND plan_version=?2 AND run_id=?3",
            params![claim.job,claim.plan_version,run],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        )?;
        if count != plan.plan.destinations.len() as i64 || primary != 1 {
            return Err(conflict());
        }
        let state = if count == verified {
            "complete"
        } else {
            "backup_pending"
        };
        transaction.execute(
            "UPDATE jobs SET state=?2,owner=NULL,lease_until=NULL WHERE id=?1",
            params![claim.job, state],
        )?;
        transaction.execute(
            "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,?3)",
            params![claim.job, claim.plan_version, state],
        )?;
        transaction.commit()?;
        Ok(state.into())
    }
}
