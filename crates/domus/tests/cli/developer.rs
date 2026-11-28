use super::*;
use std::io::{BufRead, BufReader};
use std::ops::{Deref, DerefMut};
use std::process::Stdio;
use std::time::{Duration, Instant};

fn checked(args: &[&str]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let output = run(args)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn run_with(
    executable: &std::path::Path,
    args: &[&str],
) -> Result<std::process::Output, std::io::Error> {
    Command::new(executable).args(args).output()
}

struct SupervisorGuard {
    process: std::process::Child,
    configuration: std::path::PathBuf,
}

impl SupervisorGuard {
    fn start(configuration: &str) -> Result<Self, std::io::Error> {
        let process = Command::new(env!("CARGO_BIN_EXE_domus"))
            .args(["developer-run", configuration])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(Self {
            process,
            configuration: configuration.into(),
        })
    }
}

impl Deref for SupervisorGuard {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.process
    }
}

impl DerefMut for SupervisorGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.process
    }
}

impl Drop for SupervisorGuard {
    fn drop(&mut self) {
        if matches!(self.process.try_wait(), Ok(Some(_))) {
            return;
        }

        let stop = Command::new(env!("CARGO_BIN_EXE_domus"))
            .args([
                "developer-control",
                self.configuration.to_string_lossy().as_ref(),
                "stop",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Err(error) = stop {
            eprintln!("Could not request supervisor shutdown during test cleanup: {error}");
        }

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.process.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    eprintln!(
                        "Supervisor did not stop during test cleanup; terminating its process."
                    );
                    if let Err(error) = self.process.kill() {
                        eprintln!("Could not terminate the test supervisor: {error}");
                    }
                    let _ = self.process.wait();
                    return;
                }
                Err(error) => {
                    eprintln!("Could not wait for the test supervisor: {error}");
                    return;
                }
            }
        }
    }
}

#[test]
fn developer_real_processes_retain_state_and_refuse_duplicate_start()
-> Result<(), Box<dyn std::error::Error>> {
    real_processes(false)
}

#[test]
fn developer_https_processes_validate_tls_and_retain_state()
-> Result<(), Box<dyn std::error::Error>> {
    real_processes(true)
}

