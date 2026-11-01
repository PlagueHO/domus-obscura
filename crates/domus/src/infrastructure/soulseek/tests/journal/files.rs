use super::*;
use crate::application::acquire_album;
use crate::application::acquire_transfer;
use crate::infrastructure::staging::StagingRoot;

pub(super) fn server_pair() -> std::io::Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let client = TcpStream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    server.set_read_timeout(Some(Duration::from_secs(3)))?;
    server.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok((client, server))
}

pub(super) fn request(address: SocketAddrV4) -> Vec<u8> {
    request_token(address, 999)
}

fn request_token(address: SocketAddrV4, token: u32) -> Vec<u8> {
    let mut body = Vec::new();
    string(&mut body, "fixture-peer");
    string(&mut body, "F");
    body.extend_from_slice(&u32::from(*address.ip()).to_le_bytes());
    body.extend_from_slice(&u32::from(address.port()).to_le_bytes());
    body.extend_from_slice(&token.to_le_bytes());
    body.push(0);
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body
}

fn accept(stream: &mut TcpStream) -> TestResultSend {
    queued(stream)?;
    send(stream, 40, &offer(PATH, 1000, 1))?;
    assert_eq!(
        packet(stream)?,
        [41u32.to_le_bytes().as_slice(), &17u32.to_le_bytes(), &[1]].concat()
    );
    Ok(())
}

#[test]
fn server_bound_file_channel_receives_through_real_journal_and_sealed_staging() -> TestResult {
    for disconnect_server in [false, true] {
        receive_bound_file(disconnect_server, false)?;
    }
    Ok(())
}

#[test]
fn worker_stop_during_active_receive_seals_the_file_before_the_next_boundary() -> TestResult {
    receive_bound_file(false, true)
}

fn receive_bound_file(disconnect_server: bool, stopping: bool) -> TestResult {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let mut fixture = Fixture::new()?;
    let stop = Arc::new(AtomicBool::new(false));
    fixture.store = fixture.store.with_worker_stop(stop.clone());
    let scratch = tempdir()?;
    let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
    let (client, mut server) = server_pair()?;
    let (listener, address) = listener()?;
    let endpoint = address.address;
    let source = (0..1000)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let expected = source.clone();
    let task = std::thread::spawn(move || -> TestResultSend {
        let mut control = handshake(listener.try_clone()?)?;
        accept(&mut control)?;
        send(&mut server, 18, &request(endpoint))?;
        let (mut file, _) = listener.accept()?;
        file.set_read_timeout(Some(Duration::from_secs(3)))?;
        file.set_write_timeout(Some(Duration::from_secs(3)))?;
        assert_eq!(
            packet(&mut file)?,
            [0u8]
                .into_iter()
                .chain(999u32.to_le_bytes())
                .collect::<Vec<_>>()
        );
        file.write_all(&17u32.to_le_bytes())?;
        let mut offset = [0; 8];
        file.read_exact(&mut offset)?;
        assert_eq!(offset, [0; 8]);
        if stopping {
            stop.store(true, Ordering::Release);
        }
        if disconnect_server {
            drop(server);
            std::thread::sleep(Duration::from_millis(100));
        }
        file.write_all(&source)?;
        assert_eq!(file.read(&mut [0; 1])?, 0);
        // No additional queue request after the first file is complete.
        assert_eq!(control.read(&mut [0; 1])?, 0);
        Ok(())
    });
    let mut session = SoulseekSession {
        stream: Some(Channel::server(client)?),
        username: "fixture-user".into(),
    };
    let mut peer = session.connect_address(address)?;
    let mut received = acquire_transfer(
        &mut fixture.store,
        &fixture.claim,
        &staging,
        fixture.file,
        &mut peer,
    )?;
    let mut actual = Vec::new();
    received.read_to_end(&mut actual)?;
    assert_eq!(actual, expected);
    assert_eq!(received.byte_length(), 1000);
    assert_eq!(
        received.sha256(),
        &<[u8; 32]>::from(Sha256::digest(&expected))
    );
    assert!(
        acquire_transfer(
            &mut fixture.store,
            &fixture.claim,
            &staging,
            fixture.file,
            &mut peer
        )
        .is_err()
    );
    let transfer = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
        .pop()
        .ok_or("missing transfer")?;
    assert_eq!(transfer.state, "received");
    let mut reopened = staging.received(&transfer)?;
    let mut bytes = Vec::new();
    reopened.read_to_end(&mut bytes)?;
    assert_eq!(bytes, expected);
    if stopping {
        assert!(matches!(
            fixture.store.job_work_boundary(&fixture.claim),
            Err(Error::WorkerStopping)
        ));
    }
    drop(peer);
    join(task)
}

