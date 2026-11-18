use domus::infrastructure::store::Store;
use serde_json::{Value, json};
use std::process::Command;

#[path = "support/private.rs"]
mod private;

#[test]
fn storage_root_inspection_is_read_only_and_refuses_escaping_or_missing_roots()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = private::tempdir()?;
    let media = fixture.path().join("media");
    std::fs::create_dir(&media)?;
    let sentinel = media.join("sentinel");
    std::fs::write(&sentinel, b"unchanged generated media")?;
    for (relative, success) in [("media", true), ("../media", false), ("missing", false)] {
        let output = Command::new(env!("CARGO_BIN_EXE_domus"))
            .arg("storage-root-inspect")
            .arg(fixture.path())
            .args(["generated-provider", "main", "3", "media", relative])
            .output()?;
        assert_eq!(
            output.status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if success {
            let selected: domus::infrastructure::filesystem::StorageRoot =
                serde_json::from_slice(&output.stdout)?;
            let (expected, _) = domus::infrastructure::filesystem::DirectoryExpectation::inspect(
                fixture.path(),
                &domus::domain::LibraryPath::parse("media")?,
            )?;
            assert_eq!(selected.expected, expected);
            assert_eq!(selected.binding_revision, 3);
            assert_eq!(selected.provider_scope, "generated-provider");
        }
        assert_eq!(std::fs::read(&sentinel)?, b"unchanged generated media");
        assert_eq!(std::fs::read_dir(fixture.path())?.count(), 1);
        assert_eq!(std::fs::read_dir(&media)?.count(), 1);
    }
    Ok(())
}

#[test]
fn configuration_check_reports_invalid_saved_input_without_startup_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = private::tempdir()?;
    let root = fixture.path();
    let database = root.join("state.db");
    drop(Store::open(&database)?);
    let database_before = std::fs::read(&database)?;
    let path = root.join("config.json");
    for read_only in [false, true] {
        let mut document = json!({
            "environment":"single_user", "system_settings_read_only":read_only,
            "storage":domus::infrastructure::filesystem::StorageConfiguration::inspect_initial(root, &domus::domain::LibraryPath::parse("")?)?,
            "database":"state.db", "listen":"127.0.0.1:8080", "origin":"http://127.0.0.1:8080",
            "services":{"discovery":[],"metadata":[],"artwork":[],"acquisition":[],"artist_images":[]}
        });
        for valid in [true, false] {
            if !valid {
                document["database"] = json!("missing.db");
            }
            let bytes = serde_json::to_vec(&document)?;
            std::fs::write(&path, &bytes)?;
            let output = Command::new(env!("CARGO_BIN_EXE_domus"))
                .arg("configuration-check")
                .arg(&path)
                .output()?;
            assert_eq!(
                output.status.success(),
                valid,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            if output.stdout.is_empty() {
                assert!(!valid);
                assert!(!output.stderr.is_empty());
            } else {
                let result: Value = serde_json::from_slice(&output.stdout)?;
                assert_eq!(
                    result["errors"]
                        .as_array()
                        .ok_or("missing errors")?
                        .is_empty(),
                    valid
                );
                assert!(result["state"].is_null());
            }
            assert_eq!(std::fs::read(&path)?, bytes);
            assert_eq!(std::fs::read(&database)?, database_before);
            assert!(!root.join("missing.db").exists());
            assert!(!root.join(".config.json.lock").exists());
            assert!(root.read_dir()?.all(|entry| {
                entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().ends_with(".pending"))
            }));
        }
    }
    std::fs::write(&path, b"{broken")?;
    let output = Command::new(env!("CARGO_BIN_EXE_domus"))
        .arg("configuration-check")
        .arg(&path)
        .output()?;
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&path)?, b"{broken");
    Ok(())
}
