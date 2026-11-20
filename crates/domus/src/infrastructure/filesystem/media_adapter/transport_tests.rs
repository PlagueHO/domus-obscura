use super::super::tests::Fixture;
use super::super::{MAX_PAGE, MAX_READ, MediaClient, MediaNode, Reference};
use super::*;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[path = "transport_tests/production.rs"]
mod production;

struct Certificates {
    ca: String,
    server: String,
    server_key: String,
    client: String,
    client_key: String,
    other_server: String,
    other_client: String,
    other_client_key: String,
}

impl Certificates {
    fn new() -> TestResult<Self> {
        let mut ca = CertificateParams::default();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.distinguished_name
            .push(DnType::CommonName, "Domus generated media adapter CA");
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate()?)?;
        let issue = |purpose| -> TestResult<(String, String)> {
            let key = KeyPair::generate()?;
            let mut params = CertificateParams::new(vec!["localhost".into()])?;
            params.extended_key_usages = vec![purpose];
            params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            params.not_before = rcgen::date_time_ymd(2020, 1, 1);
            params.not_after = rcgen::date_time_ymd(4090, 1, 1);
            Ok((params.signed_by(&key, &ca)?.pem(), key.serialize_pem()))
        };
        let (server, server_key) = issue(ExtendedKeyUsagePurpose::ServerAuth)?;
        let (client, client_key) = issue(ExtendedKeyUsagePurpose::ClientAuth)?;
        let (other_server, _) = issue(ExtendedKeyUsagePurpose::ServerAuth)?;
        let (other_client, other_client_key) = issue(ExtendedKeyUsagePurpose::ClientAuth)?;
        let server = format!("{server}{}", ca.pem());
        Ok(Self {
            ca: ca.pem(),
            server,
            server_key,
            client,
            client_key,
            other_server,
            other_client,
            other_client_key,
        })
    }

    fn client(&self, address: SocketAddr) -> Result<Client> {
        Client::new(
            address,
            self.ca.as_bytes(),
            self.server.as_bytes(),
            self.client.as_bytes(),
            self.client_key.as_bytes(),
        )
    }
}

struct Running {
    address: SocketAddr,
    handle: axum_server::Handle<SocketAddr>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Running {
    fn start(fixture: &Fixture, keys: &Certificates) -> TestResult<Self> {
        let adapter = PreparedAdapter::new(
            fixture.selection.clone(),
            Vec::new(),
            keys.server.as_bytes(),
            keys.server_key.as_bytes(),
            keys.ca.as_bytes(),
            keys.client.as_bytes(),
        )?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let handle = axum_server::Handle::new();
        let task = tokio::spawn(adapter.serve(listener, handle.clone()));
        Ok(Self {
            address,
            handle,
            task,
        })
    }

    async fn stop(mut self) -> TestResult {
        self.handle.graceful_shutdown(Some(Duration::from_secs(1)));
        tokio::time::timeout(Duration::from_secs(3), &mut self.task).await???;
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.handle.shutdown();
        self.task.abort();
    }
}

fn reference(response: Response) -> Result<Reference> {
    match response {
        Response::Opened { reference, .. } => Ok(reference),
        _ => Err(Error::Internal("expected a TLS media reference")),
    }
}

async fn media_client_file(
    client: &mut MediaClient,
    root: &super::super::MediaDirectory,
) -> TestResult<super::super::MediaFile> {
    let page = client.page(root, 64).await?;
    assert!(page.complete);
    assert_eq!(page.entries.len(), 1);
    let MediaNode::Directory(album) = client.open_child(root, &page.entries[0]).await? else {
        return Err("expected media album directory".into());
    };
    let page = client.page(&album, 64).await?;
    assert!(page.complete);
    let MediaNode::File(file) = client.open_child(&album, &page.entries[0]).await? else {
        return Err("expected media file".into());
    };
    Ok(file)
}

#[tokio::test]
async fn media_adapter_move_typed_native_and_tls_keep_the_exact_source() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    for index in 0..MAX_PAGE + 3 {
        std::fs::write(media.join(format!("existing-{index}")), [3])?;
    }
    for index in 0..2 {
        std::fs::create_dir(media.join(format!("source-{index}")))?;
        std::fs::write(
            media.join(format!("source-{index}")).join("keep.flac"),
            b"verified",
        )?;
    }
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let native = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    for (index, mut client) in [native, remote].into_iter().enumerate() {
        let mut root = client.open_root("library").await?;
        let name = format!("source-{index}");
        let mut listed = None;
        loop {
            let page = client.page(&root, MAX_PAGE).await?;
            listed = listed.or_else(|| page.entries.into_iter().find(|entry| entry.name == name));
            if page.complete {
                break;
            }
        }
        let listed = listed.ok_or("generated move source was not listed")?;
        let ordinary = client.open_child(&root, &listed).await?;
        assert!(client.prepare_move(&root, &listed).await.is_err());
        let MediaNode::Directory(ordinary) = ordinary else {
            return Err("expected directory".into());
        };
        client.close_directory(ordinary).await?;
        let mut stale = listed.clone();
        stale.observed.modified_nanoseconds =
            (stale.observed.modified_nanoseconds + 1) % 1_000_000_000;
        assert!(client.prepare_move(&root, &stale).await.is_err());
        let source = client.prepare_move(&root, &listed).await?;
        #[cfg(windows)]
        assert!(std::fs::rename(media.join(&name), media.join("swapped")).is_err());
        let page = client.page(source.directory(), MAX_PAGE).await?;
        assert!(page.complete);
        assert_eq!(page.entries.len(), 1);
        let MediaNode::File(file) = client
            .open_child(source.directory(), &page.entries[0])
            .await?
        else {
            return Err("expected retained source file".into());
        };
        assert_eq!(client.read(&file, 0, MAX_READ).await?, b"verified");
        client.close_file(file).await?;
        let mut checks = 0;
        let target = format!("final-{index}");
        let moved = client
            .rename_directory_checked(&mut root, source, &target, &mut || {
                checks += 1;
                Ok(())
            })
            .await?;
        assert!(checks >= 2);
        assert_eq!(moved.identity, listed.observed.identity);
        assert!(!media.join(name).exists());
        assert_eq!(
            std::fs::read(media.join(&target).join("keep.flac"))?,
            b"verified"
        );
        let mut found = false;
        loop {
            let page = client.page(&root, MAX_PAGE).await?;
            found |= page
                .entries
                .iter()
                .any(|entry| entry.name == target && entry.observed == moved);
            if page.complete {
                break;
            }
        }
        assert!(found);
        client.close_directory(root).await?;
    }
    running.stop().await
}

#[tokio::test]
async fn media_adapter_write_checkpoints_stop_before_effect_and_between_hash_steps() -> TestResult {
    use sha2::{Digest, Sha256};
    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    for index in 0..MAX_PAGE + 3 {
        std::fs::write(media.join(format!("entry-{index}")), [1])?;
    }
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let native = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    for (index, mut client) in [native, remote].into_iter().enumerate() {
        let mut root = client.open_root("library").await?;
        let name = format!("denied-{index}");
        let mut checks = 0;
        assert!(
            client
                .create_directory_checked(&mut root, &name, &mut || {
                    checks += 1;
                    if checks == 2 {
                        Err(Error::Conflict("generated claim expired".into()))
                    } else {
                        Ok(())
                    }
                })
                .await
                .is_err()
        );
        assert_eq!(checks, 2);
        assert!(!media.join(&name).exists());
        let bytes = vec![37; MAX_READ + 1];
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let name = format!("denied-file-{index}");
        checks = 0;
        assert!(
            client
                .create_file_checked(&mut root, &name, bytes.len() as u64, &hash, &mut || {
                    checks += 1;
                    if checks == 2 {
                        Err(Error::Conflict("generated claim expired".into()))
                    } else {
                        Ok(())
                    }
                })
                .await
                .is_err()
        );
        assert_eq!(checks, 2);
        assert!(!media.join(&name).exists());
        let name = format!("complete-unsealed-{index}");
        let mut output = client
            .create_file(&mut root, &name, bytes.len() as u64, &hash)
            .await?;
        for chunk in bytes.chunks(MAX_READ) {
            client.write(&mut output, chunk).await?;
        }
        checks = 0;
        assert!(
            client
                .seal_checked(output, &mut || {
                    checks += 1;
                    if checks == 2 {
                        Err(Error::Conflict("generated claim expired".into()))
                    } else {
                        Ok(())
                    }
                })
                .await
                .is_err()
        );
        assert_eq!(checks, 2);
        client.close_directory(root).await?;
        assert_eq!(std::fs::read(media.join(name))?, bytes);
    }
    running.stop().await
}

#[tokio::test]
async fn media_adapter_removal_native_and_tls_delete_only_verified_files_and_empty_directories()
-> TestResult {
    use sha2::{Digest, Sha256};
    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    for index in 0..2 {
        std::fs::write(media.join(format!("zero-{index}")), [])?;
    }
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let native = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let bytes = vec![37; MAX_READ + 3];
    let hash = format!("{:x}", Sha256::digest(&bytes));
    for (index, mut client) in [native, remote].into_iter().enumerate() {
        let mut root = client.open_root("library").await?;
        let page = client.page(&root, MAX_PAGE).await?;
        let zero = format!("zero-{index}");
        let listed = page
            .entries
            .iter()
            .find(|entry| entry.name == zero)
            .ok_or("generated empty file missing")?;
        let source = client
            .prepare_removal(&root, listed, 0, &format!("{:x}", Sha256::digest([])))
            .await?;
        client.remove_file(&mut root, source).await?;
        assert!(!media.join(zero).exists());
        let name = format!("owned-{index}");
        let mut owned = client.create_directory(&mut root, &name).await?;
        for file in ["remove.flac", "keep.flac"] {
            let mut output = client
                .create_file(&mut owned, file, bytes.len() as u64, &hash)
                .await?;
            for chunk in bytes.chunks(MAX_READ) {
                client.write(&mut output, chunk).await?;
            }
            client.close_output(output).await?;
        }
        let page = client.page(&owned, MAX_PAGE).await?;
        let listed = page
            .entries
            .iter()
            .find(|entry| entry.name == "remove.flac")
            .ok_or("generated removal file missing")?;
        let source = client
            .prepare_removal(&owned, listed, bytes.len() as u64, &hash)
            .await?;
        assert_eq!(source.observation(), &listed.observed);
        #[cfg(windows)]
        {
            assert!(std::fs::write(media.join(&name).join("remove.flac"), b"changed").is_err());
            assert!(
                std::fs::rename(
                    media.join(&name).join("remove.flac"),
                    media.join(&name).join("swapped")
                )
                .is_err()
            );
        }
        let mut checks = 0;
        client
            .remove_file_checked(&mut owned, source, &mut || {
                checks += 1;
                Ok(())
            })
            .await?;
        assert_eq!(checks, 1);
        assert!(!media.join(&name).join("remove.flac").exists());
        let page = client.page(&owned, MAX_PAGE).await?;
        assert!(page.complete);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].name, "keep.flac");
        let source = client
            .prepare_removal(&owned, &page.entries[0], bytes.len() as u64, &hash)
            .await?;
        assert!(
            client
                .remove_file_checked(&mut owned, source, &mut || Err(Error::Conflict(
                    "generated approval withdrawn".into()
                )))
                .await
                .is_err()
        );
        client.close_directory(owned).await?;
        assert_eq!(std::fs::read(media.join(&name).join("keep.flac"))?, bytes);
        let name = format!("empty-{index}");
        let empty = client.create_directory(&mut root, &name).await?;
        client.close_directory(empty).await?;
        let page = client.page(&root, MAX_PAGE).await?;
        let listed = page
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or("generated empty directory missing")?;
        let source = client.prepare_move(&root, listed).await?;
        client.remove_directory(&mut root, source).await?;
        assert!(!media.join(&name).exists());
        assert!(media.join("album").join("track.flac").is_file());
        client.close_directory(root).await?;
    }
    running.stop().await
}

