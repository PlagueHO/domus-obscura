use super::*;
use std::ffi::OsString;
use tempfile::tempdir;

fn link_directory(target: &Path, link: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()?;
        if !output.status.success() {
            return Err(Error::Conflict(format!(
                "fixture junction creation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

fn names(directory: &LibraryDirectory) -> Result<Vec<OsString>> {
    Ok(directory
        .entries()?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?)
}

#[test]
fn root_open_preserves_parent_components_and_rejects_files() -> Result<()> {
    let media = tempdir()?;
    fs::create_dir(media.path().join("ALBUM"))?;
    fs::write(media.path().join("track.mp3"), b"original")?;
    assert_eq!(
        canonical_root(&media.path().join("ALBUM").join(".."))?,
        fs::canonicalize(media.path())?
    );
    assert!(LibraryDirectory::open(Path::new(".")).is_err());
    assert!(LibraryDirectory::open(&media.path().join("track.mp3")).is_err());
    Ok(())
}

#[test]
fn roots_and_intermediate_components_never_follow_links() -> Result<()> {
    let media = tempdir()?;
    let outside = tempdir()?;
    fs::create_dir(outside.path().join("ALBUM"))?;
    fs::write(outside.path().join("sentinel"), b"outside")?;
    let link = media.path().join("linked");
    link_directory(outside.path(), &link)?;
    assert!(canonical_root(&link).is_err());
    assert!(canonical_root(&link.join("ALBUM")).is_err());
    assert!(canonical_root(&link.join("..")).is_err());
    let root = LibraryDirectory::open(media.path())?;
    assert!(
        root.open_directory(&LibraryPath::parse("linked/ALBUM")?)
            .is_err()
    );
    assert_eq!(fs::read(outside.path().join("sentinel"))?, b"outside");
    Ok(())
}

#[test]
fn queued_directory_replacement_cannot_read_the_link_target() -> Result<()> {
    let media = tempdir()?;
    let outside = tempdir()?;
    let album = media.path().join("ALBUM");
    fs::create_dir(&album)?;
    fs::write(outside.path().join("sentinel"), b"outside")?;
    let root = LibraryDirectory::open(media.path())?;
    let entries = root.entries()?.collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(entries.len(), 1);
    assert!(entries[0].metadata()?.is_dir());
    fs::rename(&album, media.path().join("moved"))?;
    link_directory(outside.path(), &album)?;
    assert!(root.open_directory(&LibraryPath::parse("ALBUM")?).is_err());
    assert_eq!(fs::read(outside.path().join("sentinel"))?, b"outside");
    Ok(())
}

#[test]
fn open_directory_stays_bound_when_its_path_is_replaced() -> Result<()> {
    let data = tempdir()?;
    let outside = tempdir()?;
    let media = data.path().join("library");
    fs::create_dir(&media)?;
    fs::write(media.join("inside.mp3"), b"inside")?;
    fs::write(outside.path().join("sentinel"), b"outside")?;
    let root = LibraryDirectory::open(&media)?;
    let renamed = fs::rename(&media, data.path().join("moved"));
    #[cfg(windows)]
    assert!(
        renamed.is_err(),
        "the Windows directory handle must deny delete sharing"
    );
    #[cfg(unix)]
    {
        renamed?;
        link_directory(outside.path(), &media)?;
    }
    assert_eq!(names(&root)?, vec![OsString::from("inside.mp3")]);
    assert_eq!(fs::read(outside.path().join("sentinel"))?, b"outside");
    Ok(())
}

#[test]
fn open_directory_stays_bound_after_an_ancestor_rename() -> Result<()> {
    let data = tempdir()?;
    let outside = tempdir()?;
    let ancestor = data.path().join("parent");
    let media = ancestor.join("library");
    fs::create_dir_all(&media)?;
    fs::write(media.join("inside.mp3"), b"inside")?;
    fs::create_dir(outside.path().join("library"))?;
    fs::write(outside.path().join("library").join("sentinel"), b"outside")?;
    let root = LibraryDirectory::open(&media)?;
    match fs::rename(&ancestor, data.path().join("moved")) {
        Ok(()) => link_directory(outside.path(), &ancestor)?,
        #[cfg(windows)]
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(error) => return Err(error.into()),
    }
    assert_eq!(names(&root)?, vec![OsString::from("inside.mp3")]);
    assert_eq!(
        fs::read(outside.path().join("library").join("sentinel"))?,
        b"outside"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn directory_sync_keeps_root_and_child_handles_after_path_replacement() -> Result<()> {
    let data = tempdir()?;
    let media = data.path().join("library");
    fs::create_dir_all(media.join("album"))?;
    let root = LibraryDirectory::open(&media)?;
    let child = root.open_directory(&LibraryPath::parse("album")?)?;
    fs::rename(&media, data.path().join("moved"))?;
    fs::write(&media, b"replacement, not a directory")?;
    sync_directory(&root.directory)?;
    sync_directory(&child.directory)?;
    assert_eq!(fs::read(&media)?, b"replacement, not a directory");
    Ok(())
}
