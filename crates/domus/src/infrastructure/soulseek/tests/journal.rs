use super::*;
use crate::application::{add_location, negotiate_transfer, scan_location};
use crate::domain::{AudioFormat, JobState, OutputSettings, Page};
use crate::infrastructure::catalog::{CatalogQuery, CatalogResult};
use crate::infrastructure::store::{JobClaim, PlanRequest, SourceFile, Store};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tempfile::{TempDir, tempdir};

struct Fixture {
    store: Store,
    claim: JobClaim,
    job: i64,
    version: i64,
    file: i64,
    database: PathBuf,
    location: i64,
    _state: TempDir,
    _library: TempDir,
}

mod files;
mod worker;

impl Fixture {
    fn new() -> std::result::Result<Self, Box<dyn std::error::Error>> {
        Self::album(AudioFormat::Flac, 1000, 1, false)
    }

    fn album(
        format: AudioFormat,
        size: i64,
        tracks: u16,
        cover: bool,
    ) -> std::result::Result<Self, Box<dyn std::error::Error>> {
        Self::build(format, size, tracks, cover, true)
    }

    fn build(
        format: AudioFormat,
        size: i64,
        tracks: u16,
        cover: bool,
        select_source: bool,
    ) -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let state = tempdir()?;
        let library = tempdir()?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let location = add_location(&store, library.path(), "main")?;
        scan_location(&mut store, location)?;
        store.save_settings(None, &OutputSettings::default())?;
        let job = store.enqueue(1)?;
        let release = "20000000-0000-4000-8000-000000000001";
        let credit = json!([{
            "artist":{"id":"10000000-0000-4000-8000-000000000001","name":"Fixture Artist"},
            "name":"Fixture Artist","joinphrase":""
        }]);
        let selected = (1..=tracks)
            .map(|track| {
                json!({
                    "id":format!("40000000-0000-4000-8000-{track:012x}"),
                    "position":track,"number":track.to_string(),
                    "title":format!("Fixture Track {track}"),"artist-credit":credit,
                    "recording":{"id":format!("50000000-0000-4000-8000-{track:012x}"),"video":false}
                })
            })
            .collect::<Vec<_>>();
        let body = serde_json::to_vec(&json!({
            "id":release, "title":"Fixture Album", "artist-credit":credit,
            "release-group":{"id":"30000000-0000-4000-8000-000000000001"},
            "media":[{"position":1,"track-count":tracks,"tracks":selected}]
        }))?;
        let query = CatalogQuery::release(release)?;
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
                locations: vec![location],
                accepts_incomplete_coverage: false,
            },
            catalog,
        )?;
        if cover {
            let candidate = store.prepare_artwork(
                job,
                plan.version,
                "123456789",
                &media::cover_source()?,
                None,
            )?;
            store.approve_artwork(job, plan.version, candidate.id, &candidate.sha256)?;
        }
        store.approve_plan(job, plan.version)?;
        let claim = store
            .claim_approved_plan(job, plan.version, "worker", 300)?
            .ok_or("missing worker claim")?
            .claim;
        let file = if select_source {
            store.advance(&claim, JobState::Queued, JobState::Searching)?;
            store.begin_acquisition(
                &claim,
                "fixture-peer",
                &(1..=tracks)
                    .map(|track| SourceFile {
                        remote_path: if tracks == 1 {
                            PATH.into()
                        } else {
                            format!("Peer\\Album\\{track}.{}", format.extension())
                        },
                        expected_bytes: size,
                    })
                    .collect::<Vec<_>>(),
            )?;
            store
                .transfers(job, plan.version, Page::new(0, 1)?)?
                .pop()
                .ok_or("missing transfer")?
                .id
        } else {
            0
        };
        Ok(Self {
            store,
            claim,
            job,
            version: plan.version,
            file,
            database,
            location,
            _state: state,
            _library: library,
        })
    }
}

