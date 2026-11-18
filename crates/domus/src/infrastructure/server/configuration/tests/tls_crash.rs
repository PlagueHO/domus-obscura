//! Process termination evidence for manual HTTPS, not managed activation.
use super::*;
use crate::infrastructure::server::{PreparedServer, tls_checkpoint};
use std::net::{SocketAddr, TcpListener};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "infrastructure::server::configuration::tests::tls_crash::backend_configuration_tls_crash_child";

struct OwnedChild {
    child: Child,
    log: PathBuf,
}

impl OwnedChild {
    fn spawn(fixture: &Fixture, mode: &str, boundary: &str, run: &str) -> TestResult<Self> {
        let log = fixture.directory.path().join(format!("{run}.log"));
        let output = std::fs::File::create(&log)?;
        let child = Command::new(std::env::current_exe()?)
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("DOMUS_TLS_CRASH_CONFIG", &fixture.path)
            .env("DOMUS_TLS_CRASH_MODE", mode)
            .env("DOMUS_TLS_CRASH_BOUNDARY", boundary)
            .stdout(Stdio::from(output.try_clone()?))
            .stderr(Stdio::from(output))
            .spawn()?;
        Ok(Self { child, log })
    }

    fn running(&mut self) -> TestResult {
        if let Some(status) = self.child.try_wait()? {
            return Err(format!(
                "TLS child exited {status}: {}",
                std::fs::read_to_string(&self.log)?
            )
            .into());
        }
        Ok(())
    }

    fn kill_and_reap(&mut self) -> TestResult {
        self.running()?;
        self.child.kill()?;
        assert!(!self.child.wait()?.success());
        Ok(())
    }

    async fn checkpoint(&mut self, root: &Path, boundary: &str) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.running()?;
            if std::fs::read_to_string(root.join("checkpoint"))
                .ok()
                .as_deref()
                == Some(boundary)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "TLS child did not reach {boundary}: {}",
                    std::fs::read_to_string(&self.log)?
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "private child; requires an exact parent-owned invocation"]
async fn backend_configuration_tls_crash_child() -> TestResult {
    let path =
        PathBuf::from(std::env::var_os("DOMUS_TLS_CRASH_CONFIG").ok_or("missing child config")?);
    let boundary = std::env::var("DOMUS_TLS_CRASH_BOUNDARY")?;
    let (_, configuration, _) = ConfigurationService::load(&path)?;
    tls_checkpoint::CHECKPOINT
        .set(tls_checkpoint::Checkpoint {
            database: configuration.database.clone(),
            directory: path.parent().ok_or("missing parent")?.into(),
            boundary: boundary.clone(),
        })
        .map_err(|_| "checkpoint was already installed")?;
    match std::env::var("DOMUS_TLS_CRASH_MODE")?.as_str() {
        "serve" => PreparedServer::load(&path)?.run().await?,
        "invalid_start" => {
            assert!(matches!(
                PreparedServer::load(&path),
                Err(Error::InvalidInput(message)) if message == "TLS chain must contain 1 through 8 certificates"
            ));
            tls_checkpoint::reach(&configuration.database, &boundary).await?;
        }
        "managed_staging" | "managed_production" => {
            managed_rejection(&configuration)?;
            tls_checkpoint::reach(&configuration.database, &boundary).await?;
        }
        _ => return Err("unknown TLS child mode".into()),
    }
    Ok(())
}

fn manual_fixture() -> TestResult<(Fixture, SocketAddr, String)> {
    let mut fixture = Fixture::new(false)?;
    let reservation = TcpListener::bind(("127.0.0.1", 0))?;
    let address = reservation.local_addr()?;
    let origin = format!("https://localhost:{}", address.port());
    let mut configuration: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    configuration["listen"] = json!(address.to_string());
    configuration["origin"] = json!(origin);
    std::fs::write(&fixture.path, serde_json::to_vec(&configuration)?)?;
    fixture.source = ConfigurationService::load(&fixture.path)?.0;
    Ok((fixture, address, origin))
}

fn client(address: SocketAddr, fixtures: &[&Fixture]) -> TestResult<reqwest::Client> {
    crate::infrastructure::tls::initialize_crypto()?;
    let roots = fixtures
        .iter()
        .map(|fixture| {
            reqwest::Certificate::from_pem(&std::fs::read(
                fixture.directory.path().join("certificate.pem"),
            )?)
            .map_err(Into::into)
        })
        .collect::<TestResult<Vec<_>>>()?;
    Ok(reqwest::Client::builder()
        .tls_certs_only(roots)
        .tls_info(true)
        .resolve("localhost", address)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(1))
        .build()?)
}

