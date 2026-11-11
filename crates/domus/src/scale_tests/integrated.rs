//! Generated same-account acquisition and sharing. All remapping exists only in
//! the private unit-test host, after normal public-address validation.
use super::*;
use std::net::SocketAddrV4;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Endpoints {
    pub server: SocketAddrV4,
    pub acquisition: SocketAddrV4,
    pub sharing: SocketAddrV4,
}

static ENDPOINTS: OnceLock<Endpoints> = OnceLock::new();

pub(crate) fn endpoint(address: SocketAddrV4) -> std::io::Result<SocketAddrV4> {
    let Some(endpoints) = ENDPOINTS.get() else {
        return Ok(address);
    };
    match (address.ip().octets(), address.port()) {
        ([8, 8, 8, 8], 2242) => Ok(endpoints.server),
        ([8, 8, 8, 8], 2234) => Ok(endpoints.acquisition),
        ([8, 8, 4, 4], 2234) => Ok(endpoints.sharing),
        _ => Err(std::io::Error::other(
            "integrated fixture rejected an unmapped endpoint",
        )),
    }
}

pub(super) fn install(endpoints: Endpoints) -> TestResult {
    for endpoint in [endpoints.server, endpoints.acquisition, endpoints.sharing] {
        if !endpoint.ip().is_loopback() || endpoint.port() == 0 {
            return Err("integration endpoint must be exact loopback with a nonzero port".into());
        }
    }
    ENDPOINTS
        .set(endpoints)
        .map_err(|_| "integration endpoints already installed")?;
    Ok(())
}

pub(super) fn audio(mp3: &[u8], full: bool) -> Vec<u8> {
    let start = 10
        + mp3[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | usize::from(*b));
    let mut bytes = mp3[..start].to_vec();
    for _ in 0..if full { 1024 } else { 128 } {
        bytes.extend_from_slice(&mp3[start..start + 1044]);
    }
    bytes
}

