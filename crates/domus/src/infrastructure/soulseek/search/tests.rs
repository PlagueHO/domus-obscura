use super::*;
use flate2::{Compression, write::ZlibEncoder};
use std::net::TcpListener;

type TestResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn compress(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(input)?;
    encoder.finish()
}

fn result_file(path: &str, attributes: &[(u32, u32)]) -> Vec<u8> {
    let mut bytes = vec![1];
    string(&mut bytes, path);
    bytes.extend_from_slice(&1000u64.to_le_bytes());
    string(&mut bytes, "");
    bytes.extend_from_slice(&(attributes.len() as u32).to_le_bytes());
    for (code, value) in attributes {
        bytes.extend_from_slice(&code.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn response() -> Vec<u8> {
    let mut bytes = Vec::new();
    string(&mut bytes, "fixture-peer");
    bytes.extend_from_slice(&17u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend(result_file(
        "Peer\\Album\\01 - One.flac",
        &[(1, 30), (4, 44100), (5, 16)],
    ));
    bytes.push(1);
    bytes.extend_from_slice(&1234u32.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes
}

#[test]
fn search_results_bind_peer_token_public_files_and_optional_private_tail() -> TestResult {
    for private in [false, true] {
        let mut bytes = response();
        if private {
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&1u32.to_le_bytes());
            bytes.extend(result_file("Private\\hidden.flac", &[]));
        }
        let zipped = compress(&bytes)?;
        let result = parse_response(&zipped, "fixture-peer", 17)?;
        assert_eq!(result.peer, "fixture-peer");
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].remote_path, "Peer\\Album\\01 - One.flac");
        assert_eq!(result.files[0].expected_bytes, 1000);
        assert_eq!(result.files[0].duration_seconds, Some(30));
        assert_eq!(result.files[0].sample_rate, Some(44100));
        assert_eq!(result.files[0].bit_depth, Some(16));
        assert_eq!(result.files[0].bitrate_kbps, None);
        assert_eq!(result.files[0].vbr, None);
        assert!(result.slot_free);
        assert_eq!(result.speed, 1234);
        assert_eq!(result.queue_length, 2);
        assert!(parse_response(&zipped, "wrong-peer", 17).is_err());
        assert!(parse_response(&zipped, "fixture-peer", 18).is_err());
    }
    Ok(())
}

#[test]
fn search_zlib_has_exact_end_checksum_and_expansion_limits() -> TestResult {
    let zipped = compress(&response())?;
    assert_eq!(expanded(&zipped)?, response());
    for length in 0..zipped.len() {
        assert!(expanded(&zipped[..length]).is_err(), "length: {length}");
    }
    let mut trailing = zipped.clone();
    trailing.push(0);
    assert!(expanded(&trailing).is_err());
    let mut checksum = zipped.clone();
    let last = checksum.len() - 1;
    checksum[last] ^= 1;
    assert!(expanded(&checksum).is_err());
    assert!(expanded(&compress(&vec![0; MAX_EXPANDED + 1])?).is_err());
    assert_eq!(
        expanded(&compress(&vec![0; MAX_EXPANDED])?)?.len(),
        MAX_EXPANDED
    );
    assert!(expanded(&vec![0; MAX_FRAME]).is_err());
    Ok(())
}

#[test]
fn search_files_reject_unsafe_paths_attributes_counts_and_trailing_fields() -> TestResult {
    for path in [
        "",
        "..\\secret.flac",
        "Album\\.\\one.flac",
        "Album\\\\one.flac",
        "a\nb.flac",
    ] {
        assert!(file(&mut Fields(&result_file(path, &[]))).is_err());
    }
    for attributes in [vec![(2, 2)], vec![(1, 2), (1, 3)], vec![(0, 1); 33]] {
        assert!(file(&mut Fields(&result_file("Album\\one.mp3", &attributes))).is_err());
    }
    let mut bytes = response();
    bytes.push(0);
    assert!(parse_response(&compress(&bytes)?, "fixture-peer", 17).is_err());
    let mut count = Vec::new();
    string(&mut count, "fixture-peer");
    count.extend_from_slice(&17u32.to_le_bytes());
    count.extend_from_slice(&1001u32.to_le_bytes());
    assert!(parse_response(&compress(&count)?, "fixture-peer", 17).is_err());
    let mut duplicate = Vec::new();
    string(&mut duplicate, "fixture-peer");
    duplicate.extend_from_slice(&17u32.to_le_bytes());
    duplicate.extend_from_slice(&2u32.to_le_bytes());
    duplicate.extend(result_file("Album\\one.flac", &[]));
    duplicate.extend(result_file("album/ONE.flac", &[]));
    assert!(parse_response(&compress(&duplicate)?, "fixture-peer", 17).is_err());
    Ok(())
}

fn request(ip: Ipv4Addr, port: u32, kind: &str) -> Vec<u8> {
    let mut body = Vec::new();
    string(&mut body, "fixture-peer");
    string(&mut body, kind);
    body.extend_from_slice(&u32::from(ip).to_le_bytes());
    body.extend_from_slice(&port.to_le_bytes());
    body.extend_from_slice(&123u32.to_le_bytes());
    body.push(0);
    body
}

#[test]
fn reverse_search_requests_reject_nonpublic_destinations_and_wrong_types() -> TestResult {
    let (peer, token) = search_request(&request(Ipv4Addr::new(8, 8, 8, 8), 4242, "P"))?;
    assert_eq!(peer.username, "fixture-peer");
    assert_eq!(
        peer.address,
        SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 4242)
    );
    assert_eq!(token, 123);
    for ip in [
        Ipv4Addr::LOCALHOST,
        Ipv4Addr::new(10, 0, 0, 1),
        Ipv4Addr::new(100, 64, 0, 1),
        Ipv4Addr::new(203, 0, 113, 1),
    ] {
        assert!(search_request(&request(ip, 4242, "P")).is_err());
    }
    for port in [0, 65536] {
        assert!(search_request(&request(Ipv4Addr::new(8, 8, 8, 8), port, "P")).is_err());
    }
    assert!(search_request(&request(Ipv4Addr::new(8, 8, 8, 8), 4242, "F")).is_err());
    Ok(())
}

#[test]
fn reverse_search_tcp_sends_independent_connection_token_and_reads_only_search_response()
-> TestResult {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = match listener.local_addr()? {
        std::net::SocketAddr::V4(address) => address,
        _ => return Err("not IPv4".into()),
    };
    let task = std::thread::spawn(move || -> std::io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut init = [0; 9];
        stream.read_exact(&mut init)?;
        assert_eq!(init, [5, 0, 0, 0, 0, 123, 0, 0, 0]);
        let body = compress(&response())?;
        send(&mut stream, 9, &body).map_err(std::io::Error::other)?;
        let mut byte = [0];
        match stream.read(&mut byte) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) => {}
            other => {
                return Err(std::io::Error::other(format!(
                    "unexpected client message: {other:?}"
                )));
            }
        }
        Ok(())
    });
    let peer = PeerAddress {
        username: "fixture-peer".into(),
        address,
    };
    let result = search_peer(&peer, 123, 17, Instant::now() + Duration::from_secs(3))?;
    assert_eq!(result.files.len(), 1);
    task.join().map_err(|_| "fixture panic")??;
    Ok(())
}

