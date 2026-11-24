use super::{JobClaim, Store, artwork, check_job_owner, planning, transfers};
use crate::domain::{JobState, OptionalTags, Page, TrackTags};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct TagReviewTrack {
    pub track_number: u16,
    pub transfer_id: i64,
    pub input_bytes: i64,
    pub input_sha256: String,
    pub source: OptionalTags,
    pub output: OptionalTags,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct TagReview {
    pub policy: crate::domain::TagPolicy,
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub question_id: i64,
    pub sha256: String,
    pub approved: bool,
    pub resumed: bool,
    pub tracks: Vec<TagReviewTrack>,
}

fn conflict() -> Error {
    Error::Conflict("tag review requires its exact current plan, inputs, cover and approval".into())
}

pub(super) fn resolved_tags(
    connection: &Connection,
    job: i64,
    version: i64,
) -> Result<Vec<TrackTags>> {
    resolve_tags(connection, job, version, true)
}

fn resolve_tags(
    connection: &Connection,
    job: i64,
    version: i64,
    require_resumed: bool,
) -> Result<Vec<TrackTags>> {
    let plan = planning::read_plan(connection, job, version)?;
    let mut tags = plan
        .plan
        .catalog
        .as_ref()
        .ok_or_else(conflict)?
        .data
        .audio_tags()?;
    if let Some(policy) = &plan.plan.settings.tag_policy {
        policy.validate()?;
        if policy.needs_review() {
            let (id, cover, approved): (i64, String, bool) = connection
                .query_row(
                    "SELECT id,cover_sha256,approved_at IS NOT NULL AND resumed_at IS NOT NULL
                 FROM tag_reviews WHERE job_id=?1 AND plan_version=?2",
                    params![job, version],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?
                .ok_or_else(conflict)?;
            if (require_resumed && !approved)
                || artwork::approved(connection, job, version)?.sha256 != cover
            {
                return Err(conflict());
            }
            let mut tracks = Vec::new();
            loop {
                let after = tracks
                    .last()
                    .map_or(0, |track: &TagReviewTrack| i64::from(track.track_number));
                let page = read_tracks(connection, id, Page::new(after, 200)?)?;
                if page.is_empty() {
                    break;
                }
                tracks.extend(page);
                if tracks.len() > 1000 {
                    return Err(conflict());
                }
            }
            if tracks.len() != tags.len() {
                return Err(conflict());
            }
            for (index, (tags, track)) in tags.iter_mut().zip(tracks).enumerate() {
                let valid: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM transfer_files WHERE id=?1 AND job_id=?2
                     AND plan_version=?3 AND track_number=?4 AND expected_bytes=?5
                     AND lower(hex(payload_sha256))=?6 AND state='received')",
                    params![
                        track.transfer_id,
                        job,
                        version,
                        track.track_number,
                        track.input_bytes,
                        track.input_sha256
                    ],
                    |row| row.get(0),
                )?;
                if !valid
                    || usize::from(track.track_number) != index + 1
                    || policy.resolve_for_format(&track.source, plan.plan.format)? != track.output
                {
                    return Err(conflict());
                }
                tags.optional_tags = Some(track.output);
            }
        } else {
            let output = policy.resolve_for_format(&OptionalTags::default(), plan.plan.format)?;
            for tags in &mut tags {
                tags.optional_tags = Some(output.clone());
            }
        }
    }
    Ok(tags)
}

