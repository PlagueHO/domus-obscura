use super::{RELEASE, TestResult, cover, cover_source, flac, mp3, release_body};
use domus::application::{add_location, scan_location};
use domus::domain::{AudioFormat, JobState, OutputSettings, Page};
use domus::infrastructure::artwork::CoverJpeg;
use domus::infrastructure::catalog::{CatalogQuery, CatalogResult};
use domus::infrastructure::media::{NormalizedAudio, normalize_audio};
use domus::infrastructure::staging::StagingRoot;
use domus::infrastructure::store::{JobClaim, NormalizationWork, PlanRequest, SourceFile, Store};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::PathBuf;
use tempfile::TempDir;
#[path = "../support/private.rs"]
mod private;
use private::tempdir;

#[path = "acquisition.rs"]
mod acquisition;
#[path = "artwork.rs"]
mod artwork;
mod artwork_questions;
mod caa;
#[path = "playback.rs"]
mod playback;
mod publication;
#[path = "staging.rs"]
mod staging;

struct Fixture {
    store: Store,
    claim: JobClaim,
    job: i64,
    version: i64,
    main: i64,
    database: PathBuf,
    source: Vec<u8>,
    cover: CoverJpeg,
    staging: Option<StagingRoot>,
    scratch: TempDir,
    _state: TempDir,
    _library: TempDir,
}

impl Fixture {
    fn new(format: AudioFormat) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build(format, false)
    }

    fn build(format: AudioFormat, staged: bool) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_inner(format, staged, true)
    }

    fn build_inner(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_receipts(format, staged, approve_cover, true)
    }

    fn build_with_receipts(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
        receive: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_backup(format, staged, approve_cover, receive, None)
    }

    fn build_with_backup(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
        receive: bool,
        backup: Option<&std::path::Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let state = tempdir()?;
        let library = tempdir()?;
        let scratch = tempdir()?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let main = add_location(&store, library.path(), "main")?;
        scan_location(&mut store, main)?;
        let mut locations = vec![main];
        if let Some(backup) = backup {
            let id = add_location(&store, backup, "backup")?;
            scan_location(&mut store, id)?;
            locations.push(id);
        }
        store.save_settings(None, &OutputSettings::default())?;
        let job = store.enqueue(1)?;
        let body = release_body(Some("2024-02"))?;
        let query = CatalogQuery::release(RELEASE)?;
        let raw = Connection::open(&database)?;
        let now: i64 = raw.query_row("SELECT unixepoch()", [], |row| row.get(0))?;
        let payload_sha256 = Sha256::digest(&body).to_vec();
        raw.execute(
            "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
             VALUES (?1,?2,?3,?4,?4+86400,?4*1000)",
            params![query.source(), body, payload_sha256, now],
        )?;
        let catalog = CatalogResult {
            source: query.source().into(),
            fetched_at: now,
            cached: true,
            payload_sha256,
            data: query.parse_release(&body)?,
        };
        let plan = store.prepare_catalog_plan(
            PlanRequest {
                job_id: job,
                expected_version: 1,
                settings_version: 1,
                format: Some(format),
                edition: catalog.data.selection(None)?,
                locations,
                accepts_incomplete_coverage: false,
            },
            catalog,
        )?;
        if approve_cover {
            let candidate =
                store.prepare_artwork(job, plan.version, "123456789", &cover_source()?, None)?;
            store.approve_artwork(job, plan.version, candidate.id, &candidate.sha256)?;
        }
        store.approve_plan(job, plan.version)?;
        let claim = store
            .claim_approved_plan(job, plan.version, "worker", 300)?
            .ok_or("missing claim")?
            .claim;
        store.advance(&claim, JobState::Queued, JobState::Searching)?;
        let source = match format {
            AudioFormat::Mp3 => mp3()?,
            AudioFormat::Flac => flac(),
        };
        let files = (1..=3)
            .map(|track| SourceFile {
                remote_path: format!("Peer\\Album\\track-{track}.{}", format.extension()),
                expected_bytes: source.len() as i64,
            })
            .collect::<Vec<_>>();
        store.begin_acquisition(&claim, "generated-peer", &files)?;
        let staging = if staged {
            Some(StagingRoot::open(&mut store, scratch.path())?)
        } else {
            None
        };
        for file in store.transfers(job, plan.version, Page::new(0, 200)?)? {
            store.start_transfer(&claim, file.id)?;
            store.accept_transfer(&claim, file.id, file.track_number.into())?;
            if !receive {
                continue;
            }
            if let Some(staging) = &staging {
                let active = store
                    .transfers(job, plan.version, Page::new(file.id - 1, 1)?)?
                    .pop()
                    .ok_or("missing active transfer")?;
                let received = staging.receive(&active, &mut Cursor::new(&source))?;
                store.receive_transfer(
                    &claim,
                    file.id,
                    received.byte_length() as i64,
                    received.sha256(),
                )?;
            } else {
                store.receive_transfer(
                    &claim,
                    file.id,
                    source.len() as i64,
                    &Sha256::digest(&source).into(),
                )?;
            }
        }
        if receive {
            store.advance(&claim, JobState::Downloading, JobState::Validating)?;
        }
        Ok(Self {
            store,
            claim,
            job,
            version: plan.version,
            main,
            database,
            source,
            cover: cover()?,
            staging,
            scratch,
            _state: state,
            _library: library,
        })
    }

    fn proof(
        &self,
        work: &NormalizationWork,
    ) -> Result<NormalizedAudio, Box<dyn std::error::Error>> {
        Ok(normalize_audio(
            &mut Cursor::new(&self.source),
            &mut Cursor::new(Vec::new()),
            work.format,
            &work.tags,
            &self.cover,
        )?)
    }

    fn bind_roots(&self) -> TestResult {
        use domus::infrastructure::filesystem::native_root_identity;
        self.store
            .bind_location(self.main, &native_root_identity(self._library.path())?)?;
        if self.staging.is_some() {
            self.store.bind_staging_root(
                self.scratch.path(),
                &native_root_identity(self.scratch.path())?,
            )?;
        }
        Ok(())
    }
}

