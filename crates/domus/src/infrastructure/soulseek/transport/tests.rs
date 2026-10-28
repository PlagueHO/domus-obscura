use super::*;
use crate::infrastructure::soulseek::messages::ServerEvent;
use crate::infrastructure::soulseek::protocol::{read_frame, string};
use std::net::TcpListener;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn sharing_existing_control_is_reused_by_acquisition_without_another_peer_init() -> TestResult {
    let (client, _server) = pair()?;
    let owner = Channel::server(client)?;
    let (client, mut remote) = pair()?;
    let address = remote.local_addr()?.to_string().parse()?;
    let mut sharing = owner.sharing_peer_for(client, "album-peer")?;
    let writing = sharing
        .writer
        .lock()
        .map_err(|_| "fixture writer poisoned")?;
    assert!(matches!(
        owner.reuse_peer("album-peer", address),
        Err(Error::Busy(_))
    ));
    drop(writing);
    assert!(
        owner
            .reuse_peer("album-peer", "8.8.8.8:1".parse()?)?
            .is_none()
    );
    let mut acquisition = owner
        .reuse_peer("album-peer", address)?
        .ok_or("existing peer not reused")?;
    assert_eq!(acquisition.identity(), sharing.identity());
    assert!(sharing.acquisition_in_use());
    assert!(owner.reuse_peer("album-peer", address)?.is_none());
    let (duplicate, mut rejected) = pair()?;
    assert!(matches!(
        owner.sharing_peer_for(duplicate, "album-peer"),
        Err(Error::Busy(_))
    ));
    assert_eq!(rejected.read(&mut [0])?, 0);
    send(&mut remote, 4, &[])?;
    let mut offer = 1u32.to_le_bytes().to_vec();
    offer.extend_from_slice(&73u32.to_le_bytes());
    string(&mut offer, "Music\\track.mp3");
    offer.extend_from_slice(&91u64.to_le_bytes());
    send(&mut remote, 40, &offer)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    assert!(matches!(
        acquisition.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Peer(super::super::messages::PeerEvent::Offer { token: 73, .. })
    ));
    assert!(matches!(
        sharing.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Peer(super::super::messages::PeerEvent::ShareList)
    ));
    acquisition.send(41, &[0; 5])?;
    assert_eq!(
        read_frame(|bytes| {
            remote.read_exact(bytes)?;
            Ok(())
        })?
        .0,
        41
    );
    drop(acquisition);
    owner.send(35, &[0; 8])?;
    assert!(!sharing.acquisition_in_use());
    sharing.send(5, &[0])?;
    assert_eq!(
        read_frame(|bytes| {
            remote.read_exact(bytes)?;
            Ok(())
        })?
        .0,
        5
    );
    Ok(())
}

#[test]
fn sharing_metadata_routes_leave_reserved_capacity_for_acquisition() -> TestResult {
    let (client, _server) = pair()?;
    let owner = Channel::server(client)?;
    let mut metadata = Vec::new();
    for index in 0..20 {
        metadata.push(owner.metadata_subscription(&format!("peer-{index}"))?);
    }
    assert!(matches!(
        owner.metadata_subscription("one-too-many"),
        Err(Error::Busy(_))
    ));
    let acquisition = owner.subscribe("album-peer")?;
    assert_eq!(acquisition.generation(), owner.generation());
    Ok(())
}

