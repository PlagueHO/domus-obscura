use super::*;
use std::net::TcpListener;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
const PATH: &str = "Peer\\Album\\one.flac";

mod journal;
#[path = "../../../tests/support/media.rs"]
mod media;

fn packet(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    assert!(length < 8192);
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn listener() -> std::io::Result<(TcpListener, PeerAddress)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    // Only unit fixtures bypass the public-IP policy. The public constructor
    // always obtains a validated address from the server session.
    let peer = PeerAddress {
        username: "fixture-peer".into(),
        address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
    };
    Ok((listener, peer))
}

fn handshake(listener: TcpListener) -> std::io::Result<TcpStream> {
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut expected = vec![1];
    string(&mut expected, "fixture-user");
    string(&mut expected, "P");
    expected.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(packet(&mut stream)?, expected);
    Ok(stream)
}

fn queued(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut expected = 43u32.to_le_bytes().to_vec();
    string(&mut expected, PATH);
    assert_eq!(packet(stream)?, expected);
    Ok(())
}

fn offer(path: &str, size: u64, direction: u32) -> Vec<u8> {
    let mut body = direction.to_le_bytes().to_vec();
    body.extend_from_slice(&17u32.to_le_bytes());
    string(&mut body, path);
    body.extend_from_slice(&size.to_le_bytes());
    body
}

#[test]
fn direct_peer_init_queue_and_upload_response_use_exact_wire_variants() -> TestResult {
    let (listener, address) = listener()?;
    let accepted = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&accepted);
    let task = std::thread::spawn(move || -> TestResultSend {
        let mut stream = handshake(listener)?;
        queued(&mut stream)?;
        let mut position = Vec::new();
        string(&mut position, PATH);
        position.extend_from_slice(&2u32.to_le_bytes());
        send(&mut stream, 44, &position)?;
        send(&mut stream, 40, &offer(PATH, 1000, 1))?;
        let expected = [41u32.to_le_bytes().as_slice(), &17u32.to_le_bytes(), &[1]].concat();
        assert_eq!(packet(&mut stream)?, expected);
        assert!(observed.load(Ordering::SeqCst));
        Ok(())
    });
    let mut session = SoulseekSession {
        stream: None,
        username: "fixture-user".into(),
    };
    let mut peer = session.connect_address(address)?;
    assert_eq!(peer.username(), "fixture-peer");
    let mut checks = 0;
    let token = peer.queue_upload(PATH, 1000, || {
        checks += 1;
        Ok(())
    })?;
    assert!(checks > 1);
    assert!(peer.queue_upload(PATH, 1000, || Ok(())).is_err());
    assert!(peer.accept_upload(18).is_err());
    accepted.store(true, Ordering::SeqCst);
    peer.accept_upload(token)?;
    assert!(peer.accept_upload(token).is_err());
    join(task)
}

type TestResultSend = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn server_requested_reverse_control_uses_its_independent_init_token() -> TestResult {
    let (listener, address) = listener()?;
    let task = std::thread::spawn(move || -> TestResultSend {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        assert_eq!(
            packet(&mut stream)?,
            [vec![0], 999u32.to_le_bytes().to_vec()].concat()
        );
        queued(&mut stream)?;
        send(&mut stream, 40, &offer(PATH, 1000, 1))?;
        assert_eq!(
            packet(&mut stream)?,
            [41u32.to_le_bytes().as_slice(), &17u32.to_le_bytes(), &[1]].concat()
        );
        Ok(())
    });
    let mut session = SoulseekSession {
        stream: None,
        username: "fixture-user".into(),
    };
    let socket = TcpStream::connect(address.address)?;
    let mut peer = session.control_from_socket(address, socket, Some(999))?;
    let token = peer.queue_upload(PATH, 1000, || Ok(()))?;
    peer.accept_upload(token)?;
    join(task)
}

