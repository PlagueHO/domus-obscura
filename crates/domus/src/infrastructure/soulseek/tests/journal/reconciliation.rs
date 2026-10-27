use super::*;
use crate::application::{acquire_album, reconcile_local_inputs};
use crate::infrastructure::staging::StagingRoot;
use crate::infrastructure::store::TransferObservation;
use std::io::Cursor;

#[test]
fn sealed_input_recovers_after_reclaim_even_when_observations_changed() -> TestResult {
    for changed in [false, true] {
        let mut fixture = Fixture::album(AudioFormat::Flac, 1000, 1, true)?;
        let scratch = tempdir()?;
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        fixture.store.start_transfer(&fixture.claim, fixture.file)?;
        fixture
            .store
            .accept_transfer(&fixture.claim, fixture.file, 17)?;
        let active = fixture
            .store
            .active_transfer(&fixture.claim, fixture.file)?;
        staging.receive(&active, &mut Cursor::new(vec![42; 1000]))?;
        let before = std::fs::read(
            scratch
                .path()
                .join(format!("transfer-{}.input", fixture.file)),
        )?;
        drop(staging);
        if changed {
            scan_location(&mut fixture.store, fixture.location)?;
        }
        Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
        let claim = fixture
            .store
            .claim_acquisition_recovery(fixture.job, fixture.version, "recovery", 300)?
            .ok_or("recovery claim missing")?
            .claim;
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        assert!(reconcile_local_inputs(&mut fixture.store, &fixture.claim, &staging).is_err());
        reconcile_local_inputs(&mut fixture.store, &claim, &staging)?;
        let received = fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
            .pop()
            .ok_or("transfer missing")?;
        assert_eq!(received.state, "received");
        assert_eq!(received.received_bytes, 1000);
        assert_eq!(received.remote_token, Some(17));
        assert_eq!(
            std::fs::read(
                scratch
                    .path()
                    .join(format!("transfer-{}.input", fixture.file))
            )?,
            before
        );
        assert_eq!(
            fixture
                .store
                .transfer_evidence(fixture.file, Page::new(0, 200)?)?
                .len(),
            1
        );
        assert!(
            fixture
                .store
                .observe_transfer(
                    &fixture.claim,
                    fixture.file,
                    (1, 1),
                    TransferObservation::Interrupted
                )
                .is_err()
        );
        if changed {
            assert!(fixture.store.acquisition_files(&claim).is_err());
        } else {
            reconcile_local_inputs(&mut fixture.store, &claim, &staging)?;
        }
    }
    Ok(())
}

#[test]
fn incomplete_or_corrupt_inputs_are_never_rewritten_or_promoted_to_received() -> TestResult {
    for fault in [
        "partial",
        "full_unsealed",
        "receipt",
        "hash",
        "orphan",
        "missing_partial",
    ] {
        let mut fixture = Fixture::album(AudioFormat::Flac, 1000, 1, true)?;
        let scratch = tempdir()?;
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        fixture.store.start_transfer(&fixture.claim, fixture.file)?;
        fixture
            .store
            .accept_transfer(&fixture.claim, fixture.file, 17)?;
        let input = scratch
            .path()
            .join(format!("transfer-{}.input", fixture.file));
        let receipt = scratch
            .path()
            .join(format!("transfer-{}.receipt", fixture.file));
        if fault == "missing_partial" {
            fixture
                .store
                .transfer_progress(&fixture.claim, fixture.file, 7)?;
        } else if matches!(fault, "receipt" | "hash" | "orphan") {
            let active = fixture
                .store
                .active_transfer(&fixture.claim, fixture.file)?;
            staging.receive(&active, &mut Cursor::new(vec![42; 1000]))?;
            if fault == "receipt" {
                std::fs::write(&receipt, b"{invalid")?;
            }
            if fault == "hash" {
                std::fs::write(&input, vec![43; 1000])?;
            }
            if fault == "orphan" {
                std::fs::remove_file(&input)?;
            }
        } else {
            std::fs::write(&input, vec![42; if fault == "partial" { 7 } else { 1000 }])?;
        }
        let before_input = if input.exists() {
            Some(std::fs::read(&input)?)
        } else {
            None
        };
        let before_receipt = if receipt.exists() {
            Some(std::fs::read(&receipt)?)
        } else {
            None
        };
        assert!(reconcile_local_inputs(&mut fixture.store, &fixture.claim, &staging).is_err());
        assert!(
            fixture
                .store
                .active_transfer(&fixture.claim, fixture.file)?
                .payload_sha256
                .is_none()
        );
        if let Some(bytes) = before_input {
            assert_eq!(std::fs::read(&input)?, bytes);
        }
        if let Some(bytes) = before_receipt {
            assert_eq!(std::fs::read(&receipt)?, bytes);
        }
    }
    Ok(())
}