#[test]
fn wrong_reverse_requests_never_connect_or_reset_the_accepted_transfer() -> TestResult {
    for fault in [
        "peer",
        "type",
        "ip",
        "port",
        "bool",
        "trailing",
        "count",
        "duplicate",
        "claim",
    ] {
        let mut fixture = Fixture::new()?;
        let scratch = tempdir()?;
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        let database = fixture.database.clone();
        let (client, mut server) = server_pair()?;
        let (listener, address) = listener()?;
        let endpoint = address.address;
        let task = std::thread::spawn(move || -> TestResultSend {
            let mut control = handshake(listener.try_clone()?)
                .map_err(|error| format!("{fault}: control handshake failed: {error}"))?;
            if fault == "count" {
                // The bounded loop needs two read passes per notification.
                control.set_read_timeout(Some(OPERATION_TIMEOUT))?;
            }
            accept(&mut control)
                .map_err(|error| format!("{fault}: upload acceptance failed: {error}"))?;
            if fault == "claim" {
                Connection::open(database)?
                    .execute("UPDATE jobs SET claim_generation=claim_generation+1", [])?;
            }
            let mut body = Vec::new();
            string(
                &mut body,
                if fault == "peer" {
                    "wrong-peer"
                } else {
                    "fixture-peer"
                },
            );
            string(&mut body, if fault == "type" { "P" } else { "F" });
            body.extend_from_slice(
                &u32::from(if fault == "ip" {
                    Ipv4Addr::new(10, 0, 0, 1)
                } else {
                    *endpoint.ip()
                })
                .to_le_bytes(),
            );
            body.extend_from_slice(
                &(if fault == "port" {
                    65536
                } else {
                    u32::from(endpoint.port())
                })
                .to_le_bytes(),
            );
            body.extend_from_slice(&999u32.to_le_bytes());
            body.push(if fault == "bool" { 2 } else { 0 });
            if fault == "trailing" {
                body.push(0);
            }
            let sent = if fault == "count" {
                (0..128).try_for_each(|_| send(&mut server, 104, &0u32.to_le_bytes()))
            } else {
                send(
                    &mut server,
                    if fault == "duplicate" { 41 } else { 18 },
                    &body,
                )
            };
            if let Err(error) = sent {
                match error {
                    Error::Io(error) if connection_closed(&error) => {}
                    other => return Err(other.into()),
                }
            }
            assert_eq!(
                control
                    .read(&mut [0; 1])
                    .map_err(|error| format!("{fault}: control closure failed: {error}"))?,
                0
            );
            // The listener has never received an F socket.
            listener.set_nonblocking(true)?;
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
            Ok(())
        });
        let mut session = SoulseekSession {
            stream: Some(Channel::server(client)?),
            username: "fixture-user".into(),
        };
        let mut peer = session.connect_address(address)?;
        let result = acquire_transfer(
            &mut fixture.store,
            &fixture.claim,
            &staging,
            fixture.file,
            &mut peer,
        );
        if fault == "count" {
            assert!(
                matches!(
                    &result,
                    Err(Error::Remote("Soulseek notification rate limit exceeded"))
                ),
                "notification burst returned {:?}",
                result.as_ref().err()
            );
        } else {
            assert!(result.is_err(), "{fault}: invalid request was accepted");
        }
        assert!(peer.file_connection(17, || Ok(())).is_err());
        drop(peer);
        let transfer = fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
            .pop()
            .ok_or("missing transfer")?;
        assert_eq!(transfer.state, "active");
        assert_eq!(transfer.remote_token, Some(17));
        assert!(staging.received(&transfer).is_err());
        assert!(session.stream.is_none());
        join(task)?;
    }
    Ok(())
}

