//! Private physical-scale hosts. The controller owns admission, process memory
//! sampling, HTTP driving and termination. No production networking bypass exists.
//!
//! The optimized unit-test executable runs `--exact scale_tests::scale_child
//! --ignored --nocapture --test-threads=1`. `DOMUS_SCALE_CONTROL` identifies an
//! exact loopback TCP listener; `DOMUS_SCALE_TOKEN` authenticates its initial
//! `{token,role:"pending",pid}` JSON line. One bounded JSON command selects the
//! role and an absolute generated root. Results and progress use that connection,
//! not stdout. A host remains alive until `{action:"stop"}` and sends `stopped`
//! only after real lifecycle draining. A lost controller also requests draining.
//!
//! Preparation with `integrated:true` adds an approved generated acquisition.
//! Start `peers`, pass its `ready.endpoints` object unchanged to `worker`, and
//! start `api`. The `integrated` driver admits acquisition alongside its scan and
//! emits `integration_ready` after scanning; only then send `{action:"start"}` to peers. This preserves
//! share-catalog invalidation rather than weakening it for the fixture. The
//! driver checks publication receipts and every published file hash. Stop worker
//! and API before stopping peers. Peer results contain actual byte counts and
//! upload/download interval overlap. They do not prove simultaneous scan I/O,
//! recovery, optimized process memory, or full-profile acceptance.
use crate::infrastructure::filesystem::{LibraryDirectory, native_root_identity};
use crate::infrastructure::lifecycle::Lifecycle;
use crate::infrastructure::production_worker::PreparedProductionWorker;
use crate::infrastructure::server::PreparedServer;
use crate::infrastructure::store::{ScanClaim, Store};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
pub(crate) mod integrated;
const GIB: u64 = 1024 * 1024 * 1024;
const RESERVED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LINE: u64 = 16 * 1024;
const SMALL_FILES: u64 = 4096;
const AUXILIARY_FILES: u64 = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    role: String,
    root: PathBuf,
    file_count: Option<u64>,
    max_run_bytes: Option<u64>,
    api_address: Option<SocketAddr>,
    #[serde(default = "small")]
    profile: String,
    #[serde(default)]
    azure_admitted: bool,
    baseline_regular_files: Option<u64>,
    manifest_sha256: Option<String>,
    #[serde(default)]
    checkpoint: bool,
    action: Option<String>,
    scan_id: Option<i64>,
    scan_owner: Option<String>,
    origin: Option<String>,
    location_id: Option<i64>,
    request_id: Option<i64>,
    #[serde(default)]
    integrated: bool,
    endpoints: Option<integrated::Endpoints>,
}

fn small() -> String {
    "small".into()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    version: u8,
    root_identity: String,
    file_count: u64,
    max_run_bytes: u64,
    profile: String,
    manifest_sha256: Option<String>,
    origin: String,
    configuration_sha256: String,
    location_id: i64,
    unavailable_location_id: i64,
    link_created: bool,
    link_error: Option<String>,
    integrated: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEvidence {
    path: String,
    bytes: u64,
    sha256: String,
    modified_ns: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Channel(Arc<Mutex<TcpStream>>);

impl Channel {
    pub(crate) fn send(&self, value: &Value) -> Result<()> {
        let mut stream = self
            .0
            .lock()
            .map_err(|_| Error::Internal("scale channel poisoned"))?;
        serde_json::to_writer(&mut *stream, value)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        Ok(())
    }
}

pub(crate) fn line(reader: &mut impl BufRead) -> Result<Value> {
    let mut bytes = Vec::new();
    reader.take(MAX_LINE + 1).read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_LINE || bytes.last() != Some(&b'\n') {
        return Err(Error::InvalidInput(
            "missing or oversized scale command".into(),
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// One checkpoint per worker process, after actual durable directory completion.
/// A lost controller releases the gate and requests normal lifecycle draining.
struct Checkpoint {
    remaining: AtomicUsize,
    released: Mutex<bool>,
    changed: Condvar,
    channel: Channel,
    lifecycle: Lifecycle,
}

static CHECKPOINT: OnceLock<Arc<Checkpoint>> = OnceLock::new();

pub(crate) fn directory_completed(claim: &ScanClaim) -> Result<()> {
    let Some(checkpoint) = CHECKPOINT.get() else {
        return Ok(());
    };
    if checkpoint
        .remaining
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
        != Ok(1)
    {
        return Ok(());
    }
    checkpoint.channel.send(&json!({
        "event":"scan_progress", "scan_id":claim.id, "location_id":claim.location,
        "scan_owner":claim.owner, "completed_directories":2
    }))?;
    let released = checkpoint
        .released
        .lock()
        .map_err(|_| Error::Internal("scale gate poisoned"))?;
    drop(
        checkpoint
            .changed
            .wait_while(released, |released| !*released)
            .map_err(|_| Error::Internal("scale gate poisoned"))?,
    );
    if checkpoint.lifecycle.stopping() {
        return Err(Error::WorkerStopping);
    }
    Ok(())
}

fn release_checkpoint() {
    if let Some(checkpoint) = CHECKPOINT.get()
        && let Ok(mut released) = checkpoint.released.lock()
    {
        *released = true;
        checkpoint.changed.notify_all();
    }
}

fn admit(command: &Command) -> Result<(u64, u64)> {
    let count = command
        .file_count
        .ok_or_else(|| Error::InvalidInput("file_count is required".into()))?;
    let limit = command
        .max_run_bytes
        .ok_or_else(|| Error::InvalidInput("max_run_bytes is required".into()))?;
    if count < 8 || limit <= RESERVED_BYTES {
        return Err(Error::InvalidInput(
            "at least eight files and more than 64 MiB are required".into(),
        ));
    }
    match command.profile.as_str() {
        "small" if count <= SMALL_FILES - AUXILIARY_FILES && limit <= GIB => {}
        "azure_full"
            if command.azure_admitted
                && command
                    .baseline_regular_files
                    .and_then(|n| n.checked_mul(2))
                    == Some(count)
                && command
                    .manifest_sha256
                    .as_ref()
                    .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())) => {
        }
        _ => {
            return Err(Error::InvalidInput(
                "scale profile exceeds its explicit admission".into(),
            ));
        }
    }
    // Charge 64 KiB per generated file and directory, plus bounded SQLite,
    // WAL, controller output and fault-file headroom. The controller must also
    // enforce actual allocation throughout execution, not just this estimate.
    let estimate = count
        .checked_add(count.div_ceil(64))
        .and_then(|n| n.checked_mul(65536))
        .and_then(|n| n.checked_add(RESERVED_BYTES));
    if estimate.is_none_or(|bytes| bytes > limit) {
        return Err(Error::InvalidInput(
            "fixture storage estimate exceeds max_run_bytes".into(),
        ));
    }
    Ok((count, limit))
}

fn validate_root(root: &Path) -> Result<()> {
    if !root.is_absolute()
        || root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::InvalidInput(
            "scale root must be a native absolute path without parent traversal".into(),
        ));
    }
    crate::infrastructure::filesystem::require_local_disk(root)
}

fn new_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}

fn tagged_media() -> TestResult<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    use id3::TagLike;
    let original = crate::media_fixtures::mp3()?;
    let start = 10
        + original[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | usize::from(*b));
    let mut tag = id3::Tag::new();
    tag.set_title("Generated scale track");
    tag.set_artist("Generated scale artist");
    tag.set_album_artist("Generated scale artist");
    tag.set_album("Generated scale album");
    tag.set_track(1);
    let mut mp3 = Vec::new();
    tag.write_to(&mut mp3, id3::Version::Id3v24)?;
    mp3.extend_from_slice(&original[start..]);
    let original = crate::media_fixtures::flac();
    let old_len = original[43..46]
        .iter()
        .fold(0usize, |n, b| (n << 8) | usize::from(*b));
    let mut comment = metaflac::block::VorbisComment::new();
    for (key, value) in [
        ("TITLE", "Generated scale track"),
        ("ARTIST", "Generated scale artist"),
        ("ALBUMARTIST", "Generated scale artist"),
        ("ALBUM", "Generated scale album"),
        ("TRACKNUMBER", "1"),
    ] {
        comment.comments.insert(key.into(), vec![value.into()]);
    }
    let comment = comment.to_bytes();
    let mut flac = original[..42].to_vec();
    flac.push(0x84);
    flac.extend_from_slice(&(comment.len() as u32).to_be_bytes()[1..]);
    flac.extend(comment);
    flac.extend_from_slice(&original[46 + old_len..]);
    let mut jpeg = Cursor::new(Vec::new());
    image::load_from_memory(&crate::media_fixtures::cover_source()?)?
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)?;
    Ok((mp3, flac, jpeg.into_inner()))
}