#[tokio::test]
async fn media_adapter_publication_snapshot_native_and_tls_share_exact_bounded_read_policy()
-> TestResult {
    use super::super::{BlockingMedia, io::MediaDirectoryIo};
    use crate::domain::LibraryPath;
    use crate::infrastructure::{
        filesystem::{LibraryDirectory, native_file_identity},
        publication::media_replacement_snapshot,
    };
    use sha2::Digest;
    let fixture = Fixture::new()?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let bytes = vec![47; MAX_READ + 19];
    std::fs::write(media.join("album").join("track.flac"), &bytes)?;
    std::fs::write(media.join("album").join("empty.bin"), [])?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let native = MediaDirectoryIo::native(LibraryDirectory::open(&media)?)?;
    let local = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        let local = MediaDirectoryIo::paired(BlockingMedia::new(local, "library".into())?)?;
        let remote = MediaDirectoryIo::paired(BlockingMedia::new(remote, "library".into())?)?;
        let mut expected = None;
        for root in [native, local, remote] {
            assert!(
                root.open_directory(&LibraryPath::parse("missing")?)?
                    .is_none()
            );
            let album = root
                .open_directory(&LibraryPath::parse("album")?)?
                .ok_or(Error::Internal("generated album missing"))?;
            let snapshot = media_replacement_snapshot(&album)?;
            let actual = LibraryDirectory::open(&media.join("album"))?.directory_file()?;
            // Compare file contents without equating native and paired identity namespaces.
            let identity = match &root {
                MediaDirectoryIo::Native(_) => native_file_identity(&actual)?,
                MediaDirectoryIo::Paired(_) => {
                    crate::infrastructure::filesystem::binding::persistent_file_identity(&actual)?
                }
            };
            assert_eq!(snapshot.0, identity);
            assert_eq!(snapshot.1.len(), 2);
            assert_eq!(snapshot.1[0].name, "empty.bin");
            assert_eq!(snapshot.1[0].bytes, 0);
            assert_eq!(snapshot.1[1].bytes, bytes.len() as u64);
            assert_eq!(
                snapshot.1[1].sha256,
                format!("{:x}", sha2::Sha256::digest(&bytes))
            );
            assert_eq!(media_replacement_snapshot(&album)?, snapshot);
            if let Some(expected) = &expected {
                assert_eq!(&snapshot.1, expected);
            } else {
                expected = Some(snapshot.1);
            }
        }
        Ok(())
    })
    .await?;
    result?;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_named_ancestry_native_and_tls_keeps_one_bounded_chain() -> TestResult {
    use super::super::{BlockingMedia, io::MediaDirectoryIo};
    use crate::domain::LibraryPath;
    use crate::infrastructure::filesystem::LibraryDirectory;
    use std::cell::Cell;

    let fixture = Fixture::new()?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let nested = vec!["n"; super::super::MAX_LIVE - 3].join("/");
    for profile in 0..3 {
        std::fs::create_dir_all(
            media
                .join(format!("scope-{profile}"))
                .join(nested.replace('/', std::path::MAIN_SEPARATOR_STR)),
        )?;
    }
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let local = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        for (profile, client) in [None, Some(local), Some(remote)].into_iter().enumerate() {
            let root = match client {
                None => MediaDirectoryIo::native(LibraryDirectory::open(&media)?)?,
                Some(client) => {
                    MediaDirectoryIo::paired(BlockingMedia::new(client, "library".into())?)?
                }
            };
            let path = LibraryPath::parse(&format!("scope-{profile}/{nested}"))?;
            let directory = root
                .open_directory(&path)?
                .ok_or(Error::Internal("generated named directory missing"))?;
            directory.check_named_directory(&|| Ok(()))?;
            let checks = Cell::new(0);
            assert!(matches!(
                directory.check_named_directory(&|| {
                    checks.set(checks.get() + 1);
                    if checks.get() == 2 {
                        Err(Error::WorkerStopping)
                    } else {
                        Ok(())
                    }
                }),
                Err(Error::WorkerStopping)
            ));
            directory.check_named_directory(&|| Ok(()))?;

            let moved_media = media.with_file_name(format!("moved-media-{profile}"));
            match std::fs::rename(&media, &moved_media) {
                Ok(()) => {
                    root.check()?;
                    assert!(root.check_named_directory(&|| Ok(())).is_err());
                    std::fs::rename(&moved_media, &media)?;
                }
                #[cfg(windows)]
                Err(error)
                    if error.kind() == std::io::ErrorKind::PermissionDenied
                        || error.raw_os_error() == Some(32) =>
                {
                    assert!(media.is_dir());
                    assert!(!moved_media.exists());
                    root.check_named_directory(&|| Ok(()))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    })
    .await?;
    result?;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_publication_sync_native_and_tls_rejects_changed_or_failed_sources()
-> TestResult {
    use super::super::{BlockingMedia, io::MediaDirectoryIo};
    use crate::infrastructure::filesystem::LibraryDirectory;

    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let local = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let native = LibraryDirectory::open(&media)?;
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        let roots = [
            MediaDirectoryIo::native(native)?,
            MediaDirectoryIo::paired(BlockingMedia::new(local, "library".into())?)?,
            MediaDirectoryIo::paired(BlockingMedia::new(remote, "library".into())?)?,
        ];
        for (profile, root) in roots.into_iter().enumerate() {
            root.sync_checked(&|| Ok(())).map_err(|error| {
                std::io::Error::other(format!("generated sync profile {profile}: {error}"))
            })?;
            let Err(error) =
                root.sync_checked(&|| Err(std::io::Error::other("sync approval withdrawn")))
            else {
                return Err(Error::Internal("sync ignored its application checkpoint"));
            };
            assert!(error.to_string().contains("sync approval withdrawn"));
        }
        Ok(())
    })
    .await?;
    result?;

    for (index, remote) in [false, true].into_iter().enumerate() {
        let connect = || -> Result<MediaClient> {
            if remote {
                MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)
            } else {
                MediaClient::local(fixture.selection.clone(), Vec::new())
            }
        };
        let mut client = connect()?;
        let root = client.open_root("library").await?;
        client.check_named_directory(&root).await?;
        let album = client
            .page(&root, MAX_PAGE)
            .await?
            .entries
            .into_iter()
            .find(|entry| entry.name == "album")
            .ok_or(Error::Internal("generated sync album missing"))?;
        let source = client.prepare_move(&root, &album).await.map_err(|error| {
            std::io::Error::other(format!("generated move profile {index}: {error}"))
        })?;
        assert!(matches!(
            client.check_named_directory(source.directory()).await,
            Err(Error::Conflict(_))
        ));
        let Err(error) = client
            .sync_directory_checked(source.directory(), &mut || Ok(()))
            .await
        else {
            return Err(Error::Internal("reviewed move source authorized sync").into());
        };
        assert!(matches!(error, Error::Conflict(message) if message.contains("read-only")));
        client.close_move(source).await?;
        std::fs::create_dir(media.join(format!("changed-sync-{index}")))?;
        assert!(
            client
                .sync_directory_checked(&root, &mut || Ok(()))
                .await
                .is_err()
        );
        drop(client);

        let alias = media.join(format!("unsafe-sync-{index}"));
        super::super::tests::directory_alias(
            &alias,
            &fixture.selection.provider.boundary.join("private"),
        )?;
        let mut client = connect()?;
        let root = client.open_root("library").await?;
        assert!(client.page(&root, MAX_PAGE).await.is_err());
        assert!(client.check_named_directory(&root).await.is_err());
        assert!(
            client
                .sync_directory_checked(&root, &mut || Ok(()))
                .await
                .is_err()
        );
        drop(client);
        #[cfg(windows)]
        std::fs::remove_dir(alias)?;
        #[cfg(unix)]
        std::fs::remove_file(alias)?;
    }
    running.stop().await
}

#[tokio::test]
async fn media_adapter_recovery_removal_native_and_tls_preserves_changed_and_nonempty_sources()
-> TestResult {
    use super::super::{BlockingMedia, io::MediaDirectoryIo};
    use crate::domain::LibraryPath;
    use crate::infrastructure::filesystem::LibraryDirectory;
    use crate::infrastructure::store::ReplacementFile;
    use sha2::{Digest, Sha256};
    use std::cell::Cell;

    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let bytes = vec![61; MAX_READ * 2 + 19];
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let mut clients = Vec::new();
    for profile in 0..3 {
        for case in 0..6 {
            let folder = media.join(format!("removal-{profile}-{case}"));
            std::fs::create_dir(&folder)?;
            if case != 5 {
                std::fs::write(folder.join("track.flac"), &bytes)?;
            }
            let client = match profile {
                0 => None,
                1 => Some(MediaClient::local(fixture.selection.clone(), Vec::new())?),
                _ => Some(MediaClient::remote(
                    fixture.selection.logical(),
                    keys.client(running.address)?,
                )?),
            };
            clients.push((profile, case, client));
        }
    }
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        for (profile, case, client) in clients {
            let root = match client {
                None => MediaDirectoryIo::native(LibraryDirectory::open(&media)?)?,
                Some(client) => {
                    MediaDirectoryIo::paired(BlockingMedia::new(client, "library".into())?)?
                }
            };
            let name = format!("removal-{profile}-{case}");
            let folder = media.join(&name);
            let directory = root
                .open_directory(&LibraryPath::parse(&name)?)?
                .ok_or(Error::Internal("generated recovery source missing"))?;
            let identity = directory.identity().to_owned();
            if case < 4 {
                let manifest = vec![ReplacementFile {
                    name: "track.flac".into(),
                    bytes: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                }];
                let (inspected_identity, inspected) = directory.inspect_recovery(&manifest)?;
                assert_eq!(inspected_identity, identity);
                let mut expected = directory.recovery_file("track.flac")?;
                assert_eq!(inspected, vec![expected.clone()]);
                assert_eq!(expected.bytes, bytes.len() as u64);
                assert_eq!(expected.sha256, manifest[0].sha256);
                assert!(directory.recovery_entry("missing.flac")?.is_none());
                assert!(directory.inspect_recovery(&[]).is_err());
                assert!(
                    directory
                        .inspect_recovery(&vec![manifest[0].clone(); 1003])
                        .is_err()
                );
                let mut wrong = manifest.clone();
                wrong[0].sha256 = "0".repeat(64);
                assert!(directory.inspect_recovery(&wrong).is_err());
                assert!(
                    directory
                        .inspect_recovery(&[manifest[0].clone(), manifest[0].clone()])
                        .is_err()
                );
                if case == 1 {
                    expected.identity = "changed-identity".into();
                } else if case == 2 {
                    expected.sha256 = "0".repeat(64);
                }
                let calls = Cell::new(0);
                let result = directory.remove_recovery_file(&expected, &|| {
                    calls.set(calls.get() + 1);
                    if case == 3 && calls.get() >= 4 {
                        Err(std::io::Error::other("generated approval withdrawn"))
                    } else {
                        Ok(())
                    }
                });
                drop(directory);
                if case == 0 {
                    result?;
                    assert!(!folder.join("track.flac").exists());
                    root.remove_recovery_directory(&name, &identity, &|| Ok(()))?;
                    assert!(!folder.exists());
                } else {
                    assert!(result.is_err(), "profile={profile}, case={case}");
                    assert_eq!(std::fs::read(folder.join("track.flac"))?, bytes);
                }
            } else {
                drop(directory);
                let expected = if case == 5 {
                    "changed-identity"
                } else {
                    &identity
                };
                assert!(
                    root.remove_recovery_directory(&name, expected, &|| Ok(()))
                        .is_err(),
                    "profile={profile}, case={case}"
                );
                assert!(folder.is_dir());
                if case == 4 {
                    assert_eq!(std::fs::read(folder.join("track.flac"))?, bytes);
                }
            }
        }
        Ok(())
    })
    .await?;
    result?;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_publication_moves_native_and_tls_require_the_exact_reviewed_source()
-> TestResult {
    use super::super::{BlockingMedia, io::MediaDirectoryIo};
    use crate::domain::LibraryPath;
    use crate::infrastructure::filesystem::LibraryDirectory;

    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let bytes = vec![37; MAX_READ * 2 + 19];
    let mut clients = Vec::new();
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    for profile in 0..3 {
        for case in 0..5 {
            let source = media.join(format!("source-{profile}-{case}"));
            std::fs::create_dir(&source)?;
            std::fs::write(source.join("track.flac"), &bytes)?;
            if case == 3 {
                let target = media.join(format!("target-{profile}-{case}"));
                std::fs::create_dir(&target)?;
                std::fs::write(target.join("sentinel"), b"keep")?;
            }
            let client = match profile {
                0 => None,
                1 => Some(MediaClient::local(fixture.selection.clone(), Vec::new())?),
                _ => Some(MediaClient::remote(
                    fixture.selection.logical(),
                    keys.client(running.address)?,
                )?),
            };
            clients.push((profile, case, client));
        }
    }
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        for (profile, case, client) in clients {
            let root = match client {
                None => MediaDirectoryIo::native(LibraryDirectory::open(&media)?)?,
                Some(client) => {
                    MediaDirectoryIo::paired(BlockingMedia::new(client, "library".into())?)?
                }
            };
            let from = format!("source-{profile}-{case}");
            let to = format!("target-{profile}-{case}");
            let directory = root
                .open_directory(&LibraryPath::parse(&from)?)?
                .ok_or(Error::Internal("generated move source missing"))?;
            let (mut identity, mut files) = directory.replacement_snapshot(&|| Ok(()))?;
            drop(directory);
            if case == 1 {
                identity = "changed-identity".into();
            } else if case == 2 {
                files[0].sha256 = "0".repeat(64);
            }
            let result = root.rename_directory(&from, &to, &identity, &files, &|| {
                if case == 4 {
                    Err(std::io::Error::other("generated approval withdrawn"))
                } else {
                    Ok(())
                }
            });
            if case == 0 {
                result?;
                assert!(!media.join(&from).exists());
                assert_eq!(std::fs::read(media.join(&to).join("track.flac"))?, bytes);
                let moved = root
                    .open_directory(&LibraryPath::parse(&to)?)?
                    .ok_or(Error::Internal("generated moved folder missing"))?;
                assert_eq!(moved.identity(), identity);
            } else {
                assert!(result.is_err(), "profile={profile}, case={case}");
                assert_eq!(std::fs::read(media.join(&from).join("track.flac"))?, bytes);
                if case == 3 {
                    assert_eq!(std::fs::read(media.join(&to).join("sentinel"))?, b"keep");
                } else {
                    assert!(!media.join(&to).exists());
                }
            }
        }
        Ok(())
    })
    .await?;
    result?;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_directory_creation_native_and_tls_keep_original_ancestry() -> TestResult {
    use super::super::{BlockingMedia, io::MediaDirectoryIo};
    use crate::infrastructure::{filesystem::LibraryDirectory, store::PublicationFile};
    use sha2::{Digest, Sha256};
    use std::cell::Cell;
    use std::io::{Seek, SeekFrom, Write};

    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    for index in 0..MAX_PAGE * 3 {
        std::fs::write(media.join(format!("unrelated-{index}")), [])?;
    }
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let local = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        for (index, client) in [None, Some(local), Some(remote)].into_iter().enumerate() {
            let root = match client {
                None => MediaDirectoryIo::native(LibraryDirectory::open(&media)?)?,
                Some(client) => {
                    MediaDirectoryIo::paired(BlockingMedia::new(client, "library".into())?)?
                }
            };
            let name = format!("created-{index}");
            let folder = root.create_directory(&name, &|| Ok(()))?;
            let child = folder.create_directory("Disc", &|| Ok(()))?;
            assert!(root.create_directory(&name, &|| Ok(())).is_err());
            assert!(
                root.create_directory(&name.to_uppercase(), &|| Ok(()))
                    .is_err()
            );
            for invalid in ["", "../escape", "a/b", "a\\b", "track:stream"] {
                assert!(root.create_directory(invalid, &|| Ok(())).is_err());
            }
            let checks = Cell::new(0);
            assert!(
                root.create_directory("cancelled-folder", &|| {
                    checks.set(checks.get() + 1);
                    if checks.get() == 3 {
                        Err(std::io::Error::other("generated claim expired"))
                    } else {
                        Ok(())
                    }
                })
                .is_err()
            );
            assert!(!media.join("cancelled-folder").exists());
            drop(root);
            drop(folder);
            child.check()?;
            #[cfg(windows)]
            assert!(std::fs::rename(media.join(&name), media.join("moved")).is_err());
            let bytes = b"generated folder output";
            let mut source = tempfile::tempfile()?;
            source.write_all(bytes)?;
            source.seek(SeekFrom::Start(0))?;
            let entry = PublicationFile {
                ordinal: 1,
                name: "track.flac".into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
                verified_at: None,
            };
            child.copy_publication_file(&entry, &mut source, &|| Ok(()))?;
            child.verify_publication_files(&[entry], &|| Ok(()))?;
            assert_eq!(
                std::fs::read(media.join(name).join("Disc").join("track.flac"))?,
                bytes
            );
        }
        Ok(())
    })
    .await?;
    result?;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_publication_copy_native_and_tls_preserve_exact_and_partial_outputs()