#[test]
fn wrong_file_token_or_short_payload_never_seals_a_bound_file_channel() -> TestResult {
    for short in [false, true] {
        let mut fixture = Fixture::new()?;
        let scratch = tempdir()?;
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        let (client, mut server) = server_pair()?;
        let (listener, address) = listener()?;
        let endpoint = address.address;
        let task = std::thread::spawn(move || -> TestResultSend {
            let mut control = handshake(listener.try_clone()?)?;
            accept(&mut control)?;
            send(&mut server, 18, &request(endpoint))?;
            let (mut file, _) = listener.accept()?;
            file.set_read_timeout(Some(Duration::from_secs(3)))?;
            file.set_write_timeout(Some(Duration::from_secs(3)))?;
            assert_eq!(
                packet(&mut file)?,
                [0u8]
                    .into_iter()
                    .chain(999u32.to_le_bytes())
                    .collect::<Vec<_>>()
            );
            file.write_all(&(if short { 17u32 } else { 18 }).to_le_bytes())?;
            if short {
                file.read_exact(&mut [0; 8])?;
                file.write_all(&[1; 999])?;
            } else {
                assert_eq!(file.read(&mut [0; 1])?, 0);
            }
            Ok(())
        });
        let mut session = SoulseekSession {
            stream: Some(Channel::server(client)?),
            username: "fixture-user".into(),
        };
        let mut peer = session.connect_address(address)?;
        assert!(
            acquire_transfer(
                &mut fixture.store,
                &fixture.claim,
                &staging,
                fixture.file,
                &mut peer
            )
            .is_err()
        );
        drop(peer);
        let transfer = fixture
            .store
            .active_transfer(&fixture.claim, fixture.file)?;
        assert!(staging.received(&transfer).is_err());
        assert!(transfer.payload_sha256.is_none());
        join(task)?;
    }
    Ok(())
}