fn save_file(root: &Path, path: &str, bytes: &[u8], manifest: &mut File) -> TestResult {
    let mut file = new_file(&root.join(path))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let evidence = FileEvidence {
        path: path.into(),
        bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        modified_ns: crate::infrastructure::library_metadata::modified_ns(&file)?,
    };
    serde_json::to_writer(&mut *manifest, &evidence)?;
    manifest.write_all(b"\n")?;
    Ok(())
}

fn create_link(root: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(
        root.join("outside"),
        root.join("media").join("excluded-link"),
    );
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(root.join("media").join("excluded-link"))
            .arg(root.join("outside"))
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "generated junction creation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    Err(std::io::Error::other("symlink fixture unsupported"))
}

fn prepare(command: &Command) -> TestResult<Value> {
    let (count, limit) = admit(command)?;
    validate_root(&command.root)?;
    let address = match command.api_address {
        Some(address) => address,
        None => std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?,
    };
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err("api_address must be loopback with an assigned port".into());
    }
    let parent = command.root.parent().ok_or("root parent is absent")?;
    let _parent = LibraryDirectory::open(parent)?;
    fs::create_dir(&command.root)?;
    crate::infrastructure::private_fixture::directory(&command.root)?;
    for child in ["media", "media/catalog", "outside", "unavailable", "state"] {
        fs::create_dir(command.root.join(child))?;
    }
    let (mp3, flac, jpeg) = tagged_media()?;
    let large = command
        .integrated
        .then(|| integrated::audio(&mp3, command.profile == "azure_full"));
    let mut manifest = new_file(&command.root.join("fixtures.jsonl"))?;
    for index in 0..count {
        let shard = format!("media/catalog/shard-{:08}", index / 64);
        if index % 64 == 0 {
            fs::create_dir(command.root.join(&shard))?;
        }
        let (extension, bytes) = if index == 0 && command.integrated {
            (
                "mp3",
                large
                    .as_deref()
                    .ok_or("missing generated integration audio")?,
            )
        } else {
            match index % 16 {
                0 => ("mp3", mp3.as_slice()),
                1 => ("flac", flac.as_slice()),
                2 => ("mp3", b"ID3\x09\x00\x00\x00\x00\x00\x00".as_slice()),
                3 => ("ogg", b"generated unsupported audio".as_slice()),
                4 => ("jpg", jpeg.as_slice()),
                n if n % 2 == 0 => ("mp3", mp3.as_slice()),
                _ => ("flac", flac.as_slice()),
            }
        };
        save_file(
            &command.root,
            &format!("{shard}/track-{index:012}.{extension}"),
            bytes,
            &mut manifest,
        )?;
    }
    save_file(
        &command.root,
        "outside/sentinel.txt",
        b"generated outside sentinel; never traverse",
        &mut manifest,
    )?;
    save_file(&command.root, "unavailable/cached.mp3", &mp3, &mut manifest)?;
    if let Some(audio) = large.as_deref() {
        fs::create_dir(command.root.join("sharing"))?;
        save_file(
            &command.root,
            "sharing/track-000000000000.mp3",
            audio,
            &mut manifest,
        )?;
    }
    manifest.sync_all()?;
    let link_error = create_link(&command.root)
        .err()
        .map(|error| error.to_string());
    let mut store = Store::open(&command.root.join("state").join("state.sqlite"))?;
    let destination = if command.integrated {
        fs::create_dir(command.root.join("destination"))?;
        let id =
            crate::application::add_location(&store, &command.root.join("destination"), "main")?;
        crate::application::scan_location(&mut store, id)?;
        Some(id)
    } else {
        None
    };
    let location_id = crate::application::add_location(
        &store,
        &command.root.join("media"),
        if command.integrated { "backup" } else { "main" },
    )?;
    let unavailable_location_id =
        crate::application::add_location(&store, &command.root.join("unavailable"), "backup")?;
    let origin = format!("http://{address}");
    let mut config = json!({
        "environment":"single_user", "system_settings_read_only":false,
        "database":store.database_path(), "listen":address, "origin":origin,
        "production_worker":{"queue_capacity":64, "acquisition":null, "sharing":null}
    });
    if let Some(destination) = destination {
        integrated::prepare(
            &command.root,
            &mut store,
            location_id,
            destination,
            large
                .as_deref()
                .ok_or("missing generated integration audio")?,
            &mut config,
        )?;
    }
    let configuration = serde_json::to_vec_pretty(&config)?;
    let mut output = new_file(&command.root.join("host.json"))?;
    output.write_all(&configuration)?;
    output.sync_all()?;
    let fixture = Fixture {
        version: 1,
        root_identity: native_root_identity(&command.root)?,
        file_count: count,
        max_run_bytes: limit,
        profile: command.profile.clone(),
        manifest_sha256: command.manifest_sha256.clone(),
        origin,
        configuration_sha256: format!("{:x}", Sha256::digest(&configuration)),
        location_id,
        unavailable_location_id,
        link_created: link_error.is_none(),
        link_error,
        integrated: command.integrated,
    };
    let mut output = new_file(&command.root.join("scale.json"))?;
    serde_json::to_writer_pretty(&mut output, &fixture)?;
    output.sync_all()?;
    Ok(json!({
        "event":"result", "role":"prepare", "fixture":fixture,
        "configuration":command.root.join("host.json"),
        "integrated_transfer_sharing":false, "memory_acceptance":false,
        "actual_os_denial":false
    }))
}