#[test]
fn sharing_and_acquisition_use_one_server_reader_without_consuming_each_others_events() -> TestResult
{
    let (client, mut server) = pair()?;
    let mut owner = Channel::server(client)?;
    owner.enable_sharing()?;
    let mut acquisition = owner.subscribe("album-peer")?;
    let mut search = Vec::new();
    string(&mut search, "search-peer");
    search.extend_from_slice(&7u32.to_le_bytes());
    string(&mut search, "artist album");
    send(&mut server, 26, &search)?;
    let mut address = Vec::new();
    string(&mut address, "album-peer");
    address.extend_from_slice(&u32::from(std::net::Ipv4Addr::new(8, 8, 8, 8)).to_le_bytes());
    address.extend_from_slice(&2234u32.to_le_bytes());
    send(&mut server, 3, &address)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    assert!(
        matches!(acquisition.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Server(ServerEvent::Address { username, .. }) if username == "album-peer")
    );
    assert!(matches!(owner.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Server(ServerEvent::Search { username, token: 7, .. }) if username == "search-peer"));
    drop(acquisition);
    owner.send(35, &[0; 8])?;
    assert_eq!(
        read_frame(|bytes| {
            server.read_exact(bytes)?;
            Ok(())
        })?
        .0,
        35
    );
    Ok(())
}

#[test]
fn sharing_large_output_streams_without_increasing_normal_frame_limits() -> TestResult {
    let (client, mut server) = pair()?;
    let channel = Channel::isolated_peer(client)?;
    let bytes = 2 * MAX_FRAME + 17;
    let reader = thread::spawn(move || -> std::io::Result<()> {
        let mut header = [0; 8];
        server.read_exact(&mut header)?;
        assert_eq!(
            u32::from_le_bytes(header[..4].try_into().map_err(std::io::Error::other)?),
            bytes as u32 + 4
        );
        assert_eq!(
            u32::from_le_bytes(header[4..].try_into().map_err(std::io::Error::other)?),
            5
        );
        let mut left = bytes;
        let mut buffer = [0; 4096];
        while left > 0 {
            let wanted = left.min(buffer.len());
            server.read_exact(&mut buffer[..wanted])?;
            assert!(buffer[..wanted].iter().all(|byte| *byte == 37));
            left -= wanted;
        }

        Ok(())
    });
    channel.send_stream(
        5,
        bytes as u32,
        &mut std::io::repeat(37),
        65_536,
        IO_TIMEOUT,
        || Ok(()),
    )?;
    reader.join().map_err(|_| "stream fixture panicked")??;
    assert!(send(&mut Vec::new(), 5, &vec![0; MAX_FRAME]).is_err());
    Ok(())
}

#[test]
fn sharing_requests_on_an_acquisition_control_socket_have_a_separate_route() -> TestResult {
    let (client, _server) = pair()?;
    let owner = Channel::server(client)?;
    let (sent, received) = mpsc::sync_channel(4);
    owner.share_connections(sent)?;
    let (client, mut remote) = pair()?;
    let mut acquisition = owner.peer(client)?;
    send(&mut remote, 4, &[])?;
    thread::sleep(Duration::from_millis(50));
    acquisition.share_control("album-peer", "8.8.8.8:2234".parse()?)?;
    let mut sharing = received.recv_timeout(Duration::from_secs(3))?.channel;
    let mut body = 1u32.to_le_bytes().to_vec();
    body.extend_from_slice(&73u32.to_le_bytes());
    string(&mut body, "Music\\album\\track.mp3");
    body.extend_from_slice(&91u64.to_le_bytes());
    send(&mut remote, 40, &body)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    assert!(matches!(
        acquisition.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Peer(super::super::messages::PeerEvent::Offer { token: 73, .. })
    ));
    assert!(matches!(
        sharing.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Peer(super::super::messages::PeerEvent::ShareList)
    ));
    sharing.send(5, &[0])?;
    acquisition.send(41, &[0; 5])?;
    assert_eq!(
        read_frame(|bytes| {
            remote.read_exact(bytes)?;
            Ok(())
        })?
        .0,
        5
    );
    assert_eq!(
        read_frame(|bytes| {
            remote.read_exact(bytes)?;
            Ok(())
        })?
        .0,
        41
    );
    Ok(())
}

