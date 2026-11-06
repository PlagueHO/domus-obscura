use crate::application::recovery::{continue_recovery, delete_recovery, preview_recovery};
use crate::application::{normalize_album, publish_new_album, receive_transfer_file};
use crate::infrastructure::filesystem::native_file_identity;
use crate::infrastructure::store::{AlbumReplacementRestoreDecision, RecoveryDecision};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "crash_tests::album::crash_child";
const START_LIMIT: Duration = Duration::from_secs(30);
const RECOVERY_LIMIT: Duration = Duration::from_secs(360);

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Action {
    Intent,
    Submission,
    Receive,
    Normalize,
    Publish,
    Replace,
    Restore,
    Delete,
    Question,
    Answer,
}

#[derive(Serialize, Deserialize)]
struct Descriptor {
    database: PathBuf,
    staging: PathBuf,
    library: PathBuf,
    job: i64,
    version: i64,
    location: i64,
    claim: JobClaim,
    source: Vec<u8>,
    run: Option<i64>,
    action: Action,
    point: String,
    peer: Option<std::net::SocketAddrV4>,
    restore: Option<AlbumReplacementRestoreDecision>,
}

struct OwnedChild {
    child: Child,
    log: PathBuf,
    deadline: Instant,
}

impl OwnedChild {
    fn start(
        descriptor: &Path,
        mode: &str,
        address: Option<std::net::SocketAddr>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let log = descriptor.with_extension(format!("{mode}.log"));
        let output = OpenOptions::new().write(true).create_new(true).open(&log)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("DOMUS_CRASH_DESCRIPTOR", descriptor)
            .env("DOMUS_CRASH_MODE", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::from(output.try_clone()?))
            .stderr(Stdio::from(output));
        if let Some(address) = address {
            command.env("DOMUS_CRASH_CHANNEL", address.to_string());
        } else {
            command.env_remove("DOMUS_CRASH_CHANNEL");
        }
        Ok(Self {
            child: command.spawn()?,
            log,
            deadline: Instant::now() + RECOVERY_LIMIT,
        })
    }

