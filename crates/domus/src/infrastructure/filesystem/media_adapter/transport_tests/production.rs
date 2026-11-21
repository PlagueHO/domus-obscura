use super::{Certificates, Fixture, Running, configured_client_fixture, select_paired_fixture};
use crate::Error;
use crate::application::recovery::{continue_recovery, delete_recovery, preview_recovery};
use crate::application::{
    normalize_album, preview_album_replacements, preview_album_restore, publish_new_album,
    restore_album_replacement, scan_location,
};
use crate::domain::{AudioFormat, JobState, OutputSettings, Page, ReplacementAction};
use crate::infrastructure::artwork::{CoverJpeg, normalize_cover};
use crate::infrastructure::catalog::{CatalogQuery, CatalogResult};
use crate::infrastructure::filesystem::{
    DirectoryExpectation, StorageConfiguration, StorageRole, StorageRoot,
};
use crate::infrastructure::server::configuration::ConfigurationService;
use crate::infrastructure::staging::StagingRoot;
use crate::infrastructure::store::{
    AlbumReplacementDecision, AlbumReplacementRestoreDecision, PlanRequest, PublicationFile,
    RecoveryDecision, SourceFile, StoreOpenMode,
};
use crate::media_fixtures::{cover_source, flac};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::{Path, PathBuf};

type TestResult = super::TestResult;
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/support/album_metadata.rs"
));

#[cfg(windows)]
#[path = "production_docker.rs"]
mod docker;

#[tokio::test]
async fn media_adapter_production_paired_publication_restore_and_recovery() -> TestResult {
    for restore_old in [false, true] {
        let mut fixture = Fixture::new()?;
        let backup = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("backup");
        std::fs::create_dir(&backup)?;
        let (expected, _) = DirectoryExpectation::inspect(
            &fixture.selection.provider.boundary,
            &crate::domain::LibraryPath::parse("parent/backup")?,
        )?;
        fixture.selection.media.push(StorageRoot {
            root_id: "backup".into(),
            binding_revision: 1,
            role: StorageRole::Media,
            provider_scope: fixture.selection.provider.scope.clone(),
            relative: "parent/backup".into(),
            expected,
        });
        fixture.selection.writable_roots = vec!["library".into(), "backup".into()];
        let main = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media");
        let keys = Certificates::new()?;
        let running = Running::start(&fixture, &keys)?;
        let (host, client, _) = configured_client_fixture(&fixture, &keys, running.address)?;
        let application = host
            .parent()
            .ok_or("generated application directory")?
            .to_path_buf();
        std::fs::create_dir(application.join("staging"))?;
        crate::infrastructure::permissions::private_fixture::directory(
            &application.join("staging"),
        )?;
        let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&host)?)?;
        let mut storage: StorageConfiguration =
            serde_json::from_value(document["storage"].clone())?;
        let (expected, _) = DirectoryExpectation::inspect(
            &application,
            &crate::domain::LibraryPath::parse("staging")?,
        )?;
        storage.roots.push(StorageRoot {
            root_id: "staging".into(),
            binding_revision: 1,
            role: StorageRole::Staging,
            provider_scope: storage.providers[0].scope.clone(),
            relative: "staging".into(),
            expected,
        });
        document["storage"] = serde_json::to_value(storage)?;
        std::fs::write(&host, serde_json::to_vec(&document)?)?;
        select_paired_fixture(&host, &client, &fixture.selection.logical())?;
        let source = ConfigurationService::open(&host)?;
        let body = release_body(Some("2024-02"))?;
        let artwork_input = cover_source()?;
        let approved_cover = cover()?;
        let result = tokio::task::spawn_blocking(move || {
            run_workflow(
                source,
                application,
                Some([main, backup]),
                restore_old,
                body,
                artwork_input,
                approved_cover,
            )
        })
        .await;
        running.stop().await?;
        fixture.directory.close()?;
        result??;
    }
    Ok(())
}

