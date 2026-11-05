//! Development-only transport for the connected UI and generated local files.
//! This router never mounts Api::router or accepts existing-library paths.
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use domus::application::{add_location, normalize_album, publish_new_album, scan_location};
use domus::domain::{JobState, LibraryPath, OutputSettings, Page};
use domus::infrastructure::api::{
    ApproveArtwork, ApproveSourceCandidate, CatalogBatchPlanRequest, CatalogPlanRequest,
};
use domus::infrastructure::browser::BrowserAssets;
use domus::infrastructure::catalog::{CatalogQuery, CatalogResult};
use domus::infrastructure::server::configuration::{
    ConfigurationReplacement, ConfigurationSectionCommand, ConfigurationSectionUpdate,
    ConfigurationService,
};
use domus::infrastructure::staging::StagingRoot;
use domus::infrastructure::store::{PlanRequest, Store};
use domus::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[path = "../tests/support/media.rs"]
#[allow(dead_code)]
mod media;
#[path = "../tests/support/private.rs"]
mod private;

const ORIGIN: &str = "http://127.0.0.1:4174";
const AUTHORITY: &str = "127.0.0.1:4174";
const ARTIST: &str = "10000000-0000-4000-8000-000000000001";
const RELEASES: [&str; 2] = [
    "20000000-0000-4000-8000-000000000001",
    "20000000-0000-4000-8000-000000000002",
];

struct Fixture {
    store: Store,
    configuration: ConfigurationService,
    staging: StagingRoot,
    assets: BrowserAssets,
    source: Vec<u8>,
    cover: Vec<u8>,
    root: TempDir,
    read_only: bool,
    runs: BTreeMap<i64, (i64, i64)>,
}

fn release_body(index: usize) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&json!({
        "id": RELEASES[index], "title": format!("Browser Album {}", index + 1),
        "date": "2026-01-02", "country": "NZ", "status": "Official",
        "artist-credit": [{"artist":{"id":ARTIST,"name":"Browser Artist"},"name":"Browser Artist","joinphrase":""}],
        "release-group": {"id":format!("30000000-0000-4000-8000-{:012}",index+1)},
        "media": [{"position":1,"format":"CD","track-count":1,"tracks":[{
            "id":format!("40000000-0000-4000-8000-{:012}",index+1),
            "position":1,"number":"1","title":"Generated Track",
            "artist-credit": [{"artist":{"id":ARTIST,"name":"Browser Artist"},"name":"Browser Artist","joinphrase":""}],
            "recording":{"id":format!("50000000-0000-4000-8000-{:012}",index+1),"video":false}
        }]}]
    }))?)
}

impl Fixture {
    fn dispose(self) -> Result<()> {
        let Self {
            store,
            configuration,
            staging,
            assets,
            source,
            cover,
            root,
            read_only: _,
            runs,
        } = self;
        drop((store, configuration, staging, assets, source, cover, runs));
        root.close()?;
        Ok(())
    }

    fn new() -> Result<Self> {
        let root = private::tempdir().map_err(|error| Error::InvalidInput(error.to_string()))?;
        for folder in ["main", "backup", "staging", "state"] {
            let path = root.path().join(folder);
            std::fs::create_dir(&path)?;
            private::directory(&path).map_err(|error| Error::InvalidInput(error.to_string()))?;
        }
        let mut store = Store::open(&root.path().join("state").join("state.db"))?;
        let main = add_location(&store, &root.path().join("main"), "main")?;
        let backup = add_location(&store, &root.path().join("backup"), "backup")?;
        scan_location(&mut store, main)?;
        scan_location(&mut store, backup)?;
        store.save_settings(None, &OutputSettings::default())?;
        let connection = rusqlite::Connection::open(store.database_path())?;
        for (index, id) in RELEASES.iter().enumerate() {
            let query = CatalogQuery::release(id)?;
            let body = release_body(index)?;
            connection.execute(
                "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
                 VALUES (?1,?2,?3,unixepoch(),unixepoch()+86400,unixepoch()*1000)",
                rusqlite::params![query.source(),body,Sha256::digest(&body).to_vec()],
            )?;
        }
        let staging = StagingRoot::open(&mut store, &root.path().join("staging"))?;
        let configuration_path = root.path().join("state").join("backend.json");
        std::fs::write(
            &configuration_path,
            serde_json::to_vec(&json!({
                "environment":"single_user", "system_settings_read_only":false,
                "database":store.database_path(), "listen":AUTHORITY, "origin":ORIGIN
            }))?,
        )?;
        let configuration = ConfigurationService::open(&configuration_path)?;
        #[cfg(not(test))]
        let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../frontend/dist-app")
            .canonicalize()?;
        #[cfg(test)]
        let assets = {
            let path = root.path().join("assets-fixture");
            std::fs::create_dir(&path)?;
            std::fs::create_dir(path.join("assets"))?;
            std::fs::write(path.join("index.html"), b"generated transport test")?;
            path
        };
        Ok(Self {
            store,
            configuration,
            staging,
            assets: BrowserAssets::load(&assets)?,
            source: media::mp3().map_err(|error| Error::InvalidInput(error.to_string()))?,
            cover: media::cover_source().map_err(|error| Error::InvalidInput(error.to_string()))?,
            root,
            read_only: false,
            runs: BTreeMap::new(),
        })
    }