fn read_tracks(connection: &Connection, id: i64, page: Page) -> Result<Vec<TagReviewTrack>> {
    let mut statement = connection.prepare(
        "SELECT track_number,transfer_id,input_bytes,input_sha256,source_body,output_body
         FROM tag_review_tracks WHERE review_id=?1 AND track_number>?2 ORDER BY track_number LIMIT ?3",
    )?;
    let rows = statement
        .query_map(params![id, page.after(), page.limit()], |row| {
            Ok((
                row.get::<_, u16>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(
            |(track_number, transfer_id, input_bytes, input_sha256, source, output)| {
                Ok(TagReviewTrack {
                    track_number,
                    transfer_id,
                    input_bytes,
                    input_sha256,
                    source: serde_json::from_str(&source)?,
                    output: serde_json::from_str(&output)?,
                })
            },
        )
        .collect()
}

impl Store {
    pub(crate) fn tag_review_inputs(&self, claim: &JobClaim) -> Result<Vec<super::TransferFile>> {
        check_job_owner(&self.connection, claim, JobState::Validating)?;
        transfers::check_received(&self.connection, claim.job, claim.plan_version)?;
        self.reconciliation_preview(claim.job, claim.plan_version)
    }

    pub fn tag_review(&self, job: i64, version: i64, page: Page) -> Result<Option<TagReview>> {
        let row = self
            .connection
            .query_row(
                "SELECT id,question_id,sha256,approved_at IS NOT NULL,resumed_at IS NOT NULL
             FROM tag_reviews WHERE job_id=?1 AND plan_version=?2",
                params![job, version],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, bool>(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(id, question_id, sha256, approved, resumed)| {
            Ok(TagReview {
                policy: planning::read_plan(&self.connection, job, version)?
                    .plan
                    .settings
                    .tag_policy
                    .ok_or_else(conflict)?,
                id,
                job_id: job,
                plan_version: version,
                question_id,
                sha256,
                approved,
                resumed,
                tracks: read_tracks(&self.connection, id, page)?,
            })
        })
        .transpose()
    }

    /// Persist the complete sealed-input snapshot and release ownership at the
    /// validating boundary. Inspection must finish before this transaction.
    pub(crate) fn save_tag_review(
        &mut self,
        claim: &JobClaim,
        cover: &str,
        tracks: &[TagReviewTrack],
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_job_owner(&transaction, claim, JobState::Validating)?;
        super::job_controls::require_running(&transaction, claim.job, claim.plan_version)?;
        let plan = planning::acquisition_plan(&transaction, claim.job, claim.plan_version)?;
        let policy = plan
            .plan
            .settings
            .tag_policy
            .as_ref()
            .ok_or_else(conflict)?;
        transfers::check_received(&transaction, claim.job, claim.plan_version)?;
        if !policy.needs_review()
            || tracks.len() != plan.plan.outputs.len()
            || artwork::approved(&transaction, claim.job, claim.plan_version)?.sha256 != cover
        {
            return Err(conflict());
        }
        for (index, track) in tracks.iter().enumerate() {
            let valid: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM transfer_files WHERE id=?1 AND job_id=?2
                 AND plan_version=?3 AND track_number=?4 AND expected_bytes=?5
                 AND lower(hex(payload_sha256))=?6 AND state='received')",
                params![
                    track.transfer_id,
                    claim.job,
                    claim.plan_version,
                    track.track_number,
                    track.input_bytes,
                    track.input_sha256
                ],
                |row| row.get(0),
            )?;
            if !valid
                || usize::from(track.track_number) != index + 1
                || policy.resolve_for_format(&track.source, plan.plan.format)? != track.output
            {
                return Err(conflict());
            }
        }
        let hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                1u8,
                claim.job,
                claim.plan_version,
                &plan.plan,
                cover,
                tracks
            ),)?)
        );
        transaction.execute(
            "INSERT INTO questions(job_id,plan_version,paused_state,prompt,choices)
             VALUES (?1,?2,'validating','Review the exact downloaded tag values before normalization.','[\"Approve exact tags\"]')",
            params![claim.job,claim.plan_version],
        )?;
        let question = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO tag_reviews(job_id,plan_version,question_id,sha256,cover_sha256) VALUES (?1,?2,?3,?4,?5)",
            params![claim.job,claim.plan_version,question,hash,cover],
        )?;
        let id = transaction.last_insert_rowid();
        for track in tracks {
            transaction.execute(
                "INSERT INTO tag_review_tracks VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id,
                    track.track_number,
                    track.transfer_id,
                    track.input_bytes,
                    track.input_sha256,
                    serde_json::to_string(&track.source)?,
                    serde_json::to_string(&track.output)?
                ],
            )?;
        }
        transaction.execute(
            "UPDATE jobs SET state='awaiting_input',owner=NULL,lease_until=NULL,
             claim_generation=claim_generation+1 WHERE id=?1",
            [claim.job],
        )?;
        super::record_job_event(&transaction, claim, "tag_review_required")?;
        transaction.commit()?;
        Ok(())
    }

    /// Approve exactly the manifest displayed by the caller. No work starts.
    pub fn approve_tag_review(&mut self, job: i64, version: i64, hash: &str) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::job_controls::require_running(&transaction, job, version)?;
        let plan = planning::approved_plan(&transaction, job, version)?;
        planning::check_catalog_plan(&plan)?;
        transfers::check_received(&transaction, job, version)?;
        let cover = artwork::approved(&transaction, job, version)?;
        resolve_tags(&transaction, job, version, false)?;
        let changed = transaction.execute(
            "UPDATE tag_reviews SET approved_at=unixepoch() WHERE job_id=?1 AND plan_version=?2
             AND sha256=?3 AND cover_sha256=?4 AND approved_at IS NULL
             AND EXISTS(SELECT 1 FROM jobs WHERE id=?1 AND plan_version=?2 AND state='awaiting_input')
             AND question_id=(SELECT MAX(id) FROM questions WHERE job_id=?1 AND plan_version=?2)",
            params![job,version,hash,cover.sha256],
        )?;
        if changed != 1 {
            return Err(conflict());
        }
        transaction.execute(
            "UPDATE questions SET answer='Approve exact tags',answered_at=unixepoch()
             WHERE id=(SELECT question_id FROM tag_reviews WHERE job_id=?1 AND plan_version=?2) AND answer IS NULL",
            params![job,version],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Restore the safe validating state. Starting the worker remains separate.
    pub fn resume_tag_review(&mut self, job: i64, version: i64, hash: &str) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::job_controls::require_running(&transaction, job, version)?;
        planning::approved_plan(&transaction, job, version)?;
        transfers::check_received(&transaction, job, version)?;
        let cover = artwork::approved(&transaction, job, version)?;
        let changed = transaction.execute(
            "UPDATE tag_reviews SET resumed_at=unixepoch() WHERE job_id=?1 AND plan_version=?2
             AND sha256=?3 AND cover_sha256=?4 AND approved_at IS NOT NULL AND resumed_at IS NULL
             AND EXISTS(SELECT 1 FROM jobs WHERE id=?1 AND plan_version=?2 AND state='awaiting_input'
             AND (owner IS NULL OR lease_until<=unixepoch()))
             AND question_id=(SELECT MAX(id) FROM questions WHERE job_id=?1 AND plan_version=?2)",
            params![job,version,hash,cover.sha256],
        )?;
        if changed != 1 {
            return Err(conflict());
        }
        resolved_tags(&transaction, job, version)?;
        transaction.execute(
            "UPDATE jobs SET state='validating',claim_generation=claim_generation+1
             WHERE id=?1 AND plan_version=?2",
            params![job, version],
        )?;
        transaction.commit()?;
        Ok(())
    }
}