async fn evidence(client: &reqwest::Client, origin: &str, fixture: &Fixture) -> TestResult<Value> {
    let response = client
        .get(format!("{origin}/api/system/configuration"))
        .send()
        .await?
        .error_for_status()?;
    let certificate = std::fs::read(fixture.directory.path().join("certificate.pem"))?;
    let expected = rustls_pemfile::certs(&mut &certificate[..])
        .next()
        .ok_or("missing fixture certificate")??;
    let peer = response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|tls| tls.peer_certificate())
        .ok_or("missing handshake certificate")?;
    assert_eq!(peer, expected.as_ref());
    Ok(serde_json::from_slice(&response.bytes().await?)?)
}

async fn ready(
    child: &mut OwnedChild,
    client: &reqwest::Client,
    origin: &str,
    fixture: &Fixture,
) -> TestResult<Value> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        child.running()?;
        match evidence(client, origin, fixture).await {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

fn operator_replace_tls(fixture: &Fixture, replacement: &Fixture) -> TestResult<Value> {
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    let certificate = fixture.directory.path().join("replacement-certificate.pem");
    let private_key = fixture.directory.path().join("replacement-key.pem");
    std::fs::copy(
        replacement.directory.path().join("certificate.pem"),
        &certificate,
    )?;
    std::fs::copy(
        replacement.directory.path().join("hidden-key.pem"),
        &private_key,
    )?;
    document["certificate_file"] = json!(certificate);
    document["private_key_file"] = json!(private_key);
    std::fs::write(&fixture.path, serde_json::to_vec(&document)?)?;
    Ok(json!(
        ConfigurationService::open(&fixture.path)?
            .read()?
            .saved_revision
    ))
}

async fn reload(client: &reqwest::Client, origin: &str, revision: &Value) -> TestResult {
    let response = client
        .post(format!("{origin}/api/system/configuration/reload"))
        .header("Origin", origin)
        .header("x-domus-csrf", "1")
        .json(&json!({"expected_revision": revision}))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::GONE);
    Ok(())
}

#[tokio::test]
async fn backend_configuration_tls_requires_stopped_operator_edit_and_restart() -> TestResult {
    let (fixture, address, origin) = manual_fixture()?;
    let replacement = Fixture::new(false)?;
    let client = client(address, &[&fixture, &replacement])?;
    let mut child = OwnedChild::spawn(&fixture, "serve", "", "initial")?;
    let initial = ready(&mut child, &client, &origin, &fixture).await?;
    let before = std::fs::read(&fixture.path)?;
    reload(&client, &origin, &initial["saved_revision"]).await?;
    assert_eq!(
        evidence(&client, &origin, &fixture).await?["active_revision"],
        initial["active_revision"]
    );
    assert_eq!(std::fs::read(&fixture.path)?, before);
    child.kill_and_reap()?;
    let revision = operator_replace_tls(&fixture, &replacement)?;
    let saved_bytes = std::fs::read(&fixture.path)?;
    let mut restarted = OwnedChild::spawn(&fixture, "serve", "", "restart")?;
    let recovered = ready(&mut restarted, &client, &origin, &replacement).await?;
    assert_eq!(recovered["saved_revision"], revision);
    assert_eq!(recovered["active_revision"], revision);
    assert_eq!(recovered["restart_required"], false);
    assert_eq!(std::fs::read(&fixture.path)?, saved_bytes);
    restarted.kill_and_reap()?;
    Ok(())
}

#[tokio::test]
async fn backend_configuration_tls_invalid_operator_edit_rejects_restart() -> TestResult {
    let (fixture, address, origin) = manual_fixture()?;
    let replacement = Fixture::new(false)?;
    let client = client(address, &[&fixture, &replacement])?;
    let root = fixture.directory.path();
    let mut child = OwnedChild::spawn(&fixture, "serve", "", "initial")?;
    ready(&mut child, &client, &origin, &fixture).await?;
    child.kill_and_reap()?;
    operator_replace_tls(&fixture, &replacement)?;
    let saved_bytes = std::fs::read(&fixture.path)?;
    std::fs::write(
        fixture.directory.path().join("replacement-certificate.pem"),
        b"invalid certificate",
    )?;
    let mut restarted = OwnedChild::spawn(
        &fixture,
        "invalid_start",
        "startup-rejected",
        "invalid-start",
    )?;
    restarted.checkpoint(root, "startup-rejected").await?;
    assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_err());
    assert_eq!(std::fs::read(&fixture.path)?, saved_bytes);
    assert_eq!(
        std::fs::read(fixture.directory.path().join("replacement-certificate.pem"))?,
        b"invalid certificate"
    );
    restarted.kill_and_reap()?;
    Ok(())
}

