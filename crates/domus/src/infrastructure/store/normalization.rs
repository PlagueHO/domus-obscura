use super::{JobClaim, Store, artwork, check_job_owner, planning, record_job_event, transfers};
use crate::domain::{AudioFormat, AudioSelection, JobState, Page, TrackTags};
use crate::infrastructure::artwork::CoverJpeg;
use crate::infrastructure::media::{NORMALIZATION_PROFILE, NormalizedAudio};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize)]
pub struct NormalizationRun {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub claim_generation: i64,
    pub profile: u8,
    pub recipe_sha256: String,
    pub cover_sha256: String,
    pub cover_bytes: u64,
    pub cover_width: u32,
    pub cover_height: u32,
    pub cover_verified_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub files: u16,
    pub verified_files: u16,
    pub staging_directory: String,
}

#[derive(Debug, Serialize)]
pub struct NormalizationWork {
    pub run_id: i64,
    pub track_number: u16,
    pub transfer_id: i64,
    pub input_bytes: u64,
    pub input_sha256: String,
    pub tags_sha256: String,
    pub staging_name: String,
    pub format: AudioFormat,
    pub audio: Option<AudioSelection>,
    pub tags: TrackTags,
    pub receipt: Option<NormalizedAudio>,
}

pub(crate) struct NormalizationGuard {
    run: i64,
    destinations: Vec<planning::PlanDestination>,
}

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn conflict() -> Error {
    Error::Conflict("normalization attempt, worker claim or immutable recipe changed".into())
}