-> TestResult {
    use super::super::{BlockingMedia, io::MediaDirectoryIo};
    use crate::domain::LibraryPath;
    use crate::infrastructure::{filesystem::LibraryDirectory, store::PublicationFile};
    use sha2::{Digest, Sha256};
    use std::io::{Seek, SeekFrom, Write};

    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    for index in 0..3 {
        std::fs::create_dir(media.join(format!("copy-{index}")))?;
    }
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let native = MediaDirectoryIo::native(LibraryDirectory::open(&media)?)?;
    let local = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        let local = MediaDirectoryIo::paired(BlockingMedia::new(local, "library".into())?)?;
        let remote = MediaDirectoryIo::paired(BlockingMedia::new(remote, "library".into())?)?;
        let bytes = vec![29; MAX_READ * 2 + 19];
        let mut source = tempfile::tempfile()?;
        source.write_all(&bytes)?;
        for (index, root) in [native, local, remote].into_iter().enumerate() {
            let folder = media.join(format!("copy-{index}"));
            let directory = root
                .open_directory(&LibraryPath::parse(&format!("copy-{index}"))?)?
                .ok_or(Error::Internal("generated output directory missing"))?;
            let mut entry = PublicationFile {
                ordinal: 1,
                name: "track.flac".into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                verified_at: None,
            };
            source.seek(SeekFrom::Start(0))?;
            directory.copy_publication_file(&entry, &mut source, &|| Ok(()))?;
            assert_eq!(std::fs::read(folder.join(&entry.name))?, bytes);
            directory.verify_publication_files(&[entry.clone()], &|| Ok(()))?;

            // Existing exact bytes close the sync-before-receipt gap without
            // consuming replacement input or opening another write stream.
            let mut empty_source = tempfile::tempfile()?;
            directory.copy_publication_file(&entry, &mut empty_source, &|| Ok(()))?;
            entry.verified_at = Some(1);
            directory.copy_publication_file(&entry, &mut empty_source, &|| Ok(()))?;
            assert_eq!(std::fs::read(folder.join(&entry.name))?, bytes);

            let mut missing = entry.clone();
            missing.name = "missing.flac".into();
            assert!(
                directory
                    .copy_publication_file(&missing, &mut source, &|| Ok(()))
                    .is_err()
            );
            assert!(!folder.join(&missing.name).exists());

            let mut collision = entry.clone();
            collision.name = "TRACK.FLAC".into();
            collision.verified_at = None;
            assert!(
                directory
                    .copy_publication_file(&collision, &mut source, &|| Ok(()))
                    .is_err()
            );
            assert_eq!(std::fs::read(folder.join(&entry.name))?, bytes);

            let mut partial = entry.clone();
            partial.name = "partial.flac".into();
            partial.verified_at = None;
            let mut short = tempfile::tempfile()?;
            short.write_all(&bytes[..MAX_READ + 3])?;
            short.seek(SeekFrom::Start(0))?;
            assert!(
                directory
                    .copy_publication_file(&partial, &mut short, &|| Ok(()))
                    .is_err()
            );
            let interrupted = std::fs::read(folder.join(&partial.name))?;
            source.seek(SeekFrom::Start(0))?;
            assert!(
                directory
                    .copy_publication_file(&partial, &mut source, &|| Ok(()))
                    .is_err()
            );
            assert_eq!(std::fs::read(folder.join(&partial.name))?, interrupted);
            assert!(
                directory
                    .verify_publication_files(&[entry.clone()], &|| Ok(()))
                    .is_err()
            );

            let mut cancelled = entry.clone();
            cancelled.name = "cancelled.flac".into();
            cancelled.verified_at = None;
            source.seek(SeekFrom::Start(0))?;
            assert!(
                directory
                    .copy_publication_file(&cancelled, &mut source, &|| {
                        if folder.join(&cancelled.name).exists() {
                            Err(std::io::Error::other("generated approval withdrawn"))
                        } else {
                            Ok(())
                        }
                    })
                    .is_err()
            );
            assert!(std::fs::read(folder.join(&cancelled.name))?.is_empty());
        }
        Ok(())
    })
    .await?;
    result?;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_publication_copy_keeps_read_only_grants_for_existing_and_missing_files()
-> TestResult {
    use super::super::{BlockingMedia, io::MediaDirectoryIo};
    use crate::domain::LibraryPath;
    use crate::infrastructure::store::PublicationFile;
    use sha2::{Digest, Sha256};

    let fixture = Fixture::new()?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media")
        .join("album");
    let bytes = std::fs::read(media.join("track.flac"))?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let local = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        for client in [local, remote] {
            let root = MediaDirectoryIo::paired(BlockingMedia::new(client, "library".into())?)?;
            let directory = root
                .open_directory(&LibraryPath::parse("album")?)?
                .ok_or(Error::Internal("generated read-only album missing"))?;
            let Err(error) = directory.sync_checked(&|| Ok(())) else {
                return Err(Error::Internal("read-only root authorized sync"));
            };
            assert!(matches!(error, Error::Conflict(message) if message.contains("read-only")));
            assert!(
                directory
                    .create_directory("missing-directory", &|| Ok(()))
                    .is_err()
            );
            assert!(!media.join("missing-directory").exists());
            let (identity, manifest) = directory.replacement_snapshot(&|| Ok(()))?;
            let Err(error) =
                root.rename_directory("album", "renamed-album", &identity, &manifest, &|| Ok(()))
            else {
                return Err(Error::Internal("read-only root authorized a move"));
            };
            assert!(matches!(error, Error::Conflict(message) if message.contains("read-only")));
            assert!(!media.with_file_name("renamed-album").exists());
            let file = std::fs::File::open(media.join("track.flac"))?;
            let expected = crate::infrastructure::store::RecoveryFile {
                name: "track.flac".into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                identity: crate::infrastructure::filesystem::binding::persistent_file_identity(
                    &file,
                )?,
            };
            drop(file);
            assert!(
                directory
                    .remove_recovery_file(&expected, &|| Ok(()))
                    .is_err()
            );
            assert!(
                root.remove_recovery_directory("album", &identity, &|| Ok(()))
                    .is_err()
            );
            assert_eq!(std::fs::read(media.join("track.flac"))?, bytes);
            let mut entry = PublicationFile {
                ordinal: 1,
                name: "track.flac".into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                verified_at: None,
            };
            let mut source = tempfile::tempfile()?;
            assert!(
                directory
                    .copy_publication_file(&entry, &mut source, &|| Ok(()))
                    .is_err()
            );
            assert!(
                directory
                    .verify_publication_files(&[entry.clone()], &|| Ok(()))
                    .is_err()
            );
            entry.name = "missing.flac".into();
            assert!(
                directory
                    .copy_publication_file(&entry, &mut source, &|| Ok(()))
                    .is_err()
            );
            assert!(!media.join("missing.flac").exists());
            assert_eq!(std::fs::read(media.join("track.flac"))?, bytes);
        }
        Ok(())
    })
    .await?;
    result?;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_verify_native_and_tls_recover_only_complete_exact_existing_files()
-> TestResult {
    use sha2::{Digest, Sha256};
    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    std::fs::write(media.join("empty"), [])?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let bytes = vec![37; MAX_READ + 3];
    let hash = format!("{:x}", Sha256::digest(&bytes));
    for (index, remote) in [false, true].into_iter().enumerate() {
        let connect = || -> Result<MediaClient> {
            if remote {
                MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)
            } else {
                MediaClient::local(fixture.selection.clone(), Vec::new())
            }
        };
        let mut client = connect()?;
        let mut root = client.open_root("library").await?;
        let complete = format!("complete-{index}");
        let partial = format!("partial-{index}");
        let different = format!("different-{index}");
        for (name, body) in [
            (&complete, bytes.clone()),
            (&partial, vec![7; 3]),
            (&different, vec![55; bytes.len()]),
        ] {
            let mut output = client
                .create_file(&mut root, name, bytes.len() as u64, &hash)
                .await?;
            for chunk in body.chunks(MAX_READ) {
                client.write(&mut output, chunk).await?;
            }
            client.close_output(output).await?;
        }
        client.close_directory(root).await?;
        drop(client);
        let mut client = connect()?;
        let root = client.open_root("library").await?;
        let page = client.page(&root, MAX_PAGE).await?;
        assert!(page.complete);
        let entry = |name: &str| {
            page.entries
                .iter()
                .find(|entry| entry.name == name)
                .ok_or("generated recovery file missing")
        };
        let listed = entry(&complete)?;
        let mut stale = listed.clone();
        stale.observed.modified_nanoseconds =
            (stale.observed.modified_nanoseconds + 1) % 1_000_000_000;
        assert!(
            client
                .verify_file(&root, &stale, bytes.len() as u64, &hash)
                .await
                .is_err()
        );
        assert!(
            client
                .verify_file(&root, entry(&partial)?, bytes.len() as u64, &hash)
                .await
                .is_err()
        );
        let mut checks = 0;
        let file = client
            .verify_file_checked(&root, listed, bytes.len() as u64, &hash, &mut || {
                checks += 1;
                Ok(())
            })
            .await?;
        assert_eq!(checks, 3);
        assert_eq!(file.observation(), &listed.observed);
        assert_eq!(
            client.read(&file, MAX_READ as u64, MAX_READ).await?,
            [37; 3]
        );
        client.close_file(file).await?;
        let alias = fixture
            .selection
            .provider
            .boundary
            .join(format!("verification-alias-{index}"));
        std::fs::hard_link(media.join(&complete), &alias)?;
        assert!(
            client
                .verify_file(&root, listed, bytes.len() as u64, &hash)
                .await
                .is_err()
        );
        std::fs::remove_file(alias)?;
        let empty = client
            .verify_file(
                &root,
                entry("empty")?,
                0,
                &format!("{:x}", Sha256::digest([])),
            )
            .await?;
        assert!(client.read(&empty, 0, MAX_READ).await?.is_empty());
        client.close_file(empty).await?;
        assert!(
            client
                .verify_file(&root, entry(&different)?, bytes.len() as u64, &hash)
                .await
                .is_err()
        );
        client.close_directory(root).await?;
        assert_eq!(std::fs::read(media.join(&complete))?, bytes);
        assert_eq!(std::fs::read(media.join(&partial))?, [7; 3]);
        assert_eq!(
            std::fs::read(media.join(&different))?,
            vec![55; bytes.len()]
        );
    }
    running.stop().await
}

#[tokio::test]
async fn media_adapter_write_typed_native_and_tls_streams_preserve_exact_outputs() -> TestResult {
    use sha2::{Digest, Sha256};
    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    for index in 0..MAX_PAGE + 3 {
        std::fs::write(media.join(format!("existing-{index}.flac")), [4])?;
    }
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let native = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let bytes = vec![37; MAX_READ + 11];
    let digest = format!("{:x}", Sha256::digest(&bytes));
    for (index, mut client) in [native, remote].into_iter().enumerate() {
        let mut root = client.open_root("library").await?;
        // Consume the cursor before creation, then require a fresh listing of
        // the changed parent rather than a stale complete/enumerating cursor.
        assert!(!client.page(&root, MAX_PAGE).await?.complete);
        assert!(client.page(&root, MAX_PAGE).await?.complete);
        let name = format!(".domus-pending-write-{index}");
        let mut directory = client.create_directory(&mut root, &name).await?;
        let mut found = false;
        loop {
            let page = client.page(&root, MAX_PAGE).await?;
            found |= page.entries.iter().any(|entry| entry.name == name);
            if page.complete {
                break;
            }
        }
        assert!(found);
        assert!(
            client
                .create_directory(&mut root, &name.to_uppercase())
                .await
                .is_err()
        );
        assert!(
            client
                .create_file(&mut directory, "invalid", 0, &digest)
                .await
                .is_err()
        );
        let mut output = client
            .create_file(&mut directory, "track.flac", bytes.len() as u64, &digest)
            .await?;
        assert!(client.write(&mut output, &[]).await.is_err());
        assert!(
            client
                .write(&mut output, &vec![0; MAX_READ + 1])
                .await
                .is_err()
        );
        for chunk in bytes.chunks(MAX_READ) {
            client.write(&mut output, chunk).await?;
        }
        assert!(client.write(&mut output, &[0]).await.is_err());
        let file = client.seal(output).await?;
        assert_eq!(file.observation().links, 1);
        assert_eq!(file.observation().length, bytes.len() as u64);
        assert_eq!(client.read(&file, 0, MAX_READ).await?, bytes[..MAX_READ]);
        assert_eq!(
            client.read(&file, MAX_READ as u64, MAX_READ).await?,
            bytes[MAX_READ..]
        );
        client.close_file(file).await?;
        let mut partial = client
            .create_file(&mut directory, "partial.flac", bytes.len() as u64, &digest)
            .await?;
        client.write(&mut partial, &[8, 9]).await?;
        client.close_output(partial).await?;
        assert!(
            client
                .create_file(&mut directory, "partial.flac", bytes.len() as u64, &digest)
                .await
                .is_err()
        );
        let mut wrong = client
            .create_file(&mut directory, "wrong.flac", bytes.len() as u64, &digest)
            .await?;
        for chunk in vec![99; bytes.len()].chunks(MAX_READ) {
            client.write(&mut wrong, chunk).await?;
        }
        assert!(client.seal(wrong).await.is_err());
        client.close_directory(root).await?;
        let path = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media")
            .join(name);
        assert_eq!(std::fs::read(path.join("track.flac"))?, bytes);
        assert_eq!(std::fs::read(path.join("partial.flac"))?, [8, 9]);
        assert_eq!(
            std::fs::read(path.join("wrong.flac"))?,
            vec![99; bytes.len()]
        );
    }
    running.stop().await
}