fn generation(
    root: &Path,
    environment: AcmeEnvironment,
) -> TestResult<crate::infrastructure::certificates::state::Generation> {
    let certificate = std::fs::read_to_string(root.join("certificate.pem"))?;
    let (not_before, not_after) =
        crate::infrastructure::tls::certificate_validity("localhost", certificate.as_bytes())?;
    Ok(crate::infrastructure::certificates::state::Generation {
        hostname: "localhost".into(),
        environment,
        certificate,
        private_key: std::fs::read_to_string(root.join("hidden-key.pem"))?,
        not_before,
        not_after,
        renew_at: crate::domain::certificates::renewal_time(not_before, not_after, None, 0)?,
        initial_renewal: None,
    })
}

fn managed_rejection(configuration: &Configuration) -> TestResult {
    use crate::infrastructure::certificates::state::StateStore;
    let path = configuration
        .database
        .parent()
        .ok_or("missing database parent")?
        .join("certificates");
    let environment = if std::env::var("DOMUS_TLS_CRASH_MODE")? == "managed_staging" {
        AcmeEnvironment::Staging
    } else {
        AcmeEnvironment::Production
    };
    let mut state = StateStore::open(&path, &"a".repeat(64), environment)?;
    if environment == AcmeEnvironment::Staging {
        assert_eq!(state.active_generation_number(), Some(1));
        assert!(matches!(
            state.activate_generation(2, "localhost"),
            Err(Error::InvalidInput(message)) if message == "only an exact-hostname production generation can activate HTTPS"
        ));
        assert_eq!(state.active_generation_number(), Some(1));
        assert!(matches!(
            StateStore::load_active(&path, &"a".repeat(64), "localhost"),
            Err(Error::InvalidInput(message)) if message == "only an exact-hostname production generation can activate HTTPS"
        ));
    } else {
        let candidate = generation(
            configuration
                .database
                .parent()
                .ok_or("missing database parent")?,
            environment,
        )?;
        // The pair is valid for manual TLS. Only independent public trust
        // prevents this generated production candidate from publication.
        crate::infrastructure::tls::configuration(
            "localhost",
            candidate.certificate.as_bytes(),
            candidate.private_key.as_bytes(),
        )?;
        assert!(matches!(
            state.publish_generation(&candidate),
            Err(Error::InvalidInput(message)) if message == "TLS certificate is invalid, expired, or has the wrong hostname"
        ));
        assert_eq!(state.active_generation_number(), None);
        assert!(!path.join("generation-1.json").exists());
    }
    let status = StateStore::status(&path, &"a".repeat(64), environment)?;
    assert!(status.recovery_required);
    assert_eq!(
        status.phase,
        crate::domain::certificates::CertificatePhase::Paused
    );
    Ok(())
}

#[tokio::test]
async fn backend_configuration_tls_crash_managed_rejection_preserves_journal_and_generations()
-> TestResult {
    use crate::infrastructure::certificates::state::StateStore;
    for environment in [AcmeEnvironment::Staging, AcmeEnvironment::Production] {
        let fixture = Fixture::new(false)?;
        let root = fixture.directory.path();
        let path = root.join("certificates");
        let mut state = StateStore::initialize(&path, &"a".repeat(64), environment)?;
        if environment == AcmeEnvironment::Staging {
            let generation = generation(root, environment)?;
            state.publish_generation(&generation)?;
            state.publish_generation(&generation)?;
        }
        drop(state);
        // A retained staging selection is journal evidence only. It is never
        // accepted as a trusted production listener, including after restart.
        let journal = path.join("lifecycle.json");
        let mut value: Value = serde_json::from_slice(&std::fs::read(&journal)?)?;
        value["phase"] = json!("paused");
        value["pause_reason"] = json!("fixture operator recovery");
        if environment == AcmeEnvironment::Staging {
            value["active_generation"] = json!(1);
            value["candidate_generation"] = json!(2);
        }
        std::fs::write(&journal, serde_json::to_vec(&value)?)?;
        let journal_bytes = std::fs::read(&journal)?;
        let generations = if environment == AcmeEnvironment::Staging {
            vec![
                std::fs::read(path.join("generation-1.json"))?,
                std::fs::read(path.join("generation-2.json"))?,
            ]
        } else {
            vec![]
        };
        let mode = if environment == AcmeEnvironment::Staging {
            "managed_staging"
        } else {
            "managed_production"
        };
        for run in ["reject", "restart-reject"] {
            let mut child = OwnedChild::spawn(&fixture, mode, "managed-rejected", run)?;
            child.checkpoint(root, "managed-rejected").await?;
            child.kill_and_reap()?;
            assert_eq!(std::fs::read(&journal)?, journal_bytes);
            for (index, bytes) in generations.iter().enumerate() {
                assert_eq!(
                    &std::fs::read(path.join(format!("generation-{}.json", index + 1)))?,
                    bytes
                );
            }
            assert!(!path.join(".lifecycle.pending").exists());
            std::fs::remove_file(root.join("checkpoint"))?;
        }
    }
    Ok(())
}
