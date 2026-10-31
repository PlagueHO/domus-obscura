use domus::domain::{LibraryPath, Page};
use domus::infrastructure::store::Store;
use tempfile::tempdir;

/// Generated recovery history evidence, not physical file or memory acceptance.
#[test]
#[ignore = "separate million-record scale check: cargo task scale"]
fn million_recovery_records_keep_inventory_pages_bounded_and_indexed()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("recovery-scale.db");
    let mut store = Store::open(&database)?;
    let settings = store.save_settings(None, &domus::domain::OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute(
        "INSERT INTO job_plans(job_id,version,settings_version,body) VALUES (?1,1,?2,'{}')",
        rusqlite::params![job, settings],
    )?;
    raw.execute(
        "WITH RECURSIVE jobs(id) AS (
             SELECT ?1+1 UNION ALL SELECT id+1 FROM jobs WHERE id<?1+100
         )
         INSERT INTO jobs(id,plan_version,state)
         SELECT id,1,'queued' FROM jobs",
        [job],
    )?;
    raw.execute(
        "WITH RECURSIVE jobs(id) AS (
             SELECT ?2+1 UNION ALL SELECT id+1 FROM jobs WHERE id<?2+100
         )
         INSERT INTO job_plans(job_id,version,settings_version,body)
         SELECT id,1,?1,'{}' FROM jobs",
        rusqlite::params![settings, job],
    )?;
    raw.execute(
        "WITH RECURSIVE n(v) AS (SELECT 1 UNION ALL SELECT v+1 FROM n WHERE v<1000000)
         INSERT INTO normalization_runs(id,job_id,plan_version,claim_generation,profile,
          recipe_sha256,cover_sha256,cover_bytes,cover_width,cover_height)
         SELECT v,?1+((v-1)%101),1,v,1,?2,?2,1,1,1 FROM n",
        rusqlite::params![job, "a".repeat(64)],
    )?;
    for after in [0, 4_000_001, 7_950_001] {
        let rows = store.recovery_inventory(Page::new(after, 50)?, Some(job))?;
        assert_eq!(rows.len(), 50, "job page after {after}");
        assert!(
            rows.iter()
                .all(|r| r.id > after && r.category == "scratch" && r.recorded_bytes.is_none())
        );
        assert!(rows.windows(2).all(|pair| pair[0].id < pair[1].id));
    }
    let sparse_job = job + 100;
    let sparse = store.recovery_inventory(Page::new(4_000_001, 50)?, Some(sparse_job))?;
    assert_eq!(sparse.len(), 50);
    assert!(
        sparse
            .iter()
            .all(|row| row.job_id == sparse_job && row.id > 4_000_001 && row.category == "scratch")
    );
    let plan = raw
        .prepare(
            "EXPLAIN QUERY PLAN SELECT id FROM recovery_inventory WHERE id IN (4000001,4000009)",
        )?
        .query_map([], |r| r.get::<_, String>(3))?
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    for index in [
        "recovery_input_cursor",
        "recovery_normalization_cursor",
        "recovery_pending_cursor",
        "recovery_retained_cursor",
    ] {
        assert!(plan.contains(index), "{index}: {plan}");
    }
    for (sql, index) in [
        (
            "EXPLAIN QUERY PLAN SELECT id FROM transfer_files WHERE job_id=101 AND id>=1 ORDER BY id LIMIT 51",
            "recovery_input_job_cursor",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM normalization_runs WHERE job_id=101 AND id>=1 ORDER BY id LIMIT 51",
            "recovery_normalization_job_cursor",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM publication_destinations
             INDEXED BY recovery_pending_job_location_cursor
             WHERE job_id=101 AND location_id=1 AND id>=1 AND published_at IS NULL
             ORDER BY id LIMIT 51",
            "recovery_pending_job_location_cursor",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM publication_destinations
             INDEXED BY recovery_pending_job_cursor
             WHERE job_id=101 AND id>=1 AND published_at IS NULL
             ORDER BY id LIMIT 51",
            "recovery_pending_job_cursor",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM publication_destinations
             INDEXED BY recovery_pending_location_cursor
             WHERE location_id=1 AND id>=1 AND published_at IS NULL
             ORDER BY id LIMIT 51",
            "recovery_pending_location_cursor",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT r.destination_id FROM publication_destinations d
             INDEXED BY recovery_destination_job_location_cursor
             JOIN publication_replacement_journal r ON r.destination_id=d.id
             WHERE d.job_id=101 AND d.location_id=1 AND d.id>=1 AND r.state!='planned'
             ORDER BY d.id LIMIT 51",
            "recovery_destination_job_location_cursor",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT r.destination_id FROM publication_destinations d
             INDEXED BY recovery_destination_job_cursor
             JOIN publication_replacement_journal r ON r.destination_id=d.id
             WHERE d.job_id=101 AND d.id>=1 AND r.state!='planned'
             ORDER BY d.id LIMIT 51",
            "recovery_destination_job_cursor",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT r.destination_id FROM publication_destinations d
             INDEXED BY recovery_destination_location_cursor
             JOIN publication_replacement_journal r ON r.destination_id=d.id
             WHERE d.location_id=1 AND d.id>=1 AND r.state!='planned'
             ORDER BY d.id LIMIT 51",
            "recovery_destination_location_cursor",
        ),
    ] {
        let details = raw
            .prepare(sql)?
            .query_map([], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?
            .join("\n");
        assert!(details.contains(index), "{index}: {details}");
    }
    Ok(())
}