#[test]
fn normalization_journal_binds_all_files_and_cover_before_handoff_in_both_formats() -> TestResult {
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let mut fixture = Fixture::new(format)?;
        assert!(
            fixture
                .store
                .advance(&fixture.claim, JobState::Validating, JobState::Normalizing)
                .is_err()
        );
        let run = fixture
            .store
            .begin_normalization(&fixture.claim, &fixture.cover)?;
        assert_eq!(run.files, 3);
        assert_eq!(run.verified_files, 0);
        assert!(run.completed_at.is_none());
        assert!(
            fixture
                .store
                .begin_normalization(&fixture.claim, &fixture.cover)
                .is_err()
        );
        assert!(
            fixture
                .store
                .finish_normalization(&fixture.claim, run.id)
                .is_err()
        );
        assert!(
            fixture
                .store
                .advance(&fixture.claim, JobState::Normalizing, JobState::Publishing)
                .is_err()
        );
        let page = fixture
            .store
            .normalization_work(&fixture.claim, run.id, Page::new(1, 1)?)?;
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].track_number, 2);
        assert_eq!(
            page[0].staging_name,
            format!("track-0002.{}", format.extension())
        );
        let work = fixture
            .store
            .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
        let first_proof = fixture.proof(&work[0])?;
        assert!(
            fixture
                .store
                .record_normalized_audio(&fixture.claim, run.id, 2, &first_proof)
                .is_err()
        );
        for item in &work {
            let proof = fixture.proof(item)?;
            assert_eq!(proof.input_bytes, item.input_bytes);
            assert_eq!(proof.input_sha256, item.input_sha256);
            assert_eq!(proof.tags_sha256, item.tags_sha256);
            fixture.store.record_normalized_audio(
                &fixture.claim,
                run.id,
                item.track_number,
                &proof,
            )?;
            fixture.store.record_normalized_audio(
                &fixture.claim,
                run.id,
                item.track_number,
                &proof,
            )?;
        }
        let raw = Connection::open(&fixture.database)?;
        let events: i64 = raw.query_row(
            "SELECT count(*) FROM events WHERE event_type = 'audio_normalized'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(events, 3);
        assert!(
            fixture
                .store
                .finish_normalization(&fixture.claim, run.id)
                .is_err()
        );
        assert!(
            fixture
                .store
                .record_normalized_cover(&fixture.claim, run.id, run.cover_bytes, &"0".repeat(64))
                .is_err()
        );
        fixture.store.record_normalized_cover(
            &fixture.claim,
            run.id,
            run.cover_bytes,
            &run.cover_sha256,
        )?;
        fixture.store.record_normalized_cover(
            &fixture.claim,
            run.id,
            run.cover_bytes,
            &run.cover_sha256,
        )?;
        let saved = fixture
            .store
            .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
        assert!(saved.iter().all(|item| item.receipt.is_some()));
        fixture.store.finish_normalization(&fixture.claim, run.id)?;
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
        let complete: bool = raw.query_row(
            "SELECT completed_at IS NOT NULL FROM normalization_runs WHERE id = ?1",
            [run.id],
            |row| row.get(0),
        )?;
        assert!(complete);
        assert!(raw.execute("UPDATE normalization_receipts SET output_bytes = output_bytes + 1 WHERE run_id = ?1", [run.id]).is_err());
        assert!(
            raw.execute("DELETE FROM normalization_runs WHERE id = ?1", [run.id])
                .is_err()
        );
        assert!(
            raw.execute(
                "UPDATE normalization_files SET staging_name = 'outside' WHERE run_id = ?1",
                [run.id]
            )
            .is_err()
        );
        assert!(
            raw.execute(
                "UPDATE normalization_runs SET cover_verified_at = NULL WHERE id = ?1",
                [run.id]
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn normalization_reclaim_keeps_old_receipts_but_never_reuses_a_stale_workers_staging_directory()
-> TestResult {
    let mut fixture = Fixture::new(AudioFormat::Mp3)?;
    let old_run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let old_work =
        fixture
            .store
            .normalization_work(&fixture.claim, old_run.id, Page::new(0, 200)?)?;
    let proof = fixture.proof(&old_work[0])?;
    fixture
        .store
        .record_normalized_audio(&fixture.claim, old_run.id, 1, &proof)?;
    drop(fixture.store);
    let raw = Connection::open(&fixture.database)?;
    raw.execute(
        "UPDATE jobs SET lease_until = unixepoch() - 1 WHERE id = ?1",
        [fixture.job],
    )?;
    fixture.store = Store::open_existing(&fixture.database)?;
    assert!(
        fixture
            .store
            .record_normalized_audio(&fixture.claim, old_run.id, 1, &proof)
            .is_err()
    );
    let new_claim = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "worker", 300)?
        .ok_or("missing reclaimed owner")?
        .claim;
    assert!(fixture.store.normalization_run(&new_claim)?.is_none());
    let run = fixture
        .store
        .begin_normalization(&new_claim, &fixture.cover)?;
    assert_ne!(run.id, old_run.id);
    assert_ne!(run.staging_directory, old_run.staging_directory);
    assert_eq!(run.recipe_sha256, old_run.recipe_sha256);
    assert_eq!(run.verified_files, 0);
    assert!(
        fixture
            .store
            .normalization_work(&fixture.claim, old_run.id, Page::new(0, 200)?)
            .is_err()
    );
    assert!(
        fixture
            .store
            .record_normalized_audio(&fixture.claim, old_run.id, 2, &proof)
            .is_err()
    );
    assert!(
        fixture
            .store
            .record_normalized_audio(&new_claim, old_run.id, 2, &proof)
            .is_err()
    );
    assert!(
        fixture
            .store
            .record_normalized_cover(
                &fixture.claim,
                old_run.id,
                old_run.cover_bytes,
                &old_run.cover_sha256
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .finish_normalization(&fixture.claim, old_run.id)
            .is_err()
    );
    let old_receipts: i64 = raw.query_row(
        "SELECT count(*) FROM normalization_receipts WHERE run_id = ?1",
        [old_run.id],
        |row| row.get(0),
    )?;
    assert_eq!(old_receipts, 1);
    let work = fixture
        .store
        .normalization_work(&new_claim, run.id, Page::new(0, 200)?)?;
    assert!(work.iter().all(|item| item.receipt.is_none()));
    Ok(())
}

#[test]
fn normalization_resumes_and_schema_nine_and_ten_upgrades_preserve_file_proofs() -> TestResult {
    let mut fixture = Fixture::new(AudioFormat::Flac)?;
    drop(fixture.store);
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots;
         DROP TABLE artwork_approvals;
         DROP TABLE artwork_candidates;
         DROP TABLE private_roots;
         DROP TABLE application_identity;
         DROP TABLE normalization_receipts;
         DROP TABLE normalization_files;
         DROP TABLE normalization_runs;
         DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations;
         PRAGMA user_version = 9;",
    )?;
    fixture.store = Store::open_existing(&fixture.database)?;
    fixture.bind_roots()?;
    let version: i64 = raw.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 20);
    let selected = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123456789",
        &cover_source()?,
        None,
    )?;
    fixture
        .store
        .approve_artwork(fixture.job, fixture.version, selected.id, &selected.sha256)?;
    let run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
    assert_eq!(work.len(), 3);
    assert!(
        work.iter()
            .all(|item| item.input_sha256 == format!("{:x}", Sha256::digest(&fixture.source)))
    );
    let proof = fixture.proof(&work[0])?;
    fixture
        .store
        .record_normalized_audio(&fixture.claim, run.id, 1, &proof)?;
    drop(fixture.store);
    raw.execute_batch(
        "DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots;
         DROP TABLE artwork_approvals;
         DROP TABLE artwork_candidates;
         DROP TABLE private_roots;
         DROP TABLE application_identity;
         DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations;
         PRAGMA user_version = 10;",
    )?;
    fixture.store = Store::open_existing(&fixture.database)?;
    fixture.bind_roots()?;
    let selected = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123456789",
        &cover_source()?,
        None,
    )?;
    fixture
        .store
        .approve_artwork(fixture.job, fixture.version, selected.id, &selected.sha256)?;
    let saved = fixture
        .store
        .normalization_run(&fixture.claim)?
        .ok_or("missing resumed run")?;
    assert_eq!(saved.id, run.id);
    assert_eq!(saved.verified_files, 1);
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
    assert_eq!(work[0].receipt.as_ref(), Some(&proof));
    assert!(work[1].receipt.is_none());
    assert!(
        fixture
            .store
            .normalization_work(&fixture.claim, run.id + 1, Page::new(0, 200)?)
            .is_err()
    );
    Ok(())
}

