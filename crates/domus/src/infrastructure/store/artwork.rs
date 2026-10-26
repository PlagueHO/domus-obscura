use super::{Store, planning};
use crate::domain::Page;
use crate::infrastructure::artwork::{COVER_PROFILE, CoverJpeg, normalize_cover_for_size};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct ArtworkCandidate {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub source: String,
    pub input_sha256: String,
    pub background: Option<[u8; 3]>,
    pub profile: u8,
    pub sha256: String,
    pub bytes: u32,
    pub width: u32,
    pub height: u32,
    pub approved_at: Option<i64>,
}

struct ArtworkPreview<'a> {
    image_id: &'a str,
    input: &'a [u8],
    background: Option<[u8; 3]>,
    evidence: Option<&'a super::caa::PreparedCaa>,
    jpeg: CoverJpeg,
}

#[cfg(test)]
#[path = "artwork/retrieval_tests.rs"]
mod retrieval_tests;

fn conflict() -> Error {
    Error::Conflict("artwork needs an exact current plan and explicit cover approval".into())
}

pub(super) fn current_plan(
    connection: &Connection,
    job: i64,
    version: i64,
) -> Result<super::StoredPlan> {
    let current: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE id = ?1 AND plan_version = ?2)",
        params![job, version],
        |row| row.get(0),
    )?;
    if !current {
        return Err(conflict());
    }
    let plan = planning::read_plan(connection, job, version)?;
    planning::check_catalog_plan(&plan)?;
    Ok(plan)
}

fn writable_plan(connection: &Connection, job: i64, version: i64) -> Result<super::StoredPlan> {
    let plan = current_plan(connection, job, version)?;
    let writable: bool = connection.query_row(
        "SELECT state IN ('queued','searching','downloading','validating','normalizing','awaiting_input')
         FROM jobs WHERE id = ?1",
        [job], |row| row.get(0),
    )?;
    if !writable {
        return Err(conflict());
    }
    planning::check_destinations(connection, &plan.plan.destinations)?;
    Ok(plan)
}

fn candidate(connection: &Connection, job: i64, version: i64, id: i64) -> Result<ArtworkCandidate> {
    let row = connection.query_row(
        "SELECT c.source, lower(hex(c.input_sha256)), c.background, c.profile,
                lower(hex(c.sha256)), length(c.jpeg), c.width, c.height, a.approved_at
         FROM artwork_candidates c LEFT JOIN artwork_approvals a
           ON a.candidate_id = c.id AND a.job_id = c.job_id AND a.plan_version = c.plan_version
         WHERE c.id = ?1 AND c.job_id = ?2 AND c.plan_version = ?3",
        params![id, job, version],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u8>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, u32>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        },
    )?;
    Ok(ArtworkCandidate {
        id,
        job_id: job,
        plan_version: version,
        source: row.0,
        input_sha256: row.1,
        background: serde_json::from_str(&row.2)?,
        profile: row.3,
        sha256: row.4,
        bytes: row.5,
        width: row.6,
        height: row.7,
        approved_at: row.8,
    })
}

