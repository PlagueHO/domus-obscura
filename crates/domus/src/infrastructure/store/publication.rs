use super::{
    JobClaim, NormalizationRun, NormalizationWork, Store, StoredPlan, artwork, check_job_owner,
    normalization, planning,
};
use crate::domain::{JobState, LibraryPath, Page, ReplacementAction};
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
    pub replacement: Option<PublicationDestinationReplacement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
pub struct PublicationDestinationReplacement {
    pub review_id: i64,
    pub original_folder: String,
    pub original_album_identity: String,
    pub original_files: Vec<ReplacementFile>,
    pub retained_folder: String,
    pub restore_folder: String,
    pub state: String,
    pub restore_status: Option<super::AlbumRestoreStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct PublicationFile {
    pub ordinal: u16,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub verified_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct ReplacementFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct AlbumReplacementPreview {
    pub job_id: i64,
    pub plan_version: i64,
    pub location_id: i64,
    pub root: String,
    pub root_identity: String,
    pub kind: String,
    pub relative_folder: String,
    pub output_files: Vec<String>,
    pub destination_available: bool,
    pub unavailable_reason: Option<String>,
    pub album_identity: Option<String>,
    pub manifest_sha256: String,
    pub existing_files: Vec<ReplacementFile>,
    pub decision_saved: bool,
    pub saved_action: Option<ReplacementAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct AlbumReplacementDecision {
    pub action: ReplacementAction,
    pub expected_manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct AlbumReplacementRestoreDecision {
    pub review_id: i64,
    pub expected_manifest_sha256: String,
    pub expected_album_identity: String,
    #[serde(default)]
    pub expected_preview_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct AlbumReplacementReview {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub location_id: i64,
    pub root: String,
    pub root_identity: String,
    pub relative_folder: String,
    pub album_identity: String,
    pub manifest_sha256: String,
    pub action: ReplacementAction,
    pub output_folder: Option<String>,
    pub approved_at: i64,
    pub files: Vec<ReplacementFile>,
}

pub(crate) struct AlbumReplacementRestore {
    pub(crate) destination_id: i64,
    pub(crate) root: String,
    pub(crate) root_identity: String,
    pub(crate) original_folder: String,
    pub(crate) original_album_identity: String,
    pub(crate) original_files: Vec<ReplacementFile>,
    pub(crate) retained_folder: String,
    pub(crate) restore_folder: String,
    pub(crate) replacement_album_identity: String,
    pub(crate) replacement_files: Vec<ReplacementFile>,
    pub(crate) state: String,
    pub(crate) review_id: i64,
    pub(crate) approved_preview: Option<String>,
}

pub(crate) struct PublicationPreparation {
    pub(crate) folder: String,
    pub(crate) published_at: Option<i64>,
    pub(crate) atomic: bool,
}

fn conflict() -> Error {
    Error::Conflict("publication requires its current approved plan, completed normalization and exact destination receipts".into())
}

pub(crate) fn replacement_hash(preview: &AlbumReplacementPreview) -> Result<String> {
    let body = serde_json::to_vec(&(
        preview.job_id,
        preview.plan_version,
        preview.location_id,
        &preview.root,
        &preview.root_identity,
        &preview.kind,
        &preview.relative_folder,
        &preview.output_files,
        &preview.album_identity,
        &preview.existing_files,
    ))?;
    Ok(format!("{:x}", Sha256::digest(body)))
}

type ReplacementReviewRow = (
    i64,
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
);

fn review(connection: &Connection, id: i64) -> Result<AlbumReplacementReview> {
    let (
        job_id,
        plan_version,
        location_id,
        root,
        root_identity,
        relative_folder,
        album_identity,
        manifest_sha256,
        action,
        output_folder,
        approved_at,
    ): ReplacementReviewRow = connection.query_row(
        "SELECT job_id,plan_version,location_id,root,root_identity,relative_folder,album_identity,
            manifest_sha256,action,output_folder,approved_at
         FROM album_replacement_reviews WHERE id=?1",
        [id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        },
    )?;
    let action = match action.as_str() {
        "skip" => ReplacementAction::Skip,
        "copy" => ReplacementAction::Copy,
        "replace" => ReplacementAction::Replace,
        _ => return Err(Error::Internal("invalid replacement action in database")),
    };
    let mut statement = connection.prepare(
        "SELECT name,bytes,sha256 FROM album_replacement_review_files
         WHERE review_id=?1 ORDER BY ordinal LIMIT 1002",
    )?;
    let files = statement
        .query_map([id], |row| {
            Ok(ReplacementFile {
                name: row.get(0)?,
                bytes: u64::try_from(row.get::<_, i64>(1)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?,
                sha256: row.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if files.len() > 1001 {
        return Err(conflict());
    }
    Ok(AlbumReplacementReview {
        id,
        job_id,
        plan_version,
        location_id,
        root,
        root_identity,
        relative_folder,
        album_identity,
        manifest_sha256,
        action,
        output_folder,
        approved_at,
        files,
    })
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
    let mut saved = connection.query_row(
        "SELECT job_id,plan_version,run_id,location_id,root,root_identity,kind,relative_folder,
          manifest_sha256,album_identity,verified_at,last_error FROM publication_destinations WHERE id=?1",
        [id],|row| Ok(PublicationDestination {
            id,job_id:row.get(0)?,plan_version:row.get(1)?,run_id:row.get(2)?,location_id:row.get(3)?,
            root:row.get(4)?,root_identity:row.get(5)?,kind:row.get(6)?,relative_folder:row.get(7)?,
            manifest_sha256:row.get(8)?,album_identity:row.get(9)?,verified_at:row.get(10)?,last_error:row.get(11)?,
            replacement:None,
        }),
    )?;
    let replacement: Option<(i64, String, String, String, String, String)> = connection
        .query_row(
            "SELECT j.review_id,r.relative_folder,r.album_identity,j.retained_folder,
                    j.restore_folder,j.state
             FROM publication_replacement_journal j
             JOIN album_replacement_reviews r ON r.id=j.review_id
             WHERE j.destination_id=?1",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    saved.replacement = replacement
        .map(
            |(
                review_id,
                original_folder,
                original_album_identity,
                retained_folder,
                restore_folder,
                state,
            )| {
                let review = review(connection, review_id)?;
                if review.album_identity != original_album_identity
                    || review.relative_folder != original_folder
                {
                    return Err(conflict());
                }
                Ok(PublicationDestinationReplacement {
                    review_id,
                    original_folder,
                    original_album_identity,
                    original_files: review.files,
                    retained_folder,
                    restore_folder,
                    state,
                    restore_status: super::restore::status(connection, id)?,
                })
            },
        )
        .transpose()?;
    Ok(saved)
}

fn bound(connection: &Connection, claim: &JobClaim, id: i64) -> Result<PublicationDestination> {
    let saved = destination(connection, id)?;
    let (_, plan) = snapshot(connection, claim, saved.run_id)?;
    let plan_folder = plan
        .plan
        .outputs
        .first()
        .ok_or_else(conflict)?
        .relative_folder
        .as_str();
    let copied_folder: Option<String> = connection
        .query_row(
            "SELECT output_folder FROM album_replacement_reviews
             WHERE job_id=?1 AND plan_version=?2 AND location_id=?3 AND action='copy'
             ORDER BY id DESC LIMIT 1",
            params![claim.job, claim.plan_version, saved.location_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if saved.job_id != claim.job
        || saved.plan_version != claim.plan_version
        || !plan.plan.destinations.iter().any(|d| {
            d.location_id == saved.location_id && d.root == saved.root && d.kind == saved.kind
        })
        || (saved.relative_folder != plan_folder
            && copied_folder.as_deref() != Some(saved.relative_folder.as_str()))
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
    pub fn album_replacement_reviews(
        &self,
        job: i64,
        version: i64,
    ) -> Result<Vec<AlbumReplacementReview>> {
        planning::read_plan(&self.connection, job, version)?;
        let ids = self
            .connection
            .prepare(
                "SELECT id FROM album_replacement_reviews
                 WHERE job_id=?1 AND plan_version=?2
                   AND id IN (
                     SELECT max(id) FROM album_replacement_reviews
                     WHERE job_id=?1 AND plan_version=?2 GROUP BY location_id
                   )
                 ORDER BY location_id LIMIT 16",
            )?
            .query_map(params![job, version], |row| row.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| review(&self.connection, id))
            .collect()
    }

    pub fn record_album_replacement_decision(
        &mut self,
        preview: &AlbumReplacementPreview,
        decision: AlbumReplacementDecision,
    ) -> Result<AlbumReplacementReview> {
        let hash = replacement_hash(preview)?;
        if !preview.destination_available
            || hash != preview.manifest_sha256
            || hash != decision.expected_manifest_sha256
            || preview.album_identity.is_none()
            || preview.existing_files.len() > 1001
            || preview.output_files.is_empty()
            || preview.output_files.len() > 1001
        {
            return Err(conflict());
        }
        LibraryPath::parse(&preview.relative_folder)?;
        let mut names = HashSet::new();
        for file in &preview.existing_files {
            LibraryPath::parse(&file.name)?;
            if file.name.contains('/')
                || file.sha256.len() != 64
                || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !names.insert(file.name.to_uppercase())
            {
                return Err(conflict());
            }
        }
        let output_folder = match decision.action {
            ReplacementAction::Copy => {
                let name = preview
                    .relative_folder
                    .rsplit('/')
                    .next()
                    .ok_or_else(conflict)?;
                let parent = preview
                    .relative_folder
                    .rsplit_once('/')
                    .map_or("", |(parent, _)| parent);
                let copy = format!("{name} (copy {}-{})", preview.job_id, preview.plan_version);
                let folder = if parent.is_empty() {
                    copy
                } else {
                    format!("{parent}/{copy}")
                };
                LibraryPath::parse(&folder)?;
                Some(folder)
            }
            ReplacementAction::Skip | ReplacementAction::Replace => None,
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plan = planning::read_plan(&transaction, preview.job_id, preview.plan_version)?;
        if plan.approved_at.is_none()
            || !plan.plan.destinations.iter().any(|target| {
                target.location_id == preview.location_id
                    && target.root == preview.root
                    && target.kind == preview.kind
            })
            || plan
                .plan
                .outputs
                .iter()
                .any(|output| output.relative_folder != preview.relative_folder)
        {
            return Err(conflict());
        }
        let state: String = transaction.query_row(
            "SELECT state FROM jobs WHERE id=?1 AND plan_version=?2",
            params![preview.job_id, preview.plan_version],
            |row| row.get(0),
        )?;
        if state != "queued" {
            return Err(conflict());
        }
        let root_identity: String = transaction.query_row(
            "SELECT identity FROM filesystem_roots WHERE root=?1 AND kind='media'",
            [&preview.root],
            |row| row.get(0),
        )?;
        if root_identity != preview.root_identity {
            return Err(conflict());
        }
        let mut output_files = vec!["folder.jpg".to_owned()];
        output_files.extend(
            plan.plan
                .outputs
                .iter()
                .map(|output| output.filename.clone()),
        );
        if output_files != preview.output_files {
            return Err(conflict());
        }
        let album_identity = preview.album_identity.as_deref().ok_or_else(conflict)?;
        if album_identity.is_empty() || album_identity.len() > 256 {
            return Err(conflict());
        }
        transaction.execute(
            "INSERT INTO album_replacement_reviews
             (job_id,plan_version,location_id,root,root_identity,relative_folder,album_identity,
              manifest_sha256,action,output_folder)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                preview.job_id,
                preview.plan_version,
                preview.location_id,
                preview.root,
                preview.root_identity,
                preview.relative_folder,
                album_identity,
                hash,
                match decision.action {
                    ReplacementAction::Skip => "skip",
                    ReplacementAction::Copy => "copy",
                    ReplacementAction::Replace => "replace",
                },
                output_folder
            ],
        )?;
        let id = transaction.last_insert_rowid();
        for (ordinal, file) in preview.existing_files.iter().enumerate() {
            transaction.execute(
                "INSERT INTO album_replacement_review_files(review_id,ordinal,name,bytes,sha256)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    id,
                    ordinal as i64,
                    file.name,
                    file.bytes as i64,
                    file.sha256
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO events(job_id,plan_version,event_type)
             VALUES (?1,?2,'album_replacement_reviewed')",
            params![preview.job_id, preview.plan_version],
        )?;
        if decision.action == ReplacementAction::Skip {
            transaction.execute(
                "UPDATE jobs SET state='cancelled',owner=NULL,lease_until=NULL
                 WHERE id=?1 AND plan_version=?2 AND state='queued'",
                params![preview.job_id, preview.plan_version],
            )?;
        }
        transaction.commit()?;
        review(&self.connection, id)
    }

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
            let replacement: Option<AlbumReplacementReview> = transaction
                .query_row(
                    "SELECT id FROM album_replacement_reviews
                     WHERE job_id=?1 AND plan_version=?2 AND location_id=?3 ORDER BY id DESC LIMIT 1",
                    params![claim.job, claim.plan_version, target.location_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .map(|id| review(&transaction, id))
                .transpose()?;
            let destination_folder = match replacement.as_ref().map(|item| item.action) {
                Some(ReplacementAction::Copy) => replacement
                    .as_ref()
                    .and_then(|item| item.output_folder.clone())
                    .ok_or_else(conflict)?,
                Some(ReplacementAction::Skip) => return Err(conflict()),
                _ => folder.clone(),
            };
            if let Some(review) = replacement.as_ref()
                && (review.root != target.root
                    || review.root_identity != root_identity
                    || review.relative_folder != *folder)
            {
                return Err(conflict());
            }
            let existing: Option<i64> = transaction.query_row(
                "SELECT id FROM publication_destinations WHERE job_id=?1 AND plan_version=?2 AND location_id=?3",
                params![claim.job,claim.plan_version,target.location_id],|row| row.get(0),
            ).optional()?;
            let id = if let Some(id) = existing {
                let prior = destination(&transaction, id)?;
                if prior.run_id != run
                    || prior.manifest_sha256 != hash
                    || prior.relative_folder != destination_folder
                    || prior.root != target.root
                    || prior.root_identity != root_identity
                    || prior.kind != target.kind
                {
                    return Err(conflict());
                }
                id
            } else {
                transaction.execute(
                    "INSERT INTO publication_destinations(job_id,plan_version,run_id,location_id,root,root_identity,kind,relative_folder,folder_key,manifest_sha256,atomic)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1)",
                    params![claim.job,claim.plan_version,run,target.location_id,target.root,root_identity,target.kind,destination_folder,destination_folder.to_uppercase(),hash],
                )?;
                let id = transaction.last_insert_rowid();
                for file in &files {
                    transaction.execute("INSERT INTO publication_files(destination_id,ordinal,name,bytes,sha256) VALUES (?1,?2,?3,?4,?5)",
                        params![id,file.ordinal,file.name,file.bytes as i64,file.sha256])?;
                }
                transaction.execute("INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'publication_planned')",params![claim.job,claim.plan_version])?;
                if let Some(review) = replacement.as_ref()
                    && review.action == ReplacementAction::Replace
                {
                    let parent = folder.rsplit_once('/').map_or("", |(parent, _)| parent);
                    let name = format!(
                        ".domus-retained-{}-{}-{}-{}",
                        claim.job, claim.plan_version, target.location_id, review.id
                    );
                    let restore_name = format!(
                        ".domus-restore-{}-{}-{}-{}",
                        claim.job, claim.plan_version, target.location_id, review.id
                    );
                    let retained = if parent.is_empty() {
                        name
                    } else {
                        format!("{parent}/{name}")
                    };
                    let restore = if parent.is_empty() {
                        restore_name
                    } else {
                        format!("{parent}/{restore_name}")
                    };
                    LibraryPath::parse(&retained)?;
                    LibraryPath::parse(&restore)?;
                    transaction.execute(
                        "INSERT INTO publication_replacement_journal
                         (destination_id,review_id,retained_folder,restore_folder,state)
                         VALUES (?1,?2,?3,?4,'planned')",
                        params![id, review.id, retained, restore],
                    )?;
                }
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

    /// Save a fresh hidden sibling path before creating a prepared directory.
    /// An unbound directory is never reopened after a crash.
    pub(crate) fn prepare_publication_path(
        &mut self,
        claim: &JobClaim,
        id: i64,
    ) -> Result<PublicationPreparation> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved = bound(&transaction, claim, id)?;
        let (mut folder, mut attempt, published_at, atomic): (
            Option<String>,
            i64,
            Option<i64>,
            bool,
        ) = transaction.query_row(
            "SELECT prepared_folder,prepare_attempt,published_at,atomic
                 FROM publication_destinations WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        if let Some(folder) = folder.as_ref()
            && (saved.album_identity.is_some() || published_at.is_some())
        {
            transaction.commit()?;
            return Ok(PublicationPreparation {
                folder: folder.clone(),
                published_at,
                atomic,
            });
        }

        if attempt >= 10_000 {
            return Err(conflict());
        }
        attempt = attempt.checked_add(1).ok_or_else(conflict)?;
        let parent = saved
            .relative_folder
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        let name = format!(".domus-pending-{}-{attempt}", saved.id);
        folder = Some(if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        });
        LibraryPath::parse(folder.as_deref().ok_or_else(conflict)?)?;
        if transaction.execute(
            "UPDATE publication_destinations SET prepared_folder=?2,prepare_attempt=?3
             WHERE id=?1 AND (album_identity IS NULL OR prepared_folder IS NULL)",
            params![id, folder, attempt],
        )? != 1
        {
            return Err(conflict());
        }
        transaction.commit()?;
        Ok(PublicationPreparation {
            folder: folder.ok_or_else(conflict)?,
            published_at,
            atomic,
        })
    }

    pub(crate) fn publication_preparation(
        &self,
        claim: &JobClaim,
        id: i64,
    ) -> Result<PublicationPreparation> {
        bound(&self.connection, claim, id)?;
        let (folder, published_at, atomic): (Option<String>, Option<i64>, bool) =
            self.connection.query_row(
                "SELECT prepared_folder,published_at,atomic
                 FROM publication_destinations WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        let folder = folder.ok_or_else(conflict)?;
        LibraryPath::parse(&folder)?;
        Ok(PublicationPreparation {
            folder,
            published_at,
            atomic,
        })
    }

    pub(crate) fn record_publication_replacement_state(
        &mut self,
        claim: &JobClaim,
        id: i64,
        state: &str,
    ) -> Result<()> {
        let (expected, timestamp) = match state {
            "retained" => ("planned", "retained_at"),
            "published" => ("retained", "published_at"),
            _ => return Err(conflict()),
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved = bound(&transaction, claim, id)?;
        if saved.replacement.is_none() {
            return Err(conflict());
        }
        let changed = transaction.execute(
            &format!(
                "UPDATE publication_replacement_journal
                 SET state=?2,{timestamp}=COALESCE({timestamp},unixepoch())
                 WHERE destination_id=?1 AND state=?3"
            ),
            params![id, state, expected],
        )?;
        if changed != 1 {
            let current: String = transaction.query_row(
                "SELECT state FROM publication_replacement_journal WHERE destination_id=?1",
                [id],
                |row| row.get(0),
            )?;
            if current != state {
                return Err(conflict());
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn prepare_album_replacement_restore(
        &mut self,
        job: i64,
        version: i64,
        location_id: i64,
        decision: Option<&AlbumReplacementRestoreDecision>,
    ) -> Result<AlbumReplacementRestore> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plan = planning::read_plan(&transaction, job, version)?;
        if plan.approved_at.is_none() {
            return Err(conflict());
        }
        let job_state: String = transaction.query_row(
            "SELECT state FROM jobs WHERE id=?1 AND plan_version=?2",
            params![job, version],
            |row| row.get(0),
        )?;
        if job_state != "complete" {
            return Err(Error::Conflict(
                "restore requires publication to be complete at every destination".into(),
            ));
        }
        let id: i64 = transaction.query_row(
            "SELECT id FROM publication_destinations
             WHERE job_id=?1 AND plan_version=?2 AND location_id=?3",
            params![job, version, location_id],
            |row| row.get(0),
        )?;
        let saved = destination(&transaction, id)?;
        let replacement = saved.replacement.as_ref().ok_or_else(conflict)?;
        if saved.verified_at.is_none()
            || saved.last_error.is_some()
            || decision.is_some_and(|decision| {
                saved.album_identity.as_deref() != Some(decision.expected_album_identity.as_str())
                    || saved.manifest_sha256 != decision.expected_manifest_sha256
                    || replacement.review_id != decision.review_id
            })
            || !matches!(
                replacement.state.as_str(),
                "published"
                    | "restore_prepared"
                    | "restore_current_moved"
                    | "restore_original_visible"
                    | "restored"
            )
        {
            return Err(conflict());
        }
        let review = review(&transaction, replacement.review_id)?;
        if review.action != ReplacementAction::Replace
            || review.job_id != job
            || review.plan_version != version
            || review.location_id != location_id
            || review.root != saved.root
            || review.root_identity != saved.root_identity
        {
            return Err(conflict());
        }
        let files = transaction
            .prepare(
                "SELECT ordinal,name,bytes,sha256,verified_at
                 FROM publication_files WHERE destination_id=?1 ORDER BY ordinal LIMIT 1002",
            )?
            .query_map([id], |row| {
                Ok(PublicationFile {
                    ordinal: row.get(0)?,
                    name: row.get(1)?,
                    bytes: u64::try_from(row.get::<_, i64>(2)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    sha256: row.get(3)?,
                    verified_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut intent = files.clone();
        for file in &mut intent {
            file.verified_at = None;
        }
        if files.is_empty()
            || files.len() > 1001
            || files.iter().any(|file| file.verified_at.is_none())
            || format!("{:x}", Sha256::digest(serde_json::to_vec(&intent)?))
                != saved.manifest_sha256
        {
            return Err(conflict());
        }
        let state = replacement.state.clone();
        let root = saved.root.clone();
        let root_identity = saved.root_identity.clone();
        let original_folder = replacement.original_folder.clone();
        let original_album_identity = replacement.original_album_identity.clone();
        let original_files = replacement.original_files.clone();
        let retained_folder = replacement.retained_folder.clone();
        let restore_folder = replacement.restore_folder.clone();
        let approval: Option<(String, String, String)> = transaction
            .query_row(
                "SELECT preview_sha256,album_identity,current_files
                 FROM album_restore_approvals WHERE destination_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let replacement_album_identity = saved.album_identity.clone().ok_or_else(conflict)?;
        let replacement_files = files
            .into_iter()
            .map(|file| ReplacementFile {
                name: file.name,
                bytes: file.bytes,
                sha256: file.sha256,
            })
            .collect();
        let (approved_preview, replacement_album_identity, replacement_files) = match approval {
            Some((digest, identity, body)) => {
                (Some(digest), identity, serde_json::from_str(&body)?)
            }
            None => (None, replacement_album_identity, replacement_files),
        };
        transaction.commit()?;
        Ok(AlbumReplacementRestore {
            destination_id: id,
            root,
            root_identity,
            original_folder,
            original_album_identity,
            original_files,
            retained_folder,
            restore_folder,
            replacement_album_identity,
            replacement_files,
            state,
            review_id: replacement.review_id,
            approved_preview,
        })
    }

    pub(crate) fn record_album_replacement_restore_state(
        &mut self,
        id: i64,
        expected: &str,
        state: &str,
    ) -> Result<()> {
        let valid = matches!(
            (expected, state),
            ("published", "restore_prepared")
                | ("restore_prepared", "restore_current_moved")
                | ("restore_current_moved", "restore_original_visible")
                | ("restore_original_visible", "restored")
        );
        if !valid {
            return Err(conflict());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: String = transaction.query_row(
            "SELECT state FROM publication_replacement_journal WHERE destination_id=?1",
            [id],
            |row| row.get(0),
        )?;
        if current == state {
            transaction.commit()?;
            return Ok(());
        }
        if current != expected
            || transaction.execute(
                "UPDATE publication_replacement_journal
                 SET state=?2,
                     restore_started_at=CASE WHEN ?2='restore_prepared'
                                             THEN COALESCE(restore_started_at,unixepoch())
                                             ELSE restore_started_at END,
                     restored_at=CASE WHEN ?2='restored' THEN COALESCE(restored_at,unixepoch())
                                      ELSE restored_at END
                 WHERE destination_id=?1 AND state=?3",
                params![id, state, expected],
            )? != 1
        {
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
        let published: bool = transaction.query_row(
            "SELECT published_at IS NOT NULL FROM publication_destinations WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        let count: i64 = transaction.query_row(
            "SELECT count(*) FROM publication_files WHERE destination_id=?1",
            [id],
            |row| row.get(0),
        )?;
        let run = normalization::read_run(&transaction, saved.run_id)?;
        if missing
            || count != i64::from(run.files) + 1
            || saved.album_identity.is_none()
            || !published
            || saved
                .replacement
                .as_ref()
                .is_some_and(|replacement| replacement.state != "published")
        {
            return Err(conflict());
        }
        transaction.execute("UPDATE publication_destinations SET verified_at=COALESCE(verified_at,unixepoch()),last_error=NULL WHERE id=?1",[id])?;
        if saved.verified_at.is_none() {
            transaction.execute("INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'destination_verified')",params![claim.job,claim.plan_version])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn record_publication_visible(&mut self, claim: &JobClaim, id: i64) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved = bound(&transaction, claim, id)?;
        if saved.album_identity.is_none()
            || saved
                .replacement
                .as_ref()
                .is_some_and(|replacement| replacement.state != "published")
        {
            return Err(conflict());
        }
        let missing: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM publication_files
             WHERE destination_id=?1 AND verified_at IS NULL)",
            [id],
            |row| row.get(0),
        )?;
        if missing {
            return Err(conflict());
        }
        transaction.execute(
            "UPDATE publication_destinations SET published_at=COALESCE(published_at,unixepoch())
             WHERE id=?1",
            [id],
        )?;
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
            "SELECT count(*),COALESCE(sum(kind='main' AND verified_at IS NOT NULL AND published_at IS NOT NULL AND last_error IS NULL),0),
             COALESCE(sum(verified_at IS NOT NULL AND published_at IS NOT NULL AND last_error IS NULL),0)
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
