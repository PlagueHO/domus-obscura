use super::*;
use domus::Error;
use rusqlite::params;

#[test]
fn catalog_evidence_is_required_for_new_transfers_but_not_existing_receipts() -> TestResult {
    let mut fixture = Fixture::new()?;
    let job = fixture.store.enqueue(1)?;
    let plan = fixture.store.prepare_plan(PlanRequest {
        audio_profile: None,
        job_id: job,
        expected_version: 1,
        settings_version: 1,
        format: None,
        edition: edition(),
        locations: vec![fixture.main],
        accepts_incomplete_coverage: false,
    })?;
    fixture.store.approve_plan(job, plan.version)?;
    let claim = fixture
        .store
        .claim_approved_plan(job, plan.version, "manual-worker", 60)?
        .ok_or("manual fixture claim missing")?
        .claim;
    fixture
        .store
        .advance(&claim, JobState::Queued, JobState::Searching)?;
    assert!(matches!(
        fixture
            .store
            .begin_acquisition(&claim, "generated-peer", &files()),
        Err(Error::Conflict(_))
    ));
    assert_eq!(fixture.store.job_state(job)?.as_deref(), Some("searching"));
    assert!(
        fixture
            .store
            .transfers(job, plan.version, Page::new(0, 200)?)?
            .is_empty()
    );

    let connection = rusqlite::Connection::open(&fixture.database)?;
    connection.execute(
        "INSERT INTO acquisition_sources(job_id,plan_version,peer) VALUES (?1,?2,'legacy-peer')",
        params![job, plan.version],
    )?;
    let mut ids = Vec::new();
    for (number, state) in [(1, "planned"), (2, "submitting")] {
        connection.execute(
            "INSERT INTO transfer_files(job_id,plan_version,track_number,remote_path,expected_bytes,state)
             VALUES (?1,?2,?3,?4,10,?5)",
            params![job,plan.version,number,format!("Generated Album\\{number}.mp3"),state],
        )?;
        ids.push(connection.last_insert_rowid());
    }
    connection.execute("UPDATE jobs SET state='downloading' WHERE id=?1", [job])?;
    assert!(matches!(
        fixture.store.start_transfer(&claim, ids[0]),
        Err(Error::Conflict(_))
    ));
    fixture.store.accept_transfer(&claim, ids[1], 7)?;
    fixture
        .store
        .receive_transfer(&claim, ids[1], 10, &Sha256::digest(b"0123456789").into())?;
    let rows = fixture
        .store
        .transfers(job, plan.version, Page::new(0, 200)?)?;
    assert_eq!(rows[0].state, "planned");
    assert_eq!(rows[1].state, "received");
    Ok(())
}

#[test]
fn catalog_frozen_approval_survives_provider_cache_eviction() -> TestResult {
    let mut fixture = Fixture::new()?;
    rusqlite::Connection::open(&fixture.database)?.execute("DELETE FROM catalog_cache", [])?;
    fixture
        .store
        .begin_acquisition(&fixture.claim, "generated-peer", &files())?;
    let rows = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    fixture.store.start_transfer(&fixture.claim, rows[0].id)?;
    assert_eq!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?[0]
            .state,
        "submitting"
    );
    Ok(())
}
