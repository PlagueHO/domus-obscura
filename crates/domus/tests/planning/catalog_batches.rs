use super::*;
use domus::Error;
use domus::domain::CatalogRelease;
use domus::infrastructure::catalog::CatalogResult;
use rusqlite::params;

type BatchEntry = (PlanRequest, CatalogResult<CatalogRelease>);

#[test]
fn catalog_batch_profiles_are_frozen_and_rejection_is_atomic() -> TestResult {
    use domus::domain::{AudioPolicy, AudioProfile};
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let policy = AudioPolicy {
        accepted: vec![AudioProfile::Mp3Cbr192, AudioProfile::Mp3Vbr],
        preferred: AudioProfile::Mp3Cbr192,
        ..AudioPolicy::default()
    };
    fixture.store.save_settings(
        Some(1),
        &OutputSettings {
            format: AudioFormat::Mp3,
            accepted_formats: Some(vec![AudioFormat::Mp3]),
            audio_policy: Some(policy),
            ..OutputSettings::default()
        },
    )?;
    let mut first = entry(&mut fixture, 1)?;
    first.0.settings_version = 2;
    first.0.format = None;
    first.0.audio_profile = Some(AudioProfile::Mp3Cbr192);
    let mut second = entry(&mut fixture, 2)?;
    second.0.settings_version = 2;
    second.0.format = None;
    second.0.audio_profile = Some(AudioProfile::Mp3Cbr128);
    let before = counts(&fixture)?;
    assert!(
        fixture
            .store
            .prepare_catalog_plans(vec![first.clone(), second.clone()])
            .is_err()
    );
    assert_eq!(counts(&fixture)?, before);
    second.0.audio_profile = Some(AudioProfile::Mp3Vbr);
    let plans = fixture.store.prepare_catalog_plans(vec![first, second])?;
    assert_eq!(plans[0].plan.audio_profile()?, AudioProfile::Mp3Cbr192);
    assert_eq!(plans[1].plan.audio_profile()?, AudioProfile::Mp3Vbr);
    assert!(plans.iter().all(|plan| plan.approved_at.is_none()));
    Ok(())
}

fn entry(fixture: &mut Fixture, number: u16) -> TestResult<BatchEntry> {
    let mut edition = edition();
    edition.release_id = format!("20000000-0000-4000-8000-{number:012x}");
    edition.album = format!("Generated Album {number}");
    for (index, track) in edition.tracks.iter_mut().enumerate() {
        track.disc = Some(1);
        track.disc_track = Some(u16::try_from(index + 1)?);
    }
    let catalog = support::catalog_snapshot(&fixture.database, &edition)?;
    Ok((
        PlanRequest {
            audio_profile: None,
            job_id: fixture.store.enqueue(1)?,
            expected_version: 1,
            settings_version: 1,
            format: Some(if number.is_multiple_of(2) {
                AudioFormat::Flac
            } else {
                AudioFormat::Mp3
            }),
            edition,
            locations: vec![fixture.main],
            accepts_incomplete_coverage: false,
        },
        catalog,
    ))
}

fn counts(fixture: &Fixture) -> TestResult<(i64, i64)> {
    let connection = rusqlite::Connection::open(&fixture.database)?;
    Ok(connection.query_row(
        "SELECT (SELECT count(*) FROM job_plans), (SELECT count(*) FROM events)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

#[test]
fn catalog_batch_freezes_independent_formats_and_requires_separate_approval() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let first = entry(&mut fixture, 1)?;
    let second = entry(&mut fixture, 2)?;
    let before = counts(&fixture)?;
    let plans = fixture.store.prepare_catalog_plans(vec![first, second])?;
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].plan.format, AudioFormat::Mp3);
    assert_eq!(plans[1].plan.format, AudioFormat::Flac);
    assert_eq!(counts(&fixture)?, (before.0 + 2, before.1 + 2));
    drop(fixture.store);
    fixture.store = Store::open_existing(&fixture.database)?;
    for plan in &plans {
        assert_eq!(plan.version, 2);
        assert!(plan.approved_at.is_none());
        assert_eq!(fixture.store.plan(plan.job_id, plan.version)?, *plan);
        assert_eq!(
            fixture.store.job_state(plan.job_id)?.as_deref(),
            Some("queued")
        );
        assert!(matches!(
            fixture
                .store
                .claim_approved_plan(plan.job_id, plan.version, "worker", 60),
            Err(Error::Conflict(_))
        ));
    }
    fixture
        .store
        .approve_plan(plans[0].job_id, plans[0].version)?;
    assert!(
        fixture
            .store
            .plan(plans[1].job_id, plans[1].version)?
            .approved_at
            .is_none()
    );
    assert_eq!(std::fs::read_dir(fixture.media.path())?.count(), 0);
    Ok(())
}