#[test]
fn sharing_control_and_failure_tokens_do_not_poison_acquisition_file_setup() -> TestResult {
    let (client, mut server) = pair()?;
    let mut owner = Channel::server(client)?;
    owner.enable_sharing()?;
    let mut acquisition = owner.subscribe("fixture-peer")?;
    let mut control = connection_request(91);
    let kind = 4 + "fixture-peer".len() + 4;
    control[kind] = b'P';
    send(&mut server, 18, &control)?;
    send(&mut server, 1001, &92u32.to_le_bytes())?;
    send(&mut server, 18, &connection_request(93))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    assert!(matches!(
        owner.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Server(ServerEvent::Connect { token: 91, .. })
    ));
    assert!(matches!(
        owner.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Server(ServerEvent::CannotConnect(92))
    ));
    assert!(matches!(
        acquisition.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Server(ServerEvent::Connect { token: 93, .. })
    ));
    Ok(())
}

#[test]
fn sharing_partial_stream_failure_closes_the_socket_before_another_frame() -> TestResult {
    let (client, mut remote) = pair()?;
    let channel = Channel::isolated_peer(client)?;
    let mut checks = 0;
    assert!(
        channel
            .send_stream(5, 8000, &mut std::io::repeat(7), 4096, IO_TIMEOUT, || {
                checks += 1;
                if checks == 3 {
                    Err(Error::WorkerStopping)
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    let mut received = Vec::new();
    remote.read_to_end(&mut received)?;
    assert_eq!(received.len(), 8 + 4096);
    assert!(channel.send(41, &[0; 5]).is_err());
    Ok(())
}

fn pair() -> std::io::Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let client = TcpStream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    server.set_read_timeout(Some(Duration::from_secs(3)))?;
    Ok((client, server))
}

fn connection_request(token: u32) -> Vec<u8> {
    let mut body = Vec::new();
    string(&mut body, "fixture-peer");
    string(&mut body, "F");
    body.extend_from_slice(&u32::from(std::net::Ipv4Addr::new(8, 8, 8, 8)).to_le_bytes());
    body.extend_from_slice(&2242u32.to_le_bytes());
    body.extend_from_slice(&token.to_le_bytes());
    body.push(0);
    body
}

#[test]
fn interleaved_events_and_partial_frames_keep_their_own_routing_identity() -> TestResult {
    let (client, mut server) = pair()?;
    let mut channel = Channel::server(client)?;
    send(&mut server, 18, &connection_request(99))?;
    let mut body = Vec::new();
    string(&mut body, "fixture-peer");
    body.extend_from_slice(&u32::from(std::net::Ipv4Addr::new(8, 8, 8, 8)).to_le_bytes());
    body.extend_from_slice(&2242u32.to_le_bytes());
    let mut bytes = Vec::new();
    send(&mut bytes, 3, &body)?;
    server.write_all(&bytes[..2])?;
    thread::sleep(Duration::from_millis(150));
    server.write_all(&bytes[2..])?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let reply = channel.receive(deadline, &mut || Ok(()), |event| {
        matches!(event, Event::Server(ServerEvent::Address { .. }))
    })?;
    assert!(
        matches!(reply, Event::Server(ServerEvent::Address { username, .. }) if username == "fixture-peer")
    );
    let request = channel.receive(deadline, &mut || Ok(()), |_| true)?;
    assert!(
        matches!(request, Event::Server(ServerEvent::Connect { token: 99, kind, .. }) if kind == "F")
    );
    Ok(())
}

#[test]
fn keepalive_runs_without_an_operation_reader_or_ping_response() -> TestResult {
    let (client, mut server) = pair()?;
    let channel = Channel::server_with_keepalive(client, Duration::from_millis(40))?;
    for _ in 0..2 {
        let (code, body) = read_frame(|bytes| {
            server.read_exact(bytes)?;
            Ok(())
        })?;
        assert_eq!(code, 32);
        assert!(body.is_empty());
    }
    let start = Instant::now();
    drop(channel);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(server.read(&mut [0])?, 0);
    Ok(())
}

#[test]
fn reconnect_generations_retire_old_events_and_connection_tokens_are_single_use() -> TestResult {
    let (client, mut server) = pair()?;
    let mut old = Channel::server(client)?;
    let generation = old.generation();
    send(&mut server, 18, &connection_request(99))?;
    assert!(matches!(
        old.receive(
            Instant::now() + Duration::from_secs(3),
            &mut || Ok(()),
            |_| true
        )?,
        Event::Server(ServerEvent::Connect { token: 99, .. })
    ));
    send(&mut server, 18, &connection_request(99))?;
    let error = old
        .receive(
            Instant::now() + Duration::from_secs(3),
            &mut || Ok(()),
            |_| true,
        )
        .err()
        .ok_or("duplicate token accepted")?;
    assert!(error.to_string().contains("duplicate connection token"));
    drop(old);
    let (client, _server) = pair()?;
    let mut fresh = Channel::server(client)?;
    assert_ne!(fresh.generation(), generation);
    assert!(
        fresh
            .receive(Instant::now(), &mut || Ok(()), |_| true)
            .is_err()
    );
    Ok(())
}

#[test]
fn stalled_or_full_consumers_do_not_allow_unbounded_event_buffering() -> TestResult {
    let (client, mut server) = pair()?;
    let mut channel = Channel::server(client)?;
    // Unknown codes fail rather than disappearing in an operation-local loop.
    send(&mut server, 9999, &[])?;
    assert!(
        channel
            .receive(
                Instant::now() + Duration::from_secs(3),
                &mut || Ok(()),
                |_| true
            )
            .is_err()
    );
    let (client, mut server) = pair()?;
    let mut channel = Channel::server(client)?;
    for token in 0..=MAX_EVENTS {
        send(&mut server, 18, &connection_request(token as u32))?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if channel.runtime.check().is_err() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "event overflow did not stop the session"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let mut received = 0;
    while channel.receive(deadline, &mut || Ok(()), |_| true).is_ok() {
        received += 1;
    }
    assert!(received <= MAX_EVENTS);
    Ok(())
}

#[test]
fn identical_offers_are_delivered_once_and_conflicting_token_reuse_is_terminal() -> TestResult {
    let (client, mut sender) = pair()?;
    let mut channel = Channel::isolated_peer(client)?;
    let mut offer = 1u32.to_le_bytes().to_vec();
    offer.extend_from_slice(&17u32.to_le_bytes());
    string(&mut offer, "fixture.flac");
    offer.extend_from_slice(&1000u64.to_le_bytes());
    send(&mut sender, 40, &offer)?;
    send(&mut sender, 40, &offer)?;
    let mut queued = Vec::new();
    string(&mut queued, "fixture.flac");
    queued.extend_from_slice(&2u32.to_le_bytes());
    send(&mut sender, 44, &queued)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    assert!(matches!(
        channel.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Peer(super::super::messages::PeerEvent::Offer { token: 17, .. })
    ));
    assert!(matches!(
        channel.receive(deadline, &mut || Ok(()), |_| true)?,
        Event::Peer(super::super::messages::PeerEvent::Position { place: 2, .. })
    ));
    let length = offer.len();
    offer[length - 8..].copy_from_slice(&1001u64.to_le_bytes());
    send(&mut sender, 40, &offer)?;
    let error = channel
        .receive(deadline, &mut || Ok(()), |_| true)
        .err()
        .ok_or("conflicting offer accepted")?;
    assert!(error.to_string().contains("reused for a different offer"));
    assert!(matches!(channel.check(), Err(Error::Remote(_))));
    assert!(matches!(channel.send(51, &queued), Err(Error::Remote(_))));
    Ok(())
}

#[test]
fn terminal_server_failure_retires_peer_channels_without_becoming_retryable() -> TestResult {
    let (client, mut server) = pair()?;
    let channel = Channel::server(client)?;
    let (client, mut remote) = pair()?;
    let mut peer = channel.peer(client)?;
    send(&mut server, 41, &[])?;
    assert!(matches!(
        peer.receive(
            Instant::now() + Duration::from_secs(3),
            &mut || Ok(()),
            |_| true
        ),
        Err(Error::Remote(message)) if message.contains("signed in elsewhere")
    ));
    assert!(matches!(peer.send(51, &[]), Err(Error::Remote(_))));
    assert_eq!(remote.read(&mut [0])?, 0);
    Ok(())
}

#[test]
fn failed_write_ack_preserves_the_terminal_connection_error() -> TestResult {
    for (disconnected, server_disconnected) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let (commands, receiver) = mpsc::sync_channel(MAX_COMMANDS);
        let state = Arc::new(State {
            error: Mutex::new(None),
            borrowers: AtomicUsize::new(0),
        });
        let failed = Arc::clone(&state);
        let failure = Arc::new(Mutex::new(None));
        let failed_session = Arc::clone(&failure);
        let owner = thread::spawn(move || {
            let Ok(Command::Send { reply, .. }) = receiver.recv_timeout(Duration::from_secs(3))
            else {
                panic!("write command not received");
            };
            {
                let Ok(mut error) = failed.error.lock() else {
                    panic!("connection state poisoned");
                };
                *error = Some(Error::Remote("fixture terminal protocol failure"));
            }
            if server_disconnected {
                let Ok(mut failure) = failed_session.lock() else {
                    panic!("session state poisoned");
                };
                *failure = Some(SessionFailure::Transport(std::io::ErrorKind::UnexpectedEof));
            }
            if !disconnected {
                assert!(
                    reply
                        .try_send(Err(Error::SoulseekTransport(
                            std::io::ErrorKind::NotConnected.into(),
                        )))
                        .is_ok(),
                    "write response receiver closed"
                );
            }
        });
        let (_sender, events) = mpsc::sync_channel(MAX_EVENTS);
        let channel = Channel {
            runtime: Arc::new(Runtime {
                commands,
                stop: Arc::new(AtomicBool::new(false)),
                thread: Some(owner),
                generation: GENERATION.fetch_add(1, Ordering::Relaxed),
                failure,
                sharing: Mutex::new(None),
            }),
            id: 1,
            events,
            pending: VecDeque::new(),
            state,
            route: None,
            writer: Arc::new(Mutex::new(())),
        };
        let error = channel
            .send(51, &[])
            .err()
            .ok_or("write unexpectedly succeeded")?;
        assert!(matches!(
            error,
            Error::Remote("fixture terminal protocol failure")
        ));
        assert!(
            super::super::ReconnectBudget::default()
                .next(&error)
                .is_none()
        );
    }
    Ok(())
}

#[test]
fn routing_deadline_is_rechecked_after_callbacks_without_losing_the_event() -> TestResult {
    let (client, _remote) = pair()?;
    let mut channel = Channel::isolated_peer(client)?;
    let (sender, events) = mpsc::sync_channel(MAX_EVENTS);
    channel.events = events;
    sender.try_send(Envelope {
        event: Some(Event::Peer(super::super::messages::PeerEvent::Position {
            path: "fixture.flac".into(),
            place: 2,
        })),
        bytes: 0,
        budget: Arc::new(AtomicUsize::new(0)),
    })?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut checks = 0;
    let result = channel.receive(
        deadline,
        &mut || {
            checks += 1;
            if checks == 2 {
                thread::sleep(
                    deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(10),
                );
            }
            Ok(())
        },
        |_| true,
    );
    assert!(matches!(
        result,
        Err(Error::Remote("Soulseek operation exceeded its time limit"))
    ));
    assert!(matches!(
        channel.receive(
            Instant::now() + Duration::from_secs(3),
            &mut || Ok(()),
            |_| true
        )?,
        Event::Peer(super::super::messages::PeerEvent::Position { place: 2, .. })
    ));
    Ok(())
}
