use std::error::Error;
use std::path::Path;
use std::process::{Command, Output};

fn prepare(cache: &Path) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new("cmake")
        .arg(format!("-DSOURCE_CACHE={}", cache.display()))
        .arg("-DOFFLINE=ON")
        .arg("-P")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("source.cmake"))
        .output()?)
}

#[test]
fn missing_offline_source_is_refused_without_creating_an_archive() -> Result<(), Box<dyn Error>> {
    let cache = tempfile::tempdir()?;
    let output = prepare(cache.path())?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Pinned SQLite source is not cached"));
    assert_eq!(std::fs::read_dir(cache.path())?.count(), 0);
    Ok(())
}

#[test]
fn changed_cached_archive_is_preserved_and_refused_before_extraction() -> Result<(), Box<dyn Error>>
{
    let cache = tempfile::tempdir()?;
    let archive = cache.path().join("libsqlite3-sys-0.38.2.crate");
    std::fs::write(&archive, b"changed source archive")?;
    let output = prepare(cache.path())?;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Pinned SQLite source checksum failed")
    );
    assert_eq!(std::fs::read(archive)?, b"changed source archive");
    assert_eq!(std::fs::read_dir(cache.path())?.count(), 1);
    Ok(())
}