/// Generated tag-index/query evidence, not physical scans or process memory.
#[test]
#[ignore = "separate million-record scale check: cargo task scale"]
fn million_library_metadata_records_keep_pages_matches_and_counts_indexed()
-> Result<(), Box<dyn std::error::Error>> {
    use domus::domain::library::{LibraryMatchQuery, LibraryMetadata};
    let data = tempdir()?;
    let media = tempdir()?;
    let database = data.path().join("library-scale.db");
    let mut store = Store::open(&database)?;
    let location = domus::application::add_location(&store, media.path(), "main")?;
    let claim = store.begin_scan(location)?;
    let raw = rusqlite::Connection::open(&database)?;
    // Bulk fixture SQL does not run the scanner's per-file lease renewal.
    raw.execute(
        "UPDATE scans SET lease_until=unixepoch()+600 WHERE id=?1",
        [claim.id],
    )?;
    raw.execute_batch(
        "WITH RECURSIVE n(v) AS (SELECT 1 UNION ALL SELECT v+1 FROM n WHERE v<10000)
         INSERT INTO library_artists(id,location_id,name_key,name)
         SELECT v,1,'artist '||v,'Artist '||v FROM n;
         WITH RECURSIVE n(v) AS (SELECT 1 UNION ALL SELECT v+1 FROM n WHERE v<100000)
         INSERT INTO library_albums(id,location_id,artist_id,title_key,title,folder)
         SELECT v,1,1+(v-1)/10,'album '||v,'Album '||v,'folder-'||v FROM n;",
    )?;
    raw.execute(
        "WITH RECURSIVE n(v) AS (SELECT 1 UNION ALL SELECT v+1 FROM n WHERE v<1000000)
         INSERT INTO entries(id,location_id,relative_path,parent_path,kind,size,scan_id)
         SELECT v,?1,'folder-'||(1+(v-1)/10)||'/track-'||v||'.mp3','folder-'||(1+(v-1)/10),'file',100,?2 FROM n",
        rusqlite::params![location,claim.id],
    )?;
    let body = serde_json::to_string(&LibraryMetadata {
        status: "tagged".into(),
        ..Default::default()
    })?;
    raw.execute(
        "INSERT INTO library_metadata(entry_id,location_id,scan_id,body,status,active,artist_id,album_id)
         SELECT id,location_id,scan_id,
         json_set(?1,'$.tags.title','Generated track',
          '$.tags.artists',json_array('Artist '||(1+(id-1)/100)),
          '$.tags.album','Album '||(1+(id-1)/10)),
         'tagged',1,1+(id-1)/100,1+(id-1)/10 FROM entries",
        [body],
    )?;
    store.complete_directory(&claim, &LibraryPath::parse("")?)?;
    assert!(store.finish_scan(&claim)?);
    for after in [0, 5000, 9950] {
        let rows = store.library_artists(location, Page::new(after, 50)?)?;
        assert_eq!(rows.len(), 50);
        assert_eq!(rows[0].id, after + 1);
        assert!(rows.iter().all(|row| row.file_count == 100));
    }
    for after in [0, 50000, 99950] {
        let rows = store.library_albums(location, None, Page::new(after, 50)?)?;
        assert_eq!(rows.len(), 50);
        assert!(rows.iter().all(|row| row.file_count == 10));
        assert_eq!(rows[0].id, after + 1);
    }
    let artist_albums = store.library_albums(location, Some(10000), Page::new(0, 50)?)?;
    assert_eq!(artist_albums.len(), 10);
    let files = store.library_album_files(location, 100000, Page::new(999990, 5)?)?;
    assert_eq!(files.len(), 5);
    assert_eq!(files[0].id, 999991);
    assert!(
        store
            .library_album_assets(location, 100000, Page::new(0, 50)?)?
            .is_empty()
    );
    assert_eq!(
        store
            .library_file(location, 1000000)?
            .metadata
            .tags
            .album
            .as_deref(),
        Some("Album 100000")
    );
    let matched = store.library_matches(
        location,
        &[LibraryMatchQuery {
            artist: "Artist 10000".into(),
            album: Some("Album 100000".into()),
        }],
    )?;
    assert_eq!(matched.matches[0].albums[0].id, 100000);
    assert_eq!(
        (
            matched.coverage.metadata_files,
            matched.coverage.metadata_issues,
            matched.coverage.unobserved_files
        ),
        (1000000, 0, 0)
    );
    for (sql, index) in [
        (
            "SELECT id FROM library_artists WHERE location_id=1 AND file_count>0 AND id>9950 ORDER BY id LIMIT 50",
            "library_artist_page",
        ),
        (
            "SELECT id FROM library_albums WHERE location_id=1 AND file_count>0 AND id>99950 ORDER BY id LIMIT 50",
            "library_album_page",
        ),
        (
            "SELECT id FROM library_albums WHERE location_id=1 AND artist_id=10000 AND file_count>0 AND id>0 ORDER BY id LIMIT 50",
            "library_artist_albums",
        ),
        (
            "SELECT id FROM library_albums WHERE location_id=1 AND artist_id=10000 AND title_key='album 100000' AND file_count>0 ORDER BY id LIMIT 50",
            "library_album_match",
        ),
        (
            "SELECT entry_id FROM library_metadata WHERE location_id=1 AND album_id=100000 AND active=1 AND entry_id>999990 ORDER BY entry_id LIMIT 50",
            "library_album_files",
        ),
    ] {
        let details = raw
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
            .query_map([], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            details.iter().any(|detail| detail.contains(index)),
            "missing {index}: {details:?}"
        );
    }
    Ok(())
}

