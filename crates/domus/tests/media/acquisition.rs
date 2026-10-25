use super::*;
use domus::application::{normalize_album, receive_transfer_file};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

type PeerConnection = (TcpStream, std::thread::JoinHandle<std::io::Result<()>>);

fn connection(
    token: u32,
    source: Vec<u8>,
    keep_open: bool,
) -> Result<PeerConnection, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let stream = TcpStream::connect(listener.local_addr()?)?;
    let (mut peer, _) = listener.accept()?;
    let task = std::thread::spawn(move || {
        peer.set_read_timeout(Some(Duration::from_secs(3)))?;
        peer.set_write_timeout(Some(Duration::from_secs(3)))?;
        peer.write_all(&token.to_le_bytes())?;
        let mut offset = [0; 8];
        peer.read_exact(&mut offset)?;
        assert_eq!(u64::from_le_bytes(offset), 0);
        peer.write_all(&source)?;
        if keep_open {
            // The uploader waits for the downloader to close a finished F
            // connection. Waiting for uploader EOF would deadlock.
            assert_eq!(peer.read(&mut [0; 1])?, 0);
        }
        Ok(())
    });
    Ok((stream, task))
}

#[test]
fn peer_album_receives_through_private_staging_journal_and_normalizes() -> TestResult {
    for format in [AudioFormat::Flac, AudioFormat::Mp3] {
        let mut fixture = Fixture::build_with_receipts(format, true, true, false)?;
        let staging = fixture.staging.as_ref().ok_or("missing staging")?;
        for transfer in fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
        {
            let token = transfer.remote_token.ok_or("missing accepted token")?;
            let (stream, task) = connection(token, fixture.source.clone(), true)?;
            let input = receive_transfer_file(
                &mut fixture.store,
                &fixture.claim,
                staging,
                transfer.id,
                "generated-peer",
                stream,
            )?;
            task.join().map_err(|_| "fixture peer panicked")??;
            assert_eq!(input.byte_length(), fixture.source.len() as u64);
            assert_eq!(
                *input.sha256(),
                <[u8; 32]>::from(Sha256::digest(&fixture.source))
            );
            let received = staging.received(
                &fixture
                    .store
                    .transfers(fixture.job, fixture.version, Page::new(transfer.id - 1, 1)?)?
                    .pop()
                    .ok_or("missing receipt")?,
            )?;
            assert_eq!(received.sha256(), input.sha256());
        }
        fixture
            .store
            .advance(&fixture.claim, JobState::Downloading, JobState::Validating)?;
        let run = normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover)?;
        assert!(run > 0);
        assert_eq!(
            fixture.store.jobs(Page::new(0, 10)?)?[0].state,
            "publishing"
        );
    }
    Ok(())
}

#[test]
fn peer_identity_token_truncation_and_stale_claim_never_record_success() -> TestResult {
    for fault in ["peer", "token", "short", "claim", "store"] {
        let mut fixture = Fixture::build_with_receipts(AudioFormat::Flac, true, true, false)?;
        let staging = fixture.staging.as_ref().ok_or("missing staging")?;
        let transfer = fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
            .pop()
            .ok_or("missing transfer")?;
        if fault == "claim" {
            Connection::open(&fixture.database)?
                .execute("UPDATE jobs SET lease_until=0 WHERE id=?1", [fixture.job])?;
        }
        let other_state = tempdir()?;
        let mut other_store = Store::open(&other_state.path().join("state.db"))?;
        let other_root = tempdir()?;
        let other_staging = StagingRoot::open(&mut other_store, other_root.path())?;
        let receive_staging = if fault == "store" {
            &other_staging
        } else {
            staging
        };
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let stream = TcpStream::connect(listener.local_addr()?)?;
        let (mut peer, _) = listener.accept()?;
        let token = transfer.remote_token.ok_or("missing token")?;
        peer.write_all(&(if fault == "token" { token + 1 } else { token }).to_le_bytes())?;
        let sender = if fault == "short" {
            let source = fixture.source[..fixture.source.len() / 2].to_vec();
            Some(std::thread::spawn(move || -> std::io::Result<()> {
                peer.read_exact(&mut [0; 8])?;
                peer.write_all(&source)?;
                Ok(())
            }))
        } else {
            drop(peer);
            None
        };
        assert!(
            receive_transfer_file(
                &mut fixture.store,
                &fixture.claim,
                receive_staging,
                transfer.id,
                if fault == "peer" {
                    "other-peer"
                } else {
                    "generated-peer"
                },
                stream
            )
            .is_err()
        );
        if let Some(task) = sender {
            task.join().map_err(|_| "peer fixture panicked")??;
        }
        let after = fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
            .pop()
            .ok_or("missing transfer")?;
        assert_eq!(after.state, "active");
        assert!(after.payload_sha256.is_none());
        assert!(staging.received(&after).is_err());
        assert!(other_staging.received(&after).is_err());
    }
    Ok(())
}