    fn catalog(&mut self, id: &str) -> Result<CatalogResult<domus::domain::CatalogRelease>> {
        let index = RELEASES
            .iter()
            .position(|release| *release == id)
            .ok_or_else(|| Error::InvalidInput("unknown generated release".into()))?;
        let body = release_body(index)?;
        let query = CatalogQuery::release(id)?;
        let cached = self
            .store
            .catalog_cache(&query)?
            .ok_or(Error::Internal("generated catalog cache is missing"))?;
        Ok(CatalogResult {
            source: query.source().into(),
            fetched_at: cached.fetched_at,
            cached: true,
            payload_sha256: Sha256::digest(&body).to_vec(),
            data: query.parse_release(&body)?,
        })
    }

    fn plan_request(
        &mut self,
        id: &str,
        request: CatalogPlanRequest,
    ) -> Result<(PlanRequest, CatalogResult<domus::domain::CatalogRelease>)> {
        let catalog = self.catalog(id)?;
        let plan = request.into_plan_request(&catalog)?;
        Ok((plan, catalog))
    }

    fn run(&mut self, job: i64, version: i64, candidate: i64) -> Result<()> {
        let claimed = self
            .store
            .claim_approved_plan(job, version, "browser-fixture", 300)?
            .ok_or_else(|| Error::Conflict("generated job is already owned".into()))?;
        let claim = claimed.claim;
        self.store.approved_artwork(job, version)?;
        self.store
            .advance(&claim, JobState::Queued, JobState::Searching)?;
        self.store.begin_reviewed_acquisition(&claim, candidate)?;
        for file in self.store.transfers(job, version, Page::new(0, 100)?)? {
            self.store.start_transfer(&claim, file.id)?;
            self.store
                .accept_transfer(&claim, file.id, 100 + file.track_number as u32)?;
            let active = self
                .store
                .transfers(job, version, Page::new(file.id - 1, 1)?)?
                .pop()
                .ok_or(Error::Internal("generated active transfer missing"))?;
            let received = self
                .staging
                .receive(&active, &mut Cursor::new(&self.source))?;
            self.store.receive_transfer(
                &claim,
                file.id,
                received.byte_length() as i64,
                received.sha256(),
            )?;
        }
        self.store
            .advance(&claim, JobState::Downloading, JobState::Validating)?;
        let cover = self.store.approved_artwork(job, version)?;
        let run = normalize_album(&mut self.store, &claim, &self.staging, &cover)?;
        publish_new_album(&mut self.store, &claim, &self.staging, run)?;
        self.runs.insert(job, (run, version));
        Ok(())
    }

