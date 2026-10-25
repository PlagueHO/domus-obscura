use super::*;
use domus::application::{add_location, scan_location};
use domus::domain::{JobState, Page};
use domus::infrastructure::catalog::CatalogQuery;
use domus::infrastructure::local_runtime::LocalRuntime;
use domus::infrastructure::staging::StagingRoot;
use domus::infrastructure::worker::LocalAcquisition;
use id3::{TagLike, Version};
use sha2::{Digest, Sha256};
use std::io::Cursor;
#[path = "../support/media.rs"]
#[allow(dead_code)]
mod media;

const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
const ARTIST: &str = "Fixture Artist";
const ALBUM: &str = "Fixture Album";
const TITLES: [&str; 3] = ["First Light", "Second Signal", "Third Return"];

fn seed_release(database: &std::path::Path) -> TestResult {
    let credit = json!([{"name":ARTIST,"joinphrase":"","artist":{
        "id":"10000000-0000-4000-8000-000000000001","name":ARTIST
    }}]);
    let track = |number: usize, position: usize| {
        json!({
            "id":format!("40000000-0000-4000-8000-{number:012}"),
            "position":position,"number":position.to_string(),"title":TITLES[number-1],
            "artist-credit":credit,"recording":{
                "id":format!("50000000-0000-4000-8000-{number:012}"),"video":false
            }
        })
    };
    let body = serde_json::to_vec(&json!({
        "id":RELEASE,"title":ALBUM,"date":"2026-10-25","artist-credit":credit,
        "release-group":{"id":"30000000-0000-4000-8000-000000000001"},
        "media":[
            {"position":1,"track-count":2,"tracks":[track(1,1),track(2,2)]},
            {"position":2,"track-count":1,"tracks":[track(3,1)]}
        ]
    }))?;
    rusqlite::Connection::open(database)?.execute(
        "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
         VALUES (?1,?2,?3,unixepoch(),unixepoch()+86400,unixepoch()*1000)",
        rusqlite::params![
            CatalogQuery::release(RELEASE)?.source(),
            body,
            Sha256::digest(&body).to_vec()
        ],
    )?;
    Ok(())
}

async fn command(
    client: &reqwest::Client,
    origin: &str,
    path: &str,
    body: Value,
) -> TestResult<Value> {
    let request = client
        .post(format!("{origin}{path}"))
        .header(header::ORIGIN, origin)
        .header("x-domus-csrf", "1");
    let request = if body.is_null() {
        request
    } else {
        request
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body)?)
    };
    let response = request.send().await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    assert!(
        status.is_success(),
        "{path}: {status} {}",
        String::from_utf8_lossy(&bytes)
    );
    if bytes.is_empty() {
        Ok(Value::Null)
    } else {
        Ok(serde_json::from_slice(&bytes)?)
    }
}