#[test]
fn normalization_receipts_survive_index_changes_but_new_work_and_handoff_stop() -> TestResult {
    let mut fixture = Fixture::new(AudioFormat::Mp3)?;
    let run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
    let proofs = work
        .iter()
        .map(|item| fixture.proof(item))
        .collect::<Result<Vec<_>, _>>()?;
    scan_location(&mut fixture.store, fixture.main)?;
    assert!(
        fixture
            .store
            .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)
            .is_err()
    );
    for (item, proof) in work.iter().zip(&proofs) {
        fixture
            .store
            .record_normalized_audio(&fixture.claim, run.id, item.track_number, proof)?;
    }
    fixture.store.record_normalized_cover(
        &fixture.claim,
        run.id,
        run.cover_bytes,
        &run.cover_sha256,
    )?;
    assert!(
        fixture
            .store
            .finish_normalization(&fixture.claim, run.id)
            .is_err()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("normalizing")
    );
    assert_eq!(
        fixture
            .store
            .normalization_run(&fixture.claim)?
            .ok_or("missing run")?
            .verified_files,
        3
    );
    Ok(())
}

#[test]
fn normalization_transactions_roll_back_intent_receipts_and_handoff_together_with_events()
-> TestResult {
    let mut fixture = Fixture::new(AudioFormat::Mp3)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER injected_failure BEFORE INSERT ON normalization_files
        WHEN NEW.track_number = 2 BEGIN SELECT RAISE(ABORT, 'injected intent failure'); END;",
    )?;
    assert!(
        fixture
            .store
            .begin_normalization(&fixture.claim, &fixture.cover)
            .is_err()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("validating")
    );
    let runs: i64 = raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| {
        row.get(0)
    })?;
    assert_eq!(runs, 0);
    raw.execute_batch("DROP TRIGGER injected_failure;")?;
    let run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
    let proof = fixture.proof(&work[0])?;
    raw.execute_batch("CREATE TRIGGER injected_failure BEFORE INSERT ON events
        WHEN NEW.event_type = 'audio_normalized' BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;")?;
    assert!(
        fixture
            .store
            .record_normalized_audio(&fixture.claim, run.id, 1, &proof)
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .normalization_run(&fixture.claim)?
            .ok_or("missing run")?
            .verified_files,
        0
    );
    raw.execute_batch("DROP TRIGGER injected_failure;")?;
    for item in &work {
        fixture.store.record_normalized_audio(
            &fixture.claim,
            run.id,
            item.track_number,
            &fixture.proof(item)?,
        )?;
    }
    fixture.store.record_normalized_cover(
        &fixture.claim,
        run.id,
        run.cover_bytes,
        &run.cover_sha256,
    )?;
    raw.execute_batch("CREATE TRIGGER injected_failure BEFORE INSERT ON events
        WHEN NEW.event_type = 'normalization_verified' BEGIN SELECT RAISE(ABORT, 'injected handoff failure'); END;")?;
    assert!(
        fixture
            .store
            .finish_normalization(&fixture.claim, run.id)
            .is_err()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("normalizing")
    );
    assert!(
        fixture
            .store
            .normalization_run(&fixture.claim)?
            .ok_or("missing run")?
            .completed_at
            .is_none()
    );
    raw.execute_batch("DROP TRIGGER injected_failure;")?;
    fixture.store.finish_normalization(&fixture.claim, run.id)?;
    Ok(())
}

#[test]
fn normalization_rejects_wrong_payload_tag_cover_profile_and_format_bindings() -> TestResult {
    let mut fixture = Fixture::new(AudioFormat::Mp3)?;
    let run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
    let original = fixture.proof(&work[0])?;
    for index in 0..9 {
        let mut proof = original.clone();
        match index {
            0 => proof.input_sha256 = "0".repeat(64),
            1 => proof.input_bytes += 1,
            2 => proof.tags_sha256 = "0".repeat(64),
            3 => proof.cover_sha256 = "0".repeat(64),
            4 => proof.profile = 2,
            5 => proof.format = AudioFormat::Flac,
            6 => proof.sha256 = "not a checksum".into(),
            7 => proof.samples_per_channel += 1,
            8 => proof.audio_bytes = proof.bytes,
            _ => return Err("invalid fixture".into()),
        }
        assert!(
            fixture
                .store
                .record_normalized_audio(&fixture.claim, run.id, 1, &proof)
                .is_err(),
            "case {index}"
        );
    }
    fixture
        .store
        .record_normalized_audio(&fixture.claim, run.id, 1, &original)?;
    let mut changed = original;
    changed.sha256 = "0".repeat(64);
    assert!(
        fixture
            .store
            .record_normalized_audio(&fixture.claim, run.id, 1, &changed)
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .normalization_run(&fixture.claim)?
            .ok_or("missing run")?
            .verified_files,
        1
    );
    Ok(())
}
