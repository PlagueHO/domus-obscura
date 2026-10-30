use super::{JobClaim, Store, validate_claim};
use crate::domain::CatalogRelease;
use crate::domain::{
    AudioFormat, AudioProfile, AudioSelection, EditionSelection, OutputPreview, OutputSettings,
};
use crate::infrastructure::catalog::{CatalogQuery, CatalogResult};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct SettingsVersion {
    pub version: i64,
    pub settings: OutputSettings,
    pub policy: crate::domain::AudioPolicy,
    pub audio_profiles: Vec<crate::domain::AudioProfileDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct PlanDestination {
    pub location_id: i64,
    pub root: String,
    pub kind: String,
    pub coverage: String,
    pub observed_at: Option<i64>,
    pub scan_id: Option<i64>,
    pub observed_count: Option<i64>,
    pub error_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct PreparedPlan {
    pub edition: EditionSelection,
    pub settings: OutputSettings,
    pub format: AudioFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioSelection>,
    pub outputs: Vec<OutputPreview>,
    pub destinations: Vec<PlanDestination>,
    pub accepts_incomplete_coverage: bool,
    #[serde(default)]
    pub catalog: Option<CatalogResult<CatalogRelease>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct StoredPlan {
    pub job_id: i64,
    pub version: i64,
    pub settings_version: i64,
    pub approved_at: Option<i64>,
    pub plan: PreparedPlan,
}

impl PreparedPlan {
    pub fn audio_profile(&self) -> Result<AudioProfile> {
        if let Some(audio) = self.audio {
            audio.validate(self.format)?;
            if !self.settings.policy().accepted.contains(&audio.profile) {
                return Err(Error::Conflict(
                    "plan audio profile is outside its frozen policy".into(),
                ));
            }
            Ok(audio.profile)
        } else {
            Ok(AudioProfile::legacy(self.format))
        }
    }
}

#[derive(Debug)]
pub struct ClaimedPlan {
    pub claim: JobClaim,
    pub plan: StoredPlan,
}

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    pub job_id: i64,
    pub expected_version: i64,
    pub settings_version: i64,
    pub format: Option<AudioFormat>,
    #[serde(default)]
    pub audio_profile: Option<AudioProfile>,
    pub edition: EditionSelection,
    pub locations: Vec<i64>,
    pub accepts_incomplete_coverage: bool,
}

impl Store {
    pub fn settings(&self) -> Result<Option<SettingsVersion>> {
        let row: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT id, body FROM settings_versions ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(version, body)| {
            let settings: OutputSettings = serde_json::from_str(&body)?;
            settings.validate()?;
            Ok(SettingsVersion {
                version,
                policy: settings.policy(),
                audio_profiles: AudioProfile::descriptors(),
                settings,
            })
        })
        .transpose()
    }

    /// Append an immutable, system-wide library settings version. No user or
    /// session owns a separate copy. An optimistic version check keeps
    /// another client's change from being overwritten by an older form.
    pub fn save_settings(
        &mut self,
        expected_version: Option<i64>,
        settings: &OutputSettings,
    ) -> Result<i64> {
        settings.validate()?;
        let body = bounded_json(settings, 16384)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<i64> =
            transaction.query_row("SELECT MAX(id) FROM settings_versions", [], |row| {
                row.get(0)
            })?;
        if current != expected_version {
            return Err(Error::Conflict(
                "settings changed; reload the current version".into(),
            ));
        }
        transaction.execute("INSERT INTO settings_versions(body) VALUES (?1)", [body])?;
        let version = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(version)
    }

    /// Freeze the exact edition, settings, output and indexed observations.
    /// This does not prove current filesystem contents or permit replacement.
    pub fn prepare_plan(&mut self, request: PlanRequest) -> Result<StoredPlan> {
        self.prepare_plan_inner(request, None)
    }

    pub fn prepare_catalog_plan(
        &mut self,
        request: PlanRequest,
        catalog: CatalogResult<CatalogRelease>,
    ) -> Result<StoredPlan> {
        self.check_catalog_evidence(&request, &catalog)?;
        self.prepare_plan_inner(request, Some(catalog))
    }

    /// Freeze independent album plans in one transaction. Nothing is approved
    /// or started. A rejected album leaves every job and event unchanged.
    pub fn prepare_catalog_plans(
        &mut self,
        requests: Vec<(PlanRequest, CatalogResult<CatalogRelease>)>,
    ) -> Result<Vec<StoredPlan>> {
        if requests.is_empty() || requests.len() > 25 {
            return Err(Error::InvalidInput(
                "a plan batch needs 1 through 25 albums".into(),
            ));
        }
        let mut jobs = HashSet::new();
        let mut releases = HashSet::new();
        let mut folders = HashSet::new();
        let mut prepared = Vec::with_capacity(requests.len());
        for (request, catalog) in requests {
            if !jobs.insert(request.job_id) || !releases.insert(catalog.data.summary.id.clone()) {
                return Err(Error::InvalidInput(
                    "batch jobs and release IDs must be distinct".into(),
                ));
            }
            self.check_catalog_evidence(&request, &catalog)?;
            let plan = self.prepare_plan_body(&request, Some(catalog))?;
            let folder = &plan.outputs[0].relative_folder;
            for destination in &plan.destinations {
                if !folders.insert((destination.location_id, folder.to_uppercase())) {
                    return Err(Error::Conflict(
                        "selected albums share a destination folder; choose distinct output paths"
                            .into(),
                    ));
                }
            }
            prepared.push((request, plan));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plans = prepared
            .into_iter()
            .map(|(request, plan)| write_plan(&transaction, &request, plan))
            .collect::<Result<Vec<_>>>()?;
        transaction.commit()?;
        Ok(plans)
    }

    fn check_catalog_evidence(
        &mut self,
        request: &PlanRequest,
        catalog: &CatalogResult<CatalogRelease>,
    ) -> Result<()> {
        let source = CatalogQuery::release(&catalog.data.summary.id)?;
        let now: i64 = self
            .connection
            .query_row("SELECT unixepoch()", [], |row| row.get(0))?;
        if catalog.source != source.source()
            || catalog.payload_sha256.len() != 32
            || catalog.fetched_at > now
            || catalog.fetched_at <= now - 86400
            || catalog.data.selection(request.edition.bucket.clone())? != request.edition
        {
            return Err(Error::Conflict(
                "catalog evidence is stale or differs from the selected edition".into(),
            ));
        }
        let cached = self.catalog_cache(&source)?.ok_or_else(|| {
            Error::Conflict("catalog evidence is no longer cached; reload the release".into())
        })?;
        if cached.payload_sha256 != catalog.payload_sha256
            || cached.fetched_at != catalog.fetched_at
            || source.parse_release(&cached.body)? != catalog.data
        {
            return Err(Error::Conflict(
                "catalog evidence differs from the validated provider response".into(),
            ));
        }
        Ok(())
    }

    fn prepare_plan_inner(
        &mut self,
        request: PlanRequest,
        catalog: Option<CatalogResult<CatalogRelease>>,
    ) -> Result<StoredPlan> {
        let plan = self.prepare_plan_body(&request, catalog)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = write_plan(&transaction, &request, plan)?;
        transaction.commit()?;
        Ok(stored)
    }

    fn prepare_plan_body(
        &self,
        request: &PlanRequest,
        catalog: Option<CatalogResult<CatalogRelease>>,
    ) -> Result<PreparedPlan> {
        if request.locations.is_empty() || request.locations.len() > 16 {
            return Err(Error::InvalidInput(
                "a plan needs a main location and at most 15 backup locations".into(),
            ));
        }
        let mut locations = request.locations.clone();
        locations.sort_unstable();
        locations.dedup();
        if locations.len() != request.locations.len() {
            return Err(Error::InvalidInput(
                "plan locations must be distinct".into(),
            ));
        }
        let settings = self.settings()?.ok_or_else(|| {
            Error::Conflict("save output settings before preparing a plan".into())
        })?;
        if settings.version != request.settings_version {
            return Err(Error::Conflict(
                "settings changed; reload the current version".into(),
            ));
        }
        settings.settings.validate()?;
        let policy = settings.settings.policy();
        let profile = request
            .audio_profile
            .or_else(|| request.format.map(AudioProfile::legacy))
            .unwrap_or(policy.preferred);
        if !policy.accepted.contains(&profile)
            || request
                .format
                .is_some_and(|format| format != profile.format())
        {
            return Err(Error::InvalidInput(
                "selected audio profile is not accepted or differs from the requested format"
                    .into(),
            ));
        }
        let mut effective = settings.settings.clone();
        effective.format = profile.format();
        let outputs = request.edition.preview(&effective)?;
        let destinations = locations
            .iter()
            .map(|id| destination(&self.connection, *id))
            .collect::<Result<Vec<_>>>()?;
        if destinations
            .iter()
            .filter(|item| item.kind == "main")
            .count()
            != 1
        {
            return Err(Error::InvalidInput(
                "a plan must include the main location".into(),
            ));
        }
        if !request.accepts_incomplete_coverage
            && destinations.iter().any(|item| item.coverage != "complete")
        {
            return Err(Error::Conflict(
                "index coverage is incomplete; an explicit warning decision is required".into(),
            ));
        }
        Ok(PreparedPlan {
            edition: request.edition.clone(),
            settings: settings.settings,
            format: effective.format,
            audio: Some(AudioSelection::new(profile)),
            outputs,
            destinations,
            accepts_incomplete_coverage: request.accepts_incomplete_coverage,
            catalog,
        })
    }

    pub fn plan(&self, job: i64, version: i64) -> Result<StoredPlan> {
        read_plan(&self.connection, job, version)
    }

    /// Approval is explicit and applies to one immutable preview. A refreshed
    /// index requires a new preview rather than silently extending consent.
    pub fn approve_plan(&mut self, job: i64, version: i64) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = read_plan(&transaction, job, version)?;
        check_destinations(&transaction, &stored.plan.destinations)?;
        let changed = transaction.execute(
            "UPDATE job_plans SET approved_at = unixepoch()
             WHERE job_id = ?1 AND version = ?2 AND approved_at IS NULL
             AND EXISTS(SELECT 1 FROM jobs WHERE id = ?1 AND plan_version = ?2
                        AND state = 'queued' AND (owner IS NULL OR lease_until <= unixepoch()))",
            params![job, version],
        )?;
        if changed != 1 {
            return Err(Error::Conflict(
                "plan was approved, changed, or is already active".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO events(job_id, event_type, plan_version) VALUES (?1, 'plan_approved', ?2)",
            params![job, version],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Acquire only an approved current plan. Callers must still check actual
    /// files through the publication journal before any filesystem effect.
    pub fn claim_approved_plan(
        &mut self,
        job: i64,
        version: i64,
        owner: &str,
        lease_seconds: u16,
    ) -> Result<Option<ClaimedPlan>> {
        self.claim_plan_inner(job, version, owner, lease_seconds, false)
    }

    /// Recovery may record already-submitted receipts after observations change.
    /// This capability is not authority for another queue or normalization.
    pub(crate) fn claim_acquisition_recovery(
        &mut self,
        job: i64,
        version: i64,
        owner: &str,
        lease_seconds: u16,
    ) -> Result<Option<ClaimedPlan>> {
        self.claim_plan_inner(job, version, owner, lease_seconds, true)
    }

    fn claim_plan_inner(
        &mut self,
        job: i64,
        version: i64,
        owner: &str,
        lease_seconds: u16,
        recovery: bool,
    ) -> Result<Option<ClaimedPlan>> {
        validate_claim(owner, lease_seconds)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let uncertain: bool = recovery
            && transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM jobs j JOIN transfer_files f ON f.job_id=j.id
             AND f.plan_version=j.plan_version WHERE j.id=?1 AND j.plan_version=?2
             AND j.state='downloading' AND f.state IN ('submitting','active'))",
                params![job, version],
                |row| row.get(0),
            )?;
        let stored = if uncertain {
            let stored = read_plan(&transaction, job, version)?;
            if stored.approved_at.is_none() {
                return Err(Error::Conflict(
                    "recovery requires the approved source plan".into(),
                ));
            }
            check_catalog_plan(&stored)?;
            stored
        } else {
            approved_plan(&transaction, job, version)?
        };
        let generation: Option<i64> = transaction
            .query_row(
                "UPDATE jobs SET owner = ?3, lease_until = unixepoch() + ?4,
             claim_generation = claim_generation + 1
             WHERE id = ?1 AND plan_version = ?2
             AND state NOT IN ('complete', 'cancelled', 'failed', 'awaiting_input')
             AND (owner IS NULL OR lease_until <= unixepoch()) RETURNING claim_generation",
                params![job, version, owner, lease_seconds],
                |row| row.get(0),
            )
            .optional()?;
        transaction.commit()?;
        Ok(generation.map(|generation| ClaimedPlan {
            claim: JobClaim {
                job,
                plan_version: version,
                owner: owner.into(),
                generation,
            },
            plan: stored,
        }))
    }
}

pub(super) fn approved_plan(connection: &Connection, job: i64, version: i64) -> Result<StoredPlan> {
    let stored = read_plan(connection, job, version)?;
    if stored.approved_at.is_none() {
        return Err(Error::Conflict(
            "the current plan needs explicit approval".into(),
        ));
    }
    check_destinations(connection, &stored.plan.destinations)?;
    Ok(stored)
}

pub(super) fn check_queued_resume(connection: &Connection, job: i64, version: i64) -> Result<()> {
    let prepared: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM job_plans WHERE job_id=?1 AND version=?2)",
        params![job, version],
        |row| row.get(0),
    )?;
    if prepared {
        let plan = read_plan(connection, job, version)?;
        check_destinations(connection, &plan.plan.destinations)?;
    }
    Ok(())
}

pub(super) fn acquisition_plan(
    connection: &Connection,
    job: i64,
    version: i64,
) -> Result<StoredPlan> {
    let stored = approved_plan(connection, job, version)?;
    check_catalog_plan(&stored)?;
    Ok(stored)
}

pub(super) fn check_catalog_plan(stored: &StoredPlan) -> Result<()> {
    let catalog = stored.plan.catalog.as_ref().ok_or_else(|| {
        Error::Conflict(
            "prepare and approve a provider-backed exact edition before acquisition".into(),
        )
    })?;
    if catalog.source != CatalogQuery::release(&catalog.data.summary.id)?.source()
        || catalog.payload_sha256.len() != 32
        || catalog.data.selection(stored.plan.edition.bucket.clone())? != stored.plan.edition
    {
        return Err(Error::Conflict(
            "frozen catalog evidence differs from the approved edition".into(),
        ));
    }
    Ok(())
}

fn write_plan(
    connection: &Connection,
    request: &PlanRequest,
    plan: PreparedPlan,
) -> Result<StoredPlan> {
    super::job_controls::require_replan_allowed(
        connection,
        request.job_id,
        request.expected_version,
    )?;
    let body = bounded_json(&plan, 2 * 1024 * 1024)?;
    let version = request
        .expected_version
        .checked_add(1)
        .filter(|_| request.expected_version > 0)
        .ok_or_else(|| Error::InvalidInput("plan version is outside the supported range".into()))?;
    let current_settings: i64 =
        connection.query_row("SELECT MAX(id) FROM settings_versions", [], |row| {
            row.get(0)
        })?;
    if current_settings != request.settings_version {
        return Err(Error::Conflict(
            "settings changed while preparing the plan".into(),
        ));
    }
    check_destinations(connection, &plan.destinations)?;
    let uncertain: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM transfer_files WHERE job_id = ?1
             AND state IN ('submitting', 'active'))",
        [request.job_id],
        |row| row.get(0),
    )?;
    if uncertain {
        return Err(Error::Conflict(
            "reconcile existing peer transfers before changing the plan".into(),
        ));
    }
    let changed = connection.execute(
        "UPDATE jobs SET plan_version = ?3, state = 'queued', owner = NULL, lease_until = NULL
         WHERE id = ?1 AND plan_version = ?2 AND state IN ('queued', 'awaiting_input')
         AND (owner IS NULL OR lease_until <= unixepoch())",
        params![request.job_id, request.expected_version, version],
    )?;
    if changed != 1 {
        return Err(Error::Conflict("job is active or its plan changed".into()));
    }
    connection.execute(
        "INSERT INTO job_plans(job_id, version, settings_version, body) VALUES (?1, ?2, ?3, ?4)",
        params![request.job_id, version, request.settings_version, body],
    )?;
    connection.execute(
        "INSERT INTO events(job_id, event_type, plan_version) VALUES (?1, 'plan_prepared', ?2)",
        params![request.job_id, version],
    )?;
    Ok(StoredPlan {
        job_id: request.job_id,
        version,
        settings_version: request.settings_version,
        approved_at: None,
        plan,
    })
}