#[tokio::test]
async fn media_adapter_write_typed_tls_defaults_deny_and_disconnect_never_reopens() -> TestResult {
    use sha2::{Digest, Sha256};
    let mut fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let mut client =
        MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let mut root = client.open_root("library").await?;
    assert!(matches!(
        client.create_directory(&mut root, "denied").await,
        Err(Error::Conflict(_))
    ));
    client.close_directory(root).await?;
    drop(client);
    running.stop().await?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    assert!(!media.join("denied").exists());
    fixture.selection.writable_roots.push("library".into());
    let running = Running::start(&fixture, &keys)?;
    let mut client =
        MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let mut root = client.open_root("library").await?;
    let digest = format!("{:x}", Sha256::digest([7; 6]));
    let mut output = client
        .create_file(&mut root, "interrupted.flac", 6, &digest)
        .await?;
    client.write(&mut output, &[7; 3]).await?;
    running.stop().await?;
    assert!(client.write(&mut output, &[7; 3]).await.is_err());
    drop(client);
    let running = Running::start(&fixture, &keys)?;
    let mut client =
        MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let mut root = client.open_root("library").await?;
    assert!(
        client
            .create_file(&mut root, "interrupted.flac", 6, &digest)
            .await
            .is_err()
    );
    client.close_directory(root).await?;
    running.stop().await?;
    assert_eq!(std::fs::read(media.join("interrupted.flac"))?, [7; 3]);
    Ok(())
}

#[tokio::test]
async fn media_adapter_typed_client_keeps_native_and_tls_read_semantics_equal() -> TestResult {
    let fixture = Fixture::new()?;
    let bytes = (0..MAX_READ + 3)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    std::fs::write(
        fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media")
            .join("album")
            .join("track.flac"),
        &bytes,
    )?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let native = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let remote = MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    for mut client in [native, remote] {
        assert!(client.open_root("unselected").await.is_err());
        let root = client.open_root("library").await?;
        let file = media_client_file(&mut client, &root).await?;
        assert_eq!(file.observation().length, bytes.len() as u64);
        assert_eq!(client.read(&file, 0, MAX_READ).await?, bytes[..MAX_READ]);
        assert_eq!(
            client.read(&file, MAX_READ as u64, MAX_READ).await?,
            bytes[MAX_READ..]
        );
        assert!(client.read(&file, bytes.len() as u64, 1).await?.is_empty());
        assert!(client.read(&file, 0, MAX_READ + 1).await.is_err());
        assert!(client.read(&file, bytes.len() as u64 + 1, 1).await.is_err());
        assert!(client.page(&root, 64).await.is_err());
        client.close_directory(root).await?;
        assert!(matches!(
            client.read(&file, 0, 1).await,
            Err(Error::Conflict(_))
        ));
        let root = client.open_root("library").await?;
        client.close_directory(root).await?;
    }
    running.stop().await
}

#[tokio::test]
async fn media_adapter_typed_client_rejects_foreign_leases_and_bounds_open_roots() -> TestResult {
    let fixture = Fixture::new()?;
    let mut client = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let mut other = MediaClient::local(fixture.selection.clone(), Vec::new())?;
    let foreign = other.open_root("library").await?;
    assert!(matches!(
        client.page(&foreign, 1).await,
        Err(Error::Conflict(_))
    ));
    other.close_directory(foreign).await?;
    let mut roots = Vec::new();
    for _ in 0..super::super::MAX_LIVE {
        roots.push(client.open_root("library").await?);
    }
    assert!(matches!(
        client.open_root("library").await,
        Err(Error::Busy(_))
    ));
    for root in roots {
        client.close_directory(root).await?;
    }
    let root = client.open_root("library").await?;
    client.close_directory(root).await?;
    Ok(())
}

#[tokio::test]
async fn media_adapter_typed_client_reports_loss_without_a_local_fallback() -> TestResult {
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let mut client =
        MediaClient::remote(fixture.selection.logical(), keys.client(running.address)?)?;
    let root = client.open_root("library").await?;
    running.stop().await?;
    assert!(matches!(client.page(&root, 1).await, Err(Error::Remote(_))));
    assert!(matches!(
        client.open_root("library").await,
        Err(Error::Remote(_))
    ));
    assert!(fixture.selection.provider.boundary.is_dir());
    Ok(())
}

#[tokio::test]
async fn media_adapter_typed_client_rejects_short_success_and_false_eof() -> TestResult {
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    for false_eof in [false, true] {
        let adapter = PreparedAdapter::new(
            fixture.selection.clone(),
            Vec::new(),
            keys.server.as_bytes(),
            keys.server_key.as_bytes(),
            keys.ca.as_bytes(),
            keys.client.as_bytes(),
        )?;
        let root_identity = fixture.selection.media[0]
            .expected
            .components
            .last()
            .ok_or("missing fixture identity")?
            .clone();
        let router = Router::new().route(
            "/v1/media",
            post(move |Json(request): Json<Request>| {
                let identity = root_identity.clone();
                async move {
                    let reference = |id| Reference {
                        connection: "a".repeat(64),
                        id,
                    };
                    let response = match request {
                        Request::OpenRoot { .. } => Response::Opened {
                            reference: reference(1),
                            observed: super::super::Observation {
                                identity: identity.clone(),
                                kind: super::super::Kind::Directory,
                                length: 0,
                                modified_seconds: 1,
                                modified_nanoseconds: 0,
                                links: 1,
                            },
                        },
                        Request::Page { .. } => Response::Page {
                            entries: vec![super::super::ListedEntry {
                                name: "generated.flac".into(),
                                observed: super::super::Observation {
                                    identity,
                                    kind: super::super::Kind::File,
                                    length: 3,
                                    modified_seconds: 1,
                                    modified_nanoseconds: 0,
                                    links: 1,
                                },
                            }],
                            complete: true,
                        },
                        Request::OpenChild { expected, .. } => Response::Opened {
                            reference: reference(2),
                            observed: expected,
                        },
                        Request::Read { .. } => Response::Read {
                            bytes: if false_eof { vec![1, 2, 3] } else { vec![1] },
                            complete: !false_eof,
                        },
                        _ => Response::Closed,
                    };
                    Json(response)
                }
            }),
        );
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let handle = axum_server::Handle::new();
        let serving_handle = handle.clone();
        let task = tokio::spawn(server::serve_listener(
            listener,
            adapter.tls,
            router,
            serving_handle,
        ));
        let running = Running {
            address,
            handle,
            task,
        };
        let mut client = MediaClient::remote(fixture.selection.logical(), keys.client(address)?)?;
        let root = client.open_root("library").await?;
        let page = client.page(&root, 1).await?;
        let MediaNode::File(file) = client.open_child(&root, &page.entries[0]).await? else {
            return Err("expected mock peer file".into());
        };
        assert!(matches!(
            client.read(&file, 0, 3).await,
            Err(Error::Remote(_))
        ));
        assert!(matches!(
            client.open_root("library").await,
            Err(Error::Remote(_))
        ));
        drop(client);
        running.stop().await?;
    }
    Ok(())
}

fn configured_client_fixture(
    fixture: &Fixture,
    keys: &Certificates,
    address: SocketAddr,
) -> TestResult<(std::path::PathBuf, std::path::PathBuf, serde_json::Value)> {
    use crate::infrastructure::server::PreparedServer;
    let application = fixture.directory.path().join("client-application");
    let host = PreparedServer::initialize_local_at(&application)?;
    for (name, value) in [
        ("ca.pem", &keys.ca),
        ("server.pem", &keys.server),
        ("client.pem", &keys.client),
        ("key.pem", &keys.client_key),
    ] {
        std::fs::write(application.join(name), value)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                application.join(name),
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
    }
    let original = serde_json::json!({
        "format_version": 1,
        "address": address,
        "selection": fixture.selection.logical(),
        "server_ca_file": "ca.pem",
        "paired_server_file": "server.pem",
        "certificate_file": "client.pem",
        "private_key_file": "key.pem",
    });
    let path = application.join("media-client.json");
    std::fs::write(&path, serde_json::to_vec(&original)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok((host, path, original))
}

fn select_paired_fixture(
    host: &std::path::Path,
    client_path: &std::path::Path,
    selection: &super::super::MediaSelection,
) -> TestResult<serde_json::Value> {
    use crate::infrastructure::filesystem::{
        RootRevision, StorageConfiguration, StorageRole, TopologyAssurance,
    };
    let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(host)?)?;
    let local: StorageConfiguration = serde_json::from_value(document["storage"].clone())?;
    let digest = selection.storage_digest(&local)?;
    let mut assurances = Vec::new();
    for remote in selection
        .media
        .iter()
        .chain(std::iter::once(&selection.private))
    {
        for root in &local.roots {
            if remote.role != StorageRole::Media && root.role != StorageRole::Media {
                continue;
            }
            let revision = |root: &crate::infrastructure::filesystem::StorageRoot| RootRevision {
                root_id: root.root_id.clone(),
                binding_revision: root.binding_revision,
            };
            let (left, right) = if remote.root_id < root.root_id {
                (remote, root)
            } else {
                (root, remote)
            };
            assurances.push(TopologyAssurance {
                left: revision(left),
                right: revision(right),
                selection_digest: digest.clone(),
            });
        }
    }
    document["paired_media"] = serde_json::json!({
        "client_file": client_path,
        "topology_assurances": assurances,
    });
    document["system_settings_read_only"] = serde_json::json!(false);
    std::fs::write(host, serde_json::to_vec(&document)?)?;
    Ok(document)
}