    fn verify(&self, job: i64, version: i64) -> Result<Value> {
        use id3::TagLike;
        let plan = self.store.plan(job, version)?;
        let cover = self.store.approved_artwork(job, version)?;
        let before = domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&self.source))?;
        let destinations = self.store.publication_destinations(job, version)?;
        let mut files = Vec::new();
        for destination in &destinations {
            if destination.verified_at.is_none() {
                continue;
            }
            let folder = PathBuf::from(&destination.root).join(
                destination
                    .relative_folder
                    .replace('/', std::path::MAIN_SEPARATOR_STR),
            );
            if std::fs::read(folder.join("folder.jpg"))? != cover.bytes() {
                return Err(Error::Conflict("folder JPEG differs from approval".into()));
            }
            for output in &plan.plan.outputs {
                let relative = format!("{}/{}", output.relative_folder, output.filename);
                let path = PathBuf::from(&destination.root)
                    .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
                let bytes = std::fs::read(&path)?;
                let after = domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&bytes))?;
                let tag = id3::Tag::read_from2(Cursor::new(&bytes))
                    .map_err(|_| Error::Internal("independent generated tag read failed"))?;
                let index = RELEASES
                    .iter()
                    .position(|id| *id == plan.plan.edition.release_id)
                    .ok_or(Error::Internal("generated release is missing"))?;
                let text = tag
                    .extended_texts()
                    .map(|text| (text.description.as_str(), text.value.as_str()))
                    .collect::<BTreeMap<_, _>>();
                for (name, expected) in [
                    ("DATE", "2026-01-02".to_owned()),
                    ("MusicBrainz Album Id", RELEASES[index].to_owned()),
                    (
                        "MusicBrainz Release Group Id",
                        format!("30000000-0000-4000-8000-{:012}", index + 1),
                    ),
                    (
                        "MusicBrainz Release Track Id",
                        format!("40000000-0000-4000-8000-{:012}", index + 1),
                    ),
                    ("MusicBrainz Artist Id", ARTIST.to_owned()),
                    ("MusicBrainz Album Artist Id", ARTIST.to_owned()),
                ] {
                    if text.get(name).copied() != Some(expected.as_str()) {
                        return Err(Error::Conflict(format!(
                            "generated output tag differs: {name}"
                        )));
                    }
                }
                let recording = format!("50000000-0000-4000-8000-{:012}", index + 1);
                if before.mpeg_sha256 != after.mpeg_sha256
                    || tag.title() != Some("Generated Track")
                    || tag.album() != Some(plan.plan.edition.album.as_str())
                    || tag.album_artist() != Some("Browser Artist")
                    || tag.track() != Some(1)
                    || tag.artist() != Some("Browser Artist")
                    || tag.total_tracks() != Some(1)
                    || tag.disc() != Some(1)
                    || tag.total_discs() != Some(1)
                    || tag
                        .unique_file_identifiers()
                        .find(|frame| frame.owner_identifier == "http://musicbrainz.org")
                        .map(|frame| frame.identifier.as_slice())
                        != Some(recording.as_bytes())
                    || tag.pictures().next().map(|picture| picture.data.as_slice())
                        != Some(cover.bytes())
                {
                    return Err(Error::Conflict(
                        "independent generated output verification failed".into(),
                    ));
                }
                files.push(relative);
            }
        }
        if files.is_empty() {
            return Err(Error::Conflict(
                "no generated publication has been verified".into(),
            ));
        }
        Ok(
            json!({"files":files,"destinations":destinations,"audio_unchanged":true,"tags_verified":true,"exact_jpeg":true}),
        )
    }

    fn configuration_command(&self, body: &[u8]) -> Result<ConfigurationSectionCommand> {
        let command: ConfigurationSectionCommand = decode(body)?;
        for section in &command.sections {
            let permitted = match section {
                ConfigurationSectionUpdate::General { browser_assets, .. } => {
                    browser_assets.is_none()
                }
                ConfigurationSectionUpdate::Identity {
                    client_secret_file, ..
                } => !matches!(client_secret_file, ConfigurationReplacement::Replace(_)),
                ConfigurationSectionUpdate::Tls {
                    certificate_file,
                    private_key_file,
                    managed_tls,
                    ..
                } => {
                    certificate_file.is_none()
                        && !matches!(private_key_file, ConfigurationReplacement::Replace(_))
                        && managed_tls.is_none()
                }
                ConfigurationSectionUpdate::Storage { database, cache } => {
                    database == &self.store.database_path().to_string_lossy() && cache.is_none()
                }
                ConfigurationSectionUpdate::MusicProviders { .. } => true,
                ConfigurationSectionUpdate::MusicAcquisition {
                    local_acquisition,
                    production_worker,
                } => {
                    local_acquisition.is_none()
                        && production_worker
                            .as_ref()
                            .is_none_or(|worker| worker.acquisition.is_none())
                }
                ConfigurationSectionUpdate::MusicSharing { sharing } => sharing
                    .as_ref()
                    .is_none_or(|sharing| sharing.account.is_none()),
            };
            if !permitted {
                return Err(Error::InvalidInput(
                    "browser fixture configuration accepts only its generated database and no additional file, root or credential references".into(),
                ));
            }
        }
        Ok(command)
    }

    fn dispatch(
        &mut self,
        method: &Method,
        path: &str,
        query: &BTreeMap<String, String>,
        body: &[u8],
    ) -> Result<Response> {
        let page = Page::new(
            integer(query, "after", 0)?,
            u16::try_from(integer(query, "limit", 50)?)
                .map_err(|_| Error::InvalidInput("invalid page limit".into()))?,
        )?;
        let command = method == Method::POST || method == Method::PUT;
        if path == "/__fixture/reset" && command {
            std::mem::replace(self, Self::new()?).dispose()?;
            return empty();
        }
        if path == "/__fixture/cover" && !command {
            return Ok(([(header::CONTENT_TYPE, "image/png")], self.cover.clone()).into_response());
        }
        if path == "/__fixture/listing" && !command {
            return json_response(json!({
                "peer":"generated-peer","folder":"Generated","directories":1,
                "files":[{"remote_path":"Generated\\01 Generated Track.mp3","expected_bytes":self.source.len(),
                    "extension":"mp3","bitrate_kbps":320,"vbr":false,"duration_seconds":null,"sample_rate":44100,"bit_depth":null}]
            }));
        }
        if path == "/__fixture/read-only" && command {
            self.read_only = decode::<Value>(body)?["read_only"]
                .as_bool()
                .ok_or_else(|| Error::InvalidInput("read_only is required".into()))?;
            return empty();
        }
        if path == "/__fixture/scan" && command {
            for location in self.store.locations(Page::new(0, 100)?)? {
                scan_location(&mut self.store, location.id)?;
            }
            return empty();
        }
        if path == "/__fixture/incomplete" && command {
            let claim = self.store.begin_scan(1)?;
            self.store
                .scan_error(&claim, "generated incomplete observation")?;
            if self.store.finish_scan(&claim)? {
                return Err(Error::Internal(
                    "incomplete generated scan was marked complete",
                ));
            }
            return empty();
        }
        if path == "/__fixture/backup" && command {
            let offline = decode::<Value>(body)?["offline"]
                .as_bool()
                .ok_or_else(|| Error::InvalidInput("offline is required".into()))?;
            let visible = self.root.path().join("backup");
            let hidden = self.root.path().join("offline-backup");
            std::fs::rename(
                if offline { &visible } else { &hidden },
                if offline { &hidden } else { &visible },
            )?;
            return empty();
        }
        match (method.as_str(), path) {
            ("GET", "/api/session") => {
                return json_response(json!({"tenant_id":"_anonymous","object_id":"_anonymous"}));
            }
            ("GET", "/api/local/status") => {
                return json_response(json!({"enabled":true,"busy":false,"last_error":null}));
            }
            ("GET", "/api/artist-photos") => return json_response(json!({"status":"disabled"})),
            ("GET", "/api/system/settings-policy") => {
                let state = self.configuration.read()?;
                return json_response(
                    json!({"read_only":self.read_only || state.active.system_settings_read_only || state.saved.system_settings_read_only}),
                );
            }
            ("GET", "/api/system/configuration") => {
                return json_response(self.configuration.read()?);
            }
            ("POST", "/api/system/configuration/sections/validate") => {
                let command = self.configuration_command(body)?;
                return json_response(self.configuration.validate_sections(command)?);
            }
            ("POST", "/api/system/configuration/sections/save") => {
                let command = self.configuration_command(body)?;
                return json_response(self.configuration.save_sections(command)?);
            }
            ("GET", "/api/settings") => return json_response(self.store.settings()?),
            ("PUT", "/api/settings") => {
                if self.read_only {
                    return Err(Error::Conflict("System settings are read-only.".into()));
                }
                self.configuration.require_editable()?;
                let value: FixtureSettings = decode(body)?;
                let version = self
                    .store
                    .save_settings(value.expected_version, &value.settings)?;
                return json_response(version);
            }
            ("GET", "/api/locations") => return json_response(self.store.locations(page)?),
            ("GET", "/api/queue") => return json_response(self.store.queue(page)?),
            ("GET", "/api/jobs") => return json_response(self.store.jobs(page)?),
            ("POST", "/api/jobs") => {
                let id = self.store.enqueue(1)?;
                return json_response(json!({"job_id":id,"plan_version":1}));
            }
            ("GET", "/api/inbox") => return json_response(self.store.inbox(page)?),
            ("POST", "/api/discovery/plans") => {
                let batch: CatalogBatchPlanRequest = decode(body)?;
                let requests = batch
                    .albums
                    .into_iter()
                    .map(|album| self.plan_request(&album.release_id, album.request))
                    .collect::<Result<Vec<_>>>()?;
                return json_response(self.store.prepare_catalog_plans(requests)?);
            }
            ("GET", "/api/discovery/artists") => {
                return json_response(
                    json!({"source":"generated","cached":true,"data":{"offset":0,"count":1,"items":[{"id":ARTIST,"name":"Browser Artist","sort_name":null,"country":"NZ","kind":"Group","disambiguation":null}]}}),
                );
            }
            ("GET", "/api/discovery/releases") => return self.release_page(),
            ("GET", "/api/recovery") => {
                return json_response(self.store.recovery_inventory_filtered(
                    page,
                    optional_integer(query, "job")?,
                    query.get("category").map(String::as_str),
                    optional_integer(query, "location")?,
                )?);
            }
            _ => {}
        }
        let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
        match parts.as_slice() {
            ["api", "discovery", "artists", id] if !command && *id == ARTIST => {
                return json_response(
                    json!({"source":"generated","cached":true,"data":{"id":ARTIST,"name":"Browser Artist","sort_name":null,"country":"NZ","kind":"Group","disambiguation":null}}),
                );
            }
            ["api", "discovery", "artists", _, "releases"] if !command => {
                return self.release_page();
            }
            ["api", "discovery", "releases", id] if !command => {
                return json_response(self.catalog(id)?);
            }
            ["api", "discovery", "releases", id, "plans"] if command => {
                let (request, catalog) = self.plan_request(id, decode(body)?)?;
                return json_response(self.store.prepare_catalog_plan(request, catalog)?);
            }
            ["api", "artist-photos", _] if !command => return json_response(Value::Null),
            ["api", "locations", id] if !command => {
                return json_response(self.store.location(parse(id)?)?);
            }
            ["api", "locations", id, "entries"] if !command => {
                return json_response(self.store.browse(
                    parse(id)?,
                    &LibraryPath::parse(query.get("folder").map_or("", String::as_str))?,
                    page,
                )?);
            }
            ["api", "locations", id, "library", "artists"] if !command => {
                return json_response(self.store.library_artists(parse(id)?, page)?);
            }
            ["api", "locations", id, "library", "albums"] if !command => {
                return json_response(self.store.library_albums(
                    parse(id)?,
                    optional_integer(query, "artist")?,
                    page,
                )?);
            }
            ["api", "locations", id, "library", "albums", album] if !command => {
                return json_response(self.store.library_album(parse(id)?, parse(album)?)?);
            }
            ["api", "locations", id, "library", "albums", album, "files"] if !command => {
                return json_response(self.store.library_album_files(
                    parse(id)?,
                    parse(album)?,
                    page,
                )?);
            }
            ["api", "locations", id, "library", "albums", album, "assets"] if !command => {
                return json_response(self.store.library_album_assets(
                    parse(id)?,
                    parse(album)?,
                    page,
                )?);
            }
            ["api", "locations", id, "library", "files", entry] if !command => {
                return json_response(self.store.library_file(parse(id)?, parse(entry)?)?);
            }
            ["api", "locations", id, "library", "coverage"] if !command => {
                return json_response(self.store.library_coverage(
                    parse(id)?,
                    &LibraryPath::parse(query.get("folder").map_or("", String::as_str))?,
                )?);
            }
            ["api", "locations", id, "library", "matches"] if command => {
                return json_response(self.store.library_matches(
                    parse(id)?,
                    &decode::<Vec<domus::domain::library::LibraryMatchQuery>>(body)?,
                )?);
            }
            ["api", "source-candidates", id, "approve"] if command => {
                let approval: ApproveSourceCandidate = decode(body)?;
                return json_response(self.store.approve_source_candidate(
                    parse(id)?,
                    approval.plan_version,
                    &approval.review_sha256,
                    approval.acknowledge_exact_edition_and_permitted_download,
                )?);
            }
            ["api", "questions", id] if !command => {
                return json_response(self.store.question_details(parse(id)?)?);
            }
            ["api", "questions", id, "answer"] if command => {
                let answer: Value = decode(body)?;
                self.store.answer(
                    parse(id)?,
                    answer["plan_version"]
                        .as_i64()
                        .ok_or_else(|| Error::InvalidInput("plan version required".into()))?,
                    answer["choice"]
                        .as_str()
                        .ok_or_else(|| Error::InvalidInput("answer required".into()))?,
                )?;
                return empty();
            }
            ["api", "job-controls", id, "continue"] if command => {
                let request: Value = decode(body)?;
                return json_response(
                    self.store.continue_job_control(
                        parse(id)?,
                        request["plan_version"]
                            .as_i64()
                            .ok_or_else(|| Error::InvalidInput("plan version required".into()))?,
                    )?,
                );
            }
            ["api", "recovery", id, "preview"] if !command => {
                return json_response(domus::application::recovery::preview_recovery(
                    &mut self.store,
                    parse(id)?,
                )?);
            }
            ["api", "recovery", id, "operation"] if !command => {
                return json_response(self.store.recovery_operation(parse(id)?)?);
            }
            ["api", "recovery", id, "delete"] if command => {
                return json_response(domus::application::recovery::delete_recovery(
                    &mut self.store,
                    parse(id)?,
                    decode(body)?,
                )?);
            }
            ["api", "recovery", id, action]
                if command && ["continue", "cleanup"].contains(action) =>
            {
                let request: domus::infrastructure::store::RecoveryContinue = decode(body)?;
                return json_response(domus::application::recovery::continue_recovery(
                    &mut self.store,
                    parse(id)?,
                    request.expected_operation_id,
                    *action == "cleanup",
                )?);
            }
            ["__fixture", "verify", job, version] if !command => {
                return json_response(self.verify(parse(job)?, parse(version)?)?);
            }
            ["__fixture", "existing", job, version] if command => {
                let plan = self.store.plan(parse(job)?, parse(version)?)?;
                let folder = self.root.path().join("main").join(
                    plan.plan.outputs[0]
                        .relative_folder
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                std::fs::create_dir_all(&folder)?;
                std::fs::write(folder.join("original.txt"), b"retained original")?;
                return empty();
            }
            ["__fixture", "change-current", job, version] if command => {
                let plan = self.store.plan(parse(job)?, parse(version)?)?;
                let folder = self.root.path().join("main").join(
                    plan.plan.outputs[0]
                        .relative_folder
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                std::fs::write(
                    folder.join("changed.txt"),
                    b"generated change after preview",
                )?;
                return empty();
            }
            ["__fixture", "verify-original", job, version] if !command => {
                let plan = self.store.plan(parse(job)?, parse(version)?)?;
                let folder = self.root.path().join("main").join(
                    plan.plan.outputs[0]
                        .relative_folder
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                if std::fs::read(folder.join("original.txt"))? != b"retained original"
                    || std::fs::read_dir(&folder)?.count() != 1
                {
                    return Err(Error::Conflict("restored original differs".into()));
                }
                return json_response(json!({"original_unchanged":true}));
            }
            _ => {}
        }
        if let ["api", "jobs", job, "plans", version, tail @ ..] = parts.as_slice() {
            let job = parse(job)?;
            let version = parse(version)?;
            if tail == ["artwork", "local"]
                && query.get("acknowledge_permitted_use").map(String::as_str) != Some("true")
            {
                return Err(Error::InvalidInput(
                    "permitted-use acknowledgement required".into(),
                ));
            }
            return self.job_route(method, job, version, tail, page, body);
        }
        if !command && (path == "/" || path.starts_with("/assets/")) {
            return Ok(self.assets.response(path));
        }
        Err(Error::InvalidInput(format!(
            "unsupported browser fixture route: {path}"
        )))
    }

    fn release_page(&mut self) -> Result<Response> {
        let items = RELEASES
            .iter()
            .map(|id| Ok(self.catalog(id)?.data.summary))
            .collect::<Result<Vec<_>>>()?;
        json_response(
            json!({"source":"generated","cached":true,"data":{"offset":0,"count":2,"next_offset":null,"items":items}}),
        )
    }

    fn job_route(
        &mut self,
        method: &Method,
        job: i64,
        version: i64,
        tail: &[&str],
        page: Page,
        body: &[u8],
    ) -> Result<Response> {
        let command = method == Method::POST;
        match tail {
            [] if !command => json_response(self.store.plan(job, version)?),
            ["approve"] if command => {
                self.store.approve_plan(job, version)?;
                empty()
            }
            ["controls"] if command => {
                let value: Value = decode(body)?;
                let action = serde_json::from_value(value["action"].clone())?;
                json_response(self.store.request_job_control(job, version, action)?)
            }
            ["source-candidates"] if !command => {
                json_response(self.store.source_candidates(job, version, page)?)
            }
            ["source-candidates"] if command => json_response(
                self.store
                    .prepare_source_candidate(job, version, decode(body)?)?,
            ),
            ["artwork"] if !command => {
                json_response(self.store.artwork_candidates(job, version, page)?)
            }
            ["artwork", "local"] if command => json_response(
                self.store
                    .prepare_uploaded_artwork_or_ask(job, version, body, None, true)?,
            ),
            ["artwork", "approve"] if command => {
                let approval: ApproveArtwork = decode(body)?;
                self.store.approve_artwork(
                    job,
                    version,
                    approval.candidate_id,
                    &approval.sha256,
                )?;
                empty()
            }
            ["artwork", id, "image"] if !command => {
                let cover = self.store.artwork_jpeg(job, version, parse(id)?)?;
                Ok((
                    [(header::CONTENT_TYPE, "image/jpeg")],
                    cover.bytes().to_vec(),
                )
                    .into_response())
            }
            ["transfers"] if !command => json_response(self.store.transfers(job, version, page)?),
            ["transfers", "summary"] if !command => {
                json_response(self.store.transfer_summary(job, version)?)
            }
            ["publication"] if !command => {
                json_response(self.store.publication_destinations(job, version)?)
            }
            ["publication", "replacement-review"] if !command => json_response(
                domus::application::preview_album_replacements(&self.store, job, version)?,
            ),
            ["publication", "replacement-review", location] if command => {
                let location = parse(location)?;
                let preview =
                    domus::application::preview_album_replacements(&self.store, job, version)?
                        .into_iter()
                        .find(|preview| preview.location_id == location)
                        .ok_or_else(|| {
                            Error::InvalidInput("unknown replacement destination".into())
                        })?;
                json_response(
                    self.store
                        .record_album_replacement_decision(&preview, decode(body)?)?,
                )
            }
            ["publication", "replacement", location, "restore"] if !command => {
                json_response(domus::application::preview_album_restore(
                    &mut self.store,
                    job,
                    version,
                    parse(location)?,
                )?)
            }
            ["publication", "replacement", location, "restore"] if command => {
                json_response(domus::application::restore_album_replacement(
                    &mut self.store,
                    job,
                    version,
                    parse(location)?,
                    decode(body)?,
                )?)
            }
            ["publication", "resume"] if command => {
                let request: domus::infrastructure::worker::ResumePublication = decode(body)?;
                let (run, retained_version) = self
                    .runs
                    .get(&job)
                    .ok_or_else(|| Error::Conflict("no retained generated run".into()))?;
                if !request.acknowledge_new_only_publication
                    || *run != request.normalization_run
                    || *retained_version != version
                {
                    return Err(Error::Conflict(
                        "review the exact retained publication run".into(),
                    ));
                }
                let claimed = self
                    .store
                    .claim_approved_plan(job, version, "browser-fixture-resume", 300)?
                    .ok_or_else(|| Error::Conflict("publication job is already owned".into()))?;
                publish_new_album(
                    &mut self.store,
                    &claimed.claim,
                    &self.staging,
                    request.normalization_run,
                )?;
                empty()
            }
            ["decision"] if !command => json_response(self.store.job_decision(job, version)?),
            ["events"] if !command => json_response(self.store.plan_events(job, version, page)?),
            ["run"] if command => {
                let request: domus::infrastructure::worker::StartLocalDownload = decode(body)?;
                if !request.acknowledge_new_only_publication {
                    return Err(Error::InvalidInput(
                        "publication acknowledgement required".into(),
                    ));
                }
                self.run(
                    job,
                    version,
                    request
                        .source_candidate
                        .ok_or_else(|| Error::InvalidInput("select a generated source".into()))?,
                )?;
                empty()
            }
            _ => Err(Error::InvalidInput(
                "unsupported generated job route".into(),
            )),
        }
    }
}

fn parse(value: &str) -> Result<i64> {
    value
        .parse()
        .map_err(|_| Error::InvalidInput("invalid fixture ID".into()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSettings {
    expected_version: Option<i64>,
    settings: OutputSettings,
}
fn integer(query: &BTreeMap<String, String>, name: &str, default: i64) -> Result<i64> {
    query.get(name).map_or(Ok(default), |value| parse(value))
}
fn optional_integer(query: &BTreeMap<String, String>, name: &str) -> Result<Option<i64>> {
    query.get(name).map(|value| parse(value)).transpose()
}
fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
    Ok(serde_json::from_slice(body)?)
}
fn json_response(value: impl Serialize) -> Result<Response> {
    Ok(Json(serde_json::to_value(value)?).into_response())
}
fn empty() -> Result<Response> {
    Ok(StatusCode::NO_CONTENT.into_response())
}
fn failure(error: Error) -> Response {
    eprintln!("browser fixture: {error}");
    let status = if matches!(error, Error::Conflict(_)) {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(json!({"error":error.to_string()}))).into_response()
}

async fn handle(
    State(shared): State<Arc<Mutex<Option<Fixture>>>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    let command = request.method() != Method::GET;
    if !peer.ip().is_loopback()
        || request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            != Some(AUTHORITY)
        || (command
            && (request
                .headers()
                .get(header::ORIGIN)
                .and_then(|value| value.to_str().ok())
                != Some(ORIGIN)
                || request
                    .headers()
                    .get("x-domus-csrf")
                    .and_then(|value| value.to_str().ok())
                    != Some("1")))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query = match axum::extract::Query::<BTreeMap<String, String>>::try_from_uri(request.uri())
    {
        Ok(query) => query.0,
        Err(_) => return failure(Error::InvalidInput("invalid fixture query".into())),
    };
    let bytes: Bytes = match to_bytes(request.into_body(), 8 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    match tokio::task::spawn_blocking(move || {
        let mut fixture = shared
            .lock()
            .map_err(|_| Error::Internal("browser fixture lock failed"))?;
        if method == Method::POST && path == "/__fixture/cleanup" {
            let owned = fixture
                .take()
                .ok_or(Error::Internal("browser fixture was already removed"))?;
            owned.dispose()?;
            return empty();
        }
        fixture
            .as_mut()
            .ok_or_else(|| Error::Conflict("browser fixture has been removed".into()))?
            .dispatch(&method, &path, &query, &bytes)
    })
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => failure(error),
        Err(_) => failure(Error::Internal("browser fixture task failed")),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let fixture = tokio::task::spawn_blocking(Fixture::new)
        .await
        .map_err(|_| Error::Internal("browser fixture setup failed"))??;
    let app = Router::new()
        .fallback(handle)
        .with_state(Arc::new(Mutex::new(Some(fixture))));
    let listener = tokio::net::TcpListener::bind(AUTHORITY).await?;
    println!("Generated connected-browser fixture: {ORIGIN}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("browser fixture shutdown signal failed: {error}");
        }
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn generated_child_directories_are_private() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new()?;
        for folder in ["main", "backup", "staging", "state"] {
            let permissions = std::fs::metadata(fixture.root.path().join(folder))?.permissions();
            assert_eq!(permissions.mode() & 0o777, 0o700, "{folder}");
        }
        fixture.dispose()
    }

    #[test]
    fn configuration_endpoints_reject_non_fixture_resources_before_validation() -> Result<()> {
        let mut fixture = Fixture::new()?;
        let revision = fixture.configuration.read()?.saved_revision;
        let external = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let keep = json!({"action":"keep"});
        let replace = json!({"action":"replace","value":external});
        let acquisition = json!({
            "staging_root":fixture.root.path().join("staging"),
            "credentials_file":replace,
            "server":"generated.invalid:2242", "acknowledge_plaintext_login":false
        });
        let managed = json!({
            "environment":"staging", "state_directory":external,
            "tenant_id":keep, "client_id":keep, "client_secret_file":keep,
            "subscription_id":"generated", "resource_group":"generated", "zone":"generated.invalid",
            "terms_of_service_agreed":false
        });
        let sections = [
            json!({"section":"storage","database":external,"cache":null}),
            json!({"section":"storage","database":fixture.root.path().join("state").join("other.db"),"cache":null}),
            json!({"section":"storage","database":fixture.root.path().join("state").join("..").join("state").join("state.db"),"cache":null}),
            json!({"section":"storage","database":fixture.store.database_path(),
                "cache":{"root":external,"item_bytes":1024,"total_bytes":4096,"entries":2}}),
            json!({"section":"general","environment":"single_user","listen":AUTHORITY,
                "origin":ORIGIN,"browser_assets":external}),
            json!({"section":"identity","tenant_id":keep,"object_id":keep,"client_id":keep,
                "client_secret_file":replace}),
            json!({"section":"tls","certificate_file":external,"private_key_file":keep,"managed_tls":null}),
            json!({"section":"tls","certificate_file":null,"private_key_file":replace,"managed_tls":null}),
            json!({"section":"tls","certificate_file":null,"private_key_file":keep,"managed_tls":managed}),
            json!({"section":"music_acquisition","local_acquisition":acquisition,"production_worker":null}),
            json!({"section":"music_acquisition","local_acquisition":null,
                "production_worker":{"queue_capacity":1,"acquisition":acquisition}}),
            json!({"section":"music_sharing","sharing":{"policy":{},"listen":null,
                "account":{"credentials_file":replace,"server":"generated.invalid:2242",
                    "acknowledge_plaintext_login":false}}}),
        ];
        for endpoint in ["validate", "save"] {
            for section in &sections {
                let body = serde_json::to_vec(&json!({
                    "expected_revision":revision, "sections":[section]
                }))?;
                let result = fixture.dispatch(
                    &Method::POST,
                    &format!("/api/system/configuration/sections/{endpoint}"),
                    &BTreeMap::new(),
                    &body,
                );
                assert!(
                    matches!(result, Err(Error::InvalidInput(ref message))
                        if message.starts_with("browser fixture configuration accepts only")),
                    "{endpoint} must reject {section} before configuration validation"
                );
                assert_eq!(fixture.configuration.read()?.saved_revision, revision);
            }
        }
        fixture.dispose()
    }

    #[test]
    fn configuration_endpoints_preserve_generated_database_and_section_saves() -> Result<()> {
        let mut fixture = Fixture::new()?;
        let revision = fixture.configuration.read()?.saved_revision;
        let body = serde_json::to_vec(&json!({
            "expected_revision":revision,
            "sections":[
                {"section":"general","environment":"single_user","listen":"127.0.0.1:4175",
                    "origin":"http://127.0.0.1:4175","browser_assets":null},
                {"section":"storage","database":fixture.store.database_path(),"cache":null}
            ]
        }))?;
        for endpoint in ["validate", "save"] {
            let response = fixture.dispatch(
                &Method::POST,
                &format!("/api/system/configuration/sections/{endpoint}"),
                &BTreeMap::new(),
                &body,
            )?;
            assert_eq!(response.status(), StatusCode::OK);
            let state = fixture.configuration.read()?;
            if endpoint == "validate" {
                assert_eq!(state.saved_revision, revision);
            } else {
                assert_ne!(state.saved_revision, revision);
                assert_eq!(state.saved.origin, "http://127.0.0.1:4175");
                assert_eq!(state.active.origin, ORIGIN);
                assert!(state.restart_required);
            }
            assert_eq!(
                fixture.store.database_path().to_string_lossy(),
                state.saved.database
            );
        }
        fixture.dispose()
    }

    #[test]
    fn generated_catalog_and_private_configuration_use_real_contracts() -> Result<()> {
        let mut fixture = Fixture::new()?;
        let owned = fixture.root.path().to_owned();
        for id in RELEASES {
            let release = fixture.catalog(id)?;
            assert_eq!(release.data.summary.id, id);
            assert_eq!(release.data.audio_tags()?.len(), 1);
        }
        let state = fixture.configuration.read()?;
        assert_eq!(state.active.origin, ORIGIN);
        assert_eq!(state.active_revision, state.saved_revision);
        assert!(!state.restart_required);
        assert!(fixture.verify(1, 2).is_err());
        assert!(
            fixture
                .dispatch(&Method::GET, "/unsupported", &BTreeMap::new(), &[])
                .is_err()
        );
        fixture.dispose()?;
        assert!(!owned.exists());
        Ok(())
    }

    #[tokio::test]
    async fn fixture_transport_rejects_missing_guards_before_any_job_creation() -> Result<()> {
        let shared = Arc::new(Mutex::new(Some(Fixture::new()?)));
        for (host, origin, csrf) in [
            ("other:4174", ORIGIN, "1"),
            (AUTHORITY, "http://other:4174", "1"),
            (AUTHORITY, ORIGIN, ""),
        ] {
            let request = Request::builder()
                .method(Method::POST)
                .uri("/api/jobs")
                .header(header::HOST, host)
                .header(header::ORIGIN, origin)
                .header("x-domus-csrf", csrf)
                .body(Body::from("{}"))
                .map_err(|_| Error::Internal("generated request failed"))?;
            let response = handle(
                State(shared.clone()),
                ConnectInfo(([127, 0, 0, 1], 1).into()),
                request,
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let fixture = shared
            .lock()
            .map_err(|_| Error::Internal("generated lock failed"))?
            .take()
            .ok_or(Error::Internal("generated fixture missing"))?;
        assert!(fixture.store.jobs(Page::new(0, 25)?)?.is_empty());
        fixture.dispose()?;
        Ok(())
    }
}
