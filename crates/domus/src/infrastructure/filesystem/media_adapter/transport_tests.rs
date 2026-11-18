use super::super::tests::Fixture;
use super::super::{MAX_READ, Reference};
use super::*;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
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
