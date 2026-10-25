use super::*;
use flate2::{Compression, write::ZlibEncoder};
use std::io::Write;
use std::net::TcpListener;

type TestResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;
const FOLDER: &str = "@@share\\Artist\\Album";

fn compress(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(input)?;
    encoder.finish()
}

fn entry(name: &str) -> Vec<u8> {
    let mut body = vec![1];
    string(&mut body, name);
    body.extend_from_slice(&1234u64.to_le_bytes());
    string(&mut body, "");
    body.extend_from_slice(&3u32.to_le_bytes());
    for (code, value) in [(0u32, 320u32), (1, 5), (2, 0)] {
        body.extend_from_slice(&code.to_le_bytes());
        body.extend_from_slice(&value.to_le_bytes());
    }
    body
}

fn listing(directories: &[(&str, Vec<&str>)]) -> Vec<u8> {
    let mut body = 17u32.to_le_bytes().to_vec();
    string(&mut body, FOLDER);
    body.extend_from_slice(&(directories.len() as u32).to_le_bytes());
    for (path, files) in directories {
        string(&mut body, path);
        body.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for name in files {
            body.extend(entry(name));
        }
    }
    body
}

#[test]
fn folder_listing_keeps_subfolders_attributes_and_exact_request_bindings() -> TestResult {
    let body = listing(&[
        (FOLDER, vec!["01 - One.mp3", "folder.jpg"]),
        ("@@share\\Artist\\Album\\Disc 2", vec!["02 - Two.mp3"]),
    ]);
    let zipped = compress(&body)?;
    let result = parse_response(&zipped, "peer", FOLDER, 17)?;
    assert_eq!(result.directories, 2);
    assert_eq!(result.files.len(), 3);
    assert_eq!(
        result.files[0].remote_path,
        format!("{FOLDER}\\01 - One.mp3")
    );
    assert_eq!(
        result.files[2].remote_path,
        format!("{FOLDER}\\Disc 2\\02 - Two.mp3")
    );
    assert_eq!(result.files[0].expected_bytes, 1234);
    assert_eq!(result.files[0].bitrate_kbps, Some(320));
    assert_eq!(result.files[0].vbr, Some(false));
    assert!(parse_response(&zipped, "peer", FOLDER, 18).is_err());
    assert!(parse_response(&zipped, "peer", "@@other", 17).is_err());
    for end in 0..zipped.len() {
        assert!(parse_response(&zipped[..end], "peer", FOLDER, 17).is_err());
    }
    Ok(())
}

#[test]
fn folder_response_rejects_escapes_duplicates_unbound_directories_and_trailing_data() -> TestResult
{
    for dirs in [
        vec![(FOLDER, vec!["../escape.flac"])],
        vec![(FOLDER, vec!["child\\one.flac"])],
        vec![(FOLDER, vec!["one.flac", "ONE.flac"])],
        vec![
            (FOLDER, vec!["one.flac"]),
            ("@@share\\Artist\\Other", vec![]),
        ],
        vec![(FOLDER, vec![]), (FOLDER, vec![])],
        vec![("@@share\\Artist\\Album\\CD1", vec!["one.flac"])],
        vec![],
    ] {
        assert!(parse_response(&compress(&listing(&dirs))?, "peer", FOLDER, 17).is_err());
    }
    let mut body = listing(&[(FOLDER, vec![])]);
    body.push(0);
    assert!(parse_response(&compress(&body)?, "peer", FOLDER, 17).is_err());
    let mut zipped = compress(&listing(&[(FOLDER, vec![])]))?;
    zipped.push(0);
    assert!(parse_response(&zipped, "peer", FOLDER, 17).is_err());
    assert!(
        parse_response(
            &compress(&vec![0; protocol::MAX_EXPANDED + 1])?,
            "peer",
            FOLDER,
            17
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn folder_listing_enforces_exact_directory_and_total_file_limits() -> TestResult {
    let names = (0..MAX_FOLDER_FILES)
        .map(|index| format!("{index}.flac"))
        .collect::<Vec<_>>();
    let refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let body = listing(&[(FOLDER, refs.clone())]);
    assert_eq!(
        parse_response(&compress(&body)?, "peer", FOLDER, 17)?
            .files
            .len(),
        MAX_FOLDER_FILES
    );
    let mut over = refs;
    over.push("extra.flac");
    assert!(parse_response(&compress(&listing(&[(FOLDER, over)]))?, "peer", FOLDER, 17).is_err());
    let subfolders = (1..MAX_FOLDER_DIRECTORIES)
        .map(|index| format!("{FOLDER}\\{index}"))
        .collect::<Vec<_>>();
    let mut dirs = vec![(FOLDER, vec![])];
    dirs.extend(subfolders.iter().map(|path| (path.as_str(), vec![])));
    assert_eq!(
        usize::from(parse_response(&compress(&listing(&dirs))?, "peer", FOLDER, 17)?.directories),
        MAX_FOLDER_DIRECTORIES
    );
    dirs.push(("@@share\\Artist\\Album\\extra", vec![]));
    assert!(parse_response(&compress(&listing(&dirs))?, "peer", FOLDER, 17).is_err());
    Ok(())
}

#[test]
fn folder_tcp_sends_only_one_correlated_metadata_request_and_closes_the_channel() -> TestResult {
    for code in [37, 9] {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let task = std::thread::spawn(move || -> TestResult {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            let mut size = [0; 4];
            stream.read_exact(&mut size)?;
            let mut init = vec![0; u32::from_le_bytes(size) as usize];
            stream.read_exact(&mut init)?;
            assert_eq!(init[0], 1);
            let mut fields = Fields(&init[1..]);
            assert_eq!(fields.text(30)?, "fixture-user");
            assert_eq!(fields.text(1)?, "P");
            assert_eq!(fields.u32()?, 0);
            fields.end()?;
            let (request, body) = frame(&mut stream, Instant::now() + Duration::from_secs(3))?;
            assert_eq!(request, 36);
            let mut fields = Fields(&body);
            assert_eq!(fields.u32()?, 17);
            assert_eq!(fields.text(4096)?, FOLDER);
            fields.end()?;
            send(
                &mut stream,
                code,
                &compress(&listing(&[(FOLDER, vec!["01 - One.mp3"])]))?,
            )?;
            assert_eq!(stream.read(&mut [0; 1])?, 0);
            Ok(())
        });
        let mut session = SoulseekSession {
            stream: None,
            username: "fixture-user".into(),
        };
        let peer = session.connect_address(PeerAddress {
            username: "fixture-peer".into(),
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, address.port()),
        })?;
        let result = peer.folder_contents(FOLDER, 17);
        if code == 37 {
            assert_eq!(result?.files.len(), 1);
        } else {
            assert!(result.is_err());
        }
        task.join().map_err(|_| "folder peer fixture panicked")??;
    }
    Ok(())
}
