#[path = "../support/private.rs"]
mod private;

include!("fixture.rs");

#[path = "acquisition.rs"]
mod acquisition;
#[path = "artwork.rs"]
mod artwork;
mod artwork_questions;
mod caa;
#[path = "playback.rs"]
mod playback;
mod publication;
mod service_acceptance;
#[path = "staging.rs"]
mod staging;

#[test]
fn normalization_profiles_legacy_plan_and_receipt_recover_without_new_fields() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Mp3, true)?;
    let mut legacy = fixture.store.plan(fixture.job, fixture.version)?;
    legacy.plan.audio = None;
    let body = serde_json::to_string(&legacy.plan)?;
    let raw = Connection::open(&fixture.database)?;
    // Reproduce a pre-upgrade immutable record, not an application mutation.
    raw.execute_batch("DROP TRIGGER immutable_plan_body;")?;
    raw.execute(
        "UPDATE job_plans SET body=?1 WHERE job_id=?2 AND version=?3",
        params![body, fixture.job, fixture.version],
    )?;
    raw.execute_batch("CREATE TRIGGER immutable_plan_body BEFORE UPDATE OF job_id, version, settings_version, body ON job_plans BEGIN SELECT RAISE(ABORT, 'plan bodies are immutable'); END;")?;
    let run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
    for item in &work {
        assert!(item.audio.is_none());
        let evidence = fixture.proof(item)?;
        assert!(evidence.audio.is_none());
        let json = serde_json::to_string(&evidence)?;
        assert!(!json.contains("\"audio\":"));
        assert_eq!(
            json,
            serde_json::to_string(&serde_json::from_str::<NormalizedAudio>(&json)?)?
        );
        fixture.store.record_normalized_audio(
            &fixture.claim,
            run.id,
            item.track_number,
            &evidence,
        )?;
    }
    let loaded = Store::open_current_existing(&fixture.database)?;
    assert_eq!(loaded.plan(fixture.job, fixture.version)?, legacy);
    let retained = loaded.normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
    assert!(retained.iter().all(|item| {
        item.receipt
            .as_ref()
            .is_some_and(|receipt| receipt.audio.is_none())
    }));
    Ok(())
}

#[test]
fn normalization_profiles_freeze_quality_reject_wrong_receipts_and_publish_every_profile()
-> TestResult {
    use domus::application::{normalize_album, publish_new_album};
    use domus::domain::{AudioProfile, AudioSelection};
    for profile in AudioProfile::ALL {
        let mut fixture = Fixture::build_with_profile(
            profile.format(),
            true,
            true,
            true,
            None,
            None,
            Some(profile),
        )?;
        let run = fixture
            .store
            .begin_normalization(&fixture.claim, &fixture.cover)?;
        let work = fixture
            .store
            .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
        assert!(
            work.iter()
                .all(|item| item.audio == Some(AudioSelection::new(profile)))
        );
        let mut wrong = fixture.proof(&work[0])?;
        wrong.audio = Some(AudioSelection::new(if profile == AudioProfile::Mp3Cbr320 {
            AudioProfile::Mp3Cbr128
        } else {
            AudioProfile::Mp3Cbr320
        }));
        assert!(
            fixture
                .store
                .record_normalized_audio(&fixture.claim, run.id, 1, &wrong)
                .is_err()
        );
        fixture
            .store
            .save_settings(Some(1), &OutputSettings::default())?;
        let staging = fixture.staging.as_ref().ok_or("staging")?;
        assert_eq!(
            normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover)?,
            run.id
        );
        publish_new_album(&mut fixture.store, &fixture.claim, staging, run.id)?;
        let plan = fixture.store.plan(fixture.job, fixture.version)?;
        assert_eq!(plan.plan.audio_profile()?, profile);
        let target = &plan.plan.destinations[0];
        let folder = PathBuf::from(&target.root).join(&plan.plan.outputs[0].relative_folder);
        for output in &plan.plan.outputs {
            let bytes = std::fs::read(folder.join(&output.filename))?;
            if profile.format() == AudioFormat::Mp3 {
                let evidence =
                    domus::infrastructure::mp3::inspect_mp3(&mut Cursor::new(bytes), profile)?;
                assert_eq!(evidence.audio_frames, 3);
            } else {
                assert!(
                    domus::infrastructure::flac::inspect_flac(&mut Cursor::new(bytes))?
                        .md5_verified
                );
            }
        }
    }
    Ok(())
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
    raw.execute_batch(include_str!("../support/drop_sharing.sql"))?;
    raw.execute_batch(
        "DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots;
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
    assert_eq!(version, 31);
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
    raw.execute_batch(include_str!("../support/drop_sharing.sql"))?;
    raw.execute_batch(
        "DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots;
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