#[test]
fn submission_and_acceptance_commit_before_their_corresponding_peer_effects() -> TestResult {
    let mut fixture = Fixture::new()?;
    let database = fixture.database.clone();
    let file = fixture.file;
    let (listener, address) = listener()?;
    let task = std::thread::spawn(move || -> TestResultSend {
        let mut stream = handshake(listener)?;
        queued(&mut stream)?;
        let raw = Connection::open(database)?;
        let state: String = raw.query_row(
            "SELECT state FROM transfer_files WHERE id=?1",
            [file],
            |row| row.get(0),
        )?;
        assert_eq!(state, "submitting");
        send(&mut stream, 40, &offer(PATH, 1000, 1))?;
        assert_eq!(
            packet(&mut stream)?,
            [41u32.to_le_bytes().as_slice(), &17u32.to_le_bytes(), &[1]].concat()
        );
        let record: (String, Option<u32>) = raw.query_row(
            "SELECT state, remote_token FROM transfer_files WHERE id=?1",
            [file],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(record, ("active".into(), Some(17)));
        Ok(())
    });
    let mut session = SoulseekSession {
        stream: None,
        username: "fixture-user".into(),
    };
    let mut peer = session.connect_address(address)?;
    negotiate_transfer(&mut fixture.store, &fixture.claim, file, &mut peer)?;
    assert!(negotiate_transfer(&mut fixture.store, &fixture.claim, file, &mut peer).is_err());
    assert_eq!(
        fixture
            .store
            .active_transfer(&fixture.claim, file)?
            .remote_token,
        Some(17)
    );
    assert!(
        fixture
            .store
            .planned_transfer(&fixture.claim, file)
            .is_err()
    );
    assert!(
        fixture
            .store
            .submitting_transfer(&fixture.claim, file)
            .is_err()
    );
    join(task)
}

#[test]
fn failed_or_lost_submission_keeps_uncertain_intent_and_never_accepts() -> TestResult {
    for fault in ["denied", "path", "ownership", "observation", "commit"] {
        let mut fixture = Fixture::new()?;
        let database = fixture.database.clone();
        if fault == "commit" {
            Connection::open(&database)?.execute_batch(
                "CREATE TRIGGER reject_active BEFORE INSERT ON events
                 WHEN NEW.event_type='transfer_active'
                 BEGIN SELECT RAISE(ABORT, 'fixture acceptance event failure'); END;",
            )?;
        }
        let (listener, address) = listener()?;
        let task = std::thread::spawn(move || -> TestResultSend {
            let mut stream = handshake(listener)?;
            queued(&mut stream)?;
            if fault == "ownership" {
                Connection::open(database)?
                    .execute("UPDATE jobs SET claim_generation=claim_generation+1", [])?;
            } else if fault == "observation" {
                let mut store = Store::open(&database)?;
                scan_location(&mut store, 1)?;
            }
            let (code, body) = if fault == "denied" {
                let mut body = Vec::new();
                string(&mut body, PATH);
                string(&mut body, "File not shared.");
                (50, body)
            } else {
                (
                    40,
                    offer(if fault == "path" { "wrong-file" } else { PATH }, 1000, 1),
                )
            };
            if let Err(error) = send(&mut stream, code, &body) {
                return match error {
                    Error::Io(error) if connection_closed(&error) => Ok(()),
                    other => Err(other.into()),
                };
            }
            match stream.read(&mut [0; 1]) {
                Ok(0) => Ok(()),
                Err(error) if connection_closed(&error) => Ok(()),
                other => Err(format!("unexpected acceptance: {other:?}").into()),
            }
        });
        let mut session = SoulseekSession {
            stream: None,
            username: "fixture-user".into(),
        };
        let mut peer = session.connect_address(address)?;
        assert!(
            negotiate_transfer(&mut fixture.store, &fixture.claim, fixture.file, &mut peer)
                .is_err()
        );
        assert!(
            negotiate_transfer(&mut fixture.store, &fixture.claim, fixture.file, &mut peer)
                .is_err()
        );
        drop(peer);
        let transfer = fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
            .pop()
            .ok_or("missing transfer")?;
        assert_eq!(transfer.state, "submitting");
        assert_eq!(transfer.remote_token, None);
        join(task)?;
    }
    Ok(())
}

#[test]
fn stale_claim_observation_or_wrong_peer_stops_before_queue_submission() -> TestResult {
    for fault in ["claim", "observation", "peer"] {
        let mut fixture = Fixture::new()?;
        if fault == "claim" {
            Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
        } else if fault == "observation" {
            scan_location(&mut fixture.store, fixture.location)?;
        }
        let (listener, mut address) = listener()?;
        if fault == "peer" {
            address.username = "other-peer".into();
        }
        let task = std::thread::spawn(move || -> TestResultSend {
            let mut stream = handshake(listener)?;
            assert_eq!(stream.read(&mut [0; 1])?, 0);
            Ok(())
        });
        let mut session = SoulseekSession {
            stream: None,
            username: "fixture-user".into(),
        };
        let mut peer = session.connect_address(address)?;
        assert!(
            negotiate_transfer(&mut fixture.store, &fixture.claim, fixture.file, &mut peer)
                .is_err()
        );
        drop(peer);
        let transfer = fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
            .pop()
            .ok_or("missing transfer")?;
        assert_eq!(transfer.state, "planned");
        join(task)?;
    }
    Ok(())
}