#[tokio::test]
async fn media_adapter_paired_jpeg_requires_exact_index_evidence_and_byte_limit() -> TestResult {
    use crate::application::{library_jpeg_input, scan_location};
    use crate::domain::{LibraryPath, Page};
    use crate::infrastructure::server::configuration::ConfigurationService;
    use crate::infrastructure::store::StoreOpenMode;
    let fixture = Fixture::new()?;
    let album = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media")
        .join("album");
    let jpeg = [
        vec![0xff, 0xd8],
        vec![73; MAX_READ * 2 + 11],
        vec![0xff, 0xd9],
    ]
    .concat();
    std::fs::write(album.join("cover.jpg"), &jpeg)?;
    let oversized = std::fs::File::create(album.join("oversized.jpg"))?;
    oversized.set_len(8 * 1024 * 1024 + 1)?;
    drop(oversized);
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, _) = configured_client_fixture(&fixture, &keys, running.address)?;
    select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    let album_again = album.clone();
    let (database, location, entry) = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut store = source.open_store(StoreOpenMode::Current)?;
        let location = store.add_paired_location("library", "main")?;
        scan_location(&mut store, location)?;
        let entries = store.browse(location, &LibraryPath::parse("album")?, Page::new(0, 20)?)?;
        let id = |name| {
            entries
                .iter()
                .find(|entry| entry.relative_path == name)
                .map(|entry| entry.id)
                .ok_or(Error::Internal("missing JPEG fixture"))
        };
        let entry = id("album/cover.jpg")?;
        assert_eq!(library_jpeg_input(&store, location, entry)?, jpeg);
        assert!(library_jpeg_input(&store, location, id("album/track.flac")?).is_err());
        assert!(matches!(
            library_jpeg_input(&store, location, id("album/oversized.jpg")?),
            Err(Error::InvalidInput(_))
        ));
        std::fs::write(album_again.join("cover.jpg"), b"changed generated JPEG")?;
        assert!(library_jpeg_input(&store, location, entry).is_err());
        scan_location(&mut store, location)?;
        assert!(library_jpeg_input(&store, location, entry).is_err());
        Ok((store.source(), location, entry))
    })
    .await??;
    running.stop().await?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let store = database.open(StoreOpenMode::ReadCurrent)?;
        assert!(library_jpeg_input(&store, location, entry).is_err());
        Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn media_adapter_paired_sharing_binds_pages_uploads_and_fences_changed_evidence() -> TestResult
{
    use crate::application::sharing::{
        BindShareFolder, InspectShareFolder, bind_share_folder, inspect_share_folder,
    };
    use crate::domain::sharing::{ShareAccess, SharePolicy, ShareRoot};
    use crate::infrastructure::{
        server::configuration::ConfigurationService,
        shares::{ShareCatalog, publication_visibility_write},
        store::StoreOpenMode,
    };
    use std::io::{Read, Write};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let fixture = Fixture::new()?;
    let album = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media")
        .join("album");
    let bytes = (0..200_000)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    std::fs::write(album.join("track.flac"), &bytes)?;
    for index in 0..65 {
        std::fs::write(
            album.join(format!("track-{index:03}.mp3")),
            b"generated encoded audio",
        )?;
    }
    std::fs::write(album.join("secret.json"), b"excluded")?;
    std::fs::create_dir(album.join(".domus-retained-generated"))?;
    std::fs::write(
        album.join(".domus-retained-generated").join("secret.mp3"),
        b"excluded",
    )?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, _) = configured_client_fixture(&fixture, &keys, running.address)?;
    select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    let (database, catalog, read) = tokio::task::spawn_blocking(move || -> Result<_> {
        let store = source.open_store(StoreOpenMode::Current)?;
        let location = store.add_paired_location("library", "main")?;
        let inspected = inspect_share_folder(
            &store,
            &InspectShareFolder {
                location_id: location,
                relative_path: "album".into(),
            },
        )?;
        assert!(!inspected.bound);
        assert!(
            bind_share_folder(
                &store,
                &BindShareFolder {
                    location_id: location,
                    relative_path: "album".into(),
                    expected_identity: "stale".into()
                }
            )
            .is_err()
        );
        let bound = bind_share_folder(
            &store,
            &BindShareFolder {
                location_id: location,
                relative_path: "album".into(),
                expected_identity: inspected.native_identity,
            },
        )?;
        assert!(bound.bound);
        let policy = SharePolicy {
            access: ShareAccess::All,
            roots: vec![ShareRoot {
                binding_id: bound
                    .binding_id
                    .ok_or(Error::Internal("missing share binding"))?,
                alias: "Music".into(),
            }],
            ..Default::default()
        };
        let stop = Arc::new(AtomicBool::new(false));
        let catalog = ShareCatalog::build(&store.source(), &policy, stop.clone())?;
        assert_eq!(catalog.counts()?, (1, 66));
        let first = catalog.page(0, 64, None, None)?;
        let rest = catalog.page(
            first.last().ok_or(Error::Internal("missing page"))?.id,
            64,
            None,
            None,
        )?;
        assert_eq!(first.len() + rest.len(), 66);
        assert_eq!(catalog.directory_page("", 20, None)?[0].wire_path, "Music");
        assert_eq!(catalog.directory_files("Music", 0, 200)?.len(), 66);
        let shared = catalog.file("Music\\track.flac")?;
        let read = catalog.open(&shared)?;
        assert!(publication_visibility_write(&store).is_err());
        for offset in [0u64, 17, 200_000] {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            let mut socket = std::net::TcpStream::connect(listener.local_addr()?)?;
            let (mut receiver, _) = listener.accept()?;
            receiver.set_read_timeout(Some(Duration::from_secs(30)))?;
            let expected = bytes.clone();
            let received = std::thread::spawn(move || -> std::io::Result<()> {
                let mut token = [0; 4];
                receiver.read_exact(&mut token)?;
                assert_eq!(u32::from_le_bytes(token), 71);
                receiver.write_all(&offset.to_le_bytes())?;
                let mut actual = vec![0; expected.len() - offset as usize];
                receiver.read_exact(&mut actual)?;
                assert_eq!(actual, expected[offset as usize..]);
                Ok(())
            });
            let mut handle = read.file.try_clone()?;
            crate::infrastructure::soulseek::sharing::transmit(
                &mut handle,
                &mut socket,
                71,
                shared.bytes,
                &policy.limits,
                || read.check(),
            )?;
            received
                .join()
                .map_err(|_| Error::Internal("generated receiver failed"))??;
        }
        read.check()?;
        assert_eq!(std::fs::read(album.join("track.flac"))?, bytes);
        stop.store(true, Ordering::Release);
        assert!(matches!(read.check(), Err(Error::WorkerStopping)));
        stop.store(false, Ordering::Release);
        std::fs::write(album.join("track.flac"), b"changed generated media")?;
        assert!(read.check().is_err());
        assert!(catalog.page(0, 200, None, None).is_err());
        assert!(catalog.open(&shared).is_err());
        Ok((store.source(), catalog, read))
    })
    .await??;
    running.stop().await?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        assert!(read.check().is_err());
        assert!(catalog.check().is_err());
        drop(read);
        drop(catalog);
        let store = database.open(StoreOpenMode::ReadCurrent)?;
        publication_visibility_write(&store)?;
        Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn media_adapter_paired_sharing_keeps_publication_receipts_and_hash_checks() -> TestResult {
    use crate::application::sharing::{
        BindShareFolder, InspectShareFolder, bind_share_folder, inspect_share_folder,
    };
    use crate::domain::{
        LibraryPath,
        sharing::{SharePolicy, ShareRoot},
    };
    use crate::infrastructure::{
        server::configuration::ConfigurationService,
        shares::{ShareCatalog, publication_visibility_write},
        store::StoreOpenMode,
    };
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, atomic::AtomicBool};
    let fixture = Fixture::new()?;
    let file = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media")
        .join("album")
        .join("track.flac");
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, _) = configured_client_fixture(&fixture, &keys, running.address)?;
    select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let store = source.open_store(StoreOpenMode::Current)?;
        let location = store.add_paired_location("library", "main")?;
        let inspected = inspect_share_folder(&store, &InspectShareFolder { location_id: location, relative_path: "album".into() })?;
        let bound = bind_share_folder(&store, &BindShareFolder { location_id: location, relative_path: "album".into(), expected_identity: inspected.native_identity })?;
        let policy = SharePolicy { roots: vec![ShareRoot { binding_id: bound.binding_id.ok_or(Error::Internal("missing binding"))?, alias: "Music".into() }], ..Default::default() };
        let record = store.location(location)?;
        let directory = store.read_registered_directory(&record.root, &LibraryPath::parse("album")?)?;
        for _ in 0..2 {
            assert_eq!(directory.entries()?.collect::<Result<Vec<_>>>()?.len(), 1);
        }
        let album_identity = directory.native_identity()?;
        let root_identity = store.root_binding_identity(&record.root, "media")?;
        drop(directory);
        let bytes = std::fs::read(&file)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let visibility = publication_visibility_write(&store)?;
        // Isolate read-side receipt enforcement. Paired publication is not enabled.
        let connection = store.sharing_connection();
        connection.pragma_update(None, "foreign_keys", false)?;
        connection.execute(
            "INSERT INTO publication_destinations(job_id,plan_version,run_id,location_id,root,root_identity,kind,relative_folder,folder_key,manifest_sha256)
             VALUES(1,1,1,?1,?2,?3,'main','album','ALBUM',?4)",
            rusqlite::params![location, record.root, root_identity, digest],
        )?;
        let destination = connection.last_insert_rowid();
        connection.execute(
            "INSERT INTO publication_files(destination_id,ordinal,name,bytes,sha256) VALUES(?1,0,'track.flac',?2,?3)",
            rusqlite::params![destination, bytes.len() as i64, digest],
        )?;
        connection.pragma_update(None, "foreign_keys", true)?;
        drop(visibility);
        let catalog = || ShareCatalog::build(&store.source(), &policy, Arc::new(AtomicBool::new(false)));
        assert_eq!(catalog()?.counts()?, (0, 0));
        let visibility = publication_visibility_write(&store)?;
        connection.execute("UPDATE publication_destinations SET album_identity=?2,verified_at=1 WHERE id=?1", rusqlite::params![destination, album_identity])?;
        drop(visibility);
        assert!(catalog().is_err());
        let visibility = publication_visibility_write(&store)?;
        connection.execute("UPDATE publication_files SET verified_at=1 WHERE destination_id=?1", [destination])?;
        drop(visibility);
        let current = catalog()?;
        assert_eq!(current.counts()?, (1, 1));
        let shared = current.file("Music\\track.flac")?;
        let read = current.open(&shared)?;
        read.check()?;
        drop(read);
        let modified = std::fs::metadata(&file)?.modified()?;
        std::fs::write(&file, vec![b'X'; bytes.len()])?;
        std::fs::OpenOptions::new().write(true).open(&file)?.set_modified(modified)?;
        assert!(current.open(&shared).is_err());
        assert!(catalog().is_err());
        Ok(())
    }).await??;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_paired_sharing_rejects_hardlinks_and_immutable_folder_replacement()
-> TestResult {
    use crate::application::sharing::{
        BindShareFolder, InspectShareFolder, bind_share_folder, inspect_share_folder,
    };
    use crate::domain::sharing::{SharePolicy, ShareRoot};
    use crate::infrastructure::{
        server::configuration::ConfigurationService, shares::ShareCatalog, store::StoreOpenMode,
    };
    use std::sync::{Arc, atomic::AtomicBool};
    let fixture = Fixture::new()?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, _) = configured_client_fixture(&fixture, &keys, running.address)?;
    select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let store = source.open_store(StoreOpenMode::Current)?;
        let location = store.add_paired_location("library", "main")?;
        let inspected = inspect_share_folder(
            &store,
            &InspectShareFolder {
                location_id: location,
                relative_path: "album".into(),
            },
        )?;
        let request = BindShareFolder {
            location_id: location,
            relative_path: "album".into(),
            expected_identity: inspected.native_identity,
        };
        let bound = bind_share_folder(&store, &request)?;
        let policy = SharePolicy {
            roots: vec![ShareRoot {
                binding_id: bound.binding_id.ok_or(Error::Internal("missing binding"))?,
                alias: "Music".into(),
            }],
            ..Default::default()
        };
        std::fs::hard_link(
            media.join("album").join("track.flac"),
            media.join("album").join("alias.flac"),
        )?;
        assert!(
            ShareCatalog::build(&store.source(), &policy, Arc::new(AtomicBool::new(false)))
                .is_err()
        );
        std::fs::remove_file(media.join("album").join("alias.flac"))?;
        let catalog =
            ShareCatalog::build(&store.source(), &policy, Arc::new(AtomicBool::new(false)))?;
        std::fs::rename(media.join("album"), media.join("old-album"))?;
        std::fs::create_dir(media.join("album"))?;
        std::fs::write(
            media.join("album").join("track.flac"),
            b"generated media bytes",
        )?;
        assert!(
            !inspect_share_folder(
                &store,
                &InspectShareFolder {
                    location_id: location,
                    relative_path: "album".into()
                }
            )?
            .bound
        );
        assert!(bind_share_folder(&store, &request).is_err());
        let current = inspect_share_folder(
            &store,
            &InspectShareFolder {
                location_id: location,
                relative_path: "album".into(),
            },
        )?;
        assert!(
            bind_share_folder(
                &store,
                &BindShareFolder {
                    expected_identity: current.native_identity,
                    ..request
                }
            )
            .is_err()
        );
        assert!(catalog.check().is_err());
        Ok(())
    })
    .await??;
    running.stop().await
}

fn generated_scan_audio() -> TestResult<(Vec<u8>, Vec<u8>)> {
    use id3::TagLike;
    use std::io::Write;
    let mut tag = id3::Tag::new();
    tag.set_title("Generated paired track");
    tag.set_artist("Generated artist");
    tag.set_album("Generated album");
    tag.set_track(2);
    tag.add_frame(id3::frame::ExtendedText {
        description: "Generated padding".into(),
        value: "x".repeat(40 * 1024),
    });
    let mut mp3 = std::io::Cursor::new(Vec::new());
    tag.write_to(&mut mp3, id3::Version::Id3v24)?;
    for _ in 0..3 {
        let mut frame = vec![0; 144000 * 320 / 44100];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0xe0, 0]);
        mp3.write_all(&frame)?;
    }
    let mut flac = b"fLaC".to_vec();
    flac.extend_from_slice(&[0, 0, 0, 34]);
    let mut info = [0; 34];
    info[10..18]
        .copy_from_slice(&((44100u64 << 44) | (1 << 41) | (15 << 36) | 44100).to_be_bytes());
    flac.extend_from_slice(&info);
    flac.extend_from_slice(&[1, 0, 128, 0]);
    flac.extend_from_slice(&vec![0; 32 * 1024]);
    let fields = [
        "ARTIST=Generated artist",
        "ALBUM=Generated album",
        "TITLE=Generated FLAC",
    ];
    let mut comments = vec![0; 4];
    comments.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for field in fields {
        comments.extend_from_slice(&(field.len() as u32).to_le_bytes());
        comments.extend_from_slice(field.as_bytes());
    }
    flac.extend_from_slice(&[0x84, 0, (comments.len() >> 8) as u8, comments.len() as u8]);
    flac.extend_from_slice(&comments);
    Ok((mp3.into_inner(), flac))
}

#[tokio::test]
async fn media_adapter_paired_scans_page_metadata_refresh_and_preserve_read_only_media()
-> TestResult {
    use crate::application::{refresh_library_folder, scan_location};
    use crate::domain::{LibraryPath, Page};
    use crate::infrastructure::server::configuration::ConfigurationService;
    use crate::infrastructure::store::StoreOpenMode;
    let fixture = Fixture::new()?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let album = media.join("album");
    let (mp3, flac) = generated_scan_audio()?;
    std::fs::write(album.join("track.mp3"), &mp3)?;
    std::fs::write(album.join("track.flac"), &flac)?;
    std::fs::write(album.join("invalid.mp3"), b"invalid generated MPEG")?;
    std::fs::write(album.join("future.m4a"), b"unsupported generated audio")?;
    std::fs::create_dir(album.join("nested"))?;
    std::fs::write(album.join("nested").join("unchanged.txt"), b"original")?;
    std::fs::create_dir(media.join(".DOMUS-retained-generated"))?;
    std::fs::write(
        media.join(".DOMUS-retained-generated").join("private.txt"),
        b"excluded",
    )?;
    for index in 0..130 {
        std::fs::write(
            album.join(format!("asset-{index:03}.txt")),
            b"generated asset",
        )?;
    }
    let expected = ["track.mp3", "track.flac"].map(|name| -> Result<_> {
        let mut file = std::fs::File::open(album.join(name))?;
        Ok(crate::infrastructure::library_metadata::inspect(
            &mut file, name,
        ))
    });
    let [mp3_metadata, flac_metadata] = expected;
    let mp3_metadata = mp3_metadata?;
    let flac_metadata = flac_metadata?;
    assert_eq!(mp3_metadata.status, "tagged");
    assert_eq!(flac_metadata.status, "tagged");
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, _) = configured_client_fixture(&fixture, &keys, running.address)?;
    select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    let album_again = album.clone();
    let (location, database) = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut store = source.open_store(StoreOpenMode::Current)?;
        let location = store.add_paired_location("library", "main")?;
        scan_location(&mut store, location)?;
        assert_eq!(store.location(location)?.coverage, "complete");
        let files = store.browse(location, &LibraryPath::parse("album")?, Page::new(0, 200)?)?;
        assert_eq!(files.len(), 135);
        for (name, expected) in [("track.mp3", mp3_metadata), ("track.flac", flac_metadata)] {
            let entry = files
                .iter()
                .find(|entry| entry.relative_path.ends_with(name))
                .ok_or(Error::Internal("missing audio"))?;
            let actual = store.library_file(location, entry.id)?.metadata;
            assert_eq!(actual.status, expected.status);
            assert_eq!(
                serde_json::to_value(actual.tags)?,
                serde_json::to_value(expected.tags)?
            );
            assert_eq!(
                serde_json::to_value(actual.audio)?,
                serde_json::to_value(expected.audio)?
            );
            assert_eq!(actual.size, expected.size);
            assert_eq!(actual.modified_ns, expected.modified_ns);
            assert!(actual.native_identity.is_some());
        }
        for (name, status) in [("invalid.mp3", "invalid"), ("future.m4a", "unsupported")] {
            let entry = files
                .iter()
                .find(|entry| entry.relative_path.ends_with(name))
                .ok_or(Error::Internal("missing diagnostic"))?;
            assert_eq!(
                store.library_file(location, entry.id)?.metadata.status,
                status
            );
        }
        assert_eq!(
            store
                .browse(location, &LibraryPath::parse("")?, Page::new(0, 200)?)?
                .len(),
            1
        );
        let observed_at = store.location(location)?.observed_at;
        std::fs::remove_file(album_again.join("asset-000.txt"))?;
        std::fs::write(album_again.join("new.txt"), b"new direct child")?;
        std::fs::write(
            album_again.join("nested").join("not-refreshed.txt"),
            b"new descendant",
        )?;
        refresh_library_folder(&mut store, location, &LibraryPath::parse("album")?)?;
        assert_eq!(store.location(location)?.observed_at, observed_at);
        assert_eq!(store.location(location)?.coverage, "complete");
        let direct = store.browse(location, &LibraryPath::parse("album")?, Page::new(0, 200)?)?;
        assert!(
            !direct
                .iter()
                .find(|entry| entry.relative_path == "album/asset-000.txt")
                .ok_or(Error::Internal("missing old entry"))?
                .present
        );
        assert!(
            direct
                .iter()
                .any(|entry| entry.relative_path == "album/new.txt" && entry.present)
        );
        let nested = store.browse(
            location,
            &LibraryPath::parse("album/nested")?,
            Page::new(0, 200)?,
        )?;
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].relative_path, "album/nested/unchanged.txt");
        let database = store.source();
        drop(store);
        let mut reopened = database.open(StoreOpenMode::Current)?;
        scan_location(&mut reopened, location)?;
        assert_eq!(
            reopened
                .browse(
                    location,
                    &LibraryPath::parse("album/nested")?,
                    Page::new(0, 200)?
                )?
                .len(),
            2
        );
        Ok((location, database))
    })
    .await??;
    assert_eq!(std::fs::read(album.join("track.mp3"))?, mp3);
    assert_eq!(std::fs::read(album.join("track.flac"))?, flac);
    running.stop().await?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut store = database.open(StoreOpenMode::Current)?;
        let before = store.browse(location, &LibraryPath::parse("album")?, Page::new(0, 200)?)?;
        assert!(scan_location(&mut store, location).is_err());
        assert_eq!(store.location(location)?.coverage, "incomplete");
        assert!(store.location(location)?.last_error.is_some());
        let after = store.browse(location, &LibraryPath::parse("album")?, Page::new(0, 200)?)?;
        assert_eq!(serde_json::to_value(before)?, serde_json::to_value(after)?);
        Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn media_adapter_selected_registration_is_frozen_and_survives_reopen_without_native_paths()