fn destination(connection: &Connection, id: i64) -> Result<PlanDestination> {
    connection
        .query_row(
            "SELECT l.id, l.root, l.kind, l.coverage, l.observed_at,
                s.id, s.observed_count, s.error_count
         FROM locations l LEFT JOIN scans s ON s.id =
             (SELECT MAX(id) FROM scans WHERE location_id = l.id)
         WHERE l.id = ?1",
            [id],
            |row| {
                Ok(PlanDestination {
                    location_id: row.get(0)?,
                    root: row.get(1)?,
                    kind: row.get(2)?,
                    coverage: row.get(3)?,
                    observed_at: row.get(4)?,
                    scan_id: row.get(5)?,
                    observed_count: row.get(6)?,
                    error_count: row.get(7)?,
                })
            },
        )
        .map_err(Error::from)
}

pub(super) fn check_destinations(
    connection: &Connection,
    expected: &[PlanDestination],
) -> Result<()> {
    for item in expected {
        if destination(connection, item.location_id)? != *item {
            return Err(Error::Conflict(
                "library observations changed; prepare and approve a new plan".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn read_plan(connection: &Connection, job: i64, version: i64) -> Result<StoredPlan> {
    let (settings_version, approved_at, body): (i64, Option<i64>, String) = connection.query_row(
        "SELECT settings_version, approved_at, body FROM job_plans WHERE job_id = ?1 AND version = ?2",
        params![job, version], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let plan: PreparedPlan = serde_json::from_str(&body)?;
    plan.settings.validate()?;
    plan.audio_profile()?;
    Ok(StoredPlan {
        job_id: job,
        version,
        settings_version,
        approved_at,
        plan,
    })
}

fn bounded_json(value: &impl Serialize, maximum: usize) -> Result<String> {
    let body = serde_json::to_string(value)?;
    if body.len() > maximum {
        return Err(Error::InvalidInput(format!(
            "saved document exceeds {maximum} bytes"
        )));
    }
    Ok(body)
}
