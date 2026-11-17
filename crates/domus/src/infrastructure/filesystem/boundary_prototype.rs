//! Development-only experiment. Candidate equality is not separation authority.
//! Only test builds include this experiment; production binding rules are unchanged.
use super::{LibraryDirectory, NativeRoot};
use crate::domain::LibraryPath;
use crate::{Error, Result};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CandidateBinding {
    boundary: String,
    chain: Vec<String>,
}

struct CandidateDirectory {
    _boundary: LibraryDirectory,
    directory: LibraryDirectory,
}

impl CandidateBinding {
    fn observed(boundary: &NativeRoot, root: &NativeRoot) -> Result<Self> {
        if boundary.ancestors.is_empty()
            || boundary.ancestors.last() != Some(&boundary.identity)
            || !root.ancestors.starts_with(&boundary.ancestors)
            || root.ancestors.last() != Some(&root.identity)
        {
            return Err(Error::Conflict(
                "prototype root is not relative to the retained boundary".into(),
            ));
        }
        Ok(Self {
            boundary: boundary.identity.clone(),
            chain: root.ancestors[boundary.ancestors.len() - 1..].to_vec(),
        })
    }

    fn open(boundary: &Path, relative: &str) -> Result<(Self, CandidateDirectory)> {
        let boundary = LibraryDirectory::open(boundary)?;
        let root = boundary.open_directory(&LibraryPath::parse(relative)?)?;
        let candidate = Self::observed(boundary.native_root(), root.native_root())?;
        Ok((
            candidate,
            CandidateDirectory {
                _boundary: boundary,
                directory: root,
            },
        ))
    }

    fn require(&self, boundary: &Path, relative: &str) -> Result<CandidateDirectory> {
        let (current, root) = Self::open(boundary, relative)?;
        if current != *self {
            return Err(Error::Conflict(
                "prototype boundary or directory chain changed".into(),
            ));
        }
        Ok(root)
    }

    fn ancestry_overlap(&self, other: &Self) -> bool {
        match (self.chain.last(), other.chain.last()) {
            (Some(left), Some(right)) => self.chain.contains(right) || other.chain.contains(left),
            _ => false,
        }
    }
}

fn native(chain: &[&str]) -> NativeRoot {
    NativeRoot {
        identity: chain.last().copied().unwrap_or("").into(),
        ancestors: chain.iter().map(|value| (*value).into()).collect(),
    }
}

#[test]
fn prototype_equality_excludes_only_ancestors_above_the_retained_boundary() -> TestResult {
    let first = native(&["container-a", "boundary", "parent", "media"]);
    let second = native(&["container-b", "boundary", "parent", "media"]);
    assert_ne!(first, second);
    let saved = CandidateBinding::observed(&native(&["container-a", "boundary"]), &first)?;
    let reopened = CandidateBinding::observed(&native(&["container-b", "boundary"]), &second)?;
    assert_eq!(saved, reopened);
    for changed in [
        native(&["container-b", "replacement-boundary", "parent", "media"]),
        native(&["container-b", "boundary", "replacement-parent", "media"]),
        native(&["container-b", "boundary", "parent", "replacement-media"]),
    ] {
        let boundary = NativeRoot {
            identity: changed.ancestors[1].clone(),
            ancestors: changed.ancestors[..2].to_vec(),
        };
        assert_ne!(saved, CandidateBinding::observed(&boundary, &changed)?);
    }
    assert!(CandidateBinding::observed(&native(&["unrelated"]), &first).is_err());
    Ok(())
}

#[test]
fn prototype_ancestry_cannot_prove_separation_across_hidden_mount_aliases() -> TestResult {
    let boundary = native(&["container", "boundary"]);
    let media = CandidateBinding::observed(
        &boundary,
        &native(&["container", "boundary", "media-native"]),
    )?;
    let staging = CandidateBinding::observed(
        &boundary,
        &native(&["container", "boundary", "staging-native"]),
    )?;
    let actual_staging_through_media = CandidateBinding::observed(
        &boundary,
        &native(&["container", "boundary", "media-native", "staging-native"]),
    )?;
    assert_eq!(
        staging.chain.last(),
        actual_staging_through_media.chain.last()
    );
    assert!(media.ancestry_overlap(&actual_staging_through_media));
    assert!(
        !media.ancestry_overlap(&staging),
        "the proposed saved chain misses an actual parent/child relationship"
    );
    assert!(media.ancestry_overlap(&media));
    Ok(())
}

#[test]
fn prototype_reopen_reads_from_the_verified_handle() -> TestResult {
    let fixture = tempfile::tempdir()?;
    std::fs::create_dir(fixture.path().join("media"))?;
    std::fs::write(fixture.path().join("media").join("sentinel"), b"generated")?;
    let (saved, first) = CandidateBinding::open(fixture.path(), "media")?;
    drop(first);
    let root = saved.require(fixture.path(), "media")?;
    let mut bytes = Vec::new();
    root.directory
        .open_file(std::ffi::OsStr::new("sentinel"))?
        .read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"generated");
    assert!(saved.require(fixture.path(), "../media").is_err());
    assert!(saved.require(fixture.path(), "missing").is_err());
    Ok(())
}

