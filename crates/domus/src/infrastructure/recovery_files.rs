//! Flat, receipt-bound recovery I/O. Callers own root validation and operation locks.
//!
//! Linux unlink is parent-relative, but cannot atomically require a particular
//! inode. The immediate identity recheck therefore also requires cooperative
//! external writers: stop other writers before recovery. Advisory application
//! locks do not exclude an unrelated process with filesystem write access.

use super::filesystem::{is_unsafe_link, native_file_identity, sync_directory};
use super::permissions::FilePermissions;
use super::store::{RecoveryFile, ReplacementFile};
use crate::domain::LibraryPath;
use crate::{Error, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs::{File, Metadata};
use std::io::Read;

const MAX_FILES: usize = 1002;

fn conflict() -> Error {
    Error::Conflict("recovery content changed or contains an unsafe filesystem object".into())
}

fn component(name: &str) -> Result<()> {
    LibraryPath::parse(name)?;
    if name.is_empty() || name.contains('/') || name.ends_with(['.', ' ']) {
        return Err(conflict());
    }
    Ok(())
}

fn checked_directory(directory: &Dir) -> Result<String> {
    let handle = directory.try_clone()?.into_std_file();
    let metadata = handle.metadata()?;
    if !metadata.is_dir() || is_unsafe_link(&metadata) {
        return Err(conflict());
    }
    FilePermissions::current()?.check(&handle, false)?;
    #[cfg(windows)]
    drop(native_file_delete::ValidatedHandle::directory(
        handle.try_clone()?,
    )?);
    native_file_identity(&handle)
}

fn options(delete: bool, directory: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        let flags = rustix::fs::OFlags::NONBLOCK
            | if directory {
                rustix::fs::OFlags::DIRECTORY
            } else {
                rustix::fs::OFlags::empty()
            };
        options.custom_flags(flags.bits() as i32);
        let _ = delete;
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        // READ_CONTROL supports the actual-handle ACL check. No write or
        // delete sharing: the object cannot be replaced while it is inspected.
        options
            .access_mode(0x8002_0000 | if delete { 0x0001_0000 } else { 0 })
            .share_mode(1);
        if directory {
            options.custom_flags(0x0200_0000); // FILE_FLAG_BACKUP_SEMANTICS
        }
    }
    options
}

/// Enumerate only names; all content and permission evidence comes from handles.
fn names(directory: &Dir) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut folded = HashSet::new();
    for entry in directory.entries()? {
        if result.len() == MAX_FILES {
            return Err(conflict());
        }
        let name = entry?.file_name().into_string().map_err(|_| conflict())?;
        component(&name)?;
        if !folded.insert(name.to_uppercase()) {
            return Err(conflict());
        }
        result.push(name);
    }
    result.sort();
    Ok(result)
}

fn exact_name(directory: &Dir, name: &str) -> Result<()> {
    component(name)?;
    if !names(directory)?.iter().any(|entry| entry == name) {
        return Err(conflict());
    }
    Ok(())
}

fn regular(handle: &File) -> Result<Metadata> {
    let metadata = handle.metadata()?;
    if !metadata.is_file() || is_unsafe_link(&metadata) || metadata.len() > i64::MAX as u64 {
        return Err(conflict());
    }
    FilePermissions::current()?.check(handle, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(conflict());
        }
    }
    #[cfg(windows)]
    drop(native_file_delete::ValidatedHandle::file(
        handle.try_clone()?,
    )?);
    Ok(metadata)
}

fn unchanged(before: &Metadata, after: &Metadata) -> bool {
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return false;
        }
    }
    true
}

fn evidence(handle: &mut File, name: &str) -> Result<RecoveryFile> {
    let before = regular(handle)?;
    let identity = native_file_identity(handle)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65_536];
    let mut total = 0u64;
    loop {
        let count = handle.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(conflict)?;
        if total > before.len() {
            return Err(conflict());
        }
        hash.update(&buffer[..count]);
    }
    let after = regular(handle)?;
    if total != before.len()
        || !unchanged(&before, &after)
        || native_file_identity(handle)? != identity
    {
        return Err(conflict());
    }
    Ok(RecoveryFile {
        name: name.into(),
        bytes: total,
        sha256: format!("{:x}", hash.finalize()),
        identity,
    })
}

