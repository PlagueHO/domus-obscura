//! Bounded generated Soulseek server and peers, outside the measured hosts.
use super::protocol::{Fields, read_frame, send, string};
use crate::infrastructure::filesystem::LibraryDirectory;
use crate::scale_tests::Channel;
use crate::scale_tests::integrated::{Album, Endpoints};
use crate::{Error, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{BufRead, Read, Write};
use std::net::{Shutdown, SocketAddrV4, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Default)]
struct Facts {
    downloads: Vec<(u64, u64)>,
    download_bytes: u64,
    upload: Option<(u64, u64)>,
    upload_bytes: u64,
    listed_files: u64,
}

struct State {
    stop: AtomicBool,
    list_requested: AtomicBool,
    logins: AtomicUsize,
    sockets: Mutex<Vec<TcpStream>>,
    server: Mutex<Option<TcpStream>>,
    channel: Channel,
    facts: Mutex<Facts>,
    start: Instant,
}

impl State {
    fn time(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    fn register(&self, socket: &TcpStream) -> Result<()> {
        socket.set_nonblocking(false)?;
        socket.set_read_timeout(Some(Duration::from_secs(120)))?;
        socket.set_write_timeout(Some(Duration::from_secs(30)))?;
        self.sockets
            .lock()
            .map_err(|_| Error::Internal("fixture socket registry poisoned"))?
            .push(socket.try_clone()?);
        Ok(())
    }
    fn accept(&self, listener: &TcpListener) -> Result<TcpStream> {
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(Error::WorkerStopping);
            }
            match listener.accept() {
                Ok((socket, _)) => {
                    self.register(&socket)?;
                    return Ok(socket);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    fn server_send(&self, code: u32, body: &[u8]) -> Result<()> {
        let mut server = self
            .server
            .lock()
            .map_err(|_| Error::Internal("fixture server poisoned"))?;
        send(
            server
                .as_mut()
                .ok_or(Error::Internal("fixture account not connected"))?,
            code,
            body,
        )
    }
}

fn frame(stream: &mut TcpStream) -> Result<(u32, Vec<u8>)> {
    read_frame(|bytes| {
        stream.read_exact(bytes)?;
        Ok(())
    })
}

fn response(stream: &mut TcpStream, expected: u32) -> Result<Vec<u8>> {
    loop {
        let (code, body) = frame(stream)?;
        if code == expected {
            return Ok(body);
        }
        if code != 9 {
            return Err(Error::Conflict(format!(
                "generated peer expected reply {expected}, received {code}"
            )));
        }
    }
}

fn init(stream: &mut TcpStream, kind: &str) -> Result<()> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if !(1..=256).contains(&length) {
        return Err(Error::Internal("fixture init length differs"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    let mut fields = Fields(&bytes[1..]);
    if bytes[0] != 1
        || fields.text(30)? != "scale-user"
        || fields.text(1)? != kind
        || fields.u32()? != 0
    {
        return Err(Error::Internal("fixture init differs"));
    }
    fields.end()
}

fn server(state: Arc<State>, listener: TcpListener) -> Result<()> {
    let mut socket = state.accept(&listener)?;
    let (code, body) = frame(&mut socket)?;
    let mut fields = Fields(&body);
    if code != 1 || fields.text(30)? != "scale-user" || fields.text(64)? != "generated-only" {
        return Err(Error::Internal("generated account login differs"));
    }
    state.logins.fetch_add(1, Ordering::AcqRel);
    let mut reply = vec![1];
    string(&mut reply, "generated scale fixture");
    reply.extend_from_slice(&0u32.to_le_bytes());
    string(&mut reply, &super::md5(&[b"generated-only"]));
    reply.push(0);
    send(&mut socket, 1, &reply)?;
    *state
        .server
        .lock()
        .map_err(|_| Error::Internal("fixture server poisoned"))? = Some(socket.try_clone()?);
    loop {
        let (code, body) = match frame(&mut socket) {
            Err(Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                ) =>
            {
                return Ok(());
            }
            other => other?,
        };
        match code {
            3 => {
                let mut fields = Fields(&body);
                let peer = fields.text(30)?;
                fields.end()?;
                let ip = match peer {
                    "scale-download" => [8, 8, 8, 8],
                    "scale-share" => [8, 8, 4, 4],
                    _ => return Err(Error::Internal("unexpected fixture peer")),
                };
                let mut reply = Vec::new();
                string(&mut reply, peer);
                reply.extend_from_slice(&u32::from(std::net::Ipv4Addr::from(ip)).to_le_bytes());
                reply.extend_from_slice(&2234u32.to_le_bytes());
                state.server_send(3, &reply)?;
            }
            2 | 28 | 32 | 35 | 100 => {}
            _ => {
                return Err(Error::InvalidInput(format!(
                    "unexpected generated server request {code}"
                )));
            }
        }
    }
}

fn download(state: Arc<State>, listener: TcpListener, audio: Arc<Vec<u8>>) -> Result<()> {
    let mut control = state.accept(&listener)?;
    init(&mut control, "P")?;
    for track in 1..=3u32 {
        let path = format!("Peer\\Album\\{track:02} Scale track {track}.mp3");
        let (code, body) = frame(&mut control)?;
        let mut fields = Fields(&body);
        if code != 43 || fields.text(4096)? != path {
            return Err(Error::Internal("generated queue request differs"));
        }
        fields.end()?;
        let token = 100 + track;
        let mut offer = 1u32.to_le_bytes().to_vec();
        offer.extend_from_slice(&token.to_le_bytes());
        string(&mut offer, &path);
        offer.extend_from_slice(&(audio.len() as u64).to_le_bytes());
        send(&mut control, 40, &offer)?;
        let (code, body) = frame(&mut control)?;
        let mut fields = Fields(&body);
        if code != 41 || fields.u32()? != token || !fields.boolean()? {
            return Err(Error::Internal("generated upload acceptance differs"));
        }
        fields.end()?;
        let indirect = 9000 + track;
        let mut request = Vec::new();
        string(&mut request, "scale-download");
        string(&mut request, "F");
        request.extend_from_slice(&u32::from(std::net::Ipv4Addr::new(8, 8, 8, 8)).to_le_bytes());
        request.extend_from_slice(&2234u32.to_le_bytes());
        request.extend_from_slice(&indirect.to_le_bytes());
        request.push(0);
        request.extend_from_slice(&[0; 8]);
        state.server_send(18, &request)?;
        let mut file = state.accept(&listener)?;
        let mut handshake = [0; 9];
        file.read_exact(&mut handshake)?;
        if handshake[..5] != [5, 0, 0, 0, 0] || handshake[5..] != indirect.to_le_bytes() {
            return Err(Error::Internal("generated file token differs"));
        }
        file.write_all(&token.to_le_bytes())?;
        let mut offset = [0; 8];
        file.read_exact(&mut offset)?;
        if offset != [0; 8] {
            return Err(Error::Internal("generated file resume offset differs"));
        }
        let start = state.time();
        state.channel.send(
            &json!({"event":"download_started","track":track,"at_ms":start,"bytes":audio.len()}),
        )?;
        for chunk in audio.chunks(512) {
            if state.stop.load(Ordering::Acquire) {
                return Err(Error::WorkerStopping);
            }
            file.write_all(chunk)?;
            std::thread::sleep(Duration::from_millis(40));
        }
        file.shutdown(Shutdown::Write)?;
        let end = state.time();
        let mut facts = state
            .facts
            .lock()
            .map_err(|_| Error::Internal("fixture facts poisoned"))?;
        facts.downloads.push((start, end));
        facts.download_bytes += audio.len() as u64;
        drop(facts);
        state.channel.send(
            &json!({"event":"download_complete","track":track,"at_ms":end,"bytes":audio.len()}),
        )?;
    }
    let mut closed = [0; 1];
    let _ = control.read(&mut closed);
    Ok(())
}

fn listing(state: &State, control: &mut TcpStream) -> Result<serde_json::Value> {
    send(control, 4, &[])?;
    let listing = response(control, 5)?;
    let mut expanded = Vec::new();
    flate2::read::ZlibDecoder::new(listing.as_slice())
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut expanded)?;
    if expanded.len() > 16 * 1024 * 1024 {
        return Err(Error::Internal("generated listing exceeded driver bound"));
    }
    let mut fields = Fields(&expanded);
    let directories = fields.u32()?;
    let mut files = 0u64;
    for _ in 0..directories {
        fields.text(4096)?;
        for _ in 0..fields.u32()? {
            super::protocol::file(&mut fields)?;
            files += 1;
        }
    }
    if fields.take(8)? != [0; 8] {
        return Err(Error::Internal("generated listing trailer differs"));
    }
    fields.end()?;
    state
        .facts
        .lock()
        .map_err(|_| Error::Internal("fixture facts poisoned"))?
        .listed_files = files;
    Ok(
        json!({"event":"listing_complete","files":files,"directories":directories,"expanded_bytes":expanded.len(),"compressed_bytes":listing.len()}),
    )
}

fn sharing(state: Arc<State>, listener: TcpListener, album: Album) -> Result<()> {
    let mut control = state.accept(&listener)?;
    init(&mut control, "P")?;
    let path = "Generated\\track-000000000000.mp3";
    let mut request = Vec::new();
    string(&mut request, path);
    send(&mut control, 43, &request)?;
    let offer = response(&mut control, 40)?;
    let mut fields = Fields(&offer);
    if fields.u32()? != 1 {
        return Err(Error::Internal("generated sharing offer differs"));
    }
    let token = fields.u32()?;
    if fields.text(4096)? != path || fields.u64()? != album.file_bytes {
        return Err(Error::Internal("generated sharing file differs"));
    }
    fields.end()?;
    let mut reply = token.to_le_bytes().to_vec();
    reply.push(1);
    send(&mut control, 41, &reply)?;
    let mut file = state.accept(&listener)?;
    init(&mut file, "F")?;
    let mut actual = [0; 4];
    file.read_exact(&mut actual)?;
    if u32::from_le_bytes(actual) != token {
        return Err(Error::Internal("generated sharing token differs"));
    }
    file.write_all(&0u64.to_le_bytes())?;
    let start = state.time();
    state
        .channel
        .send(&json!({"event":"upload_started","at_ms":start,"bytes":album.file_bytes}))?;
    let mut left = album.file_bytes;
    let mut buffer = [0; 1024];
    let mut hash = Sha256::new();
    let mut last_poll = Instant::now();
    let poll_queue = |control: &mut TcpStream| -> Result<()> {
        send(control, 51, &request)?;
        let reply = response(control, 44)?;
        let mut fields = Fields(&reply);
        if fields.text(4096)? != path || fields.u32()? != 0 {
            return Err(Error::Internal(
                "generated active-upload queue position differs",
            ));
        }
        fields.end()
    };
    while left > 0 {
        let wanted = left.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..wanted])?;
        hash.update(&buffer[..wanted]);
        left -= wanted as u64;
        std::thread::sleep(Duration::from_millis(60));
        if last_poll.elapsed() >= Duration::from_secs(3) {
            poll_queue(&mut control)?;
            last_poll = Instant::now();
        }
    }
    if format!("{:x}", hash.finalize()) != album.file_sha256 {
        return Err(Error::Internal("generated upload bytes changed"));
    }
    file.shutdown(Shutdown::Write)?;
    let end = state.time();
    let mut facts = state
        .facts
        .lock()
        .map_err(|_| Error::Internal("fixture facts poisoned"))?;
    facts.upload = Some((start, end));
    facts.upload_bytes = album.file_bytes;
    drop(facts);
    state.channel.send(&json!({"event":"upload_complete","at_ms":end,"bytes":album.file_bytes,"sha256":album.file_sha256}))?;
    while !state.list_requested.load(Ordering::Acquire) {
        if state.stop.load(Ordering::Acquire) {
            return Err(Error::WorkerStopping);
        }
        if last_poll.elapsed() >= Duration::from_secs(3) {
            poll_queue(&mut control)?;
            last_poll = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    control.set_read_timeout(Some(Duration::from_secs(1800)))?;
    let listing = listing(&state, &mut control)?;
    poll_queue(&mut control)?;
    state.channel.send(&listing)?;
    while !state.stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

pub(crate) fn run(root: &Path, reader: &mut impl BufRead, channel: Channel) -> TestResult {
    let album = crate::scale_tests::integrated::album(root)?;
    let opened = LibraryDirectory::open(root)?.open_directory(
        &crate::domain::LibraryPath::parse("media/catalog/shard-00000000")?,
    )?;
    let mut audio = Vec::new();
    opened
        .open_file(std::ffi::OsStr::new("track-000000000000.mp3"))?
        .take(8 * 1024 * 1024)
        .read_to_end(&mut audio)?;
    if audio.len() as u64 != album.file_bytes
        || format!("{:x}", Sha256::digest(&audio)) != album.file_sha256
    {
        return Err("generated integration source changed".into());
    }
    let server_listener = TcpListener::bind("127.0.0.1:0")?;
    let acquisition_listener = TcpListener::bind("127.0.0.1:0")?;
    let sharing_listener = TcpListener::bind("127.0.0.1:0")?;
    let address = |listener: &TcpListener| -> TestResult<SocketAddrV4> {
        listener.set_nonblocking(true)?;
        match listener.local_addr()? {
            std::net::SocketAddr::V4(address) => Ok(address),
            _ => Err("expected IPv4 fixture".into()),
        }
    };
    let endpoints = Endpoints {
        server: address(&server_listener)?,
        acquisition: address(&acquisition_listener)?,
        sharing: address(&sharing_listener)?,
    };
    channel.send(&json!({"event":"ready","role":"peers","endpoints":endpoints}))?;
    let state = Arc::new(State {
        stop: AtomicBool::new(false),
        list_requested: AtomicBool::new(false),
        logins: AtomicUsize::new(0),
        sockets: Mutex::new(Vec::new()),
        server: Mutex::new(None),
        channel: channel.clone(),
        facts: Mutex::new(Facts::default()),
        start: Instant::now(),
    });
    let spawn = |name: &'static str, run: Box<dyn FnOnce() -> Result<()> + Send>| {
        let state = state.clone();
        std::thread::spawn(move || {
            let result = run();
            if matches!(result, Err(Error::WorkerStopping)) && state.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            if let Err(error) = &result
                && !state.stop.load(Ordering::Acquire)
            {
                let _ = state
                    .channel
                    .send(&json!({"event":"peer_error","role":name,"message":error.to_string()}));
            }
            result.map_err(|e| e.to_string())
        })
    };
    let server_state = state.clone();
    let download_state = state.clone();
    let sharing_state = state.clone();
    let tasks = [
        spawn(
            "server",
            Box::new(move || server(server_state, server_listener)),
        ),
        spawn(
            "download",
            Box::new(move || download(download_state, acquisition_listener, Arc::new(audio))),
        ),
        spawn(
            "sharing",
            Box::new(move || sharing(sharing_state, sharing_listener, album)),
        ),
    ];
    let control = (|| -> TestResult {
        loop {
            let command = crate::scale_tests::line(reader)?;
            match command["action"].as_str() {
                Some("start") => {
                    let deadline = Instant::now() + Duration::from_secs(30);
                    while state
                        .server
                        .lock()
                        .map_err(|_| "fixture server poisoned")?
                        .is_none()
                    {
                        if Instant::now() >= deadline {
                            return Err("generated account did not connect".into());
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    let mut request = Vec::new();
                    string(&mut request, "scale-share");
                    request.extend_from_slice(&77u32.to_le_bytes());
                    string(&mut request, "track");
                    state.server_send(26, &request)?;
                    channel.send(&json!({"event":"started","role":"peers"}))?;
                }
                Some("list") => state.list_requested.store(true, Ordering::Release),
                Some("stop") => return Ok(()),
                _ => return Err("peers expects start, list or stop".into()),
            }
        }
    })();
    state.stop.store(true, Ordering::Release);
    for socket in state
        .sockets
        .lock()
        .map_err(|_| "fixture sockets poisoned")?
        .iter()
    {
        let _ = socket.shutdown(Shutdown::Both);
    }
    let mut failures = Vec::new();
    for task in tasks {
        if let Err(error) = task.join().map_err(|_| "fixture task panicked")? {
            failures.push(error);
        }
    }
    control?;
    let facts = state.facts.lock().map_err(|_| "fixture facts poisoned")?;
    let overlap = facts.upload.is_some_and(|(start, end)| {
        facts
            .downloads
            .iter()
            .any(|(a, b)| start.max(*a) < end.min(*b))
    });
    channel.send(&json!({"event":"result","role":"peers","download_bytes":facts.download_bytes,
        "upload_bytes":facts.upload_bytes,"listed_files":facts.listed_files,"account_logins":state.logins.load(Ordering::Acquire),
        "download_upload_overlap":overlap,"download_intervals_ms":facts.downloads,"upload_interval_ms":facts.upload,
        "peer_task_errors":failures,"integrated_transfer_sharing":false,"memory_acceptance":false}))?;
    Ok(())
}