fn byte_count(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let count: i64 = row.get(index)?;
    u64::try_from(count).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

pub(super) fn read_run(connection: &Connection, id: i64) -> Result<NormalizationRun> {
    connection
        .query_row(
            "SELECT r.id, r.job_id, r.plan_version, r.claim_generation, r.profile,
                r.recipe_sha256, r.cover_sha256, r.cover_bytes, r.cover_width, r.cover_height,
                r.cover_verified_at, r.completed_at,
                (SELECT count(*) FROM normalization_files f WHERE f.run_id = r.id),
                (SELECT count(*) FROM normalization_receipts f WHERE f.run_id = r.id)
         FROM normalization_runs r WHERE r.id = ?1",
            [id],
            |row| {
                Ok(NormalizationRun {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    plan_version: row.get(2)?,
                    claim_generation: row.get(3)?,
                    profile: row.get(4)?,
                    recipe_sha256: row.get(5)?,
                    cover_sha256: row.get(6)?,
                    cover_bytes: byte_count(row, 7)?,
                    cover_width: row.get(8)?,
                    cover_height: row.get(9)?,
                    cover_verified_at: row.get(10)?,
                    completed_at: row.get(11)?,
                    files: row.get(12)?,
                    verified_files: row.get(13)?,
                    staging_directory: format!("normalization-{id}"),
                })
            },
        )
        .map_err(Error::from)
}

fn owned_run(connection: &Connection, claim: &JobClaim, id: i64) -> Result<NormalizationRun> {
    check_job_owner(connection, claim, JobState::Normalizing)?;
    let run = read_run(connection, id)?;
    if run.job_id != claim.job
        || run.plan_version != claim.plan_version
        || run.claim_generation != claim.generation
        || run.profile != NORMALIZATION_PROFILE
        || run.completed_at.is_some()
    {
        return Err(conflict());
    }
    Ok(run)
}

fn preparation_owner(connection: &Connection, claim: &JobClaim) -> Result<()> {
    let state: String =
        connection.query_row("SELECT state FROM jobs WHERE id = ?1", [claim.job], |row| {
            row.get(0)
        })?;
    let state = match state.as_str() {
        "validating" => JobState::Validating,
        "normalizing" => JobState::Normalizing,
        _ => return Err(conflict()),
    };
    check_job_owner(connection, claim, state)
}

fn canonical_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_receipt(
    evidence: &NormalizedAudio,
    plan: &planning::PreparedPlan,
    input: &str,
    run: &NormalizationRun,
) -> Result<()> {
    plan.audio_profile()?;
    let format = plan.format;
    if evidence.profile != run.profile
        || evidence.format != format
        || evidence.audio != plan.audio
        || evidence.input_sha256 != input
        || evidence.cover_sha256 != run.cover_sha256
        || ![
            &evidence.input_sha256,
            &evidence.sha256,
            &evidence.audio_sha256,
            &evidence.metadata_sha256,
            &evidence.cover_sha256,
            &evidence.tags_sha256,
        ]
        .into_iter()
        .all(|value| canonical_hash(value))
        || evidence.bytes > i64::MAX as u64
        || evidence.audio_bytes == 0
        || evidence.audio_bytes >= evidence.bytes
        || evidence.bytes - evidence.audio_bytes > 8 * 1024 * 1024
        || evidence.bytes - evidence.audio_bytes < run.cover_bytes
        || evidence.frames == 0
        || evidence.samples_per_channel == 0
        || evidence.channels == 0
        || evidence.channels > 8
        || evidence.sample_rate == 0
        || evidence.sample_rate > 655_350
    {
        return Err(Error::InvalidInput(
            "normalized audio evidence differs from its input or recipe".into(),
        ));
    }
    let valid = match format {
        AudioFormat::Mp3 => {
            evidence.channels <= 2
                && [32_000, 44_100, 48_000].contains(&evidence.sample_rate)
                && evidence.metadata_frames <= 1
                && evidence.bits_per_sample.is_none()
                && evidence.md5_verified.is_none()
                && evidence.frames.checked_mul(1152) == Some(evidence.samples_per_channel)
        }
        AudioFormat::Flac => {
            evidence.metadata_frames == 0
                && evidence.md5_verified.is_some()
                && evidence
                    .bits_per_sample
                    .is_some_and(|bits| (4..=32).contains(&bits))
                && evidence.frames <= evidence.samples_per_channel
        }
    };
    if !valid {
        return Err(Error::InvalidInput(
            "normalized audio format evidence is invalid".into(),
        ));
    }
    Ok(())
}

impl Store {
    /// Freeze one private staging attempt before any normalization write.
    /// Requires the exact durably approved cover and received input bindings.
    /// This journal is not authorization to fetch or publish.
    pub fn begin_normalization(
        &mut self,
        claim: &JobClaim,
        cover: &CoverJpeg,
    ) -> Result<NormalizationRun> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        preparation_owner(&transaction, claim)?;
        super::job_controls::require_running(&transaction, claim.job, claim.plan_version)?;
        let plan = planning::acquisition_plan(&transaction, claim.job, claim.plan_version)?;
        artwork::check_cover(
            &transaction,
            claim.job,
            claim.plan_version,
            &digest(cover.bytes()),
            cover.bytes().len() as u64,
            cover.dimensions(),
        )?;
        transfers::check_received(&transaction, claim.job, claim.plan_version)?;
        let catalog = plan.plan.catalog.as_ref().ok_or_else(conflict)?;
        let tags = catalog.data.audio_tags()?;
        let mut statement = transaction.prepare(
            "SELECT id, track_number, expected_bytes, lower(hex(payload_sha256))
             FROM transfer_files WHERE job_id = ?1 AND plan_version = ?2 ORDER BY track_number",
        )?;
        let inputs = statement
            .query_map(params![claim.job, claim.plan_version], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, u16>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        if inputs.len() != tags.len()
            || inputs
                .iter()
                .enumerate()
                .any(|(index, input)| usize::from(input.1) != index + 1)
        {
            return Err(conflict());
        }
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM normalization_runs
                 WHERE job_id = ?1 AND plan_version = ?2 AND claim_generation = ?3)",
            params![claim.job, claim.plan_version, claim.generation],
            |row| row.get(0),
        )?;
        if exists {
            return Err(Error::Conflict(
                "this claim already has a normalization attempt; load its saved work".into(),
            ));
        }
        let cover_hash = digest(cover.bytes());
        let mut recipe = Sha256::new();
        recipe.update(b"domus-audio-normalization\0");
        recipe.update([NORMALIZATION_PROFILE]);
        recipe.update(plan.plan.format.extension().as_bytes());
        if let Some(audio) = plan.plan.audio {
            audio.validate(plan.plan.format)?;
            recipe.update(b"\0audio-policy\0");
            recipe.update(serde_json::to_vec(&audio)?);
        }
        recipe.update(cover_hash.as_bytes());
        let mut tag_hashes = Vec::with_capacity(tags.len());
        for (tags, input) in tags.iter().zip(&inputs) {
            let hash = digest(&serde_json::to_vec(tags)?);
            recipe.update(hash.as_bytes());
            recipe.update(input.0.to_be_bytes());
            recipe.update(input.2.to_be_bytes());
            recipe.update(input.3.as_bytes());
            tag_hashes.push(hash);
        }
        transaction.execute(
            "INSERT INTO normalization_runs(job_id, plan_version, claim_generation, profile,
                 recipe_sha256, cover_sha256, cover_bytes, cover_width, cover_height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                claim.job,
                claim.plan_version,
                claim.generation,
                NORMALIZATION_PROFILE,
                format!("{:x}", recipe.finalize()),
                cover_hash,
                cover.bytes().len() as i64,
                cover.dimensions().0,
                cover.dimensions().1
            ],
        )?;
        let run = transaction.last_insert_rowid();
        for (input, tags_hash) in inputs.iter().zip(&tag_hashes) {
            transaction.execute(
                "INSERT INTO normalization_files(run_id, track_number, transfer_id,
                     input_bytes, input_sha256, tags_sha256, staging_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    run,
                    input.1,
                    input.0,
                    input.2,
                    input.3,
                    tags_hash,
                    format!("track-{:04}.{}", input.1, plan.plan.format.extension())
                ],
            )?;
        }
        transaction.execute(
            "UPDATE jobs SET state = 'normalizing' WHERE id = ?1",
            [claim.job],
        )?;
        record_job_event(&transaction, claim, "normalization_prepared")?;
        let run = read_run(&transaction, run)?;
        transaction.commit()?;
        Ok(run)
    }

    pub fn normalization_run(&self, claim: &JobClaim) -> Result<Option<NormalizationRun>> {
        preparation_owner(&self.connection, claim)?;
        let id: Option<i64> = self.connection.query_row(
            "SELECT id FROM normalization_runs WHERE job_id = ?1 AND plan_version = ?2 AND claim_generation = ?3",
            params![claim.job, claim.plan_version, claim.generation], |row| row.get(0),
        ).optional()?;
        id.map(|id| read_run(&self.connection, id)).transpose()
    }

    pub(crate) fn normalization_guard(
        &self,
        claim: &JobClaim,
        run: i64,
    ) -> Result<NormalizationGuard> {
        owned_run(&self.connection, claim, run)?;
        let plan = planning::acquisition_plan(&self.connection, claim.job, claim.plan_version)?;
        plan.plan.audio_profile()?;
        Ok(NormalizationGuard {
            run,
            destinations: plan.plan.destinations,
        })
    }

    pub(crate) fn check_normalization_guard(
        &self,
        claim: &JobClaim,
        guard: &NormalizationGuard,
    ) -> Result<()> {
        let run = owned_run(&self.connection, claim, guard.run)?;
        artwork::check_cover(
            &self.connection,
            claim.job,
            claim.plan_version,
            &run.cover_sha256,
            run.cover_bytes,
            (run.cover_width, run.cover_height),
        )?;
        planning::check_destinations(&self.connection, &guard.destinations)
    }

    /// A bounded page of immutable bindings, not a file-open capability.
    /// The worker must create_new inside this run's private directory.
    pub fn normalization_work(
        &self,
        claim: &JobClaim,
        run: i64,
        page: Page,
    ) -> Result<Vec<NormalizationWork>> {
        owned_run(&self.connection, claim, run)?;
        self.read_normalization_work(claim, run, page)
    }

    pub(crate) fn completed_normalization(
        &self,
        claim: &JobClaim,
        id: i64,
    ) -> Result<(NormalizationRun, planning::StoredPlan)> {
        check_job_owner(&self.connection, claim, JobState::Publishing)?;
        let run = read_run(&self.connection, id)?;
        let plan = planning::acquisition_plan(&self.connection, claim.job, claim.plan_version)?;
        if run.job_id != claim.job
            || run.plan_version != claim.plan_version
            || run.claim_generation != claim.generation
            || run.profile != NORMALIZATION_PROFILE
            || run.completed_at.is_none()
            || run.cover_verified_at.is_none()
            || run.files == 0
            || run.files != run.verified_files
            || usize::from(run.files) != plan.plan.outputs.len()
        {
            return Err(conflict());
        }
        artwork::check_cover(
            &self.connection,
            claim.job,
            claim.plan_version,
            &run.cover_sha256,
            run.cover_bytes,
            (run.cover_width, run.cover_height),
        )?;
        Ok((run, plan))
    }

    pub(crate) fn completed_normalization_work(
        &self,
        claim: &JobClaim,
        run: i64,
        page: Page,
    ) -> Result<Vec<NormalizationWork>> {
        self.completed_normalization(claim, run)?;
        self.read_normalization_work(claim, run, page)
    }

    pub(super) fn read_normalization_work(
        &self,
        claim: &JobClaim,
        run: i64,
        page: Page,
    ) -> Result<Vec<NormalizationWork>> {
        let plan = planning::acquisition_plan(&self.connection, claim.job, claim.plan_version)?;
        plan.plan.audio_profile()?;
        let attempt = read_run(&self.connection, run)?;
        let tags = plan
            .plan
            .catalog
            .as_ref()
            .ok_or_else(conflict)?
            .data
            .audio_tags()?;
        let mut statement = self.connection.prepare(
            "SELECT f.track_number, f.transfer_id, f.input_bytes, f.input_sha256, f.tags_sha256,
                    f.staging_name, r.body
             FROM normalization_files f LEFT JOIN normalization_receipts r
               ON r.run_id = f.run_id AND r.track_number = f.track_number
             WHERE f.run_id = ?1 AND f.track_number > ?2 ORDER BY f.track_number LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![run, page.after(), page.limit()], |row| {
                Ok((
                    row.get::<_, u16>(0)?,
                    row.get::<_, i64>(1)?,
                    byte_count(row, 2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(number, transfer, bytes, input_hash, tags_hash, staging_name, receipt)| {
                    let tags = tags
                        .get(usize::from(number - 1))
                        .ok_or_else(conflict)?
                        .clone();
                    if digest(&serde_json::to_vec(&tags)?) != tags_hash {
                        return Err(conflict());
                    }
                    let receipt: Option<NormalizedAudio> = receipt
                        .map(|body| serde_json::from_str(&body))
                        .transpose()?;
                    if let Some(evidence) = &receipt {
                        validate_receipt(evidence, &plan.plan, &input_hash, &attempt)?;
                    }
                    Ok(NormalizationWork {
                        run_id: run,
                        track_number: number,
                        transfer_id: transfer,
                        input_bytes: bytes,
                        input_sha256: input_hash,
                        tags_sha256: tags_hash,
                        staging_name,
                        format: plan.plan.format,
                        audio: plan.plan.audio,
                        tags,
                        receipt,
                    })
                },
            )
            .collect()
    }

    /// Call only after the worker syncs its owned output and verifies it.
    /// Receipt persistence remains possible when index freshness changes.
    pub fn record_normalized_audio(
        &mut self,
        claim: &JobClaim,
        run: i64,
        track: u16,
        evidence: &NormalizedAudio,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempt = owned_run(&transaction, claim, run)?;
        let (input, input_bytes, tags_hash): (String, u64, String) = transaction.query_row(
            "SELECT input_sha256, input_bytes, tags_sha256 FROM normalization_files WHERE run_id = ?1 AND track_number = ?2",
            params![run, track], |row| Ok((row.get(0)?, byte_count(row, 1)?, row.get(2)?)),
        )?;
        if evidence.input_bytes != input_bytes || evidence.tags_sha256 != tags_hash {
            return Err(Error::InvalidInput(
                "normalized audio input length or tag binding changed".into(),
            ));
        }
        let plan = planning::read_plan(&transaction, claim.job, claim.plan_version)?;
        validate_receipt(evidence, &plan.plan, &input, &attempt)?;
        let body = serde_json::to_string(evidence)?;
        if body.len() > 16384 {
            return Err(Error::InvalidInput(
                "normalization receipt exceeds 16 KiB".into(),
            ));
        }
        let existing: Option<String> = transaction
            .query_row(
                "SELECT body FROM normalization_receipts WHERE run_id = ?1 AND track_number = ?2",
                params![run, track],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing != body {
                return Err(Error::Conflict(
                    "a different immutable normalization receipt already exists".into(),
                ));
            }
        } else {
            transaction.execute(
                "INSERT INTO normalization_receipts(run_id, track_number, output_bytes, output_sha256, body)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run, track, evidence.bytes as i64, evidence.sha256, body],
            )?;
            record_job_event(&transaction, claim, "audio_normalized")?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The worker must write, sync and read back exactly the bound JPEG as
    /// folder.jpg. This records that receipt, not a library publication.
    pub fn record_normalized_cover(
        &mut self,
        claim: &JobClaim,
        run: i64,
        bytes: u64,
        sha256: &str,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempt = owned_run(&transaction, claim, run)?;
        if bytes != attempt.cover_bytes || sha256 != attempt.cover_sha256 {
            return Err(Error::InvalidInput(
                "written folder.jpg differs from the frozen cover".into(),
            ));
        }
        if attempt.cover_verified_at.is_none() {
            transaction.execute(
                "UPDATE normalization_runs SET cover_verified_at = unixepoch() WHERE id = ?1",
                [run],
            )?;
            record_job_event(&transaction, claim, "cover_normalized")?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Hand off complete immutable evidence only. Publication must still
    /// validate actual staging bytes and obtain destination-specific consent.
    pub fn finish_normalization(&mut self, claim: &JobClaim, run: i64) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempt = owned_run(&transaction, claim, run)?;
        super::job_controls::require_running(&transaction, claim.job, claim.plan_version)?;
        let plan = planning::acquisition_plan(&transaction, claim.job, claim.plan_version)?;
        artwork::check_cover(
            &transaction,
            claim.job,
            claim.plan_version,
            &attempt.cover_sha256,
            attempt.cover_bytes,
            (attempt.cover_width, attempt.cover_height),
        )?;
        if attempt.files == 0
            || usize::from(attempt.files) != plan.plan.edition.tracks.len()
            || attempt.files != attempt.verified_files
            || attempt.cover_verified_at.is_none()
        {
            return Err(Error::Conflict(
                "every audio file and folder.jpg needs a verified receipt".into(),
            ));
        }
        transaction.execute(
            "UPDATE normalization_runs SET completed_at = unixepoch() WHERE id = ?1",
            [run],
        )?;
        transaction.execute(
            "UPDATE jobs SET state = 'publishing' WHERE id = ?1",
            [claim.job],
        )?;
        record_job_event(&transaction, claim, "normalization_verified")?;
        transaction.commit()?;
        Ok(())
    }
}