fn run_workflow(
    source: ConfigurationService,
    application: PathBuf,
    native_roots: Option<[PathBuf; 2]>,
    restore_old: bool,
    body: Vec<u8>,
    artwork_input: Vec<u8>,
    approved_cover: CoverJpeg,
) -> crate::Result<serde_json::Value> {
    let mut store = source.open_store(StoreOpenMode::Current)?;
    let mut locations = Vec::new();
    for (root, kind) in [("library", "main"), ("backup", "backup")] {
        let location = store.add_paired_location(root, kind)?;
        scan_location(&mut store, location)?;
        locations.push(location);
    }
    store.save_settings(None, &OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let query = CatalogQuery::release(RELEASE)?;
    let raw = Connection::open(store.database_path())?;
    let now: i64 = raw.query_row("SELECT unixepoch()", [], |row| row.get(0))?;
    let payload_sha256 = Sha256::digest(&body).to_vec();
    raw.execute(
        "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
                 VALUES (?1,?2,?3,?4,?4+86400,?4*1000)",
        params![query.source(), body, payload_sha256, now],
    )?;
    let catalog = CatalogResult {
        source: query.source().into(),
        fetched_at: now,
        cached: true,
        payload_sha256,
        data: query.parse_release(&body)?,
    };
    let plan = store.prepare_catalog_plan(
        PlanRequest {
            audio_profile: None,
            job_id: job,
            expected_version: 1,
            settings_version: 1,
            format: Some(AudioFormat::Flac),
            edition: catalog.data.selection(None)?,
            locations,
            accepts_incomplete_coverage: false,
        },
        catalog,
    )?;
    let artwork = store.prepare_artwork(job, plan.version, "123456789", &artwork_input, None)?;
    store.approve_artwork(job, plan.version, artwork.id, &artwork.sha256)?;
    store.approve_plan(job, plan.version)?;
    let relative = &plan.plan.outputs[0].relative_folder;
    for target in &plan.plan.destinations {
        seed_original(&store, &application, &target.root, relative)?;
    }
    for preview in preview_album_replacements(&store, job, plan.version)? {
        assert!(
            preview.destination_available,
            "{:?}",
            preview.unavailable_reason
        );
        store.record_album_replacement_decision(
            &preview,
            AlbumReplacementDecision {
                action: ReplacementAction::Replace,
                expected_manifest_sha256: preview.manifest_sha256.clone(),
            },
        )?;
    }
    let claim = store
        .claim_approved_plan(job, plan.version, "generated-worker", 300)?
        .ok_or(Error::Internal("generated claim"))?
        .claim;
    store.advance(&claim, JobState::Queued, JobState::Searching)?;
    let input = flac();
    let sources = (1..=3)
        .map(|track| SourceFile {
            remote_path: format!("Generated\\Album\\track-{track}.flac"),
            expected_bytes: input.len() as i64,
        })
        .collect::<Vec<_>>();
    store.begin_acquisition(&claim, "generated-peer", &sources)?;
    let staging = StagingRoot::open(&mut store, &application.join("staging"))?;
    for file in store.transfers(job, plan.version, Page::new(0, 200)?)? {
        store.start_transfer(&claim, file.id)?;
        store.accept_transfer(&claim, file.id, file.track_number.into())?;
        let active = store
            .transfers(job, plan.version, Page::new(file.id - 1, 1)?)?
            .pop()
            .ok_or(Error::Internal("generated transfer"))?;
        let received = staging.receive(&active, &mut Cursor::new(&input))?;
        store.receive_transfer(
            &claim,
            file.id,
            received.byte_length() as i64,
            received.sha256(),
        )?;
    }
    store.advance(&claim, JobState::Downloading, JobState::Validating)?;
    let run = normalize_album(&mut store, &claim, &staging, &approved_cover)?;
    raw.execute_batch(
        "CREATE TRIGGER interrupt_paired_copy BEFORE UPDATE OF verified_at ON publication_files
                 BEGIN SELECT RAISE(ABORT,'generated paired copy interruption'); END;",
    )?;
    assert!(publish_new_album(&mut store, &claim, &staging, run).is_err());
    assert!(
        store
            .publication_destinations(job, plan.version)?
            .iter()
            .all(|destination| destination.verified_at.is_none())
    );
    raw.execute_batch("DROP TRIGGER interrupt_paired_copy")?;
    store = source.open_store(StoreOpenMode::Current)?;
    assert_eq!(
        publish_new_album(&mut store, &claim, &staging, run)?,
        "complete"
    );
    let destinations = store.publication_destinations(job, plan.version)?;
    assert_eq!(destinations.len(), 2);
    assert!(
        destinations
            .iter()
            .all(|destination| destination.verified_at.is_some())
    );
    assert_eq!(
        store
            .recovery_operation(run * 8 + 1)?
            .ok_or(Error::Internal("scratch cleanup"))?
            .state,
        "complete"
    );
    for destination in &destinations {
        if restore_old {
            let preview =
                preview_album_restore(&mut store, job, plan.version, destination.location_id)?;
            assert!(preview.verified, "{:?}", preview.unavailable_reason);
            let mut decision = AlbumReplacementRestoreDecision {
                review_id: destination
                    .replacement
                    .as_ref()
                    .ok_or(Error::Internal("replacement"))?
                    .review_id,
                expected_manifest_sha256: destination.manifest_sha256.clone(),
                expected_album_identity: destination
                    .album_identity
                    .clone()
                    .ok_or(Error::Internal("album identity"))?,
                expected_preview_sha256: preview.preview_sha256,
            };
            raw.execute_batch(
                        "CREATE TRIGGER interrupt_paired_restore BEFORE UPDATE OF state ON publication_replacement_journal
                         WHEN NEW.state='restore_current_moved'
                         BEGIN SELECT RAISE(ABORT,'generated paired restore interruption'); END;"
                    )?;
            assert!(
                restore_album_replacement(
                    &mut store,
                    job,
                    plan.version,
                    destination.location_id,
                    decision.clone()
                )
                .is_err()
            );
            raw.execute_batch("DROP TRIGGER interrupt_paired_restore")?;
            store = source.open_store(StoreOpenMode::Current)?;
            let continued =
                preview_album_restore(&mut store, job, plan.version, destination.location_id)?;
            assert!(continued.verified, "{:?}", continued.unavailable_reason);
            decision.expected_preview_sha256 = continued.preview_sha256;
            restore_album_replacement(
                &mut store,
                job,
                plan.version,
                destination.location_id,
                decision,
            )?;
        }
    }
    let entries = store.recovery_inventory(Page::new(0, 100)?, Some(job))?;
    let mut removed = Vec::new();
    for entry in entries
        .into_iter()
        .filter(|entry| entry.category == "retained")
    {
        let preview = preview_recovery(&mut store, entry.id)?;
        assert!(preview.verified, "{:?}", preview.unavailable_reason);
        let decision = RecoveryDecision {
            expected_preview_sha256: preview
                .preview_sha256
                .ok_or(Error::Internal("recovery preview"))?,
            acknowledge_irreversible_deletion: true,
        };
        raw.execute_batch(
            "CREATE TRIGGER interrupt_paired_deletion BEFORE UPDATE OF state ON recovery_items
                     WHEN NEW.state='removed'
                     BEGIN SELECT RAISE(ABORT,'generated paired deletion interruption'); END;",
        )?;
        assert!(delete_recovery(&mut store, entry.id, decision).is_err());
        let operation = store
            .recovery_operation(entry.id)?
            .ok_or(Error::Internal("interrupted deletion"))?;
        raw.execute_batch("DROP TRIGGER interrupt_paired_deletion")?;
        store = source.open_store(StoreOpenMode::Current)?;
        assert_eq!(
            continue_recovery(&mut store, entry.id, operation.id, false)?.state,
            "complete"
        );
        let destination = destinations
            .iter()
            .find(|destination| Some(destination.location_id) == entry.location_id)
            .ok_or(Error::Internal("generated retained destination"))?;
        removed.push(json!({"kind": destination.kind, "folder": entry.folder}));
    }
    assert_eq!(removed.len(), 2);
    let mut manifests = Vec::new();
    for destination in &destinations {
        let root = store.media_directory_io(&destination.root)?;
        let folder = root
            .open_directory(&crate::domain::LibraryPath::parse(relative)?)?
            .ok_or(Error::Internal("generated visible folder"))?;
        let expected = if restore_old {
            destination
                .replacement
                .as_ref()
                .ok_or(Error::Internal("generated replacement"))?
                .original_files
                .clone()
        } else {
            store.recovery_publication_files(destination.id)?
        };
        folder.verify_replacement(&expected, &|| Ok(()))?;
        manifests.push(json!({
            "kind": destination.kind,
            "files": expected.iter().map(|file| json!({
                "name": file.name, "bytes": file.bytes, "sha256": file.sha256,
            })).collect::<Vec<_>>(),
        }));
    }
    if let Some(roots) = native_roots {
        for root in &roots {
            let folder = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            let cover = std::fs::read(folder.join("folder.jpg"))?;
            if restore_old {
                assert_eq!(cover, b"old generated cover");
                assert_eq!(
                    std::fs::read(folder.join("manual.txt"))?,
                    b"old generated extra"
                );
            } else {
                assert_eq!(format!("{:x}", Sha256::digest(&cover)), artwork.sha256);
                assert!(!folder.join("manual.txt").exists());
            }
        }
    }
    Ok(json!({"folder": relative, "manifests": manifests, "removed": removed}))
}

fn seed_original(
    store: &crate::infrastructure::store::Store,
    application: &Path,
    root: &str,
    relative: &str,
) -> crate::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let mut directory = store.media_directory_io(root)?;
    for name in relative.split('/') {
        directory = match directory.open_directory(&crate::domain::LibraryPath::parse(name)?)? {
            Some(child) => child,
            None => directory.create_directory(name, &|| Ok(()))?,
        };
    }
    for (name, bytes) in [
        ("folder.jpg", b"old generated cover".as_slice()),
        ("manual.txt", b"old generated extra".as_slice()),
    ] {
        let mut source = tempfile::NamedTempFile::new_in(application)?;
        source.write_all(bytes)?;
        source.flush()?;
        source.seek(SeekFrom::Start(0))?;
        directory.copy_publication_file(
            &PublicationFile {
                ordinal: 0,
                name: name.into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
                verified_at: None,
            },
            source.as_file_mut(),
            &|| Ok(()),
        )?;
    }
    Ok(())
}