fn load_fixture(root: &Path) -> TestResult<Fixture> {
    validate_root(root)?;
    let opened = LibraryDirectory::open(root)?;
    let mut input = opened.open_file(std::ffi::OsStr::new("scale.json"))?;
    let mut bytes = Vec::new();
    (&mut input).take(MAX_LINE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LINE {
        return Err("oversized scale manifest".into());
    }
    let fixture: Fixture = serde_json::from_slice(&bytes)?;
    if fixture.version != 1 || fixture.root_identity != native_root_identity(root)? {
        return Err("scale manifest does not identify this generated root".into());
    }
    let mut configuration = opened.open_file(std::ffi::OsStr::new("host.json"))?;
    bytes.clear();
    (&mut configuration)
        .take(MAX_LINE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LINE
        || format!("{:x}", Sha256::digest(&bytes)) != fixture.configuration_sha256
    {
        return Err("scale configuration changed".into());
    }
    Ok(fixture)
}

fn inspect(root: &Path, verify: bool) -> TestResult<Value> {
    let fixture = load_fixture(root)?;
    let store = Store::open_read_current(&root.join("state").join("state.sqlite"))?;
    let connection = rusqlite::Connection::open_with_flags(
        root.join("state").join("state.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut locations = Vec::new();
    for id in [fixture.location_id, fixture.unavailable_location_id] {
        let mut statuses = std::collections::BTreeMap::<String, i64>::new();
        let mut statement = connection.prepare("SELECT status,count(*) FROM library_metadata WHERE location_id=?1 AND active=1 GROUP BY status")?;
        for row in statement.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))? {
            let (status, count) = row?;
            statuses.insert(status, count);
        }
        let counts: (i64, i64, i64) = connection.query_row(
            "SELECT count(*),coalesce(sum(kind='file' AND present=1),0),coalesce(sum(kind='unsafe_link' AND present=1),0) FROM entries WHERE location_id=?1",
            [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let mut scans = Vec::new();
        let mut statement = connection.prepare(
            "SELECT id,status,observed_count,error_count,lease_until,owner,
             (SELECT count(*) FROM scan_directories d WHERE d.scan_id=s.id AND complete=1),
             (SELECT count(*) FROM scan_directories d WHERE d.scan_id=s.id AND complete=0)
             FROM scans s WHERE location_id=?1 ORDER BY id DESC LIMIT 16",
        )?;
        for row in statement.query_map([id], |r| {
            Ok(json!({
                "id":r.get::<_,i64>(0)?, "status":r.get::<_,String>(1)?,
                "observed_count":r.get::<_,i64>(2)?, "error_count":r.get::<_,i64>(3)?,
                "lease_until":r.get::<_,i64>(4)?, "owner":r.get::<_,String>(5)?,
                "complete_directories":r.get::<_,i64>(6)?, "pending_directories":r.get::<_,i64>(7)?
            }))
        })? {
            scans.push(row?);
        }
        locations.push(json!({
            "location":store.location(id)?, "entries":counts.0, "present_files":counts.1,
            "unsafe_links":counts.2, "metadata_statuses":statuses, "scans":scans,
            "available":crate::application::check_location_availability(&store,id).is_ok()
        }));
    }
    let checked = if verify {
        let checked = verify_files(root, fixture.file_count + 2 + u64::from(fixture.integrated))?;
        verify_index(root, &fixture, &connection)?;
        Some(checked)
    } else {
        None
    };
    let catalog_cache: (i64, i64) = connection.query_row(
        "SELECT count(*),coalesce(sum(length(body)),0) FROM catalog_cache",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let configuration: Value = serde_json::from_reader(
        LibraryDirectory::open(root)?
            .open_file(std::ffi::OsStr::new("host.json"))?
            .take(MAX_LINE + 1),
    )?;
    let sharing: crate::domain::sharing::SharePolicy = if fixture.integrated {
        serde_json::from_value(configuration["production_worker"]["sharing"]["policy"].clone())?
    } else {
        crate::domain::sharing::SharePolicy::default()
    };
    Ok(json!({
        "event":"result", "role":if verify {"verify"} else {"inspect"},
        "locations":locations, "verified_files":checked,
        "index_matches_manifest":verify,
        "expected_initial_files":fixture.file_count,
        "worker_status":store.worker_status(64)?,
        "requests":store.worker_requests(crate::domain::Page::new(0,200)?)?,
        "link_created":fixture.link_created, "link_error":fixture.link_error,
        "integrated_transfer_sharing":false, "memory_acceptance":false,
        "scan_lease_seconds":60, "worker_request_lease_seconds":300,
        "workload":{
            "queue_capacity":configuration["production_worker"]["queue_capacity"],
            "scan_lanes":1,"acquisition_lanes":1,
            "sharing_limits":sharing.limits,
            "catalog_cache":{"backend":"sqlite","configured_entries":64,
                "configured_body_bytes":16777216,"occupied_entries":catalog_cache.0,
                "occupied_body_bytes":catalog_cache.1},
            "external_file_cache":false,
            "sqlite_connection_cache_occupancy":"not measured"
        }
    }))
}

fn verify_index(root: &Path, fixture: &Fixture, connection: &rusqlite::Connection) -> TestResult {
    let opened = LibraryDirectory::open(root)?;
    let mut manifest = BufReader::new(opened.open_file(std::ffi::OsStr::new("fixtures.jsonl"))?);
    let mut query = connection.prepare(
        "SELECT count(*) FROM entries WHERE location_id=?1 AND relative_path=?2 AND kind='file' AND present=1 AND size=?3")?;
    let mut checked = 0u64;
    while !manifest.fill_buf()?.is_empty() {
        let file: FileEvidence = serde_json::from_value(line(&mut manifest)?)?;
        if let Some(path) = file.path.strip_prefix("media/") {
            let matching: i64 = query.query_row(
                rusqlite::params![fixture.location_id, path, i64::try_from(file.bytes)?],
                |row| row.get(0),
            )?;
            if matching != 1 {
                return Err("generated file is missing or changed in the index".into());
            }
            checked += 1;
        }
    }
    let present: i64 = connection.query_row(
        "SELECT count(*) FROM entries WHERE location_id=?1 AND kind='file' AND present=1",
        [fixture.location_id],
        |row| row.get(0),
    )?;
    if checked != fixture.file_count || present as u64 != checked {
        return Err("generated index has excess or missing file observations".into());
    }
    let escaped:i64 = connection.query_row(
        "SELECT count(*) FROM entries WHERE location_id=?1 AND relative_path LIKE 'excluded-link/%'",
        [fixture.location_id], |row|row.get(0))?;
    if escaped != 0 {
        return Err("scan traversed the excluded generated link".into());
    }
    Ok(())
}

fn verify_files(root: &Path, expected: u64) -> TestResult<u64> {
    let root = LibraryDirectory::open(root)?;
    let manifest = root.open_file(std::ffi::OsStr::new("fixtures.jsonl"))?;
    let mut reader = BufReader::new(manifest);
    let mut count = 0;
    while !reader.fill_buf()?.is_empty() {
        let evidence: FileEvidence = serde_json::from_value(line(&mut reader)?)?;
        let path = crate::domain::LibraryPath::parse(&evidence.path)?;
        let directory = root.open_directory(&crate::domain::LibraryPath::parse(path.parent())?)?;
        let name = path
            .as_str()
            .rsplit('/')
            .next()
            .ok_or("missing fixture name")?;
        let mut file = directory.open_file(std::ffi::OsStr::new(name))?;
        if file.metadata()?.len() != evidence.bytes
            || crate::infrastructure::library_metadata::modified_ns(&file)? != evidence.modified_ns
        {
            return Err(format!("generated fixture observation changed: {}", evidence.path).into());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
        if format!("{:x}", hash.finalize()) != evidence.sha256 {
            return Err(format!("generated fixture contents changed: {}", evidence.path).into());
        }
        count += 1;
        if count > expected {
            return Err("fixture manifest has excess records".into());
        }
    }
    if count != expected {
        return Err("fixture manifest has missing records".into());
    }
    Ok(count)
}

fn run_host(command: &Command, reader: &mut impl BufRead, channel: Channel) -> TestResult {
    let fixture = load_fixture(&command.root)?;
    if command.role == "worker" && fixture.integrated {
        integrated::install(
            command
                .endpoints
                .clone()
                .ok_or("integrated worker requires exact loopback fixture endpoints")?,
        )?;
    }
    let role = command.role.clone();
    let ready_channel = channel.clone();
    let origin = fixture.origin.clone();
    let lifecycle = Lifecycle::controlled(move || {
        ready_channel.send(&json!({
            "event":"ready", "role":role, "pid":std::process::id(), "origin":origin,
            "test_build":true, "debug_assertions":cfg!(debug_assertions),
            "integrated_transfer_sharing":false
        }))
    });
    if command.role == "worker" && command.checkpoint {
        CHECKPOINT
            .set(Arc::new(Checkpoint {
                remaining: AtomicUsize::new(2),
                released: Mutex::new(false),
                changed: Condvar::new(),
                channel: channel.clone(),
                lifecycle: lifecycle.clone(),
            }))
            .map_err(|_| "scale checkpoint already configured")?;
    }
    let run_lifecycle = lifecycle.clone();
    let config = command.root.join("host.json");
    // Match the normal foreground runtime choices; do not lower concurrency for
    // the measurement. Blocking native work remains in the production adapters.
    let (finished, result) = std::sync::mpsc::sync_channel(1);
    let api = command.role == "api";
    let host_channel = channel.clone();
    let host = std::thread::spawn(move || {
        let result = if api {
            PreparedServer::load(&config).and_then(|server| {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .map_err(Error::from)?
                    .block_on(server.run_with_lifecycle(run_lifecycle))
            })
        } else {
            PreparedProductionWorker::load(&config).and_then(|worker| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(Error::from)?
                    .block_on(worker.run_with_lifecycle(run_lifecycle))
            })
        };
        if let Err(error) = &result {
            let _ = host_channel.send(&json!({"event":"error","message":error.to_string()}));
        }
        if let Ok(stream) = host_channel.0.lock() {
            let _ = stream.shutdown(std::net::Shutdown::Read);
        }
        let _ = finished.send(result);
    });
    let control = (|| -> Result<()> {
        loop {
            let command = line(reader)?;
            match command.get("action").and_then(Value::as_str) {
                Some("release") => release_checkpoint(),
                Some("stop") => return Ok(()),
                _ => return Err(Error::InvalidInput("expected release or stop".into())),
            }
        }
    })();
    lifecycle.request_stop();
    release_checkpoint();
    let outcome = result
        .recv()
        .map_err(|_| "scale host exited without a result")?;
    host.join().map_err(|_| "scale host panicked")?;
    outcome?;
    control?;
    channel.send(&json!({"event":"stopped", "role":command.role, "pid":std::process::id()}))?;
    Ok(())
}

fn stale_claim(command: &Command) -> TestResult<Value> {
    let fixture = load_fixture(&command.root)?;
    let id = command.scan_id.ok_or("scan_id is required")?;
    let owner = command
        .scan_owner
        .as_ref()
        .ok_or("scan_owner is required")?;
    let mut store = Store::open_current_existing(&command.root.join("state").join("state.sqlite"))?;
    let connection = rusqlite::Connection::open_with_flags(
        command.root.join("state").join("state.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let stale: bool = connection.query_row(
        "SELECT owner!=?2 OR lease_until<=unixepoch() OR status!='running' FROM scans WHERE id=?1 AND location_id=?3",
        rusqlite::params![id,owner,fixture.location_id], |r|r.get(0))?;
    if !stale {
        return Err("the supplied scan claim is still live".into());
    }
    let claim = ScanClaim {
        id,
        location: fixture.location_id,
        owner: owner.clone(),
    };
    let batch_rejected = matches!(store.record_batch(&claim, &[]), Err(Error::Conflict(_)));
    let finish_rejected = matches!(store.finish_scan(&claim), Err(Error::Conflict(_)));
    if !batch_rejected || !finish_rejected {
        return Err("stale scan mutation was not rejected".into());
    }
    Ok(json!({"event":"result","role":"stale","batch_rejected":true,"finish_rejected":true}))
}

async fn http_json(request: reqwest::RequestBuilder, expected: u16) -> TestResult<Value> {
    let mut response = request.send().await?;
    if response.status().as_u16() != expected {
        return Err(format!(
            "scale API returned {}, expected {expected}",
            response.status()
        )
        .into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err("scale API response exceeds bounded driver buffer".into());
        }
        body.extend_from_slice(&chunk);
    }
    if body.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_slice(&body)?)
}

async fn browse_load(client: &reqwest::Client, origin: &str, location: i64) -> TestResult<u64> {
    let mut requests = 0;
    for path in [
        format!("/api/locations/{location}"),
        format!("/api/locations/{location}/entries?limit=16"),
        format!("/api/locations/{location}/library/artists?limit=16"),
        "/api/queue?limit=16".into(),
    ] {
        http_json(client.get(format!("{origin}{path}")), 200).await?;
        requests += 1;
    }
    let albums = http_json(
        client.get(format!(
            "{origin}/api/locations/{location}/library/albums?limit=16"
        )),
        200,
    )
    .await?;
    requests += 1;
    if let Some(album) = albums
        .as_array()
        .and_then(|a| a.first())
        .and_then(|a| a["id"].as_i64())
    {
        let album_path = format!("{origin}/api/locations/{location}/library/albums/{album}");
        http_json(client.get(&album_path), 200).await?;
        let files = http_json(client.get(format!("{album_path}/files?limit=16")), 200).await?;
        requests += 2;
        if let Some(file) = files
            .as_array()
            .and_then(|a| a.first())
            .and_then(|f| f["id"].as_i64())
        {
            http_json(
                client.get(format!(
                    "{origin}/api/locations/{location}/library/files/{file}"
                )),
                200,
            )
            .await?;
            requests += 1;
        }
    }
    Ok(requests)
}

async fn admit_scan(client: &reqwest::Client, fixture: &Fixture, location: i64) -> TestResult<i64> {
    http_json(
        client
            .post(format!("{}/api/locations/{location}/scan", fixture.origin))
            .header("origin", &fixture.origin)
            .header("x-domus-csrf", "1")
            .json(&json!({})),
        202,
    )
    .await?;
    let requests = http_json(
        client.get(format!("{}/api/worker/requests?limit=200", fixture.origin)),
        200,
    )
    .await?;
    requests
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            r["operation"]["operation"] == "scan" && r["operation"]["location_id"] == location
        })
        .filter_map(|r| r["id"].as_i64())
        .max()
        .ok_or_else(|| "scan admission did not create a durable request".into())
}

async fn wait_scan(
    client: &reqwest::Client,
    fixture: &Fixture,
    location: i64,
    request: i64,
) -> TestResult<(Value, u64)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3600);
    let mut reads = 0;
    loop {
        let state = http_json(
            client.get(format!("{}/api/worker/requests/{request}", fixture.origin)),
            200,
        )
        .await?;
        reads += browse_load(client, &fixture.origin, location).await?;
        match state["state"].as_str() {
            Some("complete" | "failed" | "waiting") => return Ok((state, reads)),
            Some("queued" | "running" | "interrupted") => {}
            _ => return Err("scale worker returned an unknown state".into()),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("scale scan deadline exceeded".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn exercise(command: &Command, channel: &Channel) -> TestResult<Value> {
    let fixture = load_fixture(&command.root)?;
    if command
        .origin
        .as_ref()
        .is_some_and(|o| o != &fixture.origin)
    {
        return Err("exercise origin differs from the frozen fixture".into());
    }
    let location = command.location_id.unwrap_or(fixture.location_id);
    if ![fixture.location_id, fixture.unavailable_location_id].contains(&location) {
        return Err("exercise location is not generated by this fixture".into());
    }
    let action = command.action.as_deref().unwrap_or("scan");
    let report = tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30)).build()?;
        // This is the real command guard, not an in-process router fixture.
        let forbidden = client.post(format!("{}/api/locations/{location}/scan",fixture.origin))
            .header("origin",&fixture.origin).json(&json!({})).send().await?;
        if forbidden.status().as_u16() != 403 { return Err("scan command accepted missing CSRF header".into()); }
        let mut phases = Vec::new();
        let iterations = if action == "scan" { 2 } else { 1 };
        for iteration in 0..iterations {
            let id = match action {
                "scan" | "admit" => admit_scan(&client,&fixture,location).await?,
                "resume" | "retry" => {
                    let id = command.request_id.ok_or("request_id is required for resume or retry")?;
                    if action == "retry" {
                        http_json(client.post(format!("{}/api/worker/requests/{id}/retry",fixture.origin))
                            .header("origin",&fixture.origin).header("x-domus-csrf","1"),202).await?;
                    }
                    id
                }
                _ => return Err("exercise action must be scan, admit, resume or retry".into()),
            };
            channel.send(&json!({"event":"scan_admitted","location_id":location,"request_id":id,"iteration":iteration}))?;
            if action == "admit" {
                return Ok(json!({"event":"result","role":"exercise","request_id":id,"admitted":true,
                    "completed":false,"integrated_transfer_sharing":false,"memory_acceptance":false}));
            }
            let (request,reads) = wait_scan(&client,&fixture,location,id).await?;
            let success = request["state"] == "complete";
            phases.push(json!({"request":request,"browse_requests":reads,"iteration":iteration}));
            if !success && action == "scan" { return Err("generated scale scan did not complete".into()); }
        }
        Ok::<Value,Box<dyn std::error::Error>>(json!({
            "event":"result","role":"exercise","phases":phases,"command_guard_verified":true,
            "initial_and_repeat_scan":action=="scan", "completed":true,
            "integrated_transfer_sharing":false,"forced_recovery_verified":false,
            "graceful_recovery_verified":false,"memory_acceptance":false
        }))
    })?;
    let mut report = report;
    report["inspection"] = inspect(&command.root, false)?;
    if action == "scan" {
        let index = usize::from(location == fixture.unavailable_location_id);
        let expected = if index == 0 { fixture.file_count } else { 1 };
        if report["inspection"]["locations"][index]["present_files"] != expected
            || report["inspection"]["locations"][index]["location"]["coverage"] != "complete"
        {
            return Err("generated scan counts or coverage differ from the frozen fixture".into());
        }
    }
    Ok(report)
}

fn fault(command: &Command) -> TestResult<Value> {
    let fixture = load_fixture(&command.root)?;
    let _worker_guard = PreparedProductionWorker::load(&command.root.join("host.json"))?;
    let store = Store::open_read_current(&command.root.join("state").join("state.sqlite"))?;
    if store.worker_status(64)?.online {
        return Err("stop the worker before changing the generated unavailable root".into());
    }
    let (from, to) = match command.action.as_deref() {
        Some("unavailable") => ("unavailable", "unavailable.offline"),
        Some("restore") => ("unavailable.offline", "unavailable"),
        _ => return Err("expected unavailable or restore fault action".into()),
    };
    if command.root.join(to).try_exists()? {
        return Err("fault destination already exists".into());
    }
    fs::rename(command.root.join(from), command.root.join(to))?;
    Ok(
        json!({"event":"result", "role":"fault", "action":command.action,
        "location_id":fixture.unavailable_location_id}),
    )
}

#[cfg(windows)]
fn lock_file(command: &Command, reader: &mut impl BufRead, channel: Channel) -> TestResult {
    use std::os::windows::fs::OpenOptionsExt;
    load_fixture(&command.root)?;
    let _lock = OpenOptions::new().read(true).share_mode(0).open(
        command
            .root
            .join("media")
            .join("catalog")
            .join("shard-00000000")
            .join("track-000000000000.mp3"),
    )?;
    channel.send(&json!({"event":"ready", "role":"lock", "actual_os_denial":true}))?;
    if line(reader)?.get("action").and_then(Value::as_str) != Some("stop") {
        return Err("lock fixture requires stop".into());
    }
    drop(_lock);
    channel.send(&json!({"event":"stopped", "role":"lock"}))?;
    Ok(())
}

/// Executed only by the explicit optimized scale controller, never normal tests.
#[test]
#[ignore = "private process entry point; requires authenticated loopback controller"]
fn scale_child() -> TestResult {
    let address: SocketAddr = std::env::var("DOMUS_SCALE_CONTROL")?.parse()?;
    if !address.ip().is_loopback() {
        return Err("scale controller must be loopback".into());
    }
    let token = std::env::var("DOMUS_SCALE_TOKEN")?;
    if !(32..=256).contains(&token.len()) || token.chars().any(char::is_control) {
        return Err(
            "scale controller token must contain 32 through 256 non-control characters".into(),
        );
    }
    let stream = TcpStream::connect_timeout(&address, Duration::from_secs(30))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let channel = Channel(Arc::new(Mutex::new(stream)));
    channel.send(&json!({"token":token, "role":"pending", "pid":std::process::id()}))?;
    let result = (|| -> TestResult {
        if cfg!(debug_assertions) {
            return Err("scale_child requires an optimized release test build".into());
        }
        crate::infrastructure::tls::initialize_crypto()?;
        let command: Command = serde_json::from_value(line(&mut reader)?)?;
        match command.role.as_str() {
            "prepare" => channel.send(&prepare(&command)?)?,
            "api" | "worker" => run_host(&command, &mut reader, channel.clone())?,
            "inspect" | "verify" => {
                channel.send(&inspect(&command.root, command.role == "verify")?)?
            }
            "fault" => channel.send(&fault(&command)?)?,
            "stale" => channel.send(&stale_claim(&command)?)?,
            "exercise" => channel.send(&exercise(&command, &channel)?)?,
            "peers" => integrated::peers(&command.root, &mut reader, channel.clone())?,
            "integrated" => channel.send(&integrated::exercise(&command.root, &channel)?)?,
            #[cfg(windows)]
            "lock" => lock_file(&command, &mut reader, channel.clone())?,
            _ => return Err("unknown or unsupported scale role".into()),
        }
        Ok(())
    })();
    if let Err(error) = &result {
        let _ = channel.send(&json!({"event":"error", "message":error.to_string()}));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> TestResult<tempfile::TempDir> {
        let directory = tempfile::Builder::new()
            .prefix(".scale-test-")
            .tempdir_in(std::env::current_dir()?)?;
        crate::infrastructure::private_fixture::directory(directory.path())?;
        Ok(directory)
    }

    fn command(root: &Path) -> TestResult<Command> {
        Ok(serde_json::from_value(json!({
            "role":"prepare", "root":root, "file_count":128,
            "max_run_bytes":128*1024*1024u64, "api_address":"127.0.0.1:18080"
        }))?)
    }

    #[test]
    fn scale_admission_rejects_storage_overflow_and_unapproved_full_profiles() -> TestResult {
        let mut c = command(Path::new("unused"))?;
        assert!(admit(&c).is_ok());
        c.file_count = Some(SMALL_FILES);
        assert!(admit(&c).is_err());
        c.file_count = Some(128);
        c.max_run_bytes = Some(GIB + 1);
        assert!(admit(&c).is_err());
        c.max_run_bytes = Some(RESERVED_BYTES + 1);
        assert!(admit(&c).is_err());
        c.max_run_bytes = Some(GIB);
        c.profile = "azure_full".into();
        c.baseline_regular_files = Some(64);
        c.manifest_sha256 = Some("a".repeat(64));
        assert!(admit(&c).is_err());
        c.azure_admitted = true;
        assert!(admit(&c).is_ok());
        c.file_count = Some(129);
        assert!(admit(&c).is_err());
        c.file_count = Some(u64::MAX);
        c.baseline_regular_files = Some(u64::MAX);
        assert!(admit(&c).is_err());
        Ok(())
    }

    #[test]
    fn scale_fixture_scans_tagged_codecs_faults_and_preserves_every_byte() -> TestResult {
        let directory = directory()?;
        let root = directory.path().join("run");
        let c = command(&root)?;
        prepare(&c)?;
        assert!(prepare(&c).is_err());
        let fixture = load_fixture(&root)?;
        let mut store = Store::open_current_existing(&root.join("state").join("state.sqlite"))?;
        crate::application::scan_location(&mut store, fixture.location_id)?;
        crate::application::scan_location(&mut store, fixture.unavailable_location_id)?;
        let report = inspect(&root, true)?;
        let main = &report["locations"][0];
        assert_eq!(main["present_files"], 128);
        assert!(main["metadata_statuses"]["tagged"].as_u64().unwrap_or(0) > 0);
        assert_eq!(main["metadata_statuses"]["invalid"], 8);
        assert_eq!(main["metadata_statuses"]["unsupported"], 8);
        assert!(main["metadata_statuses"].get("unreadable").is_none());
        assert_eq!(report["verified_files"], 130);
        if fixture.link_created {
            assert_eq!(main["unsafe_links"], 1);
        }
        let first = main["scans"][0]["id"].as_i64().ok_or("missing scan")?;
        let repeat = crate::application::scan_location(&mut store, fixture.location_id)?;
        assert_ne!(first, repeat);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let lock = OpenOptions::new().read(true).share_mode(0).open(
                root.join("media")
                    .join("catalog")
                    .join("shard-00000000")
                    .join("track-000000000000.mp3"),
            )?;
            crate::application::scan_location(&mut store, fixture.location_id)?;
            let denied = inspect(&root, false)?;
            assert_eq!(denied["locations"][0]["metadata_statuses"]["unreadable"], 1);
            assert_eq!(denied["locations"][0]["present_files"], 128);
            drop(lock);
            crate::application::scan_location(&mut store, fixture.location_id)?;
            assert!(
                inspect(&root, false)?["locations"][0]["metadata_statuses"]
                    .get("unreadable")
                    .is_none()
            );
        }
        let mut c = c;
        c.action = Some("unavailable".into());
        fault(&c)?;
        assert!(
            crate::application::scan_location(&mut store, fixture.unavailable_location_id).is_err()
        );
        let report = inspect(&root, false)?;
        assert_eq!(report["locations"][1]["location"]["coverage"], "incomplete");
        assert_eq!(report["locations"][1]["present_files"], 1);
        c.action = Some("restore".into());
        fault(&c)?;
        crate::application::scan_location(&mut store, fixture.unavailable_location_id)?;
        assert_eq!(verify_files(&root, 130)?, 130);
        fs::write(root.join("outside").join("sentinel.txt"), b"changed")?;
        assert!(verify_files(&root, 130).is_err());
        Ok(())
    }

    type HostThread = std::thread::JoinHandle<std::result::Result<(), String>>;

    fn host(command: Command) -> TestResult<(BufReader<TcpStream>, HostThread)> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let child = TcpStream::connect(listener.local_addr()?)?;
        let (controller, _) = listener.accept()?;
        controller.set_read_timeout(Some(Duration::from_secs(30)))?;
        let mut reader = BufReader::new(child.try_clone()?);
        let channel = Channel(Arc::new(Mutex::new(child)));
        let worker = std::thread::spawn(move || {
            run_host(&command, &mut reader, channel).map_err(|e| e.to_string())
        });
        Ok((BufReader::new(controller), worker))
    }

    fn stop_host(mut reader: BufReader<TcpStream>, host: HostThread) -> TestResult {
        reader.get_mut().write_all(b"{\"action\":\"stop\"}\n")?;
        let event = line(&mut reader)?;
        let result = host.join().map_err(|_| "scale test host panicked")?;
        result.map_err(std::io::Error::other)?;
        assert_eq!(event["event"], "stopped");
        Ok(())
    }

    #[test]
    fn scale_driver_uses_guarded_listener_and_real_worker_until_explicit_stop() -> TestResult {
        let directory = directory()?;
        let root = directory.path().join("run");
        let mut c = command(&root)?;
        c.api_address = None;
        prepare(&c)?;
        let mut api = command(&root)?;
        api.role = "api".into();
        let (mut api_control, api_thread) = host(api)?;
        assert_eq!(line(&mut api_control)?["event"], "ready");
        let mut worker = command(&root)?;
        worker.role = "worker".into();
        let (mut worker_control, worker_thread) = host(worker)?;
        assert_eq!(line(&mut worker_control)?["event"], "ready");
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let output = TcpStream::connect(listener.local_addr()?)?;
        let (_controller, _) = listener.accept()?;
        let channel = Channel(Arc::new(Mutex::new(output)));
        let report = exercise(&c, &channel);
        stop_host(worker_control, worker_thread)?;
        stop_host(api_control, api_thread)?;
        let report = report?;
        assert_eq!(report["command_guard_verified"], true);
        assert_eq!(report["initial_and_repeat_scan"], true);
        assert_eq!(report["inspection"]["locations"][0]["present_files"], 128);
        assert_eq!(report["integrated_transfer_sharing"], false);
        assert_eq!(verify_files(&root, 130)?, 130);
        Ok(())
    }

    #[test]
    fn scale_integrated_generated_workload_process() -> TestResult {
        let directory = directory()?;
        let stdout = new_file(&directory.path().join("child.log"))?;
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "scale_tests::tests::scale_integrated_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .stdout(stdout.try_clone()?)
            .stderr(stdout)
            .spawn()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(300);
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(format!(
                        "integrated functional child failed:\n{}",
                        fs::read_to_string(directory.path().join("child.log"))?
                    )
                    .into());
                }
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                return Err(format!(
                    "integrated functional child exceeded deadline:\n{}",
                    fs::read_to_string(directory.path().join("child.log"))?
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    #[ignore = "isolated functional integration; called by scale_integrated_generated_workload_process"]
    fn scale_integrated_child() -> TestResult {
        let directory = directory()?;
        let root = directory.path().join("run");
        let mut c = command(&root)?;
        c.api_address = None;
        c.integrated = true;
        prepare(&c)?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let socket = TcpStream::connect(listener.local_addr()?)?;
        let (control, _) = listener.accept()?;
        control.set_read_timeout(Some(Duration::from_secs(120)))?;
        let mut peers_control = BufReader::new(control);
        let mut reader = BufReader::new(socket.try_clone()?);
        let channel = Channel(Arc::new(Mutex::new(socket)));
        let peers_root = root.clone();
        let peers_thread = std::thread::spawn(move || {
            integrated::peers(&peers_root, &mut reader, channel).map_err(|e| e.to_string())
        });
        let ready = line(&mut peers_control)?;
        assert_eq!(ready["event"], "ready");
        let mut api = command(&root)?;
        api.role = "api".into();
        let (mut api_control, api_thread) = host(api)?;
        assert_eq!(line(&mut api_control)?["event"], "ready");
        let mut worker = command(&root)?;
        worker.role = "worker".into();
        worker.endpoints = Some(serde_json::from_value(ready["endpoints"].clone())?);
        let (mut worker_control, worker_thread) = host(worker)?;
        let worker_ready = line(&mut worker_control)?;
        assert_eq!(worker_ready["event"], "ready", "{worker_ready}");
        println!("integrated hosts ready");
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let socket = TcpStream::connect(listener.local_addr()?)?;
        let (control, _) = listener.accept()?;
        control.set_read_timeout(Some(Duration::from_secs(60)))?;
        let mut driver_control = BufReader::new(control);
        let channel = Channel(Arc::new(Mutex::new(socket)));
        let driver_root = root.clone();
        let driver = std::thread::spawn(move || {
            integrated::exercise(&driver_root, &channel).map_err(|e| e.to_string())
        });
        for expected in ["scan_admitted", "acquisition_admitted", "integration_ready"] {
            assert_eq!(line(&mut driver_control)?["event"], expected);
        }
        peers_control
            .get_mut()
            .write_all(b"{\"action\":\"start\"}\n")?;
        loop {
            let event = line(&mut peers_control)?;
            println!("peer: {event}");
            if event["event"] == "started" {
                break;
            }
            if event["event"] == "peer_error" {
                return Err(format!("peer failed: {event}").into());
            }
        }
        let report = driver.join().map_err(|_| "integrated driver panicked")?;
        println!("integrated driver: {report:?}");
        peers_control
            .get_mut()
            .write_all(b"{\"action\":\"list\"}\n")?;
        loop {
            let event = line(&mut peers_control)?;
            if event["event"] == "listing_complete" {
                break;
            }
            if event["event"] == "peer_error" {
                return Err(format!("peer failed: {event}").into());
            }
        }
        let worker_stopped = stop_host(worker_control, worker_thread);
        let api_stopped = stop_host(api_control, api_thread);
        peers_control
            .get_mut()
            .write_all(b"{\"action\":\"stop\"}\n")?;
        let facts = loop {
            let event = line(&mut peers_control)?;
            println!("peer: {event}");
            if event["event"] == "result" {
                break event;
            }
        };
        peers_thread
            .join()
            .map_err(|_| "integration peers panicked")?
            .map_err(std::io::Error::other)?;
        println!("stopped: worker={worker_stopped:?} api={api_stopped:?}");
        worker_stopped?;
        api_stopped?;
        let report = report.map_err(std::io::Error::other)?;
        assert_eq!(report["acquisition_publication_complete"], true);
        assert_eq!(facts["account_logins"], 1);
        let album = integrated::album(&root)?;
        assert_eq!(facts["download_bytes"], 3 * album.file_bytes);
        assert_eq!(facts["upload_bytes"], album.file_bytes);
        assert_eq!(facts["listed_files"], 121);
        assert_eq!(facts["download_upload_overlap"], true);
        assert_eq!(facts["peer_task_errors"], json!([]));
        assert_eq!(inspect(&root, true)?["verified_files"], 131);
        Ok(())
    }

    #[test]
    fn scale_protocol_rejects_unterminated_and_oversized_messages() -> Result<()> {
        assert!(line(&mut Cursor::new(b"{}")).is_err());
        assert!(line(&mut Cursor::new(vec![b'x'; MAX_LINE as usize + 1])).is_err());
        assert_eq!(
            line(&mut Cursor::new(b"{\"action\":\"stop\"}\n"))?["action"],
            "stop"
        );
        Ok(())
    }
}