#[test]
fn prototype_rejects_replaced_boundary_intermediate_and_leaf_with_copied_markers() -> TestResult {
    for replaced in ["boundary", "parent", "media"] {
        let fixture = tempfile::tempdir()?;
        let boundary = fixture.path().join("boundary");
        let parent = boundary.join("parent");
        let media = parent.join("media");
        std::fs::create_dir_all(&media)?;
        std::fs::write(media.join("owner.lock"), b"copied marker is not authority")?;
        let (saved, opened) = CandidateBinding::open(&boundary, "parent/media")?;
        drop(opened);
        let target = match replaced {
            "boundary" => &boundary,
            "parent" => &parent,
            _ => &media,
        };
        let retained = fixture.path().join("retained");
        std::fs::rename(target, &retained)?;
        std::fs::create_dir_all(&media)?;
        std::fs::write(media.join("owner.lock"), b"copied marker is not authority")?;
        assert!(
            saved.require(&boundary, "parent/media").is_err(),
            "{replaced} replacement must fail"
        );
        assert_eq!(
            std::fs::read(media.join("owner.lock"))?,
            b"copied marker is not authority"
        );
    }
    Ok(())
}

#[test]
fn prototype_concurrent_fresh_processes_use_one_generated_sqlite_binding() -> TestResult {
    let fixture = tempfile::tempdir()?;
    std::fs::create_dir(fixture.path().join("media"))?;
    let token = format!("{:032x}", rand::random::<u128>());
    std::fs::write(fixture.path().join("fixture.token"), &token)?;
    std::fs::write(fixture.path().join("media").join("sentinel"), b"generated")?;
    let (binding, opened) = CandidateBinding::open(fixture.path(), "media")?;
    drop(opened);
    let database = fixture.path().join("prototype.db");
    let connection = Connection::open(&database)?;
    connection.execute_batch(
        "CREATE TABLE storage_boundary_prototype(id INTEGER PRIMARY KEY, token TEXT NOT NULL, binding TEXT NOT NULL);
         CREATE TABLE prototype_visits(child TEXT PRIMARY KEY);",
    )?;
    connection.execute(
        "INSERT INTO storage_boundary_prototype VALUES (1,?1,?2)",
        params![token, serde_json::to_string(&binding)?],
    )?;
    drop(connection);
    let executable = std::env::current_exe()?;
    let mut children = Vec::new();
    for role in ["api-prototype", "worker-prototype", "cli-prototype"] {
        children.push(
            Command::new(&executable)
                .args([
                    "--ignored",
                    "--exact",
                    "infrastructure::filesystem::boundary_prototype::boundary_prototype_child",
                    "--nocapture",
                ])
                .env("DOMUS_PROTOTYPE_FIXTURE", fixture.path())
                .env("DOMUS_PROTOTYPE_TOKEN", &token)
                .env("DOMUS_PROTOTYPE_CHILD", role)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?,
        );
    }
    let outputs = children
        .into_iter()
        .map(|child| child.wait_with_output())
        .collect::<std::io::Result<Vec<_>>>()?;
    for output in outputs {
        assert!(
            output.status.success(),
            "prototype child failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    assert_eq!(
        connection.query_row("SELECT count(*) FROM prototype_visits", [], |row| row
            .get::<_, i64>(0))?,
        3
    );
    Ok(())
}

#[test]
#[ignore = "fixture entry point; launched by the concurrent-process test"]
fn boundary_prototype_child() -> TestResult {
    let fixture = std::env::var_os("DOMUS_PROTOTYPE_FIXTURE").ok_or("missing fixture")?;
    let token = std::env::var("DOMUS_PROTOTYPE_TOKEN")?;
    let role = std::env::var("DOMUS_PROTOTYPE_CHILD")?;
    let fixture = Path::new(&fixture);
    let root = LibraryDirectory::open(fixture)?;
    let mut observed_token = String::new();
    root.open_file(std::ffi::OsStr::new("fixture.token"))?
        .take(128)
        .read_to_string(&mut observed_token)?;
    assert_eq!(observed_token, token);
    let connection = Connection::open_with_flags(
        fixture.join("prototype.db"),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let (saved_token, binding): (String, String) = connection.query_row(
        "SELECT token, binding FROM storage_boundary_prototype WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(saved_token, token);
    let binding: CandidateBinding = serde_json::from_str(&binding)?;
    let media = binding.require(fixture, "media")?;
    let mut bytes = Vec::new();
    media
        .directory
        .open_file(std::ffi::OsStr::new("sentinel"))?
        .take(128)
        .read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"generated");
    connection.execute("INSERT INTO prototype_visits(child) VALUES (?1)", [role])?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let count: i64 =
            connection.query_row("SELECT count(*) FROM prototype_visits", [], |row| {
                row.get(0)
            })?;
        if count == 3 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "other fixture processes did not arrive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(media);
    drop(root);
    Ok(())
}