#[test]
fn received_peer_files_cannot_be_silently_resubmitted_or_overwritten() -> TestResult {
    let mut fixture = Fixture::build_with_receipts(AudioFormat::Flac, true, true, false)?;
    let staging = fixture.staging.as_ref().ok_or("missing staging")?;
    let transfer = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
        .pop()
        .ok_or("missing transfer")?;
    let (stream, task) = connection(
        transfer.remote_token.ok_or("missing token")?,
        fixture.source.clone(),
        true,
    )?;
    let input = receive_transfer_file(
        &mut fixture.store,
        &fixture.claim,
        staging,
        transfer.id,
        "generated-peer",
        stream,
    )?;
    task.join().map_err(|_| "fixture peer panicked")??;
    drop(input);
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let stream = TcpStream::connect(listener.local_addr()?)?;
    let (_peer, _) = listener.accept()?;
    assert!(
        receive_transfer_file(
            &mut fixture.store,
            &fixture.claim,
            staging,
            transfer.id,
            "generated-peer",
            stream
        )
        .is_err()
    );
    let after = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
        .pop()
        .ok_or("missing transfer")?;
    assert_eq!(after.state, "received");
    let mut original = staging.received(&after)?;
    let mut bytes = Vec::new();
    original.read_to_end(&mut bytes)?;
    assert_eq!(bytes, fixture.source);
    Ok(())
}

#[test]
fn operator_request_during_peer_receive_keeps_exact_inputs_before_stopping() -> TestResult {
    use domus::infrastructure::store::JobControlAction;
    for action in [JobControlAction::Pause, JobControlAction::Cancel] {
        let mut fixture = Fixture::build_with_receipts(AudioFormat::Flac, true, true, false)?;
        let staging = fixture.staging.as_ref().ok_or("staging")?;
        let transfers =
            fixture
                .store
                .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
        let mut control_id = None;
        for (index, transfer) in transfers.iter().enumerate() {
            let token = transfer.remote_token.ok_or("token")?;
            let (stream, task) = if index == 0 {
                let listener = TcpListener::bind(("127.0.0.1", 0))?;
                let stream = TcpStream::connect(listener.local_addr()?)?;
                let (mut peer, _) = listener.accept()?;
                let database = fixture.database.clone();
                let source = fixture.source.clone();
                let job = fixture.job;
                let version = fixture.version;
                let (send, receive) = std::sync::mpsc::channel();
                let task = std::thread::spawn(move || -> std::io::Result<()> {
                    peer.set_read_timeout(Some(Duration::from_secs(5)))?;
                    peer.set_write_timeout(Some(Duration::from_secs(5)))?;
                    peer.write_all(&token.to_le_bytes())?;
                    let mut offset = [0; 8];
                    peer.read_exact(&mut offset)?;
                    assert_eq!(offset, [0; 8]);
                    let halfway = source.len() / 2;
                    peer.write_all(&source[..halfway])?;
                    let mut store =
                        Store::open_current_existing(&database).map_err(std::io::Error::other)?;
                    let control = store
                        .request_job_control(job, version, action)
                        .map_err(std::io::Error::other)?;
                    assert!(control.applied_at.is_none());
                    send.send(control.id).map_err(std::io::Error::other)?;
                    peer.write_all(&source[halfway..])?;
                    assert_eq!(peer.read(&mut [0; 1])?, 0);
                    Ok(())
                });
                let input = receive_transfer_file(
                    &mut fixture.store,
                    &fixture.claim,
                    staging,
                    transfer.id,
                    "generated-peer",
                    stream,
                )?;
                task.join().map_err(|_| "peer panicked")??;
                assert_eq!(input.byte_length(), fixture.source.len() as u64);
                control_id = Some(receive.recv_timeout(Duration::from_secs(5))?);
                continue;
            } else {
                connection(token, fixture.source.clone(), true)?
            };
            receive_transfer_file(
                &mut fixture.store,
                &fixture.claim,
                staging,
                transfer.id,
                "generated-peer",
                stream,
            )?;
            task.join().map_err(|_| "peer panicked")??;
        }
        let control = control_id.ok_or("control")?;
        assert!(
            matches!(normalize_album(&mut fixture.store,&fixture.claim,staging,&fixture.cover),Err(domus::Error::JobStopped(id)) if id==control)
        );
        for transfer in fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
        {
            assert_eq!(transfer.state, "received");
            let input = staging.received(&transfer)?;
            assert_eq!(
                *input.sha256(),
                <[u8; 32]>::from(Sha256::digest(&fixture.source))
            );
        }
        assert!(fixture.store.renew_claim(&fixture.claim, 60).is_err());
        let raw = Connection::open(&fixture.database)?;
        assert_eq!(
            raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some(if action == JobControlAction::Pause {
                "awaiting_input"
            } else {
                "cancelled"
            })
        );
    }
    Ok(())
}