#[test]
fn search_sends_exact_server_request_and_stops_on_relogin_or_invalid_reverse_request() -> TestResult
{
    for reply in [41, 18] {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let stream = TcpStream::connect(listener.local_addr()?)?;
        let task = std::thread::spawn(move || -> TestResult {
            let (mut server, _) = listener.accept()?;
            let (code, body) = frame(&mut server, Instant::now() + Duration::from_secs(3))?;
            assert_eq!(code, 26);
            let mut fields = Fields(&body);
            assert_eq!(fields.u32()?, 17);
            assert_eq!(fields.text(256)?, "Fixture Artist Fixture Album");
            fields.end()?;
            let body = if reply == 18 {
                request(Ipv4Addr::LOCALHOST, 4242, "P")
            } else {
                Vec::new()
            };
            send(&mut server, reply, &body)?;
            server.set_read_timeout(Some(Duration::from_secs(3)))?;
            let mut byte = [0];
            assert_eq!(server.read(&mut byte)?, 0);
            Ok(())
        });
        let session = SoulseekSession {
            stream: Some(stream),
            username: "fixture-user".into(),
        };
        let error = session
            .search("Fixture Artist Fixture Album", 17)
            .err()
            .ok_or("expected search rejection")?;
        assert!(error.to_string().contains(if reply == 41 {
            "signed in elsewhere"
        } else {
            "not a public unicast"
        }));
        task.join().map_err(|_| "server fixture panic")??;
    }
    Ok(())
}

#[test]
fn search_rejects_invalid_query_or_token_before_any_server_message() -> TestResult {
    for (query, token) in [
        ("", 17),
        (" \t ", 17),
        ("a\nb", 17),
        ("Valid", 0),
        (&"x".repeat(257), 17),
    ] {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let stream = TcpStream::connect(listener.local_addr()?)?;
        let (mut server, _) = listener.accept()?;
        server.set_read_timeout(Some(Duration::from_secs(3)))?;
        let session = SoulseekSession {
            stream: Some(stream),
            username: "fixture-user".into(),
        };
        assert!(session.search(query, token).is_err());
        assert_eq!(server.read(&mut [0])?, 0);
    }
    Ok(())
}