#[test]
fn catalog_batch_rejects_duplicate_jobs_releases_forged_data_and_folder_collisions() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    for case in 0..4 {
        let first = entry(&mut fixture, case * 2 + 1)?;
        let mut second = entry(&mut fixture, case * 2 + 2)?;
        match case {
            0 => second.0.job_id = first.0.job_id,
            1 => {
                second.0.edition = first.0.edition.clone();
                second.1 = first.1.clone();
            }
            2 => second.1.data.summary.title.push_str(" forged"),
            _ => {
                let connection = rusqlite::Connection::open(&fixture.database)?;
                connection.execute(
                    "DELETE FROM catalog_cache WHERE source=?1",
                    [&second.1.source],
                )?;
                second.0.edition.album = first.0.edition.album.to_lowercase();
                second.1 = support::catalog_snapshot(&fixture.database, &second.0.edition)?;
            }
        }
        let before = counts(&fixture)?;
        assert!(
            fixture
                .store
                .prepare_catalog_plans(vec![first, second])
                .is_err()
        );
        assert_eq!(counts(&fixture)?, before);
    }
    assert!(fixture.store.prepare_catalog_plans(vec![]).is_err());
    Ok(())
}

#[test]
fn catalog_batch_rolls_back_previous_albums_when_later_job_or_sql_write_fails() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let first = entry(&mut fixture, 1)?;
    let mut second = entry(&mut fixture, 2)?;
    let first_job = first.0.job_id;
    second.0.expected_version = 7;
    let before = counts(&fixture)?;
    assert!(matches!(
        fixture.store.prepare_catalog_plans(vec![first, second]),
        Err(Error::Conflict(_))
    ));
    assert_eq!(counts(&fixture)?, before);
    assert!(fixture.store.plan(first_job, 2).is_err());
    let connection = rusqlite::Connection::open(&fixture.database)?;
    assert_eq!(
        connection.query_row(
            "SELECT plan_version FROM jobs WHERE id=?1",
            [first_job],
            |row| row.get::<_, i64>(0)
        )?,
        1
    );
    let first = entry(&mut fixture, 3)?;
    let second = entry(&mut fixture, 4)?;
    connection.execute_batch(&format!(
        "CREATE TRIGGER fail_batch BEFORE INSERT ON job_plans WHEN NEW.job_id={}
         BEGIN SELECT RAISE(ABORT,'fixture batch failure'); END;",
        second.0.job_id
    ))?;
    let first_job = first.0.job_id;
    let before = counts(&fixture)?;
    assert!(
        fixture
            .store
            .prepare_catalog_plans(vec![first, second])
            .is_err()
    );
    assert_eq!(counts(&fixture)?, before);
    assert_eq!(
        connection.query_row(
            "SELECT plan_version FROM jobs WHERE id=?1",
            params![first_job],
            |row| row.get::<_, i64>(0)
        )?,
        1
    );
    Ok(())
}

#[test]
fn catalog_batch_accepts_exact_request_bound_and_rejects_an_extra_album() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let mut entries = Vec::new();
    for number in 1..=26 {
        entries.push(entry(&mut fixture, number)?);
    }
    let before = counts(&fixture)?;
    assert!(fixture.store.prepare_catalog_plans(entries).is_err());
    assert_eq!(counts(&fixture)?, before);
    let mut entries = Vec::new();
    for number in 27..=51 {
        entries.push(entry(&mut fixture, number)?);
    }
    assert_eq!(fixture.store.prepare_catalog_plans(entries)?.len(), 25);
    Ok(())
}