/// Verify an exact flat manifest, including full streams and fresh native IDs.
/// The recipe envelope permits 1000 tracks, one cover, and one recipe file.
pub(crate) fn inspect(
    directory: &Dir,
    expected: &[ReplacementFile],
) -> Result<(String, Vec<RecoveryFile>)> {
    if expected.len() > MAX_FILES {
        return Err(conflict());
    }
    let mut manifest = BTreeMap::new();
    let mut folded = HashSet::new();
    for entry in expected {
        component(&entry.name)?;
        if !folded.insert(entry.name.to_uppercase())
            || manifest.insert(entry.name.as_str(), entry).is_some()
        {
            return Err(conflict());
        }
    }
    let identity = checked_directory(directory)?;
    let listing = names(directory)?;
    if listing.len() != manifest.len()
        || listing
            .iter()
            .any(|name| !manifest.contains_key(name.as_str()))
    {
        return Err(conflict());
    }
    let mut files = Vec::with_capacity(listing.len());
    for name in &listing {
        let expected = manifest.get(name.as_str()).ok_or_else(conflict)?;
        let mut handle = directory
            .open_with(name, &options(false, false))?
            .into_std();
        let current = evidence(&mut handle, name)?;
        if current.bytes != expected.bytes || current.sha256 != expected.sha256 {
            return Err(conflict());
        }
        files.push(current);
    }
    if names(directory)? != listing || checked_directory(directory)? != identity {
        return Err(conflict());
    }
    Ok((identity, files))
}

/// Read one exact named regular file with a bounded hash buffer.
pub(crate) fn file(directory: &Dir, name: &str) -> Result<RecoveryFile> {
    checked_directory(directory)?;
    exact_name(directory, name)?;
    let mut handle = directory
        .open_with(name, &options(false, false))?
        .into_std();
    evidence(&mut handle, name)
}

/// Remove only the exact, still-complete file bound to this receipt.
/// No file contents are rewritten, including on an error.
pub(crate) fn delete_file(directory: &Dir, expected: &RecoveryFile) -> Result<()> {
    checked_directory(directory)?;
    exact_name(directory, &expected.name)?;
    let mut handle = directory
        .open_with(&expected.name, &options(true, false))?
        .into_std();
    if evidence(&mut handle, &expected.name)? != *expected {
        return Err(conflict());
    }
    #[cfg(windows)]
    native_file_delete::ValidatedHandle::file(handle)?.delete()?;
    #[cfg(unix)]
    {
        recheck_name(directory, &expected.name, &handle)?;
        rustix::fs::unlinkat(
            directory,
            expected.name.as_str(),
            rustix::fs::AtFlags::empty(),
        )
        .map_err(std::io::Error::from)?;
    }
    #[cfg(unix)]
    drop(handle);
    sync_directory(directory)
}

/// Remove one exact empty directory. Never recurse or remove an ancestor.
pub(crate) fn remove_directory(parent: &Dir, name: &str, identity: &str) -> Result<()> {
    checked_directory(parent)?;
    exact_name(parent, name)?;
    let handle = parent.open_with(name, &options(true, true))?.into_std();
    let directory = Dir::from_std_file(handle);
    if checked_directory(&directory)? != identity || directory.entries()?.next().is_some() {
        return Err(conflict());
    }
    let handle = directory.into_std_file();
    #[cfg(windows)]
    native_file_delete::ValidatedHandle::directory(handle)?.delete()?;
    #[cfg(unix)]
    {
        recheck_name(parent, name, &handle)?;
        rustix::fs::unlinkat(parent, name, rustix::fs::AtFlags::REMOVEDIR)
            .map_err(std::io::Error::from)?;
    }
    #[cfg(unix)]
    drop(handle);
    sync_directory(parent)
}

