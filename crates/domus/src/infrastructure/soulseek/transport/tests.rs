use super::*;
use crate::infrastructure::soulseek::messages::ServerEvent;
use crate::infrastructure::soulseek::protocol::{read_frame, string};
use std::net::TcpListener;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

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
            }),
            id: 1,
            events,
            pending: VecDeque::new(),
            state,
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