#[test]
fn complete_mp3_and_flac_albums_queue_receive_and_normalize_via_bound_channels() -> TestResult {
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let source = match format {
            AudioFormat::Mp3 => media::mp3()?,
            AudioFormat::Flac => media::flac(),
        };
        let mut fixture = Fixture::album(format, source.len() as i64, 3, true)?;
        let scratch = tempdir()?;
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        let selected = fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
        let (client, mut server) = server_pair()?;
        let (listener, address) = listener()?;
        let endpoint = address.address;
        let task = std::thread::spawn(move || -> TestResultSend {
            let mut control = handshake(listener.try_clone()?)?;
            for transfer in selected {
                let mut expected = 43u32.to_le_bytes().to_vec();
                string(&mut expected, &transfer.remote_path);
                assert_eq!(packet(&mut control)?, expected);
                let token = u32::from(transfer.track_number);
                let mut offer = 1u32.to_le_bytes().to_vec();
                offer.extend_from_slice(&token.to_le_bytes());
                string(&mut offer, &transfer.remote_path);
                offer.extend_from_slice(&(source.len() as u64).to_le_bytes());
                send(&mut control, 40, &offer)?;
                assert_eq!(
                    packet(&mut control)?,
                    [41u32.to_le_bytes().as_slice(), &token.to_le_bytes(), &[1]].concat()
                );
                let connection_token = 1000 + token;
                send(&mut server, 18, &request_token(endpoint, connection_token))?;
                let (mut file, _) = listener.accept()?;
                file.set_read_timeout(Some(Duration::from_secs(3)))?;
                file.set_write_timeout(Some(Duration::from_secs(3)))?;
                assert_eq!(
                    packet(&mut file)?,
                    [0u8]
                        .into_iter()
                        .chain(connection_token.to_le_bytes())
                        .collect::<Vec<_>>()
                );
                file.write_all(&token.to_le_bytes())?;
                let mut offset = [0; 8];
                file.read_exact(&mut offset)?;
                assert_eq!(offset, [0; 8]);
                file.write_all(&source)?;
                assert_eq!(file.read(&mut [0; 1])?, 0);
            }
            Ok(())
        });
        let mut session = SoulseekSession {
            stream: Some(Channel::server(client)?),
            username: "fixture-user".into(),
        };
        let mut peer = session.connect_address(address)?;
        let run = acquire_album(&mut fixture.store, &fixture.claim, &staging, &mut peer)?;
        assert!(run > 0);
        let album = crate::application::export_playback_album(
            &fixture.store,
            &fixture.claim,
            &staging,
            run,
        )?;
        assert_eq!(std::fs::read_dir(&album)?.count(), 4);
        for track in 1..=3 {
            let name = format!(
                "Fixture Artist - Fixture Album - {track:02} - Fixture Track {track}.{}",
                format.extension()
            );
            assert!(album.join(name).is_file());
        }
        assert!(
            album.ends_with(
                std::path::Path::new("F")
                    .join("FIXTURE ARTIST")
                    .join("FIXTURE ALBUM")
            )
        );
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
        let raw = Connection::open(&fixture.database)?;
        let receipts: i64 = raw.query_row(
            "SELECT count(*) FROM normalization_receipts WHERE run_id=?1",
            [run],
            |row| row.get(0),
        )?;
        assert_eq!(receipts, 3);
        let transfers =
            fixture
                .store
                .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
        assert!(transfers.iter().all(|file| file.state == "received"));
        assert_eq!(
            crate::application::publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                &staging,
                run,
            )?,
            "complete"
        );
        let published = fixture
            ._library
            .path()
            .join("F")
            .join("FIXTURE ARTIST")
            .join("FIXTURE ALBUM");
        assert_eq!(std::fs::read_dir(&published)?.count(), 4);
        for entry in std::fs::read_dir(&album)? {
            let entry = entry?;
            assert_eq!(
                std::fs::read(entry.path())?,
                std::fs::read(published.join(entry.file_name()))?
            );
        }
        assert!(acquire_album(&mut fixture.store, &fixture.claim, &staging, &mut peer).is_err());
        join(task)?;
    }
    Ok(())
}

#[test]
fn album_preflight_rejects_invalid_inputs_and_queries_uncertainty_without_submission() -> TestResult
{
    for fault in ["cover", "uncertain", "receipt", "peer"] {
        let mut fixture = Fixture::album(AudioFormat::Flac, 1000, 3, fault != "cover")?;
        let scratch = tempdir()?;
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        if matches!(fault, "uncertain" | "receipt") {
            fixture.store.start_transfer(&fixture.claim, fixture.file)?;
        }
        if fault == "receipt" {
            fixture
                .store
                .accept_transfer(&fixture.claim, fixture.file, 17)?;
            fixture
                .store
                .receive_transfer(&fixture.claim, fixture.file, 1000, &[0; 32])?;
        }
        let (listener, mut address) = listener()?;
        if fault == "peer" {
            address.username = "wrong-peer".into();
        }
        let task = std::thread::spawn(move || -> TestResultSend {
            let mut control = handshake(listener)?;
            if fault == "uncertain" {
                let mut expected = 51u32.to_le_bytes().to_vec();
                string(&mut expected, "Peer\\Album\\1.flac");
                assert_eq!(packet(&mut control)?, expected);
                let mut body = Vec::new();
                string(&mut body, "Peer\\Album\\1.flac");
                string(&mut body, "File not shared.");
                send(&mut control, 50, &body)?;
            }
            assert_eq!(control.read(&mut [0; 1])?, 0);
            Ok(())
        });
        let mut session = SoulseekSession {
            stream: None,
            username: "fixture-user".into(),
        };
        let mut peer = session.connect_address(address)?;
        assert!(acquire_album(&mut fixture.store, &fixture.claim, &staging, &mut peer).is_err());
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("downloading")
        );
        let planned =
            fixture
                .store
                .transfers(fixture.job, fixture.version, Page::new(fixture.file, 200)?)?;
        assert!(planned.iter().all(|file| file.state == "planned"));
        drop(peer);
        join(task)?;
    }
    Ok(())
}