pub(super) fn load_jpeg(
    connection: &Connection,
    job: i64,
    version: i64,
    id: i64,
) -> Result<CoverJpeg> {
    let saved = candidate(connection, job, version, id)?;
    super::caa::read_evidence(connection, job, version, id)?;
    if saved.profile != COVER_PROFILE || saved.bytes == 0 || saved.bytes > 8 * 1024 * 1024 {
        return Err(conflict());
    }
    let (bytes, hash): (Vec<u8>, Vec<u8>) = connection.query_row(
        "SELECT jpeg, sha256 FROM artwork_candidates WHERE id = ?1 AND job_id = ?2
         AND plan_version = ?3 AND length(jpeg) BETWEEN 1 AND 8388608",
        params![id, job, version],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    CoverJpeg::restore(
        bytes,
        hash.try_into().map_err(|_| conflict())?,
        (saved.width, saved.height),
    )
}

pub(super) fn approved(
    connection: &Connection,
    job: i64,
    version: i64,
) -> Result<ArtworkCandidate> {
    let id: Option<i64> = connection
        .query_row(
            "SELECT candidate_id FROM artwork_approvals WHERE job_id = ?1 AND plan_version = ?2",
            params![job, version],
            |row| row.get(0),
        )
        .optional()?;
    candidate(connection, job, version, id.ok_or_else(conflict)?)
}

pub(super) fn check_cover(
    connection: &Connection,
    job: i64,
    version: i64,
    hash: &str,
    bytes: u64,
    dimensions: (u32, u32),
) -> Result<()> {
    let selected = approved(connection, job, version)?;
    if selected.profile != COVER_PROFILE
        || selected.sha256 != hash
        || u64::from(selected.bytes) != bytes
        || (selected.width, selected.height) != dimensions
    {
        return Err(conflict());
    }
    Ok(())
}

impl Store {
    /// Bind browser provider work to a current approved, writable plan before
    /// network I/O. Call again after I/O; this does not grant cover approval.
    pub fn artwork_provider_release(&self, job: i64, version: i64) -> Result<String> {
        Ok(self.artwork_provider_options(job, version)?.0)
    }

    pub(crate) fn artwork_provider_options(
        &self,
        job: i64,
        version: i64,
    ) -> Result<(String, Option<crate::domain::ArtworkSize>)> {
        let plan = writable_plan(&self.connection, job, version)?;
        if plan.approved_at.is_none() {
            return Err(conflict());
        }
        Ok((
            plan.plan.edition.release_id,
            plan.plan.settings.artwork_size,
        ))
    }

    /// Save only a result bound to the fresh cached original and frozen target.
    /// Processing failures use the existing typed, plan-bound question flow.
    pub(crate) fn prepare_retrieved_caa_artwork_or_ask(
        &mut self,
        job: i64,
        version: i64,
        retrieved: Result<crate::application::services::images::RetrievedImage>,
    ) -> Result<ArtworkCandidate> {
        let result = retrieved.and_then(|retrieved| {
            let (release, size) = self.artwork_provider_options(job, version)?;
            let image = retrieved.request.image_id.as_deref().ok_or_else(conflict)?;
            let target = size.map_or(1200, crate::domain::ArtworkSize::target_side);
            if retrieved.request.provider != "cover-art-archive"
                || retrieved.request.release.provider != "musicbrainz"
                || retrieved.request.release.id != release
                || retrieved.options.size != size
                || retrieved.image.profile != COVER_PROFILE
                || retrieved.image.width > target
                || retrieved.image.height > target
            {
                return Err(conflict());
            }
            let evidence = self.cached_caa_cover(&release, image)?;
            let original: [u8; 32] = Sha256::digest(&evidence.bytes).into();
            if original != retrieved.original_sha256 {
                return Err(Error::Conflict(
                    "retrieved image differs from fresh CAA evidence".into(),
                ));
            }
            let jpeg = CoverJpeg::restore(
                retrieved.image.jpeg,
                retrieved.image.sha256,
                (retrieved.image.width, retrieved.image.height),
            )?;
            self.persist_artwork_input(
                job,
                version,
                ArtworkPreview {
                    image_id: image,
                    input: &evidence.bytes,
                    background: retrieved.options.background,
                    evidence: Some(&evidence),
                    jpeg,
                },
            )
        });
        self.artwork_preparation_result(job, version, result)
    }

    pub fn prepare_local_artwork_or_ask(
        &mut self,
        job: i64,
        version: i64,
        path: &std::path::Path,
        background: Option<[u8; 3]>,
        acknowledge_permitted_use: bool,
    ) -> Result<ArtworkCandidate> {
        let result = if acknowledge_permitted_use {
            self.prepare_local_artwork(job, version, path, background, true)
        } else {
            Err(crate::domain::ArtworkIssue::Permission.into())
        };
        self.artwork_preparation_result(job, version, result)
    }

    pub fn prepare_cached_caa_artwork_or_ask(
        &mut self,
        job: i64,
        version: i64,
        image: &str,
        background: Option<[u8; 3]>,
        acknowledge_permitted_use: bool,
    ) -> Result<ArtworkCandidate> {
        if !crate::infrastructure::caa::is_image_id(image) {
            return Err(Error::InvalidInput(
                "CAA image ID must be a positive decimal identifier".into(),
            ));
        }

        let result = if acknowledge_permitted_use {
            self.prepare_cached_caa_artwork(job, version, image, background, true)
        } else {
            Err(crate::domain::ArtworkIssue::Permission.into())
        };
        self.artwork_preparation_result(job, version, result)
    }

    /// A bounded browser upload has local, content-based provenance. Permission
    /// to supply the image never approves its exact normalized JPEG preview.
    pub fn prepare_uploaded_artwork_or_ask(
        &mut self,
        job: i64,
        version: i64,
        input: &[u8],
        background: Option<[u8; 3]>,
        acknowledge_permitted_use: bool,
    ) -> Result<ArtworkCandidate> {
        let result = if !acknowledge_permitted_use {
            Err(crate::domain::ArtworkIssue::Permission.into())
        } else if input.is_empty() || input.len() > 8 * 1024 * 1024 {
            Err(Error::InvalidInput(
                "cover upload must contain 1 to 8388608 bytes".into(),
            ))
        } else {
            self.prepare_artwork_input(job, version, "local", input, background, None)
        };
        self.artwork_preparation_result(job, version, result)
    }

    /// Persist the exact JPEG preview before user approval. The trusted provider
    /// adapter must first bind image_id and input to the release's CAA listing.
    /// This method performs no provider request and does not prove those rights.
    pub fn prepare_artwork(
        &mut self,
        job: i64,
        version: i64,
        image_id: &str,
        input: &[u8],
        background: Option<[u8; 3]>,
    ) -> Result<ArtworkCandidate> {
        if !crate::infrastructure::caa::is_image_id(image_id) {
            return Err(Error::InvalidInput(
                "CAA image ID must be a positive decimal identifier".into(),
            ));
        }
        self.prepare_artwork_input(job, version, image_id, input, background, None)
    }

    /// Prepare a user-supplied local cover without contacting CAA. Its source is
    /// a content identifier, never a fabricated provider URL or a local path.
    /// Permission acknowledgement is not proof of copyright or release identity.
    /// The exact JPEG still needs separate preview approval.
    pub fn prepare_local_artwork(
        &mut self,
        job: i64,
        version: i64,
        path: &std::path::Path,
        background: Option<[u8; 3]>,
        acknowledge_permitted_use: bool,
    ) -> Result<ArtworkCandidate> {
        if !acknowledge_permitted_use {
            return Err(Error::InvalidInput(
                "local artwork requires explicit permitted-use acknowledgement".into(),
            ));
        }
        writable_plan(&self.connection, job, version)?;
        let input = crate::infrastructure::local_file::read_local(path, 8 * 1024 * 1024, false)?;
        self.prepare_artwork_input(job, version, "local", &input, background, None)
    }

    /// Freeze fresh verified CAA cache evidence atomically with the normalized
    /// preview. Permitted use and user preview approval remain separate.
    pub fn prepare_cached_caa_artwork(
        &mut self,
        job: i64,
        version: i64,
        image: &str,
        background: Option<[u8; 3]>,
        acknowledge_permitted_use: bool,
    ) -> Result<ArtworkCandidate> {
        if !acknowledge_permitted_use {
            return Err(Error::InvalidInput(
                "CAA artwork requires explicit permitted-use acknowledgement".into(),
            ));
        }
        let plan = writable_plan(&self.connection, job, version)?;
        let release = &plan
            .plan
            .catalog
            .as_ref()
            .ok_or_else(conflict)?
            .data
            .summary
            .id;
        let evidence = self.cached_caa_cover(release, image)?;
        self.prepare_artwork_input(
            job,
            version,
            image,
            &evidence.bytes,
            background,
            Some(&evidence),
        )
    }

    fn prepare_artwork_input(
        &mut self,
        job: i64,
        version: i64,
        image_id: &str,
        input: &[u8],
        background: Option<[u8; 3]>,
        evidence: Option<&super::caa::PreparedCaa>,
    ) -> Result<ArtworkCandidate> {
        let current = writable_plan(&self.connection, job, version)?;
        let jpeg = if let Some(cache) = &self.cache {
            if !self.connection.is_autocommit() {
                return Err(Error::Conflict(
                    "shared image cache work must remain outside database transactions".into(),
                ));
            }
            let now: i64 = self
                .connection
                .query_row("SELECT unixepoch()", [], |row| row.get(0))?;
            let now = u64::try_from(now).map_err(|_| conflict())?;
            crate::infrastructure::artwork::normalize_cover_cached(
                cache.as_ref(),
                input,
                background,
                current.plan.settings.artwork_size,
                now,
            )?
        } else {
            normalize_cover_for_size(input, background, current.plan.settings.artwork_size)?
        };
        self.persist_artwork_input(
            job,
            version,
            ArtworkPreview {
                image_id,
                input,
                background,
                evidence,
                jpeg,
            },
        )
    }

    fn persist_artwork_input(
        &mut self,
        job: i64,
        version: i64,
        preview: ArtworkPreview<'_>,
    ) -> Result<ArtworkCandidate> {
        let ArtworkPreview {
            image_id,
            input,
            background,
            evidence,
            jpeg,
        } = preview;
        let input_hash: [u8; 32] = Sha256::digest(input).into();
        let background = serde_json::to_string(&background)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plan = writable_plan(&transaction, job, version)?;
        if evidence.is_some_and(|evidence| {
            plan.plan
                .catalog
                .as_ref()
                .is_none_or(|catalog| catalog.data.summary.id != evidence.release)
        }) {
            return Err(conflict());
        }
        let existing: Option<i64> = transaction
            .query_row(
                "SELECT id FROM artwork_candidates WHERE job_id = ?1 AND plan_version = ?2
             AND image_id = ?3 AND input_sha256 = ?4 AND background = ?5",
                params![job, version, image_id, input_hash.as_slice(), background],
                |row| row.get(0),
            )
            .optional()?;
        let id = if let Some(id) = existing {
            let saved = load_jpeg(&transaction, job, version, id)?;
            if saved.bytes() != jpeg.bytes() {
                return Err(Error::Conflict(
                    "saved artwork differs from the current normalization profile".into(),
                ));
            }
            if evidence.is_some()
                && super::caa::read_evidence(&transaction, job, version, id)?.is_none()
            {
                return Err(Error::Conflict(
                    "legacy artwork has no frozen CAA evidence; revise the plan first".into(),
                ));
            }
            id
        } else {
            if transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM artwork_approvals WHERE job_id = ?1 AND plan_version = ?2)",
                params![job, version], |row| row.get::<_, bool>(0),
            )? { return Err(Error::Conflict("revise the plan before preparing different artwork after approval".into())); }
            let release = &plan
                .plan
                .catalog
                .as_ref()
                .ok_or_else(conflict)?
                .data
                .summary
                .id;
            // "local" cannot collide with the public CAA method's decimal ID.
            let source = if image_id == "local" {
                format!("urn:domus:local-artwork:sha256:{:x}", Sha256::digest(input))
            } else {
                format!("https://coverartarchive.org/release/{release}/{image_id}")
            };
            transaction.execute(
                "INSERT INTO artwork_candidates(job_id,plan_version,image_id,source,input_sha256,
                    background,profile,jpeg,sha256,width,height) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![job, version, image_id, source,
                    input_hash.as_slice(), background, COVER_PROFILE, jpeg.bytes(), jpeg.sha256().as_slice(),
                    jpeg.dimensions().0, jpeg.dimensions().1],
            )?;
            let id = transaction.last_insert_rowid();
            if let Some(evidence) = evidence {
                evidence.freeze(&transaction, id)?;
            }
            transaction.execute(
                "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'artwork_prepared')",
                params![job, version],
            )?;
            id
        };
        let saved = candidate(&transaction, job, version, id)?;
        transaction.commit()?;
        Ok(saved)
    }

    pub fn artwork_candidates(
        &self,
        job: i64,
        version: i64,
        page: Page,
    ) -> Result<Vec<ArtworkCandidate>> {
        planning::read_plan(&self.connection, job, version)?;
        let ids = self
            .connection
            .prepare(
                "SELECT id FROM artwork_candidates WHERE job_id = ?1 AND plan_version = ?2
             AND id > ?3 ORDER BY id LIMIT ?4",
            )?
            .query_map(params![job, version, page.after(), page.limit()], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| candidate(&self.connection, job, version, id))
            .collect()
    }

    pub fn artwork_jpeg(&self, job: i64, version: i64, id: i64) -> Result<CoverJpeg> {
        load_jpeg(&self.connection, job, version, id)
    }

    /// Approval binds the exact preview hash. Another image needs a new plan;
    /// retries of the same decision do not append another event.
    pub fn approve_artwork(&mut self, job: i64, version: i64, id: i64, sha256: &str) -> Result<()> {
        let jpeg = self.artwork_jpeg(job, version, id)?;
        if format!("{:x}", Sha256::digest(jpeg.bytes())) != sha256 {
            return Err(conflict());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        writable_plan(&transaction, job, version)?;
        let mismatch: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM normalization_runs WHERE job_id = ?1 AND plan_version = ?2
             AND (cover_sha256 != ?3 OR cover_bytes != ?4 OR cover_width != ?5 OR cover_height != ?6))",
            params![job, version, sha256, jpeg.bytes().len() as i64, jpeg.dimensions().0, jpeg.dimensions().1],
            |row| row.get(0),
        )?;
        if mismatch {
            return Err(Error::Conflict(
                "saved normalization requires its exact original cover".into(),
            ));
        }
        let prior: Option<i64> = transaction.query_row(
            "SELECT candidate_id FROM artwork_approvals WHERE job_id = ?1 AND plan_version = ?2",
            params![job, version], |row| row.get(0),
        ).optional()?;
        if let Some(prior) = prior {
            if prior != id {
                return Err(conflict());
            }
        } else {
            transaction.execute(
                "INSERT INTO artwork_approvals(job_id,plan_version,candidate_id) VALUES (?1,?2,?3)",
                params![job, version, id],
            )?;
            transaction.execute(
                "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'artwork_approved')",
                params![job, version],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn approved_artwork(&self, job: i64, version: i64) -> Result<CoverJpeg> {
        current_plan(&self.connection, job, version)?;
        let selected = approved(&self.connection, job, version)?;
        self.artwork_jpeg(job, version, selected.id)
    }
}
