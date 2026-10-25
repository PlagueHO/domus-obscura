use domus::infrastructure::soulseek::SoulseekSession;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use symphonia::core::{checksum::Md5, io::Monitor};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type PeerTask = std::thread::JoinHandle<std::io::Result<()>>;

fn pair() -> std::io::Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let client = TcpStream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    server.set_read_timeout(Some(Duration::from_secs(3)))?;
    server.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok((client, server))
}

fn text(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn send(stream: &mut TcpStream, code: u32, body: &[u8]) -> std::io::Result<()> {
    stream.write_all(&((body.len() + 4) as u32).to_le_bytes())?;
    stream.write_all(&code.to_le_bytes())?;
    stream.write_all(body)
}

fn packet(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    assert!(length <= 1024 * 1024);
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    Ok(body)
}

fn response() -> Vec<u8> {
    let mut body = vec![1];
    text(&mut body, "fixture greeting");
    body.extend_from_slice(&0x01020304u32.to_le_bytes());
    let mut md5 = Md5::default();
    md5.process_buf_bytes(b"password");
    text(
        &mut body,
        &md5.md5()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    );
    body.push(0);
    body
}

fn ready(server: &mut TcpStream) -> std::io::Result<()> {
    let mut expected = 1u32.to_le_bytes().to_vec();
    text(&mut expected, "username");
    text(&mut expected, "password");
    expected.extend_from_slice(&177u32.to_le_bytes());
    text(&mut expected, "d51c9a7e9353746a6020f9602d452929");
    expected.extend_from_slice(&1u32.to_le_bytes());
    assert_eq!(packet(server)?, expected);
    send(server, 1, &response())?;
    assert_eq!(
        packet(server)?,
        [28u32.to_le_bytes(), 2u32.to_le_bytes()].concat()
    );
    assert_eq!(
        packet(server)?,
        [35u32.to_le_bytes().as_slice(), &[0; 8]].concat()
    );
    assert_eq!(
        packet(server)?,
        [100u32.to_le_bytes().as_slice(), &[0]].concat()
    );
    Ok(())
}

fn join(task: PeerTask) -> TestResult {
    task.join().map_err(|_| "fixture server panicked")??;
    Ok(())
}

#[test]
fn server_login_matches_documented_vector_and_binds_peer_reply() -> TestResult {
    let (client, mut server) = pair()?;
    let task = std::thread::spawn(move || {
        ready(&mut server)?;
        let mut request = 3u32.to_le_bytes().to_vec();
        text(&mut request, "fixture-peer");
        assert_eq!(packet(&mut server)?, request);
        send(&mut server, 104, &60u32.to_le_bytes())?;
        let mut body = Vec::new();
        text(&mut body, "fixture-peer");
        body.extend_from_slice(&0x08080404u32.to_le_bytes());
        body.extend_from_slice(&2234u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        send(&mut server, 3, &body)
    });
    let mut session = SoulseekSession::login(client, "username", "password")?;
    assert_eq!(session.username(), "username");
    let peer = session.peer_address("fixture-peer")?;
    assert_eq!(peer.username(), "fixture-peer");
    assert_eq!(peer.address().to_string(), "8.8.4.4:2234");
    join(task)
}

#[test]
fn failed_login_never_echoes_remote_credentials_or_control_text() -> TestResult {
    for code in [
        "INVALIDPASS",
        "INVALIDUSERNAME",
        "SVRFULL",
        "password\nSECRET",
    ] {
        let (client, mut server) = pair()?;
        let task = std::thread::spawn(move || {
            packet(&mut server)?;
            let mut body = vec![0];
            text(&mut body, code);
            send(&mut server, 1, &body)
        });
        let error = match SoulseekSession::login(client, "username", "password") {
            Ok(_) => return Err("rejected login succeeded".into()),
            Err(error) => error.to_string(),
        };
        assert!(!error.contains("SECRET"));
        assert!(!error.contains('\n'));
        assert!(!error.contains("password\n"));
        join(task)?;
    }
    Ok(())
}

#[test]
fn login_rejects_invalid_lengths_flags_hashes_truncation_and_trailing_data() -> TestResult {
    for variant in 0..7 {
        let (client, mut server) = pair()?;
        let task = std::thread::spawn(move || {
            packet(&mut server)?;
            match variant {
                0 => server.write_all(&3u32.to_le_bytes()),
                1 => server.write_all(&(1024u32 * 1024 + 1).to_le_bytes()),
                2 => send(&mut server, 1, &[2]),
                3 => send(&mut server, 99, &response()),
                4 => {
                    let mut body = response();
                    body.push(0);
                    send(&mut server, 1, &body)
                }
                5 => {
                    let mut body = response();
                    let length = body.len();
                    body[length - 2] ^= 1;
                    send(&mut server, 1, &body)
                }
                _ => {
                    server.write_all(&100u32.to_le_bytes())?;
                    server.write_all(&1u32.to_le_bytes())
                }
            }
        });
        assert!(SoulseekSession::login(client, "username", "password").is_err());
        join(task)?;
    }
    Ok(())
}

#[test]
fn lookup_rejects_private_addresses_wrong_users_and_closed_sessions() -> TestResult {
    for (ip, user, port) in [
        (0x7f000001u32, "fixture-peer", 2234u32),
        (0x0a000001, "fixture-peer", 2234),
        (0xa9fe0101, "fixture-peer", 2234),
        (0x64400001, "fixture-peer", 2234),
        (0xc0000201, "fixture-peer", 2234),
        (0xe0000001, "fixture-peer", 2234),
        (0x08080404, "other-peer", 2234),
        (0x08080404, "fixture-peer", 65536),
        (0x08080404, "fixture-peer", 0),
    ] {
        let (client, mut server) = pair()?;
        let task = std::thread::spawn(move || {
            ready(&mut server)?;
            packet(&mut server)?;
            let mut body = Vec::new();
            text(&mut body, user);
            body.extend_from_slice(&ip.to_le_bytes());
            body.extend_from_slice(&port.to_le_bytes());
            send(&mut server, 3, &body)?;
            let mut byte = [0];
            assert_eq!(server.read(&mut byte)?, 0);
            Ok(())
        });
        let mut session = SoulseekSession::login(client, "username", "password")?;
        assert!(session.peer_address("fixture-peer").is_err());
        assert!(session.peer_address("fixture-peer").is_err());
        join(task)?;
    }
    Ok(())
}

#[test]
fn lookup_is_bounded_by_message_count_and_reports_duplicate_login() -> TestResult {
    for duplicate in [false, true] {
        let (client, mut server) = pair()?;
        let task = std::thread::spawn(move || {
            ready(&mut server)?;
            packet(&mut server)?;
            if duplicate {
                send(&mut server, 41, &[])?;
            } else {
                for _ in 0..128 {
                    send(&mut server, 104, &[])?;
                }
            }
            Ok(())
        });
        let mut session = SoulseekSession::login(client, "username", "password")?;
        assert!(session.peer_address("fixture-peer").is_err());
        join(task)?;
    }
    Ok(())
}