/// Generated worker-history/query evidence, not execution or process-memory evidence.
#[test]
#[ignore = "separate million-record scale check: cargo task scale"]
fn million_worker_requests_keep_filtering_and_eligible_selection_indexed()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("worker-scale.db");
    let store = Store::open(&database)?;
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch(
        "WITH RECURSIVE sequence(value) AS (
             SELECT 1 UNION ALL SELECT value+1 FROM sequence WHERE value<1000000
         )
         INSERT INTO worker_requests(lane,target,body,configuration_binding,state)
         SELECT 'acquisition','job-'||value,
                json_object('operation','acquire','job_id',value,'plan_version',1,
                    'source_candidate',NULL,'acknowledge_new_only_publication',json('true')),
                'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                CASE WHEN value<=999900 THEN 'complete' ELSE 'queued' END
         FROM sequence;",
    )?;
    let status = store.worker_status(1024)?;
    assert_eq!(status.unfinished, 100);
    assert_eq!(status.active, 0);
    assert!(!status.online);
    let rows = store.worker_requests(Page::new(999900, 50)?)?;
    assert_eq!(rows.len(), 50);
    assert_eq!(rows[0].id, 999901);
    assert_eq!(rows[49].id, 999950);
    let one = store.worker_requests_for_job(Page::new(0, 50)?, Some(1000000))?;
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].id, 1000000);
    for (sql, index) in [
        (
            "EXPLAIN QUERY PLAN SELECT id FROM worker_requests WHERE target='job-1000000' AND id>0 ORDER BY id LIMIT 50",
            "worker_request_job_history",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM worker_requests WHERE lane='acquisition' AND state IN ('queued','interrupted','running') AND (state IN ('queued','interrupted') OR (state='running' AND lease_until<=unixepoch())) AND (lease_until IS NULL OR lease_until<=unixepoch()) ORDER BY id LIMIT 1",
            "worker_request_eligible",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM worker_requests WHERE id>0 AND state!='complete' ORDER BY id LIMIT 50",
            "worker_request_unfinished_history",
        ),
    ] {
        let details = raw
            .prepare(sql)?
            .query_map([], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            details.iter().any(|detail| detail.contains(index)),
            "missing bounded query index: {details:?}"
        );
    }
    Ok(())
}