#[allow(dead_code)]
mod metadata {
    use super::*;
    use crate::infrastructure::artwork::{CoverJpeg, normalize_cover};
    use crate::media_fixtures::cover_source;
    use std::result::Result;
    include!("../../tests/support/album_metadata.rs");
    pub(super) fn body() -> TestResult<Vec<u8>> {
        release_body(Some("2024-02"))
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Album {
    pub job: i64,
    pub version: i64,
    pub candidate: i64,
    pub destination: i64,
    pub file_bytes: u64,
    pub file_sha256: String,
}

pub(super) fn prepare(
    root: &Path,
    store: &mut Store,
    scan: i64,
    destination: i64,
    audio: &[u8],
    config: &mut Value,
) -> TestResult {
    use crate::application::sharing::{
        BindShareFolder, InspectShareFolder, bind_share_folder, inspect_share_folder,
    };
    use crate::domain::sources::{SourceFileMetadata, SourceFolder};
    use crate::domain::{AudioFormat, OutputSettings};
    use crate::infrastructure::catalog::{CatalogQuery, CatalogResult};
    use crate::infrastructure::store::PlanRequest;
    fs::create_dir(root.join("staging"))?;
    crate::infrastructure::private_fixture::directory(&root.join("staging"))?;
    drop(crate::infrastructure::staging::StagingRoot::open(
        store,
        &root.join("staging"),
    )?);
    let mut credentials = new_file(&root.join("generated-credentials.json"))?;
    credentials.write_all(br#"{"username":"scale-user","password":"generated-only"}"#)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        credentials.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    credentials.sync_all()?;
    let sharing = crate::application::add_location(store, &root.join("sharing"), "backup")?;
    let mut roots = Vec::new();
    for (location_id, alias) in [(sharing, "Generated"), (scan, "Inventory")] {
        let relative_path = if location_id == scan { "catalog" } else { "" };
        let binding = inspect_share_folder(
            store,
            &InspectShareFolder {
                location_id,
                relative_path: relative_path.into(),
            },
        )?;
        let binding = bind_share_folder(
            store,
            &BindShareFolder {
                location_id,
                relative_path: relative_path.into(),
                expected_identity: binding.native_identity,
            },
        )?;
        roots.push(json!({"binding_id":binding.binding_id.ok_or("missing generated share binding")?,"alias":alias}));
    }
    config["production_worker"]["acquisition"] = json!({
        "staging_root":root.join("staging"),"credentials_file":root.join("generated-credentials.json"),
        "server":"8.8.8.8:2242","acknowledge_plaintext_login":true
    });
    config["production_worker"]["sharing"] = json!({
        "policy":{
            "access":"allowlist","allowlist":["scale-share"],
            "roots":roots
        }
    });
    store.save_settings(None, &OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let mut release: Value = serde_json::from_slice(&metadata::body()?)?;
    let mut tracks = Vec::new();
    for medium in release["media"]
        .as_array()
        .ok_or("missing generated media")?
    {
        for track in medium["tracks"]
            .as_array()
            .ok_or("missing generated tracks")?
        {
            let mut track = track.clone();
            let position = tracks.len() + 1;
            track["position"] = json!(position);
            track["number"] = json!(position.to_string());
            track["title"] = json!(format!("Scale track {position}"));
            tracks.push(track);
        }
    }
    release["media"] = json!([{"position":1,"track-count":3,"tracks":tracks}]);
    let body = serde_json::to_vec(&release)?;
    let query = CatalogQuery::release("20000000-0000-4000-8000-000000000001")?;
    let raw = rusqlite::Connection::open(store.database_path())?;
    let now: i64 = raw.query_row("SELECT unixepoch()", [], |r| r.get(0))?;
    let digest = Sha256::digest(&body).to_vec();
    raw.execute("INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms) VALUES (?1,?2,?3,?4,?4+86400,?4*1000)",
        rusqlite::params![query.source(),body,digest,now])?;
    let catalog = CatalogResult {
        source: query.source().into(),
        fetched_at: now,
        cached: true,
        payload_sha256: digest,
        data: query.parse_release(&body)?,
    };
    let plan = store.prepare_catalog_plan(
        PlanRequest {
            audio_profile: None,
            job_id: job,
            expected_version: 1,
            settings_version: 1,
            format: Some(AudioFormat::Mp3),
            edition: catalog.data.selection(None)?,
            locations: vec![destination],
            accepts_incomplete_coverage: false,
        },
        catalog,
    )?;
    let artwork = store.prepare_artwork(
        job,
        plan.version,
        "123456789",
        &crate::media_fixtures::cover_source()?,
        None,
    )?;
    store.approve_artwork(job, plan.version, artwork.id, &artwork.sha256)?;
    store.approve_plan(job, plan.version)?;
    let candidate = store.prepare_source_candidate(
        job,
        plan.version,
        SourceFolder {
            peer: "scale-download".into(),
            folder: "Peer\\Album".into(),
            directories: 1,
            files: (1..=3)
                .map(|track| SourceFileMetadata {
                    remote_path: format!("Peer\\Album\\{track:02} Scale track {track}.mp3"),
                    expected_bytes: audio.len() as i64,
                    extension: "mp3".into(),
                    bitrate_kbps: Some(320),
                    duration_seconds: None,
                    vbr: Some(false),
                    sample_rate: Some(44100),
                    bit_depth: None,
                })
                .collect(),
        },
    )?;
    store.approve_source_candidate(candidate.id, plan.version, &candidate.review_sha256, true)?;
    let album = Album {
        job,
        version: plan.version,
        candidate: candidate.id,
        destination,
        file_bytes: audio.len() as u64,
        file_sha256: format!("{:x}", Sha256::digest(audio)),
    };
    serde_json::to_writer(new_file(&root.join("integration.json"))?, &album)?;
    Ok(())
}

pub(crate) fn album(root: &Path) -> TestResult<Album> {
    let opened = LibraryDirectory::open(root)?;
    let input = opened.open_file(std::ffi::OsStr::new("integration.json"))?;
    Ok(serde_json::from_reader(input.take(MAX_LINE))?)
}

pub(super) fn peers(root: &Path, reader: &mut impl BufRead, channel: Channel) -> TestResult {
    load_fixture(root)?;
    crate::infrastructure::soulseek::scale_fixture::run(root, reader, channel)
}

fn require_completed_scan(scan: &Value) -> TestResult {
    match scan["state"].as_str() {
        Some("complete") => Ok(()),
        Some(state) => Err(format!("integrated physical scan ended in state {state}").into()),
        None => Err("integrated physical scan response omitted its state".into()),
    }
}

fn publication_evidence(root: &Path, album: &Album) -> Result<Value> {
    let store = Store::open_read_current(&root.join("state").join("state.sqlite"))?;
    let targets = store.publication_destinations(album.job, album.version)?;
    if targets.len() != 1
        || targets[0].location_id != album.destination
        || targets[0].verified_at.is_none()
        || targets[0].last_error.is_some()
    {
        return Err(Error::Conflict(
            "generated publication receipt is incomplete".into(),
        ));
    }
    let target = &targets[0];
    let root = LibraryDirectory::open(Path::new(&target.root))?;
    store.check_native_root(&target.root, "media", root.native_root())?;
    let directory =
        root.open_directory(&crate::domain::LibraryPath::parse(&target.relative_folder)?)?;
    let connection = rusqlite::Connection::open_with_flags(
        store.database_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let receipts: i64 = connection.query_row(
        "SELECT count(*) FROM normalization_receipts WHERE run_id=?1",
        [target.run_id],
        |row| row.get(0),
    )?;
    let cover = store.approved_artwork(album.job, album.version)?;
    let cover_verified:bool=connection.query_row(
        "SELECT completed_at IS NOT NULL AND cover_verified_at IS NOT NULL AND cover_sha256=?2 FROM normalization_runs WHERE id=?1",
        rusqlite::params![target.run_id,format!("{:x}",Sha256::digest(cover.bytes()))],|row|row.get(0))?;
    let mut statement=connection.prepare("SELECT name,bytes,sha256,verified_at IS NOT NULL FROM publication_files WHERE destination_id=?1 ORDER BY ordinal LIMIT 5")?;
    let files = statement
        .query_map([target.id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if receipts != 3 || !cover_verified || files.len() != 4 {
        return Err(Error::Conflict(
            "generated normalization or JPEG receipt is incomplete".into(),
        ));
    }
    for (name, bytes, expected, verified) in &files {
        let mut file = directory.open_file(std::ffi::OsStr::new(name))?;
        if !verified || file.metadata()?.len() != *bytes as u64 {
            return Err(Error::Conflict(
                "generated published file size or receipt differs".into(),
            ));
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        if format!("{:x}", hash.finalize()) != *expected {
            return Err(Error::Conflict(
                "generated published file hash differs".into(),
            ));
        }
    }
    Ok(
        json!({"normalized_tracks":receipts,"approved_jpeg_verified":cover_verified,"published_files_verified":files.len()}),
    )
}

pub(super) fn exercise(root: &Path, channel: &Channel) -> TestResult<Value> {
    let fixture = load_fixture(root)?;
    if !fixture.integrated {
        return Err("fixture was not prepared for integration".into());
    }
    let album = album(root)?;
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(30)).build()?;
        let scan=admit_scan(&client,&fixture,fixture.location_id).await?;
        channel.send(&json!({"event":"scan_admitted","location_id":fixture.location_id,"request_id":scan}))?;
        let response=http_json(client.post(format!("{}/api/jobs/{}/plans/{}/run",fixture.origin,album.job,album.version))
            .header("origin",&fixture.origin).header("x-domus-csrf","1")
            .json(&json!({"source_candidate":album.candidate,"acknowledge_new_only_publication":true})),202).await?;
        channel.send(&json!({"event":"acquisition_admitted","response":response,"job_id":album.job,"plan_version":album.version}))?;
        let (scan_result,reads)=wait_scan(&client,&fixture,fixture.location_id,scan).await?;
        require_completed_scan(&scan_result)?;
        channel.send(&json!({"event":"integration_ready","scan_completed":true,"scan_state":scan_result["state"],"scan_request_id":scan}))?;
        let deadline=tokio::time::Instant::now()+Duration::from_secs(if fixture.profile=="azure_full" {1800} else {180});
        let mut browse_requests=reads;
        loop {
            let jobs=http_json(client.get(format!("{}/api/jobs?limit=16",fixture.origin)),200).await?;
            let job=jobs.as_array().and_then(|jobs|jobs.iter().find(|job|job["id"]==album.job))
                .ok_or("generated job was not returned by the API")?;
            let requests=http_json(client.get(format!("{}/api/worker/requests?limit=16",fixture.origin)),200).await?;
            if let Some(failed)=requests.as_array().and_then(|requests|requests.iter().find(|request|request["state"]=="failed")) {
                return Err(format!("integrated worker request failed: {failed}").into());
            }
            if job["state"]=="complete" && requests.as_array().is_some_and(|requests| requests.iter().any(|request|
                request["operation"]["operation"]=="acquire" && request["operation"]["job_id"]==album.job
                    && request["state"]=="complete")) {break;}
            if matches!(job["state"].as_str(),Some("failed"|"awaiting_input"|"cancelled")) {
                return Err(format!("integrated acquisition stopped before publication: {job}").into());
            }
            if tokio::time::Instant::now()>=deadline {return Err(format!("integrated acquisition exceeded its deadline: {job}").into());}
            browse_requests+=browse_load(&client,&fixture.origin,fixture.location_id).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let evidence_root=root.to_owned();
        let publication=tokio::task::spawn_blocking(move||publication_evidence(&evidence_root,&album)).await??;
        Ok::<Value,Box<dyn std::error::Error>>(json!({
            "event":"result","role":"integrated","scan":scan_result,"browse_requests":browse_requests,
            "acquisition_publication_complete":true,
            "publication":publication,"scan_transfer_overlap":false,
            "integrated_transfer_sharing":false,"memory_acceptance":false,
            "remaining_evidence":"controller must verify peer upload/listing bytes and actual phase overlap"
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrated_scan_must_reach_complete_state() {
        assert!(require_completed_scan(&json!({"state":"complete"})).is_ok());
        for state in ["failed", "waiting", "queued", "running", "interrupted"] {
            assert!(
                require_completed_scan(&json!({"state":state}))
                    .is_err_and(|error| error.to_string().contains(state))
            );
        }
        assert!(require_completed_scan(&json!({})).is_err());
    }
}