async fn idle(client: &reqwest::Client, origin: &str) -> TestResult<Value> {
    for _ in 0..200 {
        let status: Value = serde_json::from_slice(
            &client
                .get(format!("{origin}/api/local/status"))
                .send()
                .await?
                .bytes()
                .await?,
        )?;
        if status["busy"] == false {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err("local worker did not finish".into())
}

/// The API remains the real guarded HTTPS router. Only catalog input and already
/// received peer bytes are generated. No live provider or account is contacted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_https_approves_exact_album_and_publishes_verified_mp3_folder() -> TestResult {
    let directory = private::tempdir()?;
    let database = directory.path().join("state.db");
    let library = directory.path().join("library");
    let scratch = directory.path().join("staging");
    std::fs::create_dir(&library)?;
    std::fs::create_dir(&scratch)?;
    private::directory(&scratch)?;
    let credentials = directory.path().join("credentials.json");
    std::fs::write(
        &credentials,
        br#"{"username":"fixture-user","password":"unused-fixture"}"#,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut store = Store::open(&database)?;
    let location = add_location(&store, &library, "main")?;
    scan_location(&mut store, location)?;
    store.save_settings(None, &OutputSettings::default())?;
    drop(StagingRoot::open(&mut store, &scratch)?);
    seed_release(&database)?;
    let local = LocalRuntime::new(
        database.clone(),
        LocalAcquisition {
            staging_root: scratch.clone(),
            credentials_file: credentials,
            server: "8.8.8.8:2242".parse()?,
            acknowledge_plaintext_login: true,
        },
        Default::default(),
    )?;
    let material = development_tests::self_signed()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let origin = format!("https://localhost:{}", address.port());
    let assets = directory.path().join("browser");
    std::fs::create_dir_all(assets.join("assets"))?;
    std::fs::write(
        assets.join("index.html"),
        "<!doctype html><title>Local Music</title>",
    )?;
    std::fs::write(
        assets.join("assets").join("app.js"),
        "console.log('generated asset');",
    )?;
    let api = Api::development(&origin, store)?
        .with_browser(domus::infrastructure::browser::BrowserAssets::load(
            &assets,
        )?)?
        .with_local_runtime(local)?;
    let config = tls::configuration(
        "localhost",
        material.chain.as_bytes(),
        material.key.as_bytes(),
    )?;
    let handle = axum_server::Handle::new();
    let task = tokio::spawn(domus::infrastructure::server::serve_listener(
        listener,
        config.clone(),
        api.router(),
        handle.clone(),
    ));
    let server = FixtureServer {
        address,
        tls: config,
        handle,
        task: Some(task),
    };
    let client = reqwest::Client::builder()
        .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
        .resolve("localhost", address)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let identity: Value = serde_json::from_slice(
        &client
            .get(format!("{origin}/api/session"))
            .send()
            .await?
            .bytes()
            .await?,
    )?;
    assert_eq!(identity["object_id"], "_anonymous");
    let page = client.get(&origin).send().await?;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        page.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert!(page.headers().contains_key("content-security-policy"));
    assert_eq!(
        client
            .get(format!("{origin}/assets/app.js"))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    for path in [
        "/assets/private-key.pem",
        "/assets/..%2fstate.db",
        "/assets/missing.js",
    ] {
        assert_eq!(
            client.get(format!("{origin}{path}")).send().await?.status(),
            StatusCode::NOT_FOUND
        );
    }
    let created = command(&client, &origin, "/api/jobs", json!({})).await?;
    let job = created["job_id"].as_i64().ok_or("job ID")?;
    let planned = command(
        &client,
        &origin,
        &format!("/api/discovery/releases/{RELEASE}/plans"),
        json!({
            "job_id":job,"expected_version":1,"settings_version":1,"format":"mp3",
            "locations":[location],"accepts_incomplete_coverage":false,"bucket":null
        }),
    )
    .await?;
    let version = planned["version"].as_i64().ok_or("plan version")?;
    let base = format!("/api/jobs/{job}/plans/{version}");
    command(&client, &origin, &format!("{base}/approve"), Value::Null).await?;
    // A raw upload is still subject to both command guards.
    let cover = media::cover_source()?;
    assert_eq!(
        client
            .post(format!(
                "{origin}{base}/artwork/local?acknowledge_permitted_use=true"
            ))
            .body(cover.clone())
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = client
        .post(format!(
            "{origin}{base}/artwork/local?acknowledge_permitted_use=true"
        ))
        .header(header::ORIGIN, &origin)
        .header("x-domus-csrf", "1")
        .body(cover)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let artwork: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert!(
        artwork["source"]
            .as_str()
            .ok_or("local provenance")?
            .starts_with("urn:domus:local-artwork:sha256:")
    );
    let image = client
        .get(format!("{origin}{base}/artwork/{}/image", artwork["id"]))
        .send()
        .await?
        .bytes()
        .await?;
    assert_eq!(format!("{:x}", Sha256::digest(&image)), artwork["sha256"]);
    command(
        &client,
        &origin,
        &format!("{base}/artwork/approve"),
        json!({
            "candidate_id":artwork["id"],"sha256":artwork["sha256"]
        }),
    )
    .await?;
    let source = media::mp3()?;
    let files = TITLES
        .iter()
        .enumerate()
        .map(|(index, title)| {
            json!({
                "remote_path":format!("Peer\\Album\\{:02} - {title}.mp3",index+1),
                "expected_bytes":source.len(),"extension":"mp3","bitrate_kbps":320,
                "duration_seconds":1,"vbr":false,"sample_rate":44100,"bit_depth":null
            })
        })
        .collect::<Vec<_>>();
    let candidate = command(
        &client,
        &origin,
        &format!("{base}/source-candidates"),
        json!({
            "peer":"fixture-peer","folder":"Peer\\Album","directories":1,"files":files
        }),
    )
    .await?;
    assert_eq!(candidate["assessment"]["complete_mapping"], true);
    command(
        &client,
        &origin,
        &format!("{base}/run"),
        json!({
            "source_candidate":candidate["id"],"acknowledge_new_only_publication":true
        }),
    )
    .await?;
    let rejected = idle(&client, &origin).await?;
    assert!(rejected["last_error"].is_string());
    let raw = rusqlite::Connection::open(&database)?;
    let effects: i64 = raw.query_row(
        "SELECT (SELECT COUNT(*) FROM acquisition_sources) +
         (SELECT COUNT(*) FROM transfer_files) +
         (SELECT COUNT(*) FROM jobs WHERE owner IS NOT NULL)",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        effects, 0,
        "unapproved source must fail before any acquisition effect"
    );
    drop(raw);
    command(
        &client,
        &origin,
        &format!("/api/source-candidates/{}/approve", candidate["id"]),
        json!({
            "plan_version":version,"review_sha256":candidate["review_sha256"],
            "acknowledge_exact_edition_and_permitted_download":true
        }),
    )
    .await?;
    // Seal all generated input streams with the real reviewed acquisition
    // journal. The bound Soulseek channel tests cover the socket receive lane.
    let db = database.clone();
    let staging_path = scratch.clone();
    let source_copy = source.clone();
    let candidate_id = candidate["id"].as_i64().ok_or("candidate ID")?;
    tokio::task::spawn_blocking(move || -> domus::Result<()> {
        let mut store = Store::open_current_existing(&db)?;
        let claim = store
            .claim_approved_plan(job, version, "fixture-receiver", 300)?
            .ok_or(domus::Error::Internal("fixture claim missing"))?
            .claim;
        store.advance(&claim, JobState::Queued, JobState::Searching)?;
        store.begin_reviewed_acquisition(&claim, candidate_id)?;
        let staging = StagingRoot::open(&mut store, &staging_path)?;
        for file in store.transfers(job, version, Page::new(0, 200)?)? {
            store.start_transfer(&claim, file.id)?;
            store.accept_transfer(&claim, file.id, u32::from(file.track_number))?;
            let active = store.active_transfer(&claim, file.id)?;
            let received = staging.receive(&active, &mut Cursor::new(&source_copy))?;
            store.receive_transfer(
                &claim,
                file.id,
                i64::try_from(received.byte_length())
                    .map_err(|_| domus::Error::Internal("fixture length overflow"))?,
                received.sha256(),
            )?;
        }
        // Simulate a stopped receiver with all exact sealed receipts, not an
        // uncertain submission. The runtime must finish without any socket.
        rusqlite::Connection::open(&db)?
            .execute("UPDATE jobs SET lease_until=0 WHERE id=?1", [job])?;
        Ok(())
    })
    .await??;
    assert_eq!(
        client
            .post(format!("{origin}{base}/run"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&json!({
                "source_candidate":null,"acknowledge_new_only_publication":true
            }))?)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    command(
        &client,
        &origin,
        &format!("{base}/run"),
        json!({
            "source_candidate":null,"acknowledge_new_only_publication":true
        }),
    )
    .await?;
    let status = idle(&client, &origin).await?;
    assert!(status["last_error"].is_null(), "{status}");
    let store = Store::open_read_snapshot(&database)?;
    assert_eq!(store.job_state(job)?.as_deref(), Some("complete"));
    let destinations = store.publication_destinations(job, version)?;
    assert_eq!(destinations.len(), 1);
    assert!(destinations[0].verified_at.is_some());
    let folder = library
        .join("F")
        .join("FIXTURE ARTIST")
        .join("FIXTURE ALBUM");
    let mut names = std::fs::read_dir(&folder)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    let mut expected = TITLES
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{ARTIST} - {ALBUM} - {:02} - {t}.mp3", i + 1))
        .collect::<Vec<_>>();
    expected.push("folder.jpg".into());
    expected.sort();
    assert_eq!(
        names,
        expected
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(std::fs::read(folder.join("folder.jpg"))?, image.as_ref());
    let source_proof = domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&source))?;
    for (index, title) in TITLES.iter().enumerate() {
        let path = folder.join(format!(
            "{ARTIST} - {ALBUM} - {:02} - {title}.mp3",
            index + 1
        ));
        let bytes = std::fs::read(&path)?;
        let proof = domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&bytes))?;
        assert_eq!(proof.mpeg_sha256, source_proof.mpeg_sha256);
        assert_eq!(proof.audio_frames, source_proof.audio_frames);
        let tag = id3::Tag::read_from_path(&path)?;
        assert_eq!(tag.version(), Version::Id3v23);
        assert_eq!(tag.title(), Some(*title));
        assert_eq!(tag.artist(), Some(ARTIST));
        assert_eq!(tag.album_artist(), Some(ARTIST));
        assert_eq!(tag.album(), Some(ALBUM));
        assert_eq!(tag.track(), Some((index + 1) as u32));
        assert_eq!(tag.total_tracks(), Some(3));
        assert_eq!(tag.disc(), Some(if index == 2 { 2 } else { 1 }));
        assert_eq!(tag.total_discs(), Some(2));
        assert_eq!(tag.year(), Some(2026));
        assert_eq!(
            tag.get("TDAT").and_then(|f| f.content().text()),
            Some("2510")
        );
        let extra = tag
            .extended_texts()
            .map(|t| (t.description.as_str(), t.value.as_str()))
            .collect::<HashMap<_, _>>();
        assert_eq!(extra.len(), 7);
        assert_eq!(
            extra.get("ORIGINALTRACKNUMBER"),
            Some(&if index == 1 { "2" } else { "1" })
        );
        assert_eq!(extra.get("DATE"), Some(&"2026-10-25"));
        assert_eq!(extra.get("MusicBrainz Album Id"), Some(&RELEASE));
        assert_eq!(
            extra.get("MusicBrainz Release Group Id"),
            Some(&"30000000-0000-4000-8000-000000000001")
        );
        assert_eq!(
            extra.get("MusicBrainz Release Track Id").copied(),
            Some(format!("40000000-0000-4000-8000-{:012}", index + 1).as_str())
        );
        for key in ["MusicBrainz Artist Id", "MusicBrainz Album Artist Id"] {
            assert_eq!(
                extra.get(key),
                Some(&"10000000-0000-4000-8000-000000000001")
            );
        }
        let identifiers = tag.unique_file_identifiers().collect::<Vec<_>>();
        assert_eq!(identifiers.len(), 1);
        assert_eq!(identifiers[0].owner_identifier, "http://musicbrainz.org");
        assert_eq!(
            identifiers[0].identifier,
            format!("50000000-0000-4000-8000-{:012}", index + 1).as_bytes()
        );
        let pictures = tag.pictures().collect::<Vec<_>>();
        assert_eq!(pictures.len(), 1);
        assert_eq!(pictures[0].mime_type, "image/jpeg");
        assert_eq!(
            pictures[0].picture_type,
            id3::frame::PictureType::CoverFront
        );
        assert_eq!(pictures[0].data, image.as_ref());
        assert!(tag.genre().is_none());
        assert!(!extra.contains_key("REPLAYGAIN_TRACK_GAIN"));
    }
    server.stop().await?;
    Ok(())
}