#[tokio::test(flavor = "multi_thread")]
async fn developer_library_and_soulseek_account_changes_restart_both_real_children()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempdir()?;
    let binaries = fixture.path().join("bin");
    let assets = binaries.join("browser");
    std::fs::create_dir_all(assets.join("assets"))?;
    std::fs::write(
        assets.join("index.html"),
        "<!doctype html><title>primary switch fixture</title>",
    )?;
    let executable = binaries.join(if cfg!(windows) { "domus.exe" } else { "domus" });
    std::fs::copy(env!("CARGO_BIN_EXE_domus"), &executable)?;
    let application = fixture.path().join("application");
    let initialized = run_with(
        &executable,
        &[
            "developer-init",
            "--confirm-fresh",
            application.to_str().ok_or("application")?,
        ],
    )?;
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let configuration = application.join("config.json");
    let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = socket.local_addr()?;
    drop(socket);
    let origin = format!("http://{address}");
    let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&configuration)?)?;
    document["listen"] = address.to_string().into();
    document["origin"] = origin.clone().into();
    document["browser_assets"] = assets.to_str().ok_or("assets")?.into();
    std::fs::write(&configuration, serde_json::to_vec(&document)?)?;
    let media = fixture.path().join("new-primary");
    std::fs::create_dir(&media)?;
    std::fs::write(media.join("existing.txt"), "keep original media")?;
    let backup_media = fixture.path().join("new-backup");
    std::fs::create_dir(&backup_media)?;
    std::fs::write(backup_media.join("existing.txt"), "keep backup media")?;
    let config = configuration.to_str().ok_or("configuration")?;
    let mut supervisor = SupervisorGuard::start(config)?;
    let stdout = supervisor.stdout.take().ok_or("stdout")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let wait_ready = || -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("readiness timeout")?;
            if receiver
                .recv_timeout(remaining)??
                .starts_with("Domus ready: ")
            {
                return Ok(());
            }
        }
    };
    wait_ready()?;
    let mut current = checked(&["developer-control", config, "status"])?;
    domus::infrastructure::tls::initialize_crypto()?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()?;
    let available: serde_json::Value = client
        .get(format!("{origin}/api/library/primary/control"))
        .header("origin", &origin)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(available["available"], true);
    let mut primary = None;
    for (path, backup) in [(&media, false), (&backup_media, true)] {
        let original = current;
        let preview: serde_json::Value = client
            .post(format!("{origin}/api/library/primary/inspect"))
            .header("origin", &origin)
            .header("x-domus-csrf", "1")
            .json(&serde_json::json!({"path":path,"backup":backup}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut approval = preview.as_object().ok_or("preview")?.clone();
        approval.remove("storage_layout");
        approval.insert("confirm_separation".into(), true.into());
        approval.insert("backup".into(), backup.into());
        let saved: serde_json::Value = client
            .post(format!("{origin}/api/library/primary/changes"))
            .header("origin", &origin)
            .header("x-domus-csrf", "1")
            .json(&approval)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(saved["state"], "approved");
        let id = saved["id"].as_i64().ok_or("change id")?;
        let pending: serde_json::Value = client
            .get(format!("{origin}/api/library/primary/control"))
            .header("origin", &origin)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(pending["pending_change_id"], id);
        assert_eq!(pending["pending_backup"], backup);
        assert!(
            domus::infrastructure::server::developer::control(
                &configuration,
                &format!("primary:{}", id + 1),
            )
            .await
            .is_err()
        );
        assert_eq!(
            checked(&["developer-control", config, "status"])?["processes"],
            original["processes"]
        );
        let response = client
            .post(format!(
                "{origin}/api/library/primary/changes/{id}/activate"
            ))
            .header("origin", &origin)
            .header("x-domus-csrf", "1")
            .json(&serde_json::json!({}))
            .send()
            .await?;
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        wait_ready()?;
        current = checked(&["developer-control", config, "status"])?;
        assert_eq!(current["state"], "ready");
        assert_ne!(original["revision"], current["revision"]);
        for role in ["api", "worker"] {
            let pid = |state: &serde_json::Value| -> Option<u64> {
                state["processes"]
                    .as_array()?
                    .iter()
                    .find(|process| process["role"] == role)?["pid"]
                    .as_u64()
            };
            assert!(pid(&current).is_some());
            assert_ne!(pid(&original), pid(&current), "{role} did not restart");
        }
        let result: serde_json::Value = client
            .get(format!("{origin}/api/library/primary/changes/{id}"))
            .header("origin", &origin)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(result["state"], "active");
        let locations: serde_json::Value = client
            .get(format!("{origin}/api/locations"))
            .header("origin", &origin)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let locations = locations.as_array().ok_or("locations")?;
        assert_eq!(locations.len(), if backup { 2 } else { 1 });
        assert_eq!(locations[0]["kind"], "main");
        assert_eq!(locations[0]["coverage"], "unknown");
        if backup {
            assert_eq!(Some(&locations[0]), primary.as_ref());
            assert_eq!(locations[1]["kind"], "backup");
            assert_eq!(locations[1]["coverage"], "unknown");
            assert_eq!(locations[1]["id"], result["location_id"]);
        } else {
            primary = Some(locations[0].clone());
        }
    }
    assert_eq!(
        std::fs::read_to_string(backup_media.join("existing.txt"))?,
        "keep backup media"
    );
    let account_request = serde_json::json!({
        "expected_revision":current["revision"],
        "username":"generated-process-user",
        "password":"generated-process-password"
    });
    let account_endpoint = format!("{origin}/api/system/soulseek-account/changes");
    let rejected = client
        .post(&account_endpoint)
        .header("origin", &origin)
        .json(&account_request)
        .send()
        .await?;
    assert_eq!(rejected.status(), reqwest::StatusCode::FORBIDDEN);
    let rejected = client
        .post(&account_endpoint)
        .header("origin", "http://invalid.example")
        .header("x-domus-csrf", "1")
        .json(&account_request)
        .send()
        .await?;
    assert_eq!(rejected.status(), reqwest::StatusCode::FORBIDDEN);
    let account: serde_json::Value = client
        .post(&account_endpoint)
        .header("origin", &origin)
        .header("x-domus-csrf", "1")
        .json(&account_request)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(account["state"], "approved");
    assert!(!account.to_string().contains("generated-process"));
    let account_id = account["id"].as_i64().ok_or("account ID")?;
    assert!(
        domus::infrastructure::server::developer::control(
            &configuration,
            &format!("account:{}", account_id + 1),
        )
        .await
        .is_err()
    );
    assert_eq!(
        checked(&["developer-control", config, "status"])?["processes"],
        current["processes"]
    );
    let response = client
        .post(format!("{account_endpoint}/{account_id}/activate"))
        .header("origin", &origin)
        .header("x-domus-csrf", "1")
        .json(&serde_json::json!({}))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    wait_ready()?;
    let account_current = checked(&["developer-control", config, "status"])?;
    assert_eq!(account_current["state"], "ready");
    for role in ["api", "worker"] {
        let pid = |state: &serde_json::Value| -> Option<u64> {
            state["processes"]
                .as_array()?
                .iter()
                .find(|process| process["role"] == role)?["pid"]
                .as_u64()
        };
        assert_ne!(
            pid(&account_current),
            pid(&current),
            "{role} did not restart for the account"
        );
    }
    let result: serde_json::Value = client
        .get(format!("{account_endpoint}/{account_id}"))
        .header("origin", &origin)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(result["state"], "active");
    assert_eq!(
        result["configuration_revision"],
        account_current["revision"]
    );
    let after_account: serde_json::Value = serde_json::from_slice(&std::fs::read(&configuration)?)?;
    assert_eq!(after_account["services"], document["services"]);
    assert_eq!(
        after_account["production_worker"],
        document["production_worker"]
    );
    assert!(after_account["local_acquisition"].is_null());
    assert_eq!(
        std::fs::read_to_string(media.join("existing.txt"))?,
        "keep original media"
    );
    assert!(checked(&["developer-control", config, "stop"])?["state"] == "stopped");
    assert!(supervisor.wait()?.success());
    reader.join().map_err(|_| "reader failed")?;
    Ok(())
}

fn real_processes(https: bool) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempdir()?;
    let binary_directory = fixture.path().join("bin");
    let assets = binary_directory.join("browser");
    std::fs::create_dir_all(assets.join("assets"))?;
    std::fs::write(
        assets.join("index.html"),
        b"<!doctype html><title>real developer fixture</title>",
    )?;
    let executable = binary_directory.join(
        std::path::Path::new(env!("CARGO_BIN_EXE_domus"))
            .file_name()
            .ok_or("binary name missing")?,
    );
    std::fs::copy(env!("CARGO_BIN_EXE_domus"), &executable)?;
    let application = fixture.path().join("application");
    let output = run_with(
        &executable,
        &[
            "developer-init",
            "--confirm-fresh",
            application.to_str().ok_or("path")?,
        ],
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = application.join("config.json");
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    document["listen"] = address.to_string().into();
    document["origin"] = format!("{}://{address}", if https { "https" } else { "http" }).into();
    if https {
        let key = rcgen::KeyPair::generate()?;
        let certificate =
            rcgen::CertificateParams::new(vec!["127.0.0.1".into()])?.self_signed(&key)?;
        let secrets = application.join("secrets");
        std::fs::create_dir(&secrets)?;
        private::directory(&secrets)?;
        let certificate_file = secrets.join("certificate.pem");
        let private_key_file = secrets.join("private-key.pem");
        std::fs::write(&certificate_file, certificate.pem())?;
        std::fs::write(&private_key_file, key.serialize_pem())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&private_key_file, std::fs::Permissions::from_mode(0o600))?;
        }
        document["environment"] = "development".into();
        document["certificate_file"] = certificate_file.to_str().ok_or("certificate")?.into();
        document["private_key_file"] = private_key_file.to_str().ok_or("private key")?.into();
    }
    document["browser_assets"] = assets.to_str().ok_or("assets")?.into();
    std::fs::write(&path, serde_json::to_vec(&document)?)?;
    let configuration = path.to_str().ok_or("configuration")?;
    let original = std::fs::read(&path)?;
    let repeated = run(&[
        "developer-init",
        "--confirm-fresh",
        application.to_str().ok_or("path")?,
    ])?;
    assert!(!repeated.status.success());
    assert_eq!(std::fs::read(&path)?, original);
    let inspection = checked(&["developer-inspect", configuration])?;
    assert_eq!(inspection["startup_verified"], false);
    assert_eq!(inspection["validation"]["errors"], serde_json::json!([]));
    let job = checked(&["job-create", "--host", configuration])?
        .as_i64()
        .ok_or("job identifier missing")?;
    let settings_file = application.join("test-settings.json");
    std::fs::write(
        &settings_file,
        serde_json::to_vec(&checked(&["settings-default"])?)?,
    )?;
    checked(&[
        "settings-save",
        "--host",
        configuration,
        "0",
        settings_file.to_str().ok_or("settings path")?,
    ])?;
    let settings = checked(&["settings", "--host", configuration])?;
    assert!(!settings.is_null());
    if https {
        let private_key = application.join("secrets").join("private-key.pem");
        let original_key = std::fs::read(&private_key)?;
        std::fs::write(&private_key, b"invalid TLS material")?;
        let refused = run(&["developer-run", configuration])?;
        assert!(!refused.status.success());
        assert!(!String::from_utf8_lossy(&refused.stdout).contains("Domus ready:"));
        std::fs::write(&private_key, original_key)?;
    }

    let occupied = std::net::TcpListener::bind(address)?;
    let refused = run(&["developer-run", configuration])?;
    assert!(!refused.status.success());
    assert!(!String::from_utf8_lossy(&refused.stdout).contains("Domus ready:"));
    drop(occupied);

    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let worker_lock = options.open(application.join("state").join(".domus-worker.lock"))?;
    worker_lock.try_lock()?;
    let refused = run(&["developer-run", configuration])?;
    assert!(!refused.status.success());
    assert!(!String::from_utf8_lossy(&refused.stdout).contains("Domus ready:"));
    drop(std::net::TcpListener::bind(address)?);
    drop(worker_lock);

    for iteration in 0..2 {
        let mut child = SupervisorGuard::start(configuration)?;
        let stdout = child.stdout.take().ok_or("stdout")?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            loop {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or("readiness timed out")?;
                let line = receiver.recv_timeout(remaining)??;
                if line.starts_with("Domus ready: ") {
                    break;
                }
            }
            let status = checked(&["developer-control", configuration, "status"])?;
            assert_eq!(status["state"], "ready");
            assert_eq!(status["processes"].as_array().ok_or("processes")?.len(), 2);
            let duplicate = run(&["developer-run", configuration])?;
            assert!(!duplicate.status.success());
            assert!(checked(&["developer-control", configuration, "logs"])?["lines"].is_array());
            if iteration == 1 {
                std::fs::write(&path, b"{invalid saved configuration")?;
                let status = checked(&["developer-control", configuration, "status"])?;
                assert_eq!(status["state"], "configuration_changed");
                assert!(status["inspection_error"].is_string());
            }
            Ok(())
        })();
        let stop = run(&["developer-control", configuration, "stop"])?;
        assert!(
            stop.status.success(),
            "{}",
            String::from_utf8_lossy(&stop.stderr)
        );
        let exit_deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait()? {
                assert!(status.success());
                break;
            }
            assert!(
                Instant::now() < exit_deadline,
                "supervisor did not finish drain"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        reader.join().map_err(|_| "reader failed")?;
        result?;
        if iteration == 1 {
            std::fs::write(&path, &original)?;
        }
        assert_eq!(std::fs::read(&path)?, original);
        let state = checked(&["developer-inspect", configuration])?;
        assert_eq!(
            state["configuration"]["active_revision"],
            inspection["configuration"]["active_revision"]
        );
        assert_eq!(checked(&["settings", "--host", configuration])?, settings);
    }
    let next = checked(&["job-create", "--host", configuration])?;
    assert_eq!(next.as_i64(), Some(job + 1));
    Ok(())
}
