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
fn local_storage_checks_the_opened_filesystem_and_refuses_links() -> Result<()> {
    let fixture = tempdir()?;
    let state = fixture.path().join("state");
    let link = fixture.path().join("linked");
    fs::create_dir(&state)?;
    let opened = LibraryDirectory::open(&state)?;
    assert_eq!(
        filesystem_class(&opened.directory_file()?)?,
        FilesystemClass::Local
    );
    require_local_disk(&state)?;
    require_local_disk(&state.join("not-created.db"))?;
    assert_eq!(fs::read_dir(&state)?.count(), 0);
    link_directory(&state, &link)?;
    assert!(require_local_disk(&link).is_err());
    assert!(require_local_disk(&link.join("not-created.db")).is_err());
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
        Err(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(32) => {}
        Err(error) => return Err(error.into()),
    }
    assert_eq!(names(&root)?, vec![OsString::from("inside.mp3")]);
    assert_eq!(
        fs::read(outside.path().join("library").join("sentinel"))?,
        b"outside"
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn child_retains_ancestor_pins_after_the_original_root_is_dropped() -> Result<()> {
    let fixture = tempdir()?;
    let parent = fixture.path().join("parent");
    let moved = fixture.path().join("moved");
    fs::create_dir_all(parent.join("album"))?;
    fs::write(parent.join("album").join("sentinel"), b"retained")?;
    let root = LibraryDirectory::open(&parent)?;
    let child = root.open_dir_nofollow("album")?.try_clone()?;
    drop(root);
    let error = fs::rename(&parent, &moved)
        .err()
        .ok_or(Error::Internal("retained Windows ancestor was renamed"))?;
    assert_eq!(error.raw_os_error(), Some(32));
    assert_eq!(names(&child)?, vec![OsString::from("sentinel")]);
    drop(child);
    fs::rename(&parent, &moved)?;
    assert_eq!(fs::read(moved.join("album").join("sentinel"))?, b"retained");
    Ok(())
}

#[cfg(unix)]
#[test]
fn retained_parent_metadata_survives_namespace_replacement() -> Result<()> {
    let fixture = tempdir()?;
    let parent = fixture.path().join("parent");
    let moved = fixture.path().join("moved");
    fs::create_dir_all(parent.join("album"))?;
    fs::write(parent.join("sentinel"), b"retained")?;
    let root = LibraryDirectory::open(&parent)?;
    let child = root.open_dir_nofollow("album")?.try_clone()?;
    drop(root);
    fs::rename(&parent, &moved)?;
    fs::create_dir_all(parent.join("album"))?;
    let retained_parent = child
        .parents
        .last()
        .ok_or(Error::Internal("child lost its retained parent"))?;
    assert!(retained_parent.symlink_metadata("sentinel")?.is_file());
    let mut handles = child
        .parents
        .iter()
        .map(Dir::try_clone)
        .collect::<std::io::Result<Vec<_>>>()?;
    handles.push(child.directory.try_clone()?);
    assert_eq!(NativeRoot::read(&handles)?, *child.native_root());
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