/// Generated plan-history/query evidence only, not worker or memory evidence.
#[test]
#[ignore = "separate million-record scale check: cargo task scale"]
fn million_events_filter_by_plan_before_pagination() -> Result<(), Box<dyn std::error::Error>> {
    use domus::application::{add_location, scan_location};
    use domus::domain::{EditionSelection, OutputSettings, PlannedTrack};
    use domus::infrastructure::store::PlanRequest;
    let data = tempdir()?;
    let media = tempdir()?;
    let database = data.path().join("event-scale.db");
    let mut store = Store::open(&database)?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;
    store.save_settings(None, &OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let other = store.enqueue(1)?;
    let saved = store.prepare_plan(PlanRequest {
        audio_profile: None,
        job_id: job,
        expected_version: 1,
        settings_version: 1,
        format: None,
        locations: vec![location],
        accepts_incomplete_coverage: false,
        edition: EditionSelection {
            release_id: "12345678-1234-4321-9876-123456789abc".into(),
            album_artist: "Generated Artist".into(),
            album: "Generated Album".into(),
            bucket: None,
            tracks: vec![PlannedTrack {
                artist: "Generated Artist".into(),
                title: "Generated Track".into(),
                disc: None,
                disc_track: None,
            }],
        },
    })?;
    let mut cursor = store.last_event_id()?;
    let raw = rusqlite::Connection::open(&database)?;
    let initial_count: i64 = raw.query_row("SELECT count(*) FROM events", [], |row| row.get(0))?;
    raw.execute(
        "WITH RECURSIVE sequence(value) AS (
             SELECT 1 UNION ALL SELECT value+1 FROM sequence WHERE value<1000000
         )
         INSERT INTO events(job_id,plan_version,event_type)
         SELECT CASE WHEN value<=500000 THEN ?1 ELSE ?2 END,
                CASE WHEN value<=999900 THEN 1 ELSE ?3 END, 'generated_event'
         FROM sequence",
        rusqlite::params![other, job, saved.version],
    )?;
    let mut count = 0;
    loop {
        let rows = store.plan_events(job, saved.version, Page::new(cursor, 50)?)?;
        assert!(rows.len() <= 50);
        if rows.is_empty() {
            break;
        }
        for event in rows {
            assert!(event.id > cursor);
            assert_eq!(event.job_id, job);
            assert_eq!(event.plan_version, Some(saved.version));
            assert_eq!(event.event_type, "generated_event");
            cursor = event.id;
            count += 1;
        }
    }
    assert_eq!(count, 100);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM events", [], |row| row
            .get::<_, i64>(0))?,
        initial_count + 1000000
    );
    Ok(())
}