-> TestResult {
    use crate::infrastructure::server::configuration::ConfigurationService;
    use crate::infrastructure::store::StoreOpenMode;
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, client_document) =
        configured_client_fixture(&fixture, &keys, running.address)?;
    select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    let host_again = host.clone();
    let location = tokio::task::spawn_blocking(move || -> Result<i64> {
        let store = source.open_store(StoreOpenMode::Current)?;
        assert!(store.add_paired_location("unselected", "main").is_err());
        assert!(store.add_paired_location("library", "invalid").is_err());
        let location = store.add_paired_location("library", "main")?;
        assert!(store.add_paired_location("library", "backup").is_err());
        assert!(store.add_paired_location("library", "main").is_err());
        let record = store.location(location)?;
        assert_eq!(record.root, "paired-media-root:library");
        assert!(
            record
                .native_identity
                .as_ref()
                .is_some_and(|identity| identity.starts_with("paired-media-v1:"))
        );
        assert!(store.open_registered_root(&record.root, "media").is_err());
        assert!(store.registered_root_path(&record.root, "media").is_err());
        {
            let root = store.media_directory_io(&record.root)?;
            let album = root
                .open_directory(&crate::domain::LibraryPath::parse("album")?)?
                .ok_or(Error::Internal("generated album missing"))?;
            let (_, files) =
                crate::infrastructure::publication::media_replacement_snapshot(&album)?;
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].name, "track.flac");
        }
        crate::application::check_location_availability(&store, location)?;
        let reopen = store.source();
        drop(store);
        std::fs::write(&path, b"invalid replacement document")?;
        let store = reopen.open(StoreOpenMode::ReadCurrent)?;
        crate::application::check_location_availability(&store, location)?;
        assert!(ConfigurationService::open(&host_again).is_err());
        std::fs::write(&path, serde_json::to_vec(&client_document)?)?;
        let fresh = ConfigurationService::open(&host_again)?;
        let reopened = fresh.open_store(StoreOpenMode::ReadCurrent)?;
        crate::application::check_location_availability(&reopened, location)?;
        let snapshot = fresh.open_store(StoreOpenMode::Snapshot)?;
        assert!(snapshot.media_directory_io(&record.root).is_err());
        assert!(matches!(
            crate::application::check_location_availability(&snapshot, location),
            Err(Error::Conflict(_))
        ));
        Ok(location)
    })
    .await??;
    running.stop().await?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let source = ConfigurationService::open(&host)?;
        let mut store = source.open_store(StoreOpenMode::Current)?;
        let before = store.location(location)?;
        assert!(matches!(
            store.media_directory_io(&before.root),
            Err(Error::Remote(_))
        ));
        assert!(matches!(
            crate::application::check_location_availability(&store, location),
            Err(Error::Remote(_))
        ));
        assert_eq!(store.location(location)?.root, before.root);
        assert_eq!(store.location(location)?.coverage, before.coverage);
        assert_eq!(store.location(location)?.observed_at, before.observed_at);
        assert_eq!(store.location(location)?.last_error, before.last_error);
        assert!(crate::application::scan_location(&mut store, location).is_err());
        assert_eq!(store.location(location)?.coverage, "incomplete");
        assert!(store.location(location)?.last_error.is_some());
        assert!(store.open_registered_root(&before.root, "media").is_err());
        Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn media_adapter_selected_root_lifecycle_never_creates_or_adopts_a_missing_or_copied_root()
-> TestResult {
    use crate::application::{
        check_location_availability, scan_location,
        sharing::{BindShareFolder, InspectShareFolder, bind_share_folder, inspect_share_folder},
    };
    use crate::domain::{LibraryPath, Page};
    use crate::infrastructure::{
        server::configuration::ConfigurationService, store::StoreOpenMode,
    };

    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, mut client_document) =
        configured_client_fixture(&fixture, &keys, running.address)?;
    select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let held = media.with_file_name("original-media");
    let active_media = media.clone();
    let active_held = held.clone();
    let (location, binding, before, cached) = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut store = source.open_store(StoreOpenMode::Current)?;
        let location = store.add_paired_location("library", "main")?;
        scan_location(&mut store, location)?;
        let inspect = InspectShareFolder {
            location_id: location,
            relative_path: "album".into(),
        };
        let initial = inspect_share_folder(&store, &inspect)?;
        let bound = bind_share_folder(
            &store,
            &BindShareFolder {
                location_id: location,
                relative_path: inspect.relative_path.clone(),
                expected_identity: initial.native_identity.clone(),
            },
        )?;
        let before = store.location(location)?;
        let cached = serde_json::to_value(store.browse(
            location,
            &LibraryPath::parse("album")?,
            Page::new(0, 20)?,
        )?)?;
        if cfg!(windows) {
            let Err(error) = std::fs::rename(&active_media, &active_held) else {
                return Err(Error::Internal(
                    "the retained Windows selection permitted root renaming",
                ));
            };
            assert_eq!(error.raw_os_error(), Some(32));
            assert!(active_media.is_dir());
            assert!(!active_held.exists());
        }
        check_location_availability(&store, location)?;
        Ok((location, bound, before, cached))
    })
    .await??;
    running.stop().await?;
    std::fs::rename(&media, &held)?;
    assert!(Running::start(&fixture, &keys).is_err());
    assert!(
        !media.exists(),
        "missing selected roots must not be created"
    );
    std::fs::create_dir_all(media.join("album"))?;
    std::fs::copy(
        held.join("album").join("track.flac"),
        media.join("album").join("track.flac"),
    )?;
    assert!(Running::start(&fixture, &keys).is_err());
    std::fs::remove_file(media.join("album").join("track.flac"))?;
    std::fs::remove_dir(media.join("album"))?;
    std::fs::remove_dir(&media)?;
    std::fs::rename(&held, &media)?;
    let restarted = Running::start(&fixture, &keys)?;
    client_document["address"] = serde_json::json!(restarted.address.to_string());
    std::fs::write(&path, serde_json::to_vec(&client_document)?)?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let source = ConfigurationService::open(&host)?;
        let mut store = source.open_store(StoreOpenMode::Current)?;
        check_location_availability(&store, location)?;
        let record = store.location(location)?;
        assert_eq!(record.native_identity, before.native_identity);
        assert_eq!(record.coverage, before.coverage);
        assert_eq!(record.observed_at, before.observed_at);
        assert_eq!(record.last_error, before.last_error);
        assert_eq!(
            serde_json::to_value(store.browse(
                location,
                &LibraryPath::parse("album")?,
                Page::new(0, 20)?,
            )?)?,
            cached
        );
        let current = inspect_share_folder(
            &store,
            &InspectShareFolder {
                location_id: location,
                relative_path: "album".into(),
            },
        )?;
        assert!(current.bound);
        assert_eq!(current.binding_id, binding.binding_id);
        assert_eq!(current.native_identity, binding.native_identity);
        scan_location(&mut store, location)?;
        assert_eq!(store.location(location)?.coverage, "complete");
        Ok(())
    })
    .await??;
    restarted.stop().await
}

#[tokio::test]
async fn media_adapter_paired_scan_stop_resumes_and_unsafe_refresh_never_confirms_absence()
-> TestResult {
    use crate::application::{refresh_library_folder, scan_location};
    use crate::domain::{LibraryPath, Page};
    use crate::infrastructure::server::configuration::ConfigurationService;
    use crate::infrastructure::store::StoreOpenMode;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, _) = configured_client_fixture(&fixture, &keys, running.address)?;
    select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    let target = fixture.selection.provider.boundary.join("private");
    tokio::task::spawn_blocking(move || -> Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut store = source
            .open_store(StoreOpenMode::Current)?
            .with_worker_stop(stop.clone());
        let location = store.add_paired_location("library", "main")?;
        stop.store(true, Ordering::Release);
        assert!(matches!(
            scan_location(&mut store, location),
            Err(Error::WorkerStopping)
        ));
        stop.store(false, Ordering::Release);
        scan_location(&mut store, location)?;
        assert_eq!(store.location(location)?.coverage, "complete");
        let before = store.browse(location, &LibraryPath::parse("album")?, Page::new(0, 10)?)?;
        assert_eq!(before.len(), 1);
        std::fs::remove_file(media.join("album").join("track.flac"))?;
        let alias = media.join("album").join("alias");
        super::super::tests::directory_alias(&alias, &target)?;
        assert!(
            refresh_library_folder(&mut store, location, &LibraryPath::parse("album")?).is_err()
        );
        assert_eq!(store.location(location)?.coverage, "complete");
        let coverage = store.library_coverage(location, &LibraryPath::parse("album")?)?;
        assert_eq!(coverage.folder_state.as_deref(), Some("incomplete"));
        assert!(coverage.folder_error.is_some());
        let after = store.browse(location, &LibraryPath::parse("album")?, Page::new(0, 10)?)?;
        assert_eq!(serde_json::to_value(before)?, serde_json::to_value(after)?);
        Ok(())
    })
    .await??;
    running.stop().await
}

#[tokio::test]
async fn media_adapter_selected_configuration_requires_exact_topology_and_refuses_known_overlap()
-> TestResult {
    use crate::infrastructure::server::configuration::ConfigurationService;
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let reserved = TcpListener::bind(("127.0.0.1", 0))?;
    let (host, path, client_document) =
        configured_client_fixture(&fixture, &keys, reserved.local_addr()?)?;
    let original = select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    ConfigurationService::open(&host)?;
    for change in 0..5 {
        let mut document = original.clone();
        let assurances = document["paired_media"]["topology_assurances"]
            .as_array_mut()
            .ok_or("missing fixture assurances")?;
        match change {
            0 => {
                assurances.pop();
            }
            1 => {
                assurances[0]["selection_digest"] = serde_json::json!("0".repeat(64));
            }
            2 => {
                assurances[0]["left"]["binding_revision"] = serde_json::json!(2);
            }
            3 => {
                assurances.push(assurances[0].clone());
            }
            _ => {
                assurances[0]["right"]["root_id"] = serde_json::json!("unselected");
            }
        }
        std::fs::write(&host, serde_json::to_vec(&document)?)?;
        assert!(ConfigurationService::open(&host).is_err());
    }
    std::fs::write(&host, serde_json::to_vec(&original)?)?;
    let mut selection = fixture.selection.logical();
    let native: crate::infrastructure::filesystem::StorageConfiguration =
        serde_json::from_value(original["storage"].clone())?;
    let state = native
        .roots
        .iter()
        .find(|root| root.role == crate::infrastructure::filesystem::StorageRole::State)
        .ok_or("missing state")?;
    selection.media[0].relative = state.relative.clone();
    selection.media[0].expected = state.expected.clone();
    selection.private.expected.components[0] = state.expected.components[0].clone();
    let mut changed_client = client_document.clone();
    changed_client["selection"] = serde_json::to_value(&selection)?;
    std::fs::write(&path, serde_json::to_vec(&changed_client)?)?;
    select_paired_fixture(&host, &path, &selection)?;
    assert!(matches!(
        ConfigurationService::open(&host),
        Err(Error::Conflict(_))
    ));
    Ok(())
}