#[cfg(unix)]
fn recheck_name(parent: &Dir, name: &str, handle: &File) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let held = handle.metadata()?;
    let named = rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
        .map_err(std::io::Error::from)?;
    if named.st_dev != held.dev()
        || named.st_ino != held.ino()
        || named.st_size < 0
        || named.st_size as u64 != held.len()
        || (!held.is_dir() && named.st_nlink != 1)
    {
        return Err(conflict());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn fixture() -> std::result::Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        // Keep generated test state under the workspace, never an ambient root.
        let root = tempfile::Builder::new()
            .prefix(".recovery-files-test-")
            .tempdir_in(std::env::current_dir()?)?;
        super::super::permissions::private_fixture::directory(root.path())?;
        Ok(root)
    }

    fn open(path: &Path) -> Result<Dir> {
        Ok(Dir::open_ambient_dir(path, cap_std::ambient_authority())?)
    }

    fn write(directory: &Dir, name: &str, content: &[u8]) -> Result<ReplacementFile> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut handle = directory.open_with(name, &options)?;
        handle.write_all(content)?;
        handle.sync_all()?;
        Ok(ReplacementFile {
            name: name.into(),
            bytes: content.len() as u64,
            sha256: format!("{:x}", Sha256::digest(content)),
        })
    }

    #[test]
    fn exact_manifest_returns_sorted_fresh_evidence() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        let z = write(&directory, "z.flac", b"last")?;
        let a = write(&directory, "a.flac", b"first")?;
        let (identity, files) = inspect(&directory, &[z, a])?;
        assert_eq!(identity, checked_directory(&directory)?);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], file(&directory, "a.flac")?);
        assert_eq!(files[1], file(&directory, "z.flac")?);
        assert_ne!(files[0].identity, files[1].identity);
        Ok(())
    }

    #[test]
    fn manifest_rejects_missing_extra_changed_and_subdirectory() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        let expected = write(&directory, "track.flac", b"original")?;
        assert!(inspect(&directory, &[]).is_err());
        let mut wrong = expected.clone();
        wrong.bytes += 1;
        assert!(inspect(&directory, &[wrong]).is_err());
        let extra = write(&directory, "extra.flac", b"extra")?;
        assert!(inspect(&directory, std::slice::from_ref(&expected)).is_err());
        directory.remove_file(&extra.name)?;
        directory.create_dir("child")?;
        assert!(inspect(&directory, std::slice::from_ref(&expected)).is_err());
        directory.remove_dir("child")?;
        directory.write(&expected.name, b"modified")?;
        assert!(inspect(&directory, std::slice::from_ref(&expected)).is_err());
        directory.remove_file(&expected.name)?;
        assert!(inspect(&directory, &[expected]).is_err());
        Ok(())
    }

    #[test]
    fn deletion_requires_exact_identity_size_and_hash_and_removes_only_one_file() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        write(&directory, "track.flac", b"original")?;
        write(&directory, "keep.flac", b"keep")?;
        let expected = file(&directory, "track.flac")?;
        let mut wrong = expected.clone();
        wrong.identity.push('x');
        assert!(delete_file(&directory, &wrong).is_err());
        wrong = expected.clone();
        wrong.bytes += 1;
        assert!(delete_file(&directory, &wrong).is_err());
        wrong = expected.clone();
        wrong.sha256 = "0".repeat(64);
        assert!(delete_file(&directory, &wrong).is_err());
        assert_eq!(file(&directory, "track.flac")?, expected);
        delete_file(&directory, &expected)?;
        assert!(file(&directory, "track.flac").is_err());
        assert_eq!(directory.read("keep.flac")?, b"keep");
        assert!(delete_file(&directory, &expected).is_err());
        Ok(())
    }

    #[test]
    fn changed_receipt_is_not_deleted() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        write(&directory, "track.flac", b"original")?;
        let expected = file(&directory, "track.flac")?;
        directory.write("track.flac", b"modified")?;
        assert!(delete_file(&directory, &expected).is_err());
        assert_eq!(directory.read("track.flac")?, b"modified");
        Ok(())
    }

    #[test]
    fn identical_content_in_a_replacement_inode_is_not_deleted() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        write(&directory, "track.flac", b"original")?;
        let expected = file(&directory, "track.flac")?;
        // Keep the old object alive so this test does not depend on inode reuse.
        let held = std::fs::File::open(root.path().join("track.flac"))?;
        directory.remove_file("track.flac")?;
        write(&directory, "track.flac", b"original")?;
        assert!(delete_file(&directory, &expected).is_err());
        assert_eq!(directory.read("track.flac")?, b"original");
        drop(held);
        Ok(())
    }

    #[test]
    fn recipe_file_limit_is_exact_and_bounded() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        let mut expected = Vec::new();
        for index in 0..MAX_FILES {
            expected.push(write(&directory, &format!("{index:04}.flac"), b"")?);
        }
        assert_eq!(inspect(&directory, &expected)?.1.len(), MAX_FILES);
        write(&directory, "overflow.flac", b"")?;
        assert!(inspect(&directory, &expected).is_err());
        expected.push(ReplacementFile {
            name: "overflow.flac".into(),
            bytes: 0,
            sha256: format!("{:x}", Sha256::digest(b"")),
        });
        assert!(inspect(&directory, &expected).is_err());
        Ok(())
    }

    #[test]
    fn remove_directory_requires_exact_identity_and_empty_content() -> TestResult {
        let root = fixture()?;
        let parent = open(root.path())?;
        parent.create_dir("scratch")?;
        let directory = parent.open_dir("scratch")?;
        let identity = checked_directory(&directory)?;
        write(&directory, "keep.flac", b"keep")?;
        assert!(remove_directory(&parent, "scratch", &identity).is_err());
        delete_file(&directory, &file(&directory, "keep.flac")?)?;
        drop(directory);
        assert!(remove_directory(&parent, "scratch", "wrong-identity").is_err());
        remove_directory(&parent, "scratch", &identity)?;
        assert!(parent.symlink_metadata("scratch").is_err());
        Ok(())
    }

    #[test]
    fn invalid_components_and_case_aliases_are_rejected() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        write(&directory, "Track.flac", b"track")?;
        for name in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "track:stream",
            "track.",
            "track ",
        ] {
            assert!(file(&directory, name).is_err(), "{name:?}");
        }
        assert!(file(&directory, "track.flac").is_err());
        let expected = ReplacementFile {
            name: "track.flac".into(),
            bytes: 5,
            sha256: format!("{:x}", Sha256::digest(b"track")),
        };
        assert!(inspect(&directory, &[expected]).is_err());
        Ok(())
    }

    #[test]
    fn hardlinks_cannot_grant_content_authority() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        let expected = write(&directory, "track.flac", b"track")?;
        std::fs::hard_link(
            root.path().join("track.flac"),
            root.path().join("alias.flac"),
        )?;
        assert!(file(&directory, "track.flac").is_err());
        assert!(inspect(&directory, &[expected]).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_case_duplicates_and_untrusted_permissions_are_rejected() -> TestResult {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = fixture()?;
        let directory = open(root.path())?;
        let expected = write(&directory, "track.flac", b"track")?;
        symlink("track.flac", root.path().join("link.flac"))?;
        assert!(file(&directory, "link.flac").is_err());
        directory.remove_file("link.flac")?;
        write(&directory, "TRACK.FLAC", b"track")?;
        assert!(file(&directory, "track.flac").is_err());
        directory.remove_file("TRACK.FLAC")?;
        std::fs::set_permissions(
            root.path().join("track.flac"),
            std::fs::Permissions::from_mode(0o666),
        )?;
        assert!(file(&directory, "track.flac").is_err());
        assert!(inspect(&directory, &[expected]).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn directory_symlink_and_nonregular_file_are_not_traversed() -> TestResult {
        use std::os::unix::fs::symlink;
        let root = fixture()?;
        let parent = open(root.path())?;
        parent.create_dir("target")?;
        let target = parent.open_dir("target")?;
        let identity = checked_directory(&target)?;
        symlink("target", root.path().join("link"))?;
        assert!(remove_directory(&parent, "link", &identity).is_err());
        assert!(parent.open_dir("target").is_ok());
        parent.remove_file("link")?;
        rustix::fs::mknodat(
            &parent,
            "fifo",
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )?;
        assert!(file(&parent, "fifo").is_err());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn alternate_streams_and_concurrent_writers_are_rejected() -> TestResult {
        let root = fixture()?;
        let directory = open(root.path())?;
        let expected = write(&directory, "track.flac", b"track")?;
        std::fs::write(root.path().join("track.flac:hidden"), b"hidden")?;
        assert!(file(&directory, "track.flac").is_err());
        assert!(inspect(&directory, &[expected]).is_err());
        write(&directory, "other.flac", b"other")?;
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .open(root.path().join("other.flac"))?;
        assert!(file(&directory, "other.flac").is_err());
        drop(writer);
        assert!(file(&directory, "other.flac").is_ok());
        Ok(())
    }
}
