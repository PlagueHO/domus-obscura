use super::super::tests::Fixture;
use super::super::{MAX_READ, MediaClient, MediaNode, Reference};
use super::*;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

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
