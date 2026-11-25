use super::*;
use std::io::{BufRead, BufReader};
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

fn real_processes(https: bool) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempdir()?;
    let application = fixture.path().join("application");
    let output = run(&[
        "developer-init",
        "--confirm-fresh",
        application.to_str().ok_or("path")?,
    ])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = application.join("config.json");
    let assets = fixture.path().join("browser");
    std::fs::create_dir(&assets)?;
    std::fs::create_dir(assets.join("assets"))?;
    std::fs::write(
        assets.join("index.html"),
        b"<!doctype html><title>real developer fixture</title>",
    )?;
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_domus"))
            .args(["developer-run", configuration])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
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