#[test]
fn login_disconnect_remains_a_transient_transport_error() -> TestResult {
    let (listener, address) = listener()?;
    let task = std::thread::spawn(move || -> TestResultSend {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        assert_eq!(&packet(&mut stream)?[..4], &1u32.to_le_bytes());
        Ok(())
    });
    assert!(matches!(
        SoulseekSession::login(
            TcpStream::connect(address.address)?,
            "fixture-user",
            "generated-only"
        ),
        Err(Error::SoulseekTransport(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof
    ));
    join(task)
}

fn join(task: std::thread::JoinHandle<TestResultSend>) -> TestResult {
    task.join()
        .map_err(|_| "peer fixture panicked")?
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    Ok(())
}

#[test]
fn invalid_offers_denials_and_unsupported_messages_close_without_acceptance() -> TestResult {
    for variant in 0..7 {
        let (listener, address) = listener()?;
        let task = std::thread::spawn(move || -> TestResultSend {
            let mut stream = handshake(listener)?;
            queued(&mut stream)?;
            let (code, body) = match variant {
                0 => (40, offer("other-file", 1000, 1)),
                1 => (40, offer(PATH, 999, 1)),
                2 => (40, offer(PATH, 1000, 0)),
                3 => {
                    let mut body = offer(PATH, 1000, 1);
                    body.push(0);
                    (40, body)
                }
                4 => {
                    let mut body = Vec::new();
                    string(&mut body, PATH);
                    string(&mut body, "password\nSECRET");
                    (50, body)
                }
                5 => {
                    let mut body = Vec::new();
                    string(&mut body, PATH);
                    (46, body)
                }
                _ => (9, vec![]),
            };
            send(&mut stream, code, &body)?;
            assert_eq!(stream.read(&mut [0; 1])?, 0);
            Ok(())
        });
        let mut session = SoulseekSession {
            stream: None,
            username: "fixture-user".into(),
        };
        let mut peer = session.connect_address(address)?;
        let error = peer
            .queue_upload(PATH, 1000, || Ok(()))
            .err()
            .ok_or("accepted invalid offer")?;
        assert!(!error.to_string().contains("SECRET"));
        assert!(peer.accept_upload(17).is_err());
        assert!(peer.queue_upload(PATH, 1000, || Ok(())).is_err());
        join(task)?;
    }
    Ok(())
}

#[test]
fn ownership_check_failure_during_reply_prevents_acceptance() -> TestResult {
    let (listener, address) = listener()?;
    let lost = Arc::new(AtomicBool::new(false));
    let remote = Arc::clone(&lost);
    let task = std::thread::spawn(move || -> TestResultSend {
        let mut stream = handshake(listener)?;
        queued(&mut stream)?;
        remote.store(true, Ordering::SeqCst);
        if let Err(error) = send(&mut stream, 40, &offer(PATH, 1000, 1)) {
            return match error {
                Error::Io(error) if connection_closed(&error) => Ok(()),
                other => Err(other.into()),
            };
        }
        match stream.read(&mut [0; 1]) {
            Ok(0) => Ok(()),
            Err(error) if connection_closed(&error) => Ok(()),
            other => Err(format!("unexpected peer response: {other:?}").into()),
        }
    });
    let mut session = SoulseekSession {
        stream: None,
        username: "fixture-user".into(),
    };
    let mut peer = session.connect_address(address)?;
    let result = peer.queue_upload(PATH, 1000, || {
        if lost.load(Ordering::SeqCst) {
            return Err(Error::Conflict("worker ownership lost".into()));
        }
        Ok(())
    });
    assert!(matches!(result, Err(Error::Conflict(message)) if message == "worker ownership lost"));
    assert!(peer.accept_upload(17).is_err());
    join(task)
}

fn connection_closed(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
    )
}

#[test]
fn expired_deadline_rejects_even_an_already_buffered_message() -> TestResult {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let mut sender = TcpStream::connect(listener.local_addr()?)?;
    let (mut receiver, _) = listener.accept()?;
    send(&mut sender, 3, &[])?;
    assert!(frame(&mut receiver, Instant::now() - Duration::from_millis(1)).is_err());
    Ok(())
}

#[test]
fn reconnect_budget_is_finite_and_never_retries_protocol_storage_or_approval_errors() {
    let mut budget = ReconnectBudget::default();
    for (index, seconds) in [1, 2, 4].into_iter().enumerate() {
        assert_eq!(
            budget.next(&Error::SoulseekTransport(
                std::io::ErrorKind::ConnectionReset.into()
            )),
            Some(Duration::from_secs(seconds))
        );
        assert_eq!(usize::from(budget.attempts), index + 1);
        assert_eq!(budget.next(&Error::Remote("Soulseek rejected login")), None);
        assert_eq!(
            budget.next(&Error::Io(std::io::ErrorKind::ConnectionReset.into())),
            None
        );
        assert_eq!(
            budget.next(&Error::Conflict("approval changed".into())),
            None
        );
    }

    assert_eq!(
        budget.next(&Error::SoulseekTransport(
            std::io::ErrorKind::UnexpectedEof.into()
        )),
        None
    );
}

#[test]
fn queue_timeout_never_resubmits_or_treats_no_reply_as_absence() -> TestResult {
    let (listener, address) = listener()?;
    let task = std::thread::spawn(move || -> TestResultSend {
        let mut peer = handshake(listener)?;
        let mut expected = 51u32.to_le_bytes().to_vec();
        string(&mut expected, PATH);
        assert_eq!(packet(&mut peer)?, expected);
        assert_eq!(
            peer.read(&mut [0])?,
            0,
            "queue timeout sent another command"
        );
        Ok(())
    });
    let mut session = SoulseekSession {
        stream: None,
        username: "fixture-user".into(),
    };
    let mut peer = session.connect_address(address)?;
    let error = peer
        .wait_upload_until(
            PATH,
            1000,
            false,
            |_| Ok(()),
            Instant::now() + Duration::from_millis(100),
            Duration::from_secs(60),
        )
        .err()
        .ok_or("silent queue unexpectedly accepted")?;
    assert!(error.to_string().contains("time limit"));
    assert_eq!(QUEUE_TIMEOUT, Duration::from_secs(1800));
    assert_eq!(QUEUE_POLL, Duration::from_secs(60));
    join(task)
}