#[test]
fn retained_submission_queries_queue_and_accepts_only_the_original_exact_source() -> TestResult {
    for accepted in [false, true] {
        let source = media::flac();
        let mut fixture = Fixture::album(AudioFormat::Flac, source.len() as i64, 1, true)?;
        fixture.store.start_transfer(&fixture.claim, fixture.file)?;
        if accepted {
            fixture
                .store
                .accept_transfer(&fixture.claim, fixture.file, 17)?;
        }
        let scratch = tempdir()?;
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        let (client, mut server) = files::server_pair()?;
        let (listener, address) = listener()?;
        let endpoint = address.address;
        let database = fixture.database.clone();
        let id = fixture.file;
        let task = std::thread::spawn(move || -> TestResultSend {
            let mut control = handshake(listener.try_clone()?)?;
            let mut expected = 51u32.to_le_bytes().to_vec();
            string(&mut expected, PATH);
            assert_eq!(
                packet(&mut control)?,
                expected,
                "recovery must not send QueueUpload"
            );
            let mut queued = Vec::new();
            string(&mut queued, PATH);
            queued.extend_from_slice(&2u32.to_le_bytes());
            send(&mut control, 44, &queued)?;
            send(&mut control, 40, &offer(PATH, source.len() as u64, 1))?;
            assert_eq!(
                packet(&mut control)?,
                [41u32.to_le_bytes().as_slice(), &17u32.to_le_bytes(), &[1]].concat()
            );
            let raw = Connection::open(database)?;
            let token: u32 = raw.query_row(
                "SELECT remote_token FROM transfer_files WHERE id=?1",
                [id],
                |row| row.get(0),
            )?;
            assert_eq!(token, 17);
            send(&mut server, 18, &files::request(endpoint))?;
            let (mut file, _) = listener.accept()?;
            file.set_read_timeout(Some(Duration::from_secs(3)))?;
            assert_eq!(
                packet(&mut file)?,
                [vec![0], 999u32.to_le_bytes().to_vec()].concat()
            );
            file.write_all(&17u32.to_le_bytes())?;
            let mut offset = [1; 8];
            file.read_exact(&mut offset)?;
            assert_eq!(offset, [0; 8]);
            file.write_all(&source)?;
            assert_eq!(file.read(&mut [0])?, 0);
            Ok(())
        });
        let mut session = SoulseekSession {
            stream: Some(Channel::server(client)?),
            username: "fixture-user".into(),
        };
        let mut peer = session.connect_address(address)?;
        acquire_album(&mut fixture.store, &fixture.claim, &staging, &mut peer)?;
        assert!(
            fixture
                .store
                .transfer_summary(fixture.job, fixture.version)?
                .all_received
        );
        let evidence = fixture
            .store
            .transfer_evidence(fixture.file, Page::new(0, 200)?)?;
        assert!(
            evidence
                .iter()
                .any(|entry| entry.kind == "queued" && entry.value == 2)
        );
        assert!(
            evidence
                .iter()
                .any(|entry| entry.kind == "offered" && entry.value == 17)
        );
        assert!(evidence.iter().all(|entry| entry.kind != "submitted"));
        join(task)?;
    }
    Ok(())
}

#[test]
fn active_recovery_rejects_a_changed_token_without_acceptance_or_another_submission() -> TestResult
{
    let mut fixture = Fixture::album(AudioFormat::Flac, 1000, 1, true)?;
    fixture.store.start_transfer(&fixture.claim, fixture.file)?;
    fixture
        .store
        .accept_transfer(&fixture.claim, fixture.file, 17)?;
    let scratch = tempdir()?;
    let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
    let (listener, address) = listener()?;
    let task = std::thread::spawn(move || -> TestResultSend {
        let mut control = handshake(listener)?;
        let mut expected = 51u32.to_le_bytes().to_vec();
        string(&mut expected, PATH);
        assert_eq!(packet(&mut control)?, expected);
        let mut changed = offer(PATH, 1000, 1);
        changed[4..8].copy_from_slice(&18u32.to_le_bytes());
        send(&mut control, 40, &changed)?;
        assert_eq!(control.read(&mut [0])?, 0, "wrong offer was accepted");
        Ok(())
    });
    let mut session = SoulseekSession {
        stream: None,
        username: "fixture-user".into(),
    };
    let mut peer = session.connect_address(address)?;
    let error = acquire_album(&mut fixture.store, &fixture.claim, &staging, &mut peer)
        .err()
        .ok_or("changed token accepted")?;
    assert!(
        error
            .to_string()
            .contains("changed the accepted transfer token")
    );
    let transfer = fixture
        .store
        .active_transfer(&fixture.claim, fixture.file)?;
    assert_eq!(transfer.remote_token, Some(17));
    assert!(staging.retained_input(&transfer, &|| Ok(()))?.is_none());
    drop(peer);
    join(task)
}