    fn ready(
        &mut self,
        listener: &TcpListener,
        expected: &str,
    ) -> Result<TcpStream, Box<dyn std::error::Error>> {
        listener.set_nonblocking(true)?;
        let deadline = Instant::now() + START_LIMIT;
        loop {
            match listener.accept() {
                Ok((channel, _)) => {
                    channel.set_nonblocking(false)?;
                    channel.set_read_timeout(Some(START_LIMIT))?;
                    let mut line = String::new();
                    BufReader::new((&channel).take(128)).read_line(&mut line)?;
                    assert_eq!(line.trim_end(), expected);
                    assert!(
                        self.child.try_wait()?.is_none(),
                        "child exited at checkpoint"
                    );
                    return Ok(channel);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if let Some(status) = self.child.try_wait()? {
                return Err(format!(
                    "child exited before {expected}: {status}\n{}",
                    std::fs::read_to_string(&self.log)?
                )
                .into());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "child did not reach {expected}\n{}",
                    std::fs::read_to_string(&self.log)?
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn terminate(&mut self) -> TestResult {
        assert!(self.child.try_wait()?.is_none());
        self.child.kill()?;
        assert!(
            !self.child.wait()?.success(),
            "forced termination returned success"
        );
        Ok(())
    }

    fn finish(&mut self) -> TestResult {
        loop {
            if let Some(status) = self.child.try_wait()? {
                if !status.success() {
                    return Err(format!(
                        "recovery failed: {status}\n{}",
                        std::fs::read_to_string(&self.log)?
                    )
                    .into());
                }
                return Ok(());
            }
            if Instant::now() >= self.deadline {
                return Err(format!(
                    "recovery exceeded its deadline\n{}",
                    std::fs::read_to_string(&self.log)?
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = self
                    .child
                    .kill()
                    .and_then(|()| self.child.wait().map(|_| ()))
                {
                    eprintln!("crash fixture child cleanup failed: {error}");
                }
            }
            Err(error) => eprintln!("crash fixture child status failed: {error}"),
        }
    }
}

fn files(source: &[u8]) -> Vec<SourceFile> {
    (1..=3)
        .map(|track| SourceFile {
            remote_path: format!("Peer\\Album\\track-{track}.mp3"),
            expected_bytes: source.len() as i64,
        })
        .collect()
}

fn integrity(database: &Path) -> TestResult {
    let raw = Connection::open(database)?;
    assert_eq!(
        raw.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?,
        "ok"
    );
    assert!(!raw.prepare("PRAGMA foreign_key_check")?.exists([])?);
    Ok(())
}

fn restore_decision(
    store: &mut Store,
    descriptor: &Descriptor,
) -> Result<AlbumReplacementRestoreDecision, Box<dyn std::error::Error>> {
    let destination = store
        .publication_destinations(descriptor.job, descriptor.version)?
        .into_iter()
        .find(|target| target.location_id == descriptor.location)
        .ok_or("restore destination")?;
    let preview = crate::application::preview_album_restore(
        store,
        descriptor.job,
        descriptor.version,
        descriptor.location,
    )?;
    assert!(preview.verified, "{:?}", preview.unavailable_reason);
    Ok(AlbumReplacementRestoreDecision {
        review_id: destination.replacement.ok_or("replacement")?.review_id,
        expected_manifest_sha256: destination.manifest_sha256,
        expected_album_identity: destination.album_identity.ok_or("album identity")?,
        expected_preview_sha256: preview.preview_sha256,
    })
}

fn retained_id(store: &Store, descriptor: &Descriptor) -> Result<i64, Box<dyn std::error::Error>> {
    Ok(store
        .recovery_inventory(Page::new(0, 100)?, Some(descriptor.job))?
        .into_iter()
        .find(|entry| entry.category == "retained")
        .ok_or("retained version")?
        .id)
}

fn receive(store: &mut Store, descriptor: &Descriptor, staging: &StagingRoot) -> TestResult {
    let transfer = store
        .transfers(descriptor.job, descriptor.version, Page::new(0, 1)?)?
        .pop()
        .ok_or("transfer")?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let stream = TcpStream::connect(listener.local_addr()?)?;
    let (mut peer, _) = listener.accept()?;
    let source = descriptor.source.clone();
    let token = transfer.remote_token.ok_or("accepted token")?;
    let sender = std::thread::spawn(move || -> std::io::Result<()> {
        peer.set_write_timeout(Some(START_LIMIT))?;
        peer.set_read_timeout(Some(START_LIMIT))?;
        peer.write_all(&token.to_le_bytes())?;
        let mut offset = [0; 8];
        peer.read_exact(&mut offset)?;
        assert_eq!(u64::from_le_bytes(offset), 0);
        peer.write_all(&source)?;
        Ok(())
    });
    receive_transfer_file(
        store,
        &descriptor.claim,
        staging,
        transfer.id,
        "generated-peer",
        stream,
    )?;
    sender.join().map_err(|_| "fixture sender panicked")??;
    Ok(())
}

fn perform(store: &mut Store, descriptor: &Descriptor) -> TestResult {
    use crate::domain::Question;
    match descriptor.action {
        Action::Intent | Action::Submission => {
            store.begin_acquisition(
                &descriptor.claim,
                "generated-peer",
                &files(&descriptor.source),
            )?;
            if matches!(descriptor.action, Action::Submission) {
                let transfer = store
                    .transfers(descriptor.job, descriptor.version, Page::new(0, 1)?)?
                    .pop()
                    .ok_or("planned transfer")?;
                crate::infrastructure::soulseek::SoulseekSession::with_crash_fixture_peer(
                    descriptor.peer.ok_or("fixture peer")?,
                    |peer| {
                        crate::application::negotiate_transfer(
                            store,
                            &descriptor.claim,
                            transfer.id,
                            peer,
                        )
                    },
                )?;
            }
        }
        Action::Receive => {
            let staging = StagingRoot::open(store, &descriptor.staging)?;
            receive(store, descriptor, &staging)?;
        }
        Action::Normalize => {
            let staging = StagingRoot::open(store, &descriptor.staging)?;
            let artwork = store.approved_artwork(descriptor.job, descriptor.version)?;
            normalize_album(store, &descriptor.claim, &staging, &artwork)?;
        }
        Action::Publish | Action::Replace => {
            let staging = StagingRoot::open(store, &descriptor.staging)?;
            assert_eq!(
                publish_new_album(
                    store,
                    &descriptor.claim,
                    &staging,
                    descriptor.run.ok_or("run")?
                )?,
                "complete"
            );
        }
        Action::Restore => {
            let decision = descriptor.restore.clone().ok_or("saved restore decision")?;
            crate::application::restore_album_replacement(
                store,
                descriptor.job,
                descriptor.version,
                descriptor.location,
                decision,
            )?;
        }
        Action::Delete => {
            let id = retained_id(store, descriptor)?;
            let preview = preview_recovery(store, id)?;
            assert!(preview.verified);
            delete_recovery(
                store,
                id,
                RecoveryDecision {
                    expected_preview_sha256: preview.preview_sha256.ok_or("deletion preview")?,
                    acknowledge_irreversible_deletion: true,
                },
            )?;
        }
        Action::Question | Action::Answer => {
            let id = store.ask(
                &descriptor.claim,
                JobState::Downloading,
                &Question::new(
                    "Inspect generated recovery evidence".into(),
                    vec!["Keep paused".into(), "Cancel".into()],
                )?,
            )?;
            if matches!(descriptor.action, Action::Answer) {
                store.answer(id, descriptor.version, "Keep paused")?;
            }
        }
    }
    Ok(())
}

fn reclaim(
    store: &mut Store,
    descriptor: &Descriptor,
) -> Result<JobClaim, Box<dyn std::error::Error>> {
    let raw = Connection::open(&descriptor.database)?;
    let deadline = Instant::now() + Duration::from_secs(330);
    while !raw.query_row(
        "SELECT lease_until<=unixepoch() FROM jobs WHERE id=?1",
        [descriptor.job],
        |row| row.get::<_, bool>(0),
    )? {
        if Instant::now() >= deadline {
            return Err("persisted claim did not expire within the recovery deadline".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(store.renew_claim(&descriptor.claim, 300).is_err());
    let claim = store
        .claim_approved_plan(descriptor.job, descriptor.version, "process-recovery", 300)?
        .ok_or("recovery claim")?
        .claim;
    assert!(store.renew_claim(&descriptor.claim, 300).is_err());
    assert!(
        store
            .begin_acquisition(&descriptor.claim, "other-peer", &files(&descriptor.source))
            .is_err()
    );
    Ok(claim)
}

fn recover(store: &mut Store, descriptor: &Descriptor) -> TestResult {
    match descriptor.action {
        Action::Question | Action::Answer => {
            let raw = Connection::open(&descriptor.database)?;
            assert_eq!(
                raw.query_row("SELECT count(*) FROM questions", [], |row| row
                    .get::<_, i64>(0))?,
                1
            );
            let id = raw.query_row("SELECT id FROM questions", [], |row| row.get::<_, i64>(0))?;
            let question = store.question(id)?;
            assert_eq!(question.plan_version, descriptor.version);
            assert_eq!(question.choices, ["Keep paused", "Cancel"]);
            assert_eq!(
                question.answer.as_deref(),
                matches!(descriptor.action, Action::Answer).then_some("Keep paused")
            );
            assert_eq!(
                store.job_state(descriptor.job)?.as_deref(),
                Some("awaiting_input")
            );
            assert!(
                store
                    .answer(question.id, descriptor.version + 1, "Cancel")
                    .is_err()
            );
            assert!(store.renew_claim(&descriptor.claim, 300).is_err());
        }
        Action::Restore => {
            let raw = Connection::open(&descriptor.database)?;
            let approval = || {
                raw.query_row(
                "SELECT preview_sha256,album_identity,current_files FROM album_restore_approvals",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            };
            let approved = approval()?;
            for _ in 0..2 {
                let decision = restore_decision(store, descriptor)?;
                crate::application::restore_album_replacement(
                    store,
                    descriptor.job,
                    descriptor.version,
                    descriptor.location,
                    decision,
                )?;
                let preview = crate::application::preview_album_restore(
                    store,
                    descriptor.job,
                    descriptor.version,
                    descriptor.location,
                )?;
                assert!(preview.verified, "{:?}", preview.unavailable_reason);
                assert_eq!(preview.state, "restored");
                assert_eq!(
                    approval()?,
                    approved,
                    "continuation changed the saved approval"
                );
            }
            assert_eq!(
                store.job_state(descriptor.job)?.as_deref(),
                Some("complete")
            );
            assert_eq!(
                raw.query_row("SELECT count(*) FROM album_restore_approvals", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                1
            );
        }
        Action::Delete => {
            let id = retained_id(store, descriptor)?;
            let operation = store
                .recovery_operation(id)?
                .ok_or("deletion operation")?
                .id;
            assert!(
                crate::application::preview_album_restore(
                    store,
                    descriptor.job,
                    descriptor.version,
                    descriptor.location
                )
                .is_err()
            );
            for _ in 0..2 {
                let operation = continue_recovery(store, id, operation, false)?;
                assert_eq!(operation.state, "complete");
            }
        }
        action => {
            let claim = reclaim(store, descriptor)?;
            let staging = StagingRoot::open(store, &descriptor.staging)?;
            match action {
                Action::Intent | Action::Submission => {
                    let transfers =
                        store.transfers(descriptor.job, descriptor.version, Page::new(0, 10)?)?;
                    assert_eq!(transfers.len(), 3);
                    assert!(
                        store
                            .begin_acquisition(&claim, "other-peer", &files(&descriptor.source))
                            .is_err()
                    );
                    assert!(
                        store
                            .start_transfer(&descriptor.claim, transfers[0].id)
                            .is_err()
                    );
                    if matches!(action, Action::Submission) {
                        assert_eq!(transfers[0].state, "submitting");
                        assert!(store.start_transfer(&claim, transfers[0].id).is_err());
                        assert!(transfers[0].remote_token.is_none());
                        let result = crate::infrastructure::soulseek::SoulseekSession::with_crash_fixture_peer(
                            descriptor.peer.ok_or("fixture peer")?,
                            |peer| crate::application::acquire_album(store, &claim, &staging, peer),
                        );
                        assert!(
                            result.is_err(),
                            "lost peer response became successful acquisition"
                        );
                        assert_eq!(
                            store.transfers(
                                descriptor.job,
                                descriptor.version,
                                Page::new(0, 1)?
                            )?[0]
                                .state,
                            "submitting"
                        );
                    } else {
                        assert!(transfers.iter().all(|transfer| transfer.state == "planned"));
                    }
                }
                Action::Receive => {
                    let result =
                        crate::application::reconcile_local_inputs(store, &claim, &staging);
                    if matches!(
                        descriptor.point.as_str(),
                        "receive_writing" | "receive_synced"
                    ) {
                        assert!(result.is_err(), "unsealed input was adopted");
                        assert!(!descriptor.staging.join("transfer-1.receipt").exists());
                    } else {
                        result?;
                        let transfers = store.transfers(
                            descriptor.job,
                            descriptor.version,
                            Page::new(0, 10)?,
                        )?;
                        assert_eq!(transfers[0].state, "received");
                        assert_eq!(
                            staging.received(&transfers[0])?.sha256(),
                            &<[u8; 32]>::from(Sha256::digest(&descriptor.source))
                        );
                        assert!(store.start_transfer(&claim, transfers[0].id).is_err());
                    }
                }
                Action::Normalize => {
                    assert!(
                        store.normalization_run(&claim)?.is_none(),
                        "reclaim reused an earlier generation's run"
                    );
                    let artwork = store.approved_artwork(descriptor.job, descriptor.version)?;
                    let run = normalize_album(store, &claim, &staging, &artwork)?;
                    assert_eq!(
                        store.job_state(descriptor.job)?.as_deref(),
                        Some("publishing")
                    );
                    assert!(normalize_album(store, &descriptor.claim, &staging, &artwork).is_err());
                    let raw = Connection::open(&descriptor.database)?;
                    assert_eq!(
                        raw.query_row(
                            "SELECT count(*) FROM normalization_receipts WHERE run_id=?1",
                            [run],
                            |row| row.get::<_, i64>(0)
                        )?,
                        3
                    );
                }
                Action::Publish | Action::Replace => {
                    let raw = Connection::open(&descriptor.database)?;
                    let before: i64 =
                        raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| {
                            row.get(0)
                        })?;
                    let result =
                        publish_new_album(store, &claim, &staging, descriptor.run.ok_or("run")?);
                    if descriptor.point == "publication_writing" {
                        assert!(result.is_err(), "partial output was repaired");
                    } else {
                        assert_eq!(result?, "complete");
                        verify_publication(store, descriptor)?;
                    }
                    assert_eq!(
                        raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| row
                            .get::<_, i64>(0))?,
                        before
                    );
                    assert!(store.start_transfer(&claim, 1).is_err());
                }
                _ => return Err("unexpected recovery action".into()),
            }
        }
    }
    integrity(&descriptor.database)
}

fn verify_publication(store: &Store, descriptor: &Descriptor) -> TestResult {
    let raw = Connection::open(&descriptor.database)?;
    let rows = raw.prepare("SELECT d.root,d.relative_folder,f.name,f.sha256 FROM publication_destinations d JOIN publication_files f ON f.destination_id=d.id WHERE f.verified_at IS NOT NULL")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(rows.len(), 4);
    for (root, folder, name, hash) in rows {
        let file = Path::new(&root)
            .join(folder.replace('/', std::path::MAIN_SEPARATOR_STR))
            .join(name);
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(file)?)), hash);
    }
    assert_eq!(
        store.job_state(descriptor.job)?.as_deref(),
        Some("complete")
    );
    Ok(())
}

#[test]
#[ignore = "only the owning process test supplies a private descriptor and control channel"]
fn crash_child() -> TestResult {
    let descriptor = std::env::var_os("DOMUS_CRASH_DESCRIPTOR").ok_or("descriptor")?;
    let descriptor: Descriptor = serde_json::from_slice(&std::fs::read(descriptor)?)?;
    let mut store = Store::open_current_existing(&descriptor.database)?;
    let mode = std::env::var("DOMUS_CRASH_MODE")?;
    if mode == "recover" {
        return recover(&mut store, &descriptor);
    }
    if mode == "backup_pending" || mode == "backup_complete" {
        let staging = StagingRoot::open(&mut store, &descriptor.staging)?;
        let next;
        let claim = if mode == "backup_complete" {
            assert!(store.renew_claim(&descriptor.claim, 300).is_err());
            next = store
                .claim_approved_plan(descriptor.job, descriptor.version, "backup-retry", 300)?
                .ok_or("backup retry claim")?
                .claim;
            &next
        } else {
            &descriptor.claim
        };
        let outcome = publish_new_album(&mut store, claim, &staging, descriptor.run.ok_or("run")?)?;
        assert_eq!(
            outcome,
            if mode == "backup_pending" {
                "backup_pending"
            } else {
                "complete"
            }
        );
        return integrity(&descriptor.database);
    }
    if mode == "partial_publication" {
        let staging = StagingRoot::open(&mut store, &descriptor.staging)?;
        assert!(
            publish_new_album(
                &mut store,
                &descriptor.claim,
                &staging,
                descriptor.run.ok_or("run")?
            )
            .is_err()
        );
        return integrity(&descriptor.database);
    }
    if mode == "reject_restore" {
        let preview = crate::application::preview_album_restore(
            &mut store,
            descriptor.job,
            descriptor.version,
            descriptor.location,
        )?;
        assert!(!preview.verified);
        assert!(preview.preview_sha256.is_none());
        let result = crate::application::restore_album_replacement(
            &mut store,
            descriptor.job,
            descriptor.version,
            descriptor.location,
            descriptor.restore.clone().ok_or("saved restore decision")?,
        );
        assert!(
            matches!(result, Err(crate::Error::Conflict(ref message)) if message.starts_with("restore cannot verify this destination:")),
            "{result:?}"
        );
        return integrity(&descriptor.database);
    }
    if mode == "reject_delete" {
        let id = retained_id(&store, &descriptor)?;
        let operation = store
            .recovery_operation(id)?
            .ok_or("deletion operation")?
            .id;
        assert!(continue_recovery(&mut store, id, operation, false).is_err());
        return integrity(&descriptor.database);
    }
    let channel = TcpStream::connect(std::env::var("DOMUS_CRASH_CHANNEL")?)?;
    channel.set_read_timeout(Some(Duration::from_secs(120)))?;
    channel.set_write_timeout(Some(START_LIMIT))?;
    super::PROBE.set(Some(super::Probe {
        target: descriptor.point.clone(),
        channel,
        fail_write: mode == "disk_full",
    }));
    if mode == "hold_database" {
        let raw = Connection::open(&descriptor.database)?;
        raw.execute_batch("BEGIN IMMEDIATE")?;
        super::checkpoint(&descriptor.point)?;
        return Err("database holder resumed".into());
    }
    if mode == "hold_staging" {
        let _staging = StagingRoot::open(&mut store, &descriptor.staging)?;
        super::checkpoint(&descriptor.point)?;
        return Err("staging holder resumed".into());
    }
    #[cfg(windows)]
    if mode == "hold_file" {
        use std::os::windows::fs::OpenOptionsExt;
        let _file = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(descriptor.staging.join("transfer-1.input"))?;
        super::checkpoint(&descriptor.point)?;
        return Err("file holder resumed".into());
    }
    if mode == "disk_full" {
        let Err(error) = perform(&mut store, &descriptor) else {
            return Err("injected full write succeeded".into());
        };
        assert!(
            matches!(error.downcast_ref::<crate::Error>(), Some(crate::Error::Io(error)) if error.kind() == std::io::ErrorKind::StorageFull),
            "{error}"
        );
        return integrity(&descriptor.database);
    }
    perform(&mut store, &descriptor)?;
    Err(format!("operation returned without reaching {}", descriptor.point).into())
}

#[derive(Debug, PartialEq, Eq)]
struct FileEvidence {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

fn inventory(root: &Path) -> Result<BTreeMap<String, FileEvidence>, Box<dyn std::error::Error>> {
    let mut result = BTreeMap::new();
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            assert!(!kind.is_symlink(), "fixture contains a link");
            if kind.is_dir() {
                directories.push(entry.path());
            } else {
                let file = File::open(entry.path())?;
                result.insert(
                    native_file_identity(&file)?,
                    FileEvidence {
                        path: entry.path().strip_prefix(root)?.to_owned(),
                        bytes: file.metadata()?.len(),
                        sha256: format!("{:x}", Sha256::digest(std::fs::read(entry.path())?)),
                    },
                );
            }
        }
    }
    Ok(result)
}

struct Case {
    fixture: Fixture,
    descriptor: PathBuf,
    action: Action,
    point: &'static str,
    before: BTreeMap<String, FileEvidence>,
    library_before: BTreeMap<String, FileEvidence>,
    peer: Option<PeerFixture>,
    directories: BTreeMap<String, PathBuf>,
}

fn prepare(
    action: Action,
    point: &'static str,
    format: AudioFormat,
) -> Result<Case, Box<dyn std::error::Error>> {
    let receive = !matches!(
        action,
        Action::Intent | Action::Submission | Action::Receive | Action::Question | Action::Answer
    );
    let replacement = matches!(action, Action::Replace | Action::Restore | Action::Delete)
        .then_some(ReplacementAction::Replace);
    let source = (format == AudioFormat::Mp3)
        .then(|| super::profiles::frames(&[320; 128], false, false, 0, false));
    let mut fixture = Fixture::build_with_inputs(
        format,
        true,
        true,
        receive,
        None,
        replacement,
        FixtureInputs {
            source,
            defer_acquisition: matches!(action, Action::Intent | Action::Submission),
            ..Default::default()
        },
    )?;
    let run = if matches!(
        action,
        Action::Publish | Action::Replace | Action::Restore | Action::Delete
    ) {
        Some(normalize_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            &fixture.cover,
        )?)
    } else {
        None
    };
    if matches!(action, Action::Restore | Action::Delete) {
        assert_eq!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run.ok_or("run")?
            )?,
            "complete"
        );
    }
    case_from_fixture(fixture, action, point, run)
}

fn case_from_fixture(
    mut fixture: Fixture,
    action: Action,
    point: &'static str,
    run: Option<i64>,
) -> Result<Case, Box<dyn std::error::Error>> {
    std::fs::write(
        fixture._library.path().join("unrelated.txt"),
        b"unrelated generated sentinel",
    )?;
    let descriptor = fixture._state.path().join("crash.json");
    let peer = if matches!(action, Action::Submission) {
        Some(PeerFixture::start()?)
    } else {
        None
    };
    let body = serde_json::to_vec(&serde_json::json!({
        "database":fixture.database, "staging":fixture.scratch.path(), "library":fixture._library.path(),
        "job":fixture.job, "version":fixture.version, "location":fixture.main,
        "claim":fixture.claim, "source":fixture.source, "run":run, "action":action, "point":point,
        "peer":peer.as_ref().map(|peer| peer.address),
        "restore":null,
    }))?;
    let mut body: Descriptor = serde_json::from_slice(&body)?;
    if matches!(action, Action::Restore) {
        body.restore = Some(restore_decision(&mut fixture.store, &body)?);
    }
    std::fs::write(&descriptor, serde_json::to_vec(&body)?)?;
    drop(fixture.staging.take());
    Ok(Case {
        fixture,
        descriptor,
        action,
        point,
        before: BTreeMap::new(),
        library_before: BTreeMap::new(),
        peer,
        directories: BTreeMap::new(),
    })
}

fn directory_inventory(
    root: &Path,
) -> Result<BTreeMap<String, PathBuf>, Box<dyn std::error::Error>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        result.insert(
            crate::infrastructure::filesystem::native_root_identity(&directory)?,
            directory.strip_prefix(root)?.to_owned(),
        );
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(result)
}

fn checkpoint_state(case: &Case) -> TestResult {
    let raw = Connection::open(&case.fixture.database)?;
    let state = case
        .fixture
        .store
        .job_state(case.fixture.job)?
        .ok_or("job state")?;
    let count = |sql: &str| raw.query_row(sql, [], |row| row.get::<_, i64>(0));
    match case.action {
        Action::Intent => {
            assert_eq!(state, "downloading");
            assert_eq!(
                count("SELECT count(*) FROM transfer_files WHERE state='planned'")?,
                3
            );
        }
        Action::Submission => {
            assert_eq!(
                count(
                    "SELECT count(*) FROM transfer_files WHERE state='submitting' AND remote_token IS NULL"
                )?,
                1
            );
        }
        Action::Receive => {
            assert_eq!(
                count("SELECT count(*) FROM transfer_files WHERE state='received'")?,
                i64::from(case.point == "receive_recorded")
            );
            let input = case.fixture.scratch.path().join("transfer-1.input");
            let bytes = input.metadata()?.len();
            if case.point == "receive_writing" {
                assert!(bytes > 0 && bytes < case.fixture.source.len() as u64);
            } else {
                assert_eq!(bytes, case.fixture.source.len() as u64);
            }
            assert_eq!(
                case.fixture
                    .scratch
                    .path()
                    .join("transfer-1.receipt")
                    .exists(),
                matches!(case.point, "receive_sealed" | "receive_recorded")
            );
        }
        Action::Normalize => {
            assert_eq!(state, "normalizing");
            assert_eq!(
                count("SELECT count(*) FROM normalization_runs WHERE completed_at IS NULL")?,
                1
            );
            let expected = match case.point {
                "normalization_receipt" => 1,
                "normalization_handoff" => 3,
                _ => 0,
            };
            assert_eq!(
                count("SELECT count(*) FROM normalization_receipts")?,
                expected
            );
        }
        Action::Publish | Action::Replace => {
            assert_eq!(state, "publishing");
            assert_eq!(count("SELECT count(*) FROM publication_destinations")?, 1);
            let expected = match case.point {
                "publication_intent" | "publication_synced" => 0,
                "publication_writing" | "publication_receipt" => 1,
                _ => 4,
            };
            assert_eq!(
                count("SELECT count(*) FROM publication_files WHERE verified_at IS NOT NULL")?,
                expected
            );
        }
        Action::Restore => {
            assert_eq!(state, "complete");
            assert_eq!(count("SELECT count(*) FROM album_restore_approvals")?, 1);
            let expected = match case.point {
                "restore_intent" | "restore_current_renamed" => "restore_prepared",
                "restore_current_moved" | "restore_original_renamed" => "restore_current_moved",
                "restore_original_visible" | "restore_displaced_renamed" => {
                    "restore_original_visible"
                }
                "restored" => "restored",
                _ => return Err("unrecognized restore checkpoint".into()),
            };
            assert_eq!(
                raw.query_row(
                    "SELECT state FROM publication_replacement_journal",
                    [],
                    |row| row.get::<_, String>(0)
                )?,
                expected
            );
        }
        Action::Delete => {
            assert_eq!(state, "complete");
            assert_eq!(
                count("SELECT count(*) FROM recovery_operations WHERE kind='delete'")?,
                1
            );
            assert_eq!(
                count(
                    "SELECT count(*) FROM recovery_operations WHERE kind='delete' AND state='complete'"
                )?,
                i64::from(case.point == "deletion_complete")
            );
        }
        Action::Question | Action::Answer => {
            assert_eq!(state, "awaiting_input");
            assert_eq!(count("SELECT count(*) FROM questions")?, 1);
            assert_eq!(
                count("SELECT count(*) FROM questions WHERE answer IS NOT NULL")?,
                i64::from(matches!(case.action, Action::Answer))
            );
        }
    }
    Ok(())
}

fn assert_preserved(
    before: &BTreeMap<String, FileEvidence>,
    after: &BTreeMap<String, FileEvidence>,
    moved: bool,
) {
    for (identity, expected) in before {
        let observed = after
            .get(identity)
            .unwrap_or_else(|| panic!("lost fixture file: {expected:?}"));
        assert_eq!(observed.bytes, expected.bytes);
        assert_eq!(observed.sha256, expected.sha256);
        if !moved {
            assert_eq!(observed.path, expected.path);
        }
    }
}

#[test]
fn cross_process_crash_recovery_matrix() -> TestResult {
    let mut specifications = vec![
        (Action::Intent, "transfer_intent", AudioFormat::Mp3),
        (Action::Submission, "submission_sent", AudioFormat::Mp3),
        (Action::Question, "question_saved", AudioFormat::Mp3),
        (Action::Answer, "answer_saved", AudioFormat::Mp3),
    ];
    for point in [
        "receive_writing",
        "receive_synced",
        "receive_sealed",
        "receive_recorded",
    ] {
        specifications.push((Action::Receive, point, AudioFormat::Mp3));
    }
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        for point in [
            "normalization_intent",
            "normalization_writing",
            "normalization_synced",
            "normalization_receipt",
            "normalization_handoff",
        ] {
            specifications.push((Action::Normalize, point, format));
        }
    }
    for point in [
        "publication_intent",
        "publication_writing",
        "publication_synced",
        "publication_receipt",
        "publication_verified",
        "publication_visible",
    ] {
        specifications.push((Action::Publish, point, AudioFormat::Mp3));
    }
    for point in ["replacement_retained", "replacement_visible"] {
        specifications.push((Action::Replace, point, AudioFormat::Flac));
    }
    for point in [
        "restore_intent",
        "restore_current_renamed",
        "restore_current_moved",
        "restore_original_renamed",
        "restore_original_visible",
        "restore_displaced_renamed",
        "restored",
    ] {
        specifications.push((Action::Restore, point, AudioFormat::Flac));
    }
    for point in [
        "deletion_intent",
        "deletion_file_removed",
        "deletion_directory_removed",
        "deletion_complete",
    ] {
        specifications.push((Action::Delete, point, AudioFormat::Flac));
    }
    let mut cases = Vec::new();
    for (action, point, format) in specifications {
        let mut case = prepare(action, point, format)?;
        let original = inventory(case.fixture._library.path())?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let mut child = OwnedChild::start(&case.descriptor, "crash", Some(listener.local_addr()?))?;
        let channel = child.ready(&listener, point)?;
        if let Some(peer) = &case.peer {
            let deadline = Instant::now() + START_LIMIT;
            while peer.submissions.load(std::sync::atomic::Ordering::SeqCst) != 1 {
                if Instant::now() >= deadline {
                    return Err("peer did not observe the original submission".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        integrity(&case.fixture.database)?;
        checkpoint_state(&case)?;
        if matches!(
            action,
            Action::Receive | Action::Normalize | Action::Publish | Action::Replace
        ) {
            assert!(
                StagingRoot::open(&mut case.fixture.store, case.fixture.scratch.path()).is_err(),
                "live child lost staging ownership"
            );
        }
        child.terminate()?;
        drop(channel);
        case.before = inventory(case.fixture.scratch.path())?;
        case.library_before = inventory(case.fixture._library.path())?;
        if !matches!(action, Action::Delete) {
            assert_preserved(&original, &case.library_before, true);
        }
        case.directories = directory_inventory(case.fixture._library.path())?;
        eprintln!("terminated at {point} ({format:?})");
        cases.push(case);
    }
    // All killed owners' real leases expire concurrently. No timestamp edits
    // or test lease overrides stand in for fenced recovery.
    let mut children = cases
        .iter()
        .map(|case| OwnedChild::start(&case.descriptor, "recover", None))
        .collect::<Result<Vec<_>, _>>()?;
    let mut failures = Vec::new();
    for (case, child) in cases.iter_mut().zip(&mut children) {
        if let Err(error) = child.finish() {
            failures.push(format!("{} ({:?}): {error}", case.point, case.action));
            continue;
        }
        if let Some(peer) = &mut case.peer {
            peer.finish()?;
            assert_eq!(
                peer.submissions.load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert_eq!(peer.queries.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
        integrity(&case.fixture.database)?;
        let after = inventory(case.fixture.scratch.path())?;
        if !matches!(case.action, Action::Publish | Action::Replace)
            || case.point == "publication_writing"
        {
            assert_preserved(&case.before, &after, false);
        }
        let library_after = inventory(case.fixture._library.path())?;
        let directories = directory_inventory(case.fixture._library.path())?;
        for (identity, path) in &case.directories {
            if matches!(case.action, Action::Delete)
                && path.components().any(|part| {
                    part.as_os_str()
                        .to_string_lossy()
                        .starts_with(".domus-retained-")
                })
            {
                assert!(
                    !directories.contains_key(identity),
                    "deleted retained folder still exists"
                );
            } else {
                assert!(
                    directories.contains_key(identity),
                    "lost fixture directory: {path:?}"
                );
            }
        }
        if matches!(case.action, Action::Delete) {
            let current: BTreeMap<_, _> = case
                .library_before
                .iter()
                .filter(|(_, file)| {
                    !file.path.components().any(|part| {
                        part.as_os_str()
                            .to_string_lossy()
                            .starts_with(".domus-retained-")
                    })
                })
                .map(|(id, evidence)| {
                    (
                        id.clone(),
                        FileEvidence {
                            path: evidence.path.clone(),
                            bytes: evidence.bytes,
                            sha256: evidence.sha256.clone(),
                        },
                    )
                })
                .collect();
            assert_preserved(&current, &library_after, false);
        } else {
            assert_preserved(&case.library_before, &library_after, true);
        }
        assert_eq!(
            std::fs::read(case.fixture._library.path().join("unrelated.txt"))?,
            b"unrelated generated sentinel"
        );
        eprintln!("recovered {} ({:?})", case.point, case.action);
    }
    if !failures.is_empty() {
        return Err(failures.join("\n").into());
    }
    Ok(())
}

struct PeerFixture {
    address: std::net::SocketAddrV4,
    submissions: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    queries: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    task: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

fn packet(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > 8192 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut packet = vec![0; length];
    stream.read_exact(&mut packet)?;
    Ok(packet)
}

impl PeerFixture {
    fn finish(&mut self) -> TestResult {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(task) = self.task.take() {
            task.join().map_err(|_| "fixture peer panicked")??;
        }
        Ok(())
    }

    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let std::net::SocketAddr::V4(address) = listener.local_addr()? else {
            return Err("IPv4 fixture".into());
        };
        let submissions = Arc::new(AtomicUsize::new(0));
        let queries = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let counts = (submissions.clone(), queries.clone(), stop.clone());
        let task = std::thread::spawn(move || {
            while !counts.2.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(START_LIMIT))?;
                stream.set_write_timeout(Some(START_LIMIT))?;
                assert_eq!(packet(&mut stream)?[0], 1);
                let request = packet(&mut stream)?;
                let code =
                    u32::from_le_bytes(request[..4].try_into().map_err(std::io::Error::other)?);
                if code == 43 {
                    // Report an offer to the peer. The killed client never persists
                    // its reply. Recovery must query (51), not submit (43) again.
                    let mut offer = 1u32.to_le_bytes().to_vec();
                    offer.extend_from_slice(&17u32.to_le_bytes());
                    offer.extend_from_slice(&request[4..]);
                    offer.extend_from_slice(&(1044u64 * 128).to_le_bytes());
                    stream.write_all(&((offer.len() + 4) as u32).to_le_bytes())?;
                    stream.write_all(&40u32.to_le_bytes())?;
                    stream.write_all(&offer)?;
                    counts.0.fetch_add(1, Ordering::SeqCst);
                } else {
                    assert_eq!(code, 51);
                    counts.1.fetch_add(1, Ordering::SeqCst);
                    // Drop the recovery query response. Transport loss cannot
                    // authorize a replacement submission.
                }
            }
            Ok(())
        });
        Ok(Self {
            address,
            submissions,
            queries,
            stop,
            task: Some(task),
        })
    }
}

impl Drop for PeerFixture {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("fixture peer cleanup failed: {error}");
        }
    }
}