#[test]
fn normalization_request_stops_between_files_and_retains_verified_output() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let staging = fixture.staging.as_ref().ok_or("staging")?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER request_generated_pause AFTER INSERT ON normalization_receipts
         WHEN NEW.track_number=1
         BEGIN
           INSERT INTO job_controls(job_id,plan_version,action)
           SELECT job_id,plan_version,'pause' FROM normalization_runs WHERE id=NEW.run_id;
         END;",
    )?;
    assert!(matches!(
        normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover),
        Err(domus::Error::JobStopped(_))
    ));
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("awaiting_input")
    );
    let (run, output, hash): (i64, i64, String) = raw.query_row(
        "SELECT run_id,output_bytes,output_sha256 FROM normalization_receipts",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert!(output > 0);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM normalization_receipts", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    assert!(raw.query_row(
        "SELECT completed_at IS NULL FROM normalization_runs WHERE id=?1",
        [run],
        |row| row.get::<_, bool>(0)
    )?,);
    assert!(fixture.store.renew_claim(&fixture.claim, 60).is_err());
    assert_eq!(
        raw.query_row(
            "SELECT output_sha256 FROM normalization_receipts WHERE run_id=?1",
            [run],
            |row| row.get::<_, String>(0)
        )?,
        hash
    );
    let name: String = raw.query_row(
        "SELECT staging_name FROM normalization_files WHERE run_id=?1 AND track_number=1",
        [run],
        |row| row.get(0),
    )?;
    let output_path = fixture
        .scratch
        .path()
        .join(format!("normalization-{run}"))
        .join(name);
    let retained = std::fs::read(&output_path)?;
    assert_eq!(retained.len() as i64, output);
    assert_eq!(format!("{:x}", Sha256::digest(&retained)), hash);
    raw.execute_batch("DROP TRIGGER request_generated_pause;")?;
    let (control, question): (i64, i64) =
        raw.query_row("SELECT id,question_id FROM job_controls", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    fixture.store.answer(question, fixture.version, "Resume")?;
    assert_eq!(
        fixture
            .store
            .continue_job_control(control, fixture.version)?,
        "normalizing"
    );
    let claim = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "resumed-worker", 300)?
        .ok_or("claim")?
        .claim;
    let resumed = normalize_album(&mut fixture.store, &claim, staging, &fixture.cover)?;
    assert_ne!(resumed, run);
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("publishing")
    );
    assert_eq!(std::fs::read(&output_path)?, retained);
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM normalization_receipts WHERE run_id=?1",
            [resumed],
            |row| row.get::<_, i64>(0)
        )?,
        3
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM normalization_receipts WHERE run_id=?1",
            [run],
            |row| row.get::<_, i64>(0)
        )?,
        1
    );
    assert!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
            .iter()
            .all(|file| file.state == "received")
    );
    Ok(())
}

#[test]
fn ownership_loss_after_offset_stops_sealing_and_receipt_commit() -> TestResult {
    let mut fixture = Fixture::build_with_receipts(AudioFormat::Flac, true, true, false)?;
    let staging = fixture.staging.as_ref().ok_or("missing staging")?;
    let transfer = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
        .pop()
        .ok_or("missing transfer")?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let stream = TcpStream::connect(listener.local_addr()?)?;
    let (mut peer, _) = listener.accept()?;
    let database = fixture.database.clone();
    let job = fixture.job;
    let source = fixture.source.clone();
    let token = transfer.remote_token.ok_or("missing token")?;
    let task = std::thread::spawn(
        move || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            peer.set_read_timeout(Some(Duration::from_secs(3)))?;
            peer.set_write_timeout(Some(Duration::from_secs(3)))?;
            peer.write_all(&token.to_le_bytes())?;
            peer.read_exact(&mut [0; 8])?;
            Connection::open(database)?.execute(
                "UPDATE jobs SET claim_generation=claim_generation+1 WHERE id=?1",
                [job],
            )?;
            peer.write_all(&source)?;
            Ok(())
        },
    );
    assert!(
        receive_transfer_file(
            &mut fixture.store,
            &fixture.claim,
            staging,
            transfer.id,
            "generated-peer",
            stream
        )
        .is_err()
    );
    let outcome = task.join().map_err(|_| "peer fixture panicked")?;
    outcome.map_err(|error| error.to_string())?;
    let after = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
        .pop()
        .ok_or("missing transfer")?;
    assert_eq!(after.state, "active");
    assert!(after.payload_sha256.is_none());
    assert!(staging.received(&after).is_err());
    Ok(())
}
