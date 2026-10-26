use super::*;

#[test]
fn transfer_summary_covers_large_albums_and_keeps_uncertainty_visible() -> TestResult {
    for track_count in [137, 1000] {
        let mut selection = edition();
        selection.tracks = (1..=track_count)
            .map(|number| PlannedTrack {
                artist: "Generated Artist".into(),
                title: format!("Track {number}"),
                disc: Some(1),
                disc_track: None,
            })
            .collect();
        let mut fixture = Fixture::with_edition(selection)?;
        let empty = fixture
            .store
            .transfer_summary(fixture.job, fixture.version)?;
        assert_eq!(empty.expected_tracks, track_count);
        assert_eq!(empty.total, 0);
        assert!(!empty.all_received);
        let source: Vec<_> = (1..=track_count)
            .map(|number| SourceFile {
                remote_path: format!("Generated Album\\{number}.mp3"),
                expected_bytes: 10,
            })
            .collect();
        fixture
            .store
            .begin_acquisition(&fixture.claim, "generated-peer", &source)?;
        let mut rows = Vec::new();
        let mut after = 0;
        loop {
            let page =
                fixture
                    .store
                    .transfers(fixture.job, fixture.version, Page::new(after, 50)?)?;
            if page.is_empty() {
                break;
            }
            after = page.last().ok_or("receipt missing")?.id;
            rows.extend(page);
        }
        assert_eq!(rows.len(), usize::from(track_count));
        assert!(rows.windows(2).all(|pair| pair[0].id < pair[1].id));
        let all_planned = fixture
            .store
            .transfer_summary(fixture.job, fixture.version)?;
        assert_eq!(all_planned.planned, track_count);
        assert!(!all_planned.all_received);
        let digest: [u8; 32] = Sha256::digest(b"0123456789").into();
        for file in &rows[..rows.len() - 1] {
            if file.track_number % 100 == 0 {
                fixture.store.renew_claim(&fixture.claim, 60)?;
            }
            fixture.store.start_transfer(&fixture.claim, file.id)?;
            fixture
                .store
                .accept_transfer(&fixture.claim, file.id, u32::from(file.track_number))?;
            fixture
                .store
                .receive_transfer(&fixture.claim, file.id, 10, &digest)?;
        }
        let last = rows.last().ok_or("last receipt missing")?.id;
        let pending = fixture
            .store
            .transfer_summary(fixture.job, fixture.version)?;
        assert_eq!(pending.received, track_count - 1);
        assert_eq!(pending.planned, 1);
        assert!(!pending.all_received);
        fixture.store.start_transfer(&fixture.claim, last)?;
        let uncertain = fixture
            .store
            .transfer_summary(fixture.job, fixture.version)?;
        assert_eq!(uncertain.submitting, 1);
        assert_eq!(uncertain.received, track_count - 1);
        assert!(!uncertain.all_received);
        fixture.store.accept_transfer(&fixture.claim, last, 1001)?;
        let active = fixture
            .store
            .transfer_summary(fixture.job, fixture.version)?;
        assert_eq!(active.active, 1);
        assert!(!active.all_received);
        fixture
            .store
            .receive_transfer(&fixture.claim, last, 10, &digest)?;
        let observer = rusqlite::Connection::open(&fixture.database)?;
        let before: i64 = observer.pragma_query_value(None, "data_version", |row| row.get(0))?;
        let snapshot = Store::open_read_snapshot(&fixture.database)?;
        let complete = snapshot.transfer_summary(fixture.job, fixture.version)?;
        assert_eq!(complete.job_id, fixture.job);
        assert_eq!(complete.plan_version, fixture.version);
        assert_eq!(complete.expected_tracks, track_count);
        assert_eq!(complete.total, track_count);
        assert_eq!(complete.received, track_count);
        assert_eq!(
            (complete.planned, complete.submitting, complete.active),
            (0, 0, 0)
        );
        assert!(complete.all_received);
        assert_eq!(
            fixture
                .store
                .transfers(fixture.job, fixture.version, Page::new(0, 100)?)?
                .len(),
            100
        );
        for (job, version) in [
            (fixture.job, fixture.version + 1),
            (fixture.job + 1, fixture.version),
            (0, fixture.version),
        ] {
            assert!(fixture.store.transfer_summary(job, version).is_err());
        }
        let after: i64 = observer.pragma_query_value(None, "data_version", |row| row.get(0))?;
        assert_eq!(before, after, "diagnostic reads must not write");
    }
    Ok(())
}

#[test]
fn transfer_summary_does_not_mix_jobs() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture
        .store
        .begin_acquisition(&fixture.claim, "generated-peer", &files())?;
    let other = fixture.store.enqueue(1)?;
    let mut other_edition = edition();
    other_edition.release_id = "22345678-1234-4321-9876-123456789abc".into();
    let catalog = support::catalog_snapshot(&fixture.database, &other_edition)?;
    let plan = fixture.store.prepare_catalog_plan(
        PlanRequest {
            job_id: other,
            expected_version: 1,
            settings_version: 1,
            format: None,
            edition: catalog.data.selection(None)?,
            locations: vec![fixture.main],
            accepts_incomplete_coverage: false,
        },
        catalog,
    )?;
    let summary = fixture.store.transfer_summary(other, plan.version)?;
    assert_eq!(summary.expected_tracks, 2);
    assert_eq!(summary.total, 0);
    assert!(!summary.all_received);
    assert_eq!(
        fixture
            .store
            .transfer_summary(fixture.job, fixture.version)?
            .planned,
        2
    );
    Ok(())
}
