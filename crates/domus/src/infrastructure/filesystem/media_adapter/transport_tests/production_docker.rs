use super::*;
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs only generated state against an existing runtime image. The helper is
/// copied into a private volume, not added to the distributed runtime image.
#[tokio::test]
#[ignore = "requires the source-matched runtime image and exported Linux test helper"]
async fn media_adapter_production_docker_workflow() -> TestResult {
    let run = match std::env::var("DOMUS_MEDIA_WORKFLOW_RUN") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err("the generated workflow report nonce encoding is invalid".into()),
    };
    if run.as_ref().is_some_and(|value| {
        value.len() != 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err("the generated workflow report nonce is invalid".into());
    }
    let image = std::env::var("DOMUS_MEDIA_WORKFLOW_IMAGE")?;
    let helper =
        PathBuf::from(std::env::var_os("DOMUS_MEDIA_WORKFLOW_BINARY").ok_or(
            "DOMUS_MEDIA_WORKFLOW_BINARY must identify the exported Linux test executable",
        )?);
    let revision = std::fs::read_to_string(helper.with_extension("revision"))?;
    let revision = revision.trim().to_owned();
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the helper source revision is invalid".into());
    }
    let inspected = docker(&["image", "inspect", &image], None, 30)
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    let head = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .output()?;
    if !head.status.success()
        || !status.status.success()
        || !status.stdout.is_empty()
        || std::str::from_utf8(&head.stdout)?.trim() != revision
    {
        return Err("the coordinator checkout must match the clean helper source".into());
    }
    let inspected: serde_json::Value = serde_json::from_slice(&inspected)?;
    let candidate = &inspected[0];
    let image_id = candidate["Id"]
        .as_str()
        .ok_or("missing candidate image ID")?;
    if candidate["Os"] != "linux"
        || candidate["Architecture"] != "amd64"
        || candidate["Config"]["Labels"]["org.opencontainers.image.revision"] != revision
        || candidate["Config"]["Labels"]["io.domus.source.dirty"] != "false"
    {
        return Err("the runtime candidate must match the clean helper source".into());
    }
    let image_id = image_id.to_owned();
    let mut cases = Vec::new();
    for restore_old in [false, true] {
        let mut fixture = Fixture::new()?;
        let parent = fixture.selection.provider.boundary.join("parent");
        std::fs::create_dir(parent.join("backup"))?;
        let (expected, _) = DirectoryExpectation::inspect(
            &fixture.selection.provider.boundary,
            &crate::domain::LibraryPath::parse("parent/backup")?,
        )?;
        fixture.selection.media.push(StorageRoot {
            root_id: "backup".into(),
            binding_revision: 1,
            role: StorageRole::Media,
            scan_only: false,
            provider_scope: fixture.selection.provider.scope.clone(),
            relative: "parent/backup".into(),
            expected,
        });
        fixture.selection.writable_roots = vec!["library".into(), "backup".into()];
        let keys = Certificates::new()?;
        let running = Running::start(&fixture, &keys)?;
        // Confirm readiness through the same authenticated native channel first.
        let readiness = async {
            let mut client = keys.client(running.address)?;
            client.execute(&fixture.request()?).await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        if let Err(error) = readiness {
            running.stop().await?;
            fixture.directory.close()?;
            return Err(error);
        }
        let input = serde_json::to_vec(&json!({
            "revision": revision, "selection": fixture.selection.logical(),
            "address": running.address, "restore_old": restore_old,
            "ca": keys.ca, "server": keys.server,
            "client": keys.client, "key": keys.client_key,
        }))?;
        let selected_image = image_id.clone();
        let selected_helper = helper.clone();
        let result = tokio::task::spawn_blocking(move || {
            run_guest(&selected_image, &selected_helper, &input)
        })
        .await;
        running.stop().await?;
        let verification: TestResult = match result {
            Ok(Ok(evidence)) => verify_windows(&parent, restore_old, &evidence),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(error.into()),
        };
        fixture.directory.close()?;
        verification?;
        cases.push(if restore_old { "restored" } else { "published" });
    }
    let filename = run.as_ref().map_or_else(
        || "media-adapter-docker-workflow.json".to_owned(),
        |value| format!("media-adapter-docker-workflow-{value}.json"),
    );
    let report = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .ok_or("missing workspace")?
        .join("target")
        .join(filename);
    let content = serde_json::to_vec_pretty(&json!({
        "scope": "windows-docker-desktop-generated-media-workflow",
        "revision": revision, "image_id": image_id, "cases": cases,
        "run_nonce": run,
        "result": "passed", "native_windows_bytes": true,
        "native_windows_retained_absence": true,
    }))?;
    if run.is_some() {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(report)?
            .write_all(&content)?;
    } else {
        std::fs::write(report, content)?;
    }
    Ok(())
}

type SendResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn run_guest(image: &str, helper: &Path, input: &[u8]) -> SendResult<serde_json::Value> {
    let name = format!(
        "domus-media-workflow-{}",
        &super::super::Session::new()?.connection[..16]
    );
    let volume = format!("{name}-state");
    let seed = format!("{name}-seed");
    let mount = format!("type=volume,source={volume},target=/checks,volume-nocopy");
    let mut owned_containers = Vec::new();
    docker(&["volume", "create", &volume], None, 30)?;
    let result = (|| {
        docker(
            &[
                "create",
                "--pull",
                "never",
                "--network",
                "none",
                "--user",
                "0:0",
                "--name",
                &seed,
                "--mount",
                &mount,
                "--entrypoint",
                "/bin/sh",
                image,
                "-c",
                "set -eu; chmod 0700 /checks; cp /tmp/media-workflow /checks/media-workflow; chmod 0755 /checks/media-workflow; chown 1000:1000 /checks /checks/media-workflow",
            ],
            None,
            30,
        )?;
        owned_containers.push(seed.as_str());
        let helper = helper.to_str().ok_or("helper path is not UTF-8")?;
        docker(
            &["cp", helper, &format!("{seed}:/tmp/media-workflow")],
            None,
            30,
        )?;
        docker(&["start", "--attach", &seed], None, 30)?;
        docker(
            &[
                "create",
                "--pull",
                "never",
                "--read-only",
                "--network",
                "host",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges",
                "--memory",
                "1g",
                "--cpus",
                "1",
                "--pids-limit",
                "128",
                "--name",
                &name,
                "--mount",
                &mount,
                "-i",
                "--workdir",
                "/checks",
                "--entrypoint",
                "/checks/media-workflow",
                image,
                "media_adapter_production_guest_workflow",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ],
            None,
            30,
        )?;
        owned_containers.push(name.as_str());
        let output = docker(
            &["start", "--attach", "--interactive", &name],
            Some(input),
            180,
        )?;
        let output = std::str::from_utf8(&output)?;
        let mut records = output.lines().filter_map(|line| {
            line.split_once("DOMUS_MEDIA_WORKFLOW_RESULT=")
                .map(|(_, value)| value)
        });
        let evidence = records
            .next()
            .ok_or("guest workflow returned no evidence")?;
        if records.next().is_some() || !output.contains("1 passed; 0 failed") {
            return Err("guest workflow did not run exactly one successful case".into());
        }
        Ok(serde_json::from_str(evidence)?)
    })();
    let mut cleanup_errors = Vec::new();
    for container in owned_containers {
        if let Err(error) = docker(&["rm", "--force", container], None, 30) {
            cleanup_errors.push(error.to_string());
        }
    }
    if let Err(error) = docker(&["volume", "rm", &volume], None, 30) {
        cleanup_errors.push(error.to_string());
    }
    if !cleanup_errors.is_empty() {
        return Err(format!(
            "workflow result: {}; owned Docker cleanup failed: {}",
            result
                .as_ref()
                .err()
                .map_or("passed".into(), ToString::to_string),
            cleanup_errors.join("; ")
        )
        .into());
    }
    result
}

fn docker(arguments: &[&str], input: Option<&[u8]>, seconds: u64) -> SendResult<Vec<u8>> {
    let mut output = tempfile::tempfile()?;
    let mut child = Command::new("docker")
        .args(arguments)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(output.try_clone()?)
        .stderr(Stdio::inherit())
        .spawn()?;
    if let Some(input) = input {
        if input.len() > 262_144 {
            child.kill()?;
            child.wait()?;
            return Err("generated Docker input exceeds its limit".into());
        }
        let written = child
            .stdin
            .take()
            .ok_or("missing Docker input")?
            .write_all(input);
        if let Err(error) = written {
            child.kill()?;
            child.wait()?;
            return Err(error.into());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline || output.metadata()?.len() > 4 * 1024 * 1024 {
            child.kill()?;
            child.wait()?;
            return Err("owned Docker process exceeded its deadline or output limit".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    output.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    output.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if !status.success() || bytes.len() > 4 * 1024 * 1024 {
        return Err(format!(
            "Docker {} failed with {status}: {}",
            arguments[0],
            String::from_utf8_lossy(&bytes)
        )
        .into());
    }
    Ok(bytes)
}

fn verify_windows(parent: &Path, restored: bool, evidence: &serde_json::Value) -> TestResult {
    let relative = crate::domain::LibraryPath::parse(
        evidence["folder"]
            .as_str()
            .ok_or("missing visible folder")?,
    )?;
    let manifests = evidence["manifests"]
        .as_array()
        .ok_or("missing manifests")?;
    assert_eq!(manifests.len(), 2);
    for (kind, directory) in [("main", "media"), ("backup", "backup")] {
        let manifest = manifests
            .iter()
            .find(|item| item["kind"] == kind)
            .ok_or("missing root manifest")?;
        let folder = parent
            .join(directory)
            .join(relative.as_str().replace('/', "\\"));
        let files = manifest["files"]
            .as_array()
            .ok_or("missing file manifest")?;
        assert_eq!(std::fs::read_dir(&folder)?.count(), files.len());
        for file in files {
            let name = crate::domain::LibraryPath::parse(
                file["name"].as_str().ok_or("missing file name")?,
            )?;
            if name.as_str().contains('/') {
                return Err("generated manifest must be flat".into());
            }
            let bytes = std::fs::read(folder.join(name.as_str()))?;
            assert_eq!(Some(bytes.len() as u64), file["bytes"].as_u64());
            assert_eq!(
                Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
                file["sha256"].as_str()
            );
        }
        let actual = std::fs::read(folder.join("folder.jpg"))?;
        if restored {
            assert_eq!(actual, b"old generated cover");
            assert_eq!(
                std::fs::read(folder.join("manual.txt"))?,
                b"old generated extra"
            );
        } else {
            assert_eq!(actual, cover()?.bytes());
            assert!(!folder.join("manual.txt").exists());
        }
    }
    let removed = evidence["removed"]
        .as_array()
        .ok_or("missing removal evidence")?;
    assert_eq!(removed.len(), 2);
    for entry in removed {
        let root = match entry["kind"].as_str() {
            Some("main") => "media",
            Some("backup") => "backup",
            _ => return Err("unknown generated removal root".into()),
        };
        let relative = crate::domain::LibraryPath::parse(
            entry["folder"].as_str().ok_or("missing retained folder")?,
        )?;
        match std::fs::symlink_metadata(
            parent.join(root).join(relative.as_str().replace('/', "\\")),
        ) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("Windows retained folder absence was not confirmed".into()),
        }
    }
    Ok(())
}