/// Private test-process entry point. It requires fresh container storage and
/// pairing material from the generated Windows coordinator, never library paths.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "private generated Docker workflow; requires coordinator input"]
fn media_adapter_production_guest_workflow() -> TestResult {
    use crate::infrastructure::server::PreparedServer;
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;

    let mut input = Vec::new();
    std::io::stdin().take(262_145).read_to_end(&mut input)?;
    if input.len() > 262_144 {
        return Err("generated media fixture input exceeds its limit".into());
    }
    let payload: serde_json::Value = serde_json::from_slice(&input)?;
    let revision = option_env!("DOMUS_MEDIA_FIXTURE_REVISION")
        .ok_or("fixture binary needs its build-time source revision")?;
    if payload["revision"].as_str() != Some(revision) {
        return Err("fixture binary and candidate source revisions differ".into());
    }
    let selection: super::super::super::MediaSelection =
        serde_json::from_value(payload["selection"].clone())?;
    selection.validate()?;
    if selection.media.len() != 2
        || !["library", "backup"]
            .iter()
            .all(|id| selection.media.iter().any(|root| root.root_id == *id))
    {
        return Err("generated fixture requires exactly main and backup roots".into());
    }
    let address: std::net::SocketAddr = serde_json::from_value(payload["address"].clone())?;
    if !address.ip().is_loopback() {
        return Err("generated fixture requires the loopback adapter".into());
    }
    let restore_old = payload["restore_old"]
        .as_bool()
        .ok_or("missing generated workflow case")?;
    {
        let directory = tempfile::Builder::new()
            .prefix(".domus-guest-")
            .tempdir_in("/checks")?;
        let application = directory.path().join("application");
        let host = PreparedServer::initialize_local_at(&application)?;
        for (name, key) in [
            ("ca.pem", "ca"),
            ("server.pem", "server"),
            ("client.pem", "client"),
            ("key.pem", "key"),
        ] {
            let content = payload[key]
                .as_str()
                .ok_or("missing generated pairing material")?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(application.join(name))?;
            file.write_all(content.as_bytes())?;
        }
        let client = application.join("media-client.json");
        let mut client_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&client)?;
        client_file.write_all(&serde_json::to_vec(&json!({
            "format_version": 1, "address": address, "selection": selection,
            "server_ca_file": "ca.pem", "paired_server_file": "server.pem",
            "certificate_file": "client.pem", "private_key_file": "key.pem",
        }))?)?;
        drop(client_file);
        std::fs::create_dir(application.join("staging"))?;
        crate::infrastructure::permissions::private_fixture::directory(
            &application.join("staging"),
        )?;
        let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&host)?)?;
        let mut storage: StorageConfiguration =
            serde_json::from_value(document["storage"].clone())?;
        let (expected, _) = DirectoryExpectation::inspect(
            &application,
            &crate::domain::LibraryPath::parse("staging")?,
        )?;
        storage.roots.push(StorageRoot {
            root_id: "staging".into(),
            binding_revision: 1,
            role: StorageRole::Staging,
            provider_scope: storage.providers[0].scope.clone(),
            relative: "staging".into(),
            expected,
        });
        document["storage"] = serde_json::to_value(storage)?;
        std::fs::write(&host, serde_json::to_vec(&document)?)?;
        select_paired_fixture(&host, &client, &selection)?;
        let result = run_workflow(
            ConfigurationService::open(&host)?,
            application,
            None,
            restore_old,
            release_body(Some("2024-02"))?,
            cover_source()?,
            cover()?,
        );
        directory.close()?;
        println!("DOMUS_MEDIA_WORKFLOW_RESULT={}", result?);
    }
    Ok(())
}