#[test]
fn evidence_bound_keeps_duplicates_idempotent_and_rejects_invalid_remote_bindings() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.store.start_transfer(&fixture.claim, fixture.file)?;
    assert!(
        fixture
            .store
            .observe_transfer(
                &fixture.claim,
                fixture.file,
                (0, 1),
                TransferObservation::Queued(0)
            )
            .is_err()
    );
    let raw = Connection::open(&fixture.database)?;
    raw.execute(
        "WITH RECURSIVE values_to_add(value) AS (
             SELECT 0 UNION ALL SELECT value+1 FROM values_to_add WHERE value<4095
         ) INSERT INTO transfer_evidence
         (transfer_id,claim_generation,session_generation,connection_id,kind,value)
         SELECT ?1,(SELECT claim_generation FROM jobs WHERE id=?2),1,1,'queued',value
         FROM values_to_add",
        params![fixture.file, fixture.job],
    )?;
    fixture.store.observe_transfer(
        &fixture.claim,
        fixture.file,
        (1, 1),
        TransferObservation::Queued(0),
    )?;
    assert!(
        fixture
            .store
            .observe_transfer(
                &fixture.claim,
                fixture.file,
                (1, 1),
                TransferObservation::Queued(4096)
            )
            .is_err()
    );
    assert_eq!(
        raw.query_row(
            "SELECT COUNT(*) FROM transfer_evidence WHERE transfer_id=?1",
            [fixture.file],
            |row| row.get::<_, i64>(0),
        )?,
        4096
    );
    Ok(())
}

#[test]
fn queue_evidence_is_fenced_idempotent_paginated_and_immutable() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.store.start_transfer(&fixture.claim, fixture.file)?;
    for _ in 0..2 {
        fixture.store.observe_transfer(
            &fixture.claim,
            fixture.file,
            (1, 1),
            TransferObservation::Queued(0),
        )?;
    }
    fixture.store.observe_transfer(
        &fixture.claim,
        fixture.file,
        (1, 1),
        TransferObservation::Offered(17),
    )?;
    let first = fixture
        .store
        .transfer_evidence(fixture.file, Page::new(0, 1)?)?;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].kind, "queued");
    let second = fixture
        .store
        .transfer_evidence(fixture.file, Page::new(first[0].id, 1)?)?;
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].value, 17);
    let raw = Connection::open(&fixture.database)?;
    assert!(
        raw.execute("UPDATE transfer_evidence SET value=1", [])
            .is_err()
    );
    assert!(raw.execute("DELETE FROM transfer_evidence", []).is_err());
    raw.execute("UPDATE jobs SET claim_generation=claim_generation+1", [])?;
    assert!(
        fixture
            .store
            .observe_transfer(
                &fixture.claim,
                fixture.file,
                (1, 1),
                TransferObservation::Interrupted
            )
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .transfer_evidence(fixture.file, Page::new(0, 200)?)?
            .len(),
        2
    );
    Ok(())
}

#[test]
fn legacy_transfer_upgrade_keeps_source_tokens_and_bytes_without_inventing_evidence() -> TestResult
{
    let mut fixture = Fixture::new()?;
    fixture.store.start_transfer(&fixture.claim, fixture.file)?;
    fixture
        .store
        .accept_transfer(&fixture.claim, fixture.file, 17)?;
    let scratch = tempdir()?;
    let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
    let active = fixture
        .store
        .active_transfer(&fixture.claim, fixture.file)?;
    staging.receive(&active, &mut Cursor::new(vec![42; 1000]))?;
    drop(staging);
    drop(fixture.store);
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("DROP TABLE transfer_evidence; PRAGMA user_version=21;")?;
    drop(raw);
    fixture.store = Store::open_existing(&fixture.database)?;
    assert!(
        fixture
            .store
            .transfer_evidence(fixture.file, Page::new(0, 200)?)?
            .is_empty()
    );
    let active = fixture
        .store
        .active_transfer(&fixture.claim, fixture.file)?;
    assert_eq!(active.remote_token, Some(17));
    assert_eq!(active.peer, "fixture-peer");
    let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
    reconcile_local_inputs(&mut fixture.store, &fixture.claim, &staging)?;
    assert!(
        fixture
            .store
            .transfer_summary(fixture.job, fixture.version)?
            .all_received
    );
    Ok(())
}