/// Indexed-record/query evidence only. This does not measure physical scanning,
/// tag decoding, transfers, services, or combined process resident memory.
#[test]
#[ignore = "separate million-record scale check: cargo task scale"]
fn million_records_remain_paginated() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let media = tempdir()?;
    let database = data.path().join("scale.db");
    let mut store = Store::open(&database)?;
    let root = std::fs::canonicalize(media.path())?;
    let location = store.add_location(root.to_str().ok_or("non-Unicode root")?, "main")?;
    let claim = store.begin_scan(location)?;
    let root = LibraryPath::parse("")?;
    store.complete_directory(&claim, &root)?;
    assert!(store.finish_scan(&claim)?);
    // Generated query evidence needs no live scan lease during fixture loading.
    let mut raw = rusqlite::Connection::open(&database)?;
    let transaction = raw.transaction()?;
    assert_eq!(
        transaction.execute(
            "WITH RECURSIVE n(v) AS (SELECT 0 UNION ALL SELECT v+1 FROM n WHERE v<999999)
             INSERT INTO entries(location_id,relative_path,parent_path,kind,size,modified_at,scan_id)
             SELECT ?1,printf('track-%07d.mp3',v),'','file',1024,1,?2 FROM n",
            rusqlite::params![location, claim.id],
        )?,
        1_000_000
    );
    assert_eq!(
        transaction.execute(
            "UPDATE scans SET observed_count=1000000 WHERE id=?1 AND status='complete'",
            [claim.id],
        )?,
        1
    );
    transaction.commit()?;
    let mut cursor = 0;
    let mut count = 0;
    loop {
        let page = store.browse(location, &root, Page::new(cursor, 200)?)?;
        assert!(page.len() <= 200);
        if page.is_empty() {
            break;
        }
        for entry in page {
            assert!(entry.id > cursor);
            cursor = entry.id;
            count += 1;
        }
    }
    assert_eq!(count, 1_000_000);
    Ok(())
}

/// Generated queue-history evidence, not downloads or process-memory evidence.
#[test]
#[ignore = "separate million-record scale check: cargo task scale"]
fn million_queue_records_filter_before_pagination() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("queue-scale.db");
    let store = Store::open(&database)?;
    let mut raw = rusqlite::Connection::open(&database)?;
    let transaction = raw.transaction()?;
    transaction.execute_batch(
        "WITH RECURSIVE sequence(value) AS (
             SELECT 1 UNION ALL SELECT value+1 FROM sequence WHERE value<1000000
         )
         INSERT INTO jobs(id,plan_version,state)
         SELECT value,1,CASE WHEN value<=999900 THEN 'complete' ELSE 'queued' END
         FROM sequence;
         UPDATE queue_presentations SET completed_at=unixepoch()-604800;",
    )?;
    transaction.commit()?;
    let mut cursor = 0;
    let mut count = 0;
    loop {
        let rows = store.queue(Page::new(cursor, 50)?)?;
        assert!(rows.len() <= 50);
        if rows.is_empty() {
            break;
        }
        for row in rows {
            assert!(row.id > 999900 && row.id > cursor);
            assert_eq!(row.state, "queued");
            cursor = row.id;
            count += 1;
        }
    }
    assert_eq!(count, 100);
    assert_eq!(cursor, 1000000);
    assert_eq!(store.jobs(Page::new(0, 50)?)?[0].id, 1);
    Ok(())
}