#[tokio::test]
async fn media_adapter_selected_saved_binding_rejects_replacement_selection_and_provider_removal()
-> TestResult {
    use crate::infrastructure::server::configuration::ConfigurationService;
    use crate::infrastructure::store::StoreOpenMode;
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let (host, path, mut client_document) =
        configured_client_fixture(&fixture, &keys, running.address)?;
    let original = select_paired_fixture(&host, &path, &fixture.selection.logical())?;
    let source = ConfigurationService::open(&host)?;
    tokio::task::spawn_blocking(move || {
        source
            .open_store(StoreOpenMode::Current)?
            .add_paired_location("library", "main")
    })
    .await??;
    let mut selection = fixture.selection.logical();
    selection.revision += 1;
    client_document["selection"] = serde_json::to_value(&selection)?;
    std::fs::write(&path, serde_json::to_vec(&client_document)?)?;
    select_paired_fixture(&host, &path, &selection)?;
    ConfigurationService::open(&host)?.open_store(StoreOpenMode::ReadCurrent)?;
    selection.media[0].binding_revision += 1;
    client_document["selection"] = serde_json::to_value(&selection)?;
    std::fs::write(&path, serde_json::to_vec(&client_document)?)?;
    select_paired_fixture(&host, &path, &selection)?;
    assert!(
        ConfigurationService::open(&host)?
            .open_store(StoreOpenMode::ReadCurrent)
            .is_err()
    );
    let mut removed = original;
    removed["paired_media"] = serde_json::Value::Null;
    std::fs::write(&host, serde_json::to_vec(&removed)?)?;
    assert!(
        ConfigurationService::open(&host)?
            .open_store(StoreOpenMode::ReadCurrent)
            .is_err()
    );
    running.stop().await
}

#[tokio::test]
async fn media_adapter_selected_foreign_profile_is_passive_and_never_admitted_locally() -> TestResult
{
    use crate::infrastructure::filesystem::DirectoryIdentityProfile;
    use crate::infrastructure::server::configuration::ConfigurationService;
    use crate::infrastructure::store::StoreOpenMode;
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let reserved = TcpListener::bind(("127.0.0.1", 0))?;
    let (host, path, mut document) =
        configured_client_fixture(&fixture, &keys, reserved.local_addr()?)?;
    let mut selection = fixture.selection.logical();
    for (index, root) in selection
        .media
        .iter_mut()
        .chain(std::iter::once(&mut selection.private))
        .enumerate()
    {
        #[cfg(windows)]
        {
            root.expected.profile = DirectoryIdentityProfile::LinuxExportfs;
            root.expected.components = (0..root.expected.components.len())
                .map(|depth| {
                    format!(
                        "linuxfh:0000000000000001:00000001:{:02x}",
                        if depth == 0 {
                            0
                        } else {
                            (index + 1) * 32 + depth
                        }
                    )
                })
                .collect();
        }
        #[cfg(not(windows))]
        {
            root.expected.profile = DirectoryIdentityProfile::WindowsFileId128;
            root.expected.components = (0..root.expected.components.len())
                .map(|depth| {
                    format!(
                        "win128:0000000000000001:{:032x}",
                        if depth == 0 {
                            0
                        } else {
                            (index + 1) * 32 + depth
                        }
                    )
                })
                .collect();
        }
    }
    document["selection"] = serde_json::to_value(&selection)?;
    std::fs::write(&path, serde_json::to_vec(&document)?)?;
    select_paired_fixture(&host, &path, &selection)?;
    let source = ConfigurationService::open(&host)?;
    assert_eq!(
        source
            .open_store(StoreOpenMode::ReadCurrent)?
            .locations(crate::domain::Page::new(0, 10)?)?
            .len(),
        0
    );
    for root in &selection.media {
        assert!(
            root.expected
                .open(
                    &fixture.selection.provider.boundary,
                    &crate::domain::LibraryPath::parse(&root.relative)?
                )
                .is_err()
        );
    }
    // No listener is serving on the reserved port; preparation must not connect.
    assert_eq!(
        reserved.local_addr()?.port(),
        document["address"]
            .as_str()
            .ok_or("missing address")?
            .parse::<SocketAddr>()?
            .port()
    );
    Ok(())
}

#[tokio::test]
async fn media_adapter_client_configuration_is_private_passive_and_peer_bound() -> TestResult {
    use crate::infrastructure::server::configuration::ConfigurationService;

    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let reserved = TcpListener::bind(("127.0.0.1", 0))?;
    let address = reserved.local_addr()?;
    let (host, path, original) = configured_client_fixture(&fixture, &keys, address)?;
    let source = ConfigurationService::open(&host)?;
    let mut client = source.prepare_media_client(&path)?;
    for (field, invalid) in [
        ("address", serde_json::json!("192.0.2.1:8088")),
        ("address", serde_json::json!("127.0.0.1:0")),
        ("private_key_file", serde_json::json!("../key.pem")),
        ("private_key_file", serde_json::json!("")),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut document = original.clone();
        document[field] = invalid;
        std::fs::write(&path, serde_json::to_vec(&document)?)?;
        assert!(source.prepare_media_client(&path).is_err());
    }
    drop(reserved);
    let adapter = PreparedAdapter::new(
        fixture.selection.clone(),
        Vec::new(),
        keys.server.as_bytes(),
        keys.server_key.as_bytes(),
        keys.ca.as_bytes(),
        keys.client.as_bytes(),
    )?;
    let listener = TcpListener::bind(address)?;
    let handle = axum_server::Handle::new();
    let task = tokio::spawn(adapter.serve(listener, handle.clone()));
    let running = Running {
        address,
        handle,
        task,
    };
    let root = client.open_root("library").await?;
    let file = media_client_file(&mut client, &root).await?;
    assert_eq!(client.read(&file, 0, 64).await?, b"generated media bytes");
    client.close_directory(root).await?;
    drop(client);
    running.stop().await
}

fn configured_fixture(
    fixture: &Fixture,
    keys: &Certificates,
    address: SocketAddr,
) -> TestResult<(std::path::PathBuf, serde_json::Value)> {
    let private = fixture.selection.provider.boundary.join("private");
    for (name, bytes) in [
        ("server.pem", &keys.server),
        ("server-key.pem", &keys.server_key),
        ("client-ca.pem", &keys.ca),
        ("paired-client.pem", &keys.client),
    ] {
        std::fs::write(private.join(name), bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(private.join(name), std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let document = serde_json::json!({
        "format_version": 1,
        "listen": address,
        "selection": fixture.selection,
        "certificate_file": "server.pem",
        "private_key_file": "server-key.pem",
        "client_ca_file": "client-ca.pem",
        "paired_client_file": "paired-client.pem",
    });
    let path = private.join("adapter.json");
    std::fs::write(&path, serde_json::to_vec(&document)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok((path, document))
}

struct ConfiguredRunning {
    lifecycle: crate::infrastructure::lifecycle::Lifecycle,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Drop for ConfiguredRunning {
    fn drop(&mut self) {
        self.lifecycle.request_stop();
        self.task.abort();
    }
}

#[tokio::test]
async fn media_adapter_configuration_is_passive_and_activation_is_frozen() -> TestResult {
    use crate::infrastructure::server::configuration::ConfigurationService;

    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let reserved = TcpListener::bind(("127.0.0.1", 0))?;
    let address = reserved.local_addr()?;
    let (path, mut document) = configured_fixture(&fixture, &keys, address)?;
    let prepared = ConfigurationService::prepare_media_adapter(&path)?;
    // Preparation must succeed while the selected listen port is occupied.
    document["selection"]["revision"] = 2.into();
    std::fs::write(&path, serde_json::to_vec(&document)?)?;
    drop(reserved);
    let ready = Arc::new(tokio::sync::Notify::new());
    let notify = ready.clone();
    let lifecycle = crate::infrastructure::lifecycle::Lifecycle::controlled(move || {
        notify.notify_one();
        Ok(())
    });
    let task = tokio::spawn(prepared.run_with_lifecycle(lifecycle.clone()));
    let mut running = ConfiguredRunning { lifecycle, task };
    tokio::time::timeout(Duration::from_secs(3), ready.notified()).await?;
    let mut client = keys.client(address)?;
    let root = reference(client.execute(&fixture.request()?).await?)?;
    let Response::Page { entries, complete } = client
        .execute(&Request::Page {
            directory: root,
            limit: 64,
        })
        .await?
    else {
        return Err("expected configured adapter page".into());
    };
    assert!(complete);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "album");
    running.lifecycle.request_stop();
    tokio::time::timeout(Duration::from_secs(12), &mut running.task).await???;
    Ok(())
}

#[test]
fn media_adapter_configuration_rejects_unsafe_inputs_without_binding() -> TestResult {
    use crate::infrastructure::server::configuration::ConfigurationService;

    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let reserved = TcpListener::bind(("127.0.0.1", 0))?;
    let (path, original) = configured_fixture(&fixture, &keys, reserved.local_addr()?)?;
    for (field, invalid) in [
        ("format_version", serde_json::json!(2)),
        ("listen", serde_json::json!("0.0.0.0:8088")),
        ("listen", serde_json::json!("127.0.0.1:0")),
        ("private_key_file", serde_json::json!("../server-key.pem")),
        ("private_key_file", serde_json::json!("/server-key.pem")),
        (
            "private_key_file",
            serde_json::json!("server-key.pem:stream"),
        ),
        ("private_key_file", serde_json::json!("missing.pem")),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut document = original.clone();
        document[field] = invalid;
        std::fs::write(&path, serde_json::to_vec(&document)?)?;
        assert!(
            ConfigurationService::prepare_media_adapter(&path).is_err(),
            "accepted invalid adapter field {field}"
        );
    }
    let mut document = original.clone();
    document["selection"]["media"][0]["binding_revision"] = 0.into();
    std::fs::write(&path, serde_json::to_vec(&document)?)?;
    assert!(ConfigurationService::prepare_media_adapter(&path).is_err());
    let mut document = original.clone();
    document["selection"]["private"] = original["selection"]["media"][0].clone();
    std::fs::write(&path, serde_json::to_vec(&document)?)?;
    assert!(ConfigurationService::prepare_media_adapter(&path).is_err());
    let mut bounded_document = serde_json::to_vec(&original)?;
    bounded_document.resize(262144, b' ');
    let mut bounded_key = keys.server_key.as_bytes().to_vec();
    bounded_key.resize(16384, b' ');
    let mut bounded_certificate = keys.server.as_bytes().to_vec();
    bounded_certificate.resize(65536, b' ');
    std::fs::write(path.with_file_name("server.pem"), &bounded_certificate)?;
    std::fs::write(path.with_file_name("server-key.pem"), &bounded_key)?;
    std::fs::write(&path, &bounded_document)?;
    drop(ConfigurationService::prepare_media_adapter(&path)?);
    bounded_certificate.push(b' ');
    std::fs::write(path.with_file_name("server.pem"), &bounded_certificate)?;
    assert!(ConfigurationService::prepare_media_adapter(&path).is_err());
    bounded_certificate.pop();
    std::fs::write(path.with_file_name("server.pem"), bounded_certificate)?;
    bounded_document.push(b' ');
    std::fs::write(&path, &bounded_document)?;
    assert!(ConfigurationService::prepare_media_adapter(&path).is_err());
    std::fs::write(&path, serde_json::to_vec(&original)?)?;
    bounded_key.push(b' ');
    std::fs::write(path.with_file_name("server-key.pem"), bounded_key)?;
    assert!(ConfigurationService::prepare_media_adapter(&path).is_err());
    Ok(())
}

#[tokio::test]
async fn media_adapter_configured_inputs_cannot_be_exported_through_media_aliases() -> TestResult {
    use crate::infrastructure::server::configuration::ConfigurationService;

    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let reserved = TcpListener::bind(("127.0.0.1", 0))?;
    let address = reserved.local_addr()?;
    let (path, _) = configured_fixture(&fixture, &keys, address)?;
    let media = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media");
    std::fs::hard_link(
        path.with_file_name("server-key.pem"),
        media.join("key-alias"),
    )?;
    let prepared = ConfigurationService::prepare_media_adapter(&path)?;
    drop(reserved);
    let ready = Arc::new(tokio::sync::Notify::new());
    let notify = ready.clone();
    let lifecycle = crate::infrastructure::lifecycle::Lifecycle::controlled(move || {
        notify.notify_one();
        Ok(())
    });
    let task = tokio::spawn(prepared.run_with_lifecycle(lifecycle.clone()));
    let mut running = ConfiguredRunning { lifecycle, task };
    tokio::time::timeout(Duration::from_secs(3), ready.notified()).await?;
    let mut client = keys.client(address)?;
    let root = reference(client.execute(&fixture.request()?).await?)?;
    assert!(matches!(
        client
            .execute(&Request::Page {
                directory: root,
                limit: 64
            })
            .await,
        Err(Error::Conflict(_))
    ));
    running.lifecycle.request_stop();
    tokio::time::timeout(Duration::from_secs(12), &mut running.task).await???;
    Ok(())
}

#[tokio::test]
async fn media_adapter_configured_stop_before_start_never_opens_a_listener() -> TestResult {
    use crate::infrastructure::server::configuration::ConfigurationService;

    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let reserved = TcpListener::bind(("127.0.0.1", 0))?;
    let (path, _) = configured_fixture(&fixture, &keys, reserved.local_addr()?)?;
    let prepared = ConfigurationService::prepare_media_adapter(&path)?;
    let lifecycle = crate::infrastructure::lifecycle::Lifecycle::controlled(|| {
        Err(Error::Internal("stopped adapter must not report readiness"))
    });
    lifecycle.request_stop();
    prepared.run_with_lifecycle(lifecycle).await?;
    Ok(())
}

#[tokio::test]
async fn media_adapter_tls_reads_exact_handles_and_rejects_other_connections() -> TestResult {
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let mut client = keys.client(running.address)?;
    let root = reference(client.execute(&fixture.request()?).await?)?;
    let mut directory = root.clone();
    for _ in 0..2 {
        let Response::Page { entries, complete } = client
            .execute(&Request::Page {
                directory: directory.clone(),
                limit: 64,
            })
            .await?
        else {
            return Err("expected directory page".into());
        };
        assert!(complete);
        assert_eq!(entries.len(), 1);
        let entry = entries
            .into_iter()
            .next()
            .ok_or("missing generated child")?;
        directory = reference(
            client
                .execute(&Request::OpenChild {
                    parent: directory,
                    name: entry.name,
                    expected: entry.observed,
                })
                .await?,
        )?;
    }
    let file = directory;
    let Response::Read { bytes, complete } = client
        .execute(&Request::Read {
            file: file.clone(),
            offset: 0,
            count: MAX_READ,
        })
        .await?
    else {
        return Err("expected file bytes".into());
    };
    assert!(complete);
    assert_eq!(bytes, b"generated media bytes");
    let mut other = keys.client(running.address)?;
    assert!(matches!(
        other
            .execute(&Request::Read {
                file: file.clone(),
                offset: 0,
                count: 1
            })
            .await,
        Err(Error::Conflict(_))
    ));
    client.execute(&Request::Close { reference: root }).await?;
    assert!(matches!(
        client
            .execute(&Request::Read {
                file,
                offset: 0,
                count: 1
            })
            .await,
        Err(Error::Conflict(_))
    ));
    drop(client);
    drop(other);
    running.stop().await
}

#[tokio::test]
async fn media_adapter_tls_pins_both_peers_in_addition_to_ca_validation() -> TestResult {
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let mut wrong_client = Client::new(
        running.address,
        keys.ca.as_bytes(),
        keys.server.as_bytes(),
        keys.other_client.as_bytes(),
        keys.other_client_key.as_bytes(),
    )?;
    assert!(matches!(
        wrong_client.execute(&fixture.request()?).await,
        Err(Error::Remote(_))
    ));
    let mut wrong_server = Client::new(
        running.address,
        keys.ca.as_bytes(),
        keys.other_server.as_bytes(),
        keys.client.as_bytes(),
        keys.client_key.as_bytes(),
    )?;
    assert!(matches!(
        wrong_server.execute(&fixture.request()?).await,
        Err(Error::Remote(_))
    ));
    tls::initialize_crypto()?;
    let anonymous = reqwest::Client::builder()
        .tls_certs_only([reqwest::Certificate::from_pem(keys.ca.as_bytes())?])
        .resolve("localhost", running.address)
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()?;
    assert!(
        anonymous
            .post(format!(
                "https://localhost:{}/v1/media",
                running.address.port()
            ))
            .json(&fixture.request()?)
            .send()
            .await
            .is_err()
    );
    let mut valid = keys.client(running.address)?;
    valid.execute(&fixture.request()?).await?;
    drop(valid);
    running.stop().await
}

#[tokio::test]
async fn media_adapter_tls_rejects_browsers_wrong_hosts_and_oversized_frames() -> TestResult {
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let client = keys.client(running.address)?;
    for (name, value) in [
        ("origin", "https://localhost"),
        ("cookie", "session=fixture"),
        ("sec-fetch-site", "same-origin"),
        ("host", "other.invalid"),
    ] {
        let response = client
            .http
            .post(&client.url)
            .header("x-domus-media-adapter", "1")
            .header(name, value)
            .json(&fixture.request()?)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let response = client
        .http
        .post(&client.url)
        .header("x-domus-media-adapter", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(vec![b' '; MAX_FRAME + 1])
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(encode(&"a".repeat(MAX_FRAME - 2))?.len(), MAX_FRAME);
    assert!(encode(&"a".repeat(MAX_FRAME)).is_err());
    assert!(
        Client::new(
            "192.0.2.1:443".parse()?,
            keys.ca.as_bytes(),
            keys.server.as_bytes(),
            keys.client.as_bytes(),
            keys.client_key.as_bytes()
        )
        .is_err()
    );
    drop(client);
    running.stop().await
}

#[tokio::test]
async fn media_adapter_tls_loss_and_restart_never_reuse_live_references() -> TestResult {
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let mut client = keys.client(running.address)?;
    let root = reference(client.execute(&fixture.request()?).await?)?;
    running.stop().await?;
    assert!(matches!(
        client
            .execute(&Request::Page {
                directory: root.clone(),
                limit: 1
            })
            .await,
        Err(Error::Remote(_))
    ));
    assert!(client.unavailable);
    let restarted = Running::start(&fixture, &keys)?;
    let mut fresh = keys.client(restarted.address)?;
    fresh.execute(&fixture.request()?).await?;
    assert!(matches!(
        fresh
            .execute(&Request::Page {
                directory: root,
                limit: 1
            })
            .await,
        Err(Error::Conflict(_))
    ));
    drop(fresh);
    restarted.stop().await
}

#[test]
fn media_adapter_rejects_success_shaped_wrong_or_oversized_responses() -> TestResult {
    let reference = Reference {
        connection: "a".repeat(64),
        id: 1,
    };
    let page = Request::Page {
        directory: reference.clone(),
        limit: 1,
    };
    assert!(require_response(&page, &Response::Closed).is_err());
    assert!(
        require_response(
            &page,
            &Response::Page {
                entries: Vec::new(),
                complete: false
            }
        )
        .is_err()
    );
    let read = Request::Read {
        file: reference,
        offset: 0,
        count: 1,
    };
    assert!(
        require_response(
            &read,
            &Response::Read {
                bytes: vec![0; 2],
                complete: true
            }
        )
        .is_err()
    );
    assert!(
        require_response(
            &read,
            &Response::Read {
                bytes: Vec::new(),
                complete: false
            }
        )
        .is_err()
    );
    assert!(
        require_response(
            &read,
            &Response::Read {
                bytes: vec![42],
                complete: false
            }
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn media_adapter_pinning_keeps_webpki_hostname_and_usage_checks() -> TestResult {
    let keys = Certificates::new()?;
    let roots = anchors(keys.ca.as_bytes())?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = PinnedServer {
        inner: rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots.clone()),
            provider.clone(),
        )
        .build()?,
        certificate: leaf(keys.server.as_bytes())?,
    };
    assert!(
        server
            .verify_server_cert(
                &leaf(keys.server.as_bytes())?,
                &[],
                &ServerName::try_from("wrong.invalid")?,
                &[],
                UnixTime::now(),
            )
            .is_err()
    );
    let wrong_usage = PinnedClient {
        inner: rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            provider,
        )
        .build()?,
        certificate: leaf(keys.server.as_bytes())?,
    };
    assert!(
        wrong_usage
            .verify_client_cert(&leaf(keys.server.as_bytes())?, &[], UnixTime::now())
            .is_err()
    );
    Ok(())
}

#[test]
fn media_adapter_rejects_untrusted_pairing_before_activation() -> TestResult {
    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let unrelated = Certificates::new()?;
    assert!(
        PreparedAdapter::new(
            fixture.selection.clone(),
            Vec::new(),
            keys.server.as_bytes(),
            keys.server_key.as_bytes(),
            keys.ca.as_bytes(),
            unrelated.client.as_bytes(),
        )
        .is_err()
    );
    assert!(
        Client::new(
            "127.0.0.1:1".parse()?,
            keys.ca.as_bytes(),
            unrelated.server.as_bytes(),
            keys.client.as_bytes(),
            keys.client_key.as_bytes(),
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires the current debug domus binary; explicit generated foreground diagnostic"]
async fn media_adapter_cli_foreground_probe() -> TestResult {
    use std::process::{Child, Command, ExitStatus, Stdio};

    async fn wait(child: &mut Child) -> TestResult<ExitStatus> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = child.try_wait()? {
                    return Ok::<_, std::io::Error>(status);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
        .map_err(Into::into)
    }

    async fn checked(command: &mut Command) -> TestResult<ExitStatus> {
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let result = wait(&mut child).await;
        if result.is_err() {
            child.kill()?;
            child.wait()?;
        }
        result
    }

    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let reserved = TcpListener::bind(("127.0.0.1", 0))?;
    let address = reserved.local_addr()?;
    let (path, _) = configured_fixture(&fixture, &keys, address)?;
    let (host, client_path, _) = configured_client_fixture(&fixture, &keys, address)?;
    select_paired_fixture(&host, &client_path, &fixture.selection.logical())?;
    let binary = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .ok_or("workspace root is unavailable")?
        .join("target")
        .join("debug")
        .join(format!("domus{}", std::env::consts::EXE_SUFFIX));
    if !checked(Command::new(&binary).arg("media-adapter-check").arg(&path))
        .await?
        .success()
    {
        return Err("foreground CLI configuration check failed".into());
    }
    if !checked(
        Command::new(&binary)
            .arg("media-client-check")
            .arg("--host")
            .arg(&host)
            .arg(&client_path),
    )
    .await?
    .success()
    {
        return Err("passive client CLI configuration check failed".into());
    }
    if !checked(
        Command::new(&binary)
            .arg("media-storage-digest")
            .arg("--host")
            .arg(&host)
            .arg(&client_path),
    )
    .await?
    .success()
    {
        return Err("passive media topology digest failed".into());
    }
    drop(reserved);
    let mut child = Command::new(&binary)
        .arg("media-adapter")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let result = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if child.try_wait()?.is_some() {
                    return Err("foreground adapter exited before readiness".into());
                }
                if tokio::net::TcpStream::connect(address).await.is_ok() {
                    return Ok::<_, Box<dyn std::error::Error>>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        let mut client = keys.client(address)?;
        reference(client.execute(&fixture.request()?).await?)?;
        for (root_id, success) in [("library", true), ("unselected", false)] {
            let status = checked(
                Command::new(&binary)
                    .arg("media-root-check")
                    .arg("--host")
                    .arg(&host)
                    .arg(&client_path)
                    .arg(root_id),
            )
            .await?;
            assert_eq!(status.success(), success);
        }
        for (command, arguments, success) in [
            ("location-add-paired", vec!["unselected", "main"], false),
            ("location-add-paired", vec!["library", "main"], true),
            ("location-check", vec!["1"], true),
            ("location-add-paired", vec!["library", "backup"], false),
        ] {
            let status = checked(
                Command::new(&binary)
                    .arg(command)
                    .arg("--host")
                    .arg(&host)
                    .args(arguments),
            )
            .await?;
            assert_eq!(status.success(), success);
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    child.wait()?;
    result
}

/// This diagnostic uses only an already-built image and generated material.
/// It does not enable host networking or count as full Docker acceptance.
#[cfg(windows)]
#[tokio::test]
#[ignore = "requires an existing Docker candidate; explicit host-network diagnostic"]
async fn media_adapter_docker_loopback_probe() -> TestResult {
    use base64::Engine;
    use std::io::Read;
    use std::process::{Command, Stdio};

    let fixture = Fixture::new()?;
    let keys = Certificates::new()?;
    let running = Running::start(&fixture, &keys)?;
    let mut native = keys.client(running.address)?;
    native.execute(&fixture.request()?).await?;
    drop(native);
    let server = leaf(keys.server.as_bytes())?;
    let (_, parsed) = x509_parser::parse_x509_certificate(&server)?;
    let pin = format!(
        "sha256//{}",
        base64::engine::general_purpose::STANDARD.encode(<sha2::Sha256 as sha2::Digest>::digest(
            parsed.public_key().raw
        ),)
    );
    let identity = format!("{}{}", keys.client, keys.client_key);
    let request = fixture.request()?;
    let body = encode(&request)?;
    let mut input = format!("{}\n{}\n{}\n", keys.ca.len(), identity.len(), body.len()).into_bytes();
    input.extend_from_slice(keys.ca.as_bytes());
    input.extend_from_slice(identity.as_bytes());
    input.extend_from_slice(&body);
    let name = format!(
        "domus-media-adapter-proof-{}",
        &Session::new()?.connection[..16]
    );
    let image = match std::env::var("DOMUS_MEDIA_ADAPTER_PROBE_IMAGE") {
        Ok(image) => image,
        Err(std::env::VarError::NotPresent) => "domus-obscura:storage-candidate".into(),
        Err(error) => return Err(error.into()),
    };
    let script = r#"set -eu
umask 077
read -r ca_size
read -r identity_size
read -r body_size
d=$(mktemp -d)
trap 'rm -f -- "$d/ca" "$d/identity" "$d/body"; rmdir -- "$d"' EXIT
head -c "$ca_size" > "$d/ca"
head -c "$identity_size" > "$d/identity"
head -c "$body_size" > "$d/body"
curl --silent --show-error --fail-with-body --noproxy '*' --http1.1 \
  --cacert "$d/ca" --cert "$d/identity" --key "$d/identity" --pinnedpubkey "$1" \
  --resolve "localhost:$2:127.0.0.1" --connect-timeout 3 --max-time 5 --max-filesize 262144 \
  -H 'content-type: application/json' -H 'x-domus-media-adapter: 1' \
  --data-binary "@$d/body" "https://localhost:$2/v1/media"
"#;
    let mut child = Command::new("docker")
        .args([
            "run",
            "--pull",
            "never",
            "--rm",
            "--read-only",
            "--network",
            "host",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--memory",
            "64m",
            "--cpus",
            "0.5",
            "--tmpfs",
            "/tmp:rw,noexec,nosuid,nodev,size=1m,mode=0700,uid=1000,gid=1000",
            "--name",
            &name,
            "-i",
            "--entrypoint",
            "/bin/bash",
            &image,
            "-c",
            script,
            "media-adapter-proof",
            &pin,
            &running.address.port().to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("Docker proof input is unavailable")?
        .write_all(&input)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if tokio::time::Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("Docker adapter proof exceeded its process deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let mut output = Vec::new();
    let mut errors = Vec::new();
    child
        .stdout
        .take()
        .ok_or("Docker proof output is unavailable")?
        .take((MAX_FRAME + 1) as u64)
        .read_to_end(&mut output)?;
    child
        .stderr
        .take()
        .ok_or("Docker proof diagnostics are unavailable")?
        .take(16384)
        .read_to_end(&mut errors)?;
    let response = serde_json::from_slice::<Response>(&output);
    let passed = status.success()
        && response
            .as_ref()
            .is_ok_and(|response| require_response(&request, response).is_ok());
    // Do not publish keys, root paths, native identities or live references.
    let report = serde_json::json!({
        "scope": "windows-docker-desktop-adapter-channel-only",
        "image": image,
        "native_tls": true,
        "guest_authenticated_root_open": passed,
        "guest_exit_code": status.code(),
        "diagnostic": String::from_utf8_lossy(&errors),
    });
    let report_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .ok_or("workspace root is unavailable")?
        .join("target")
        .join("media-adapter-docker-loopback.json");
    std::fs::write(report_path, serde_json::to_vec_pretty(&report)?)?;
    running.stop().await?;
    if !passed {
        return Err("Linux-container adapter channel failed; see target\\media-adapter-docker-loopback.json".into());
    }
    Ok(())
}
