//! Internal Windows effects on retained ordinary handles.
//!
//! There are no native path APIs. New directories return their original creation
//! handle, never a checked-name reopen. Callers must open each component without
//! following links, exclude write/delete sharing, check the actual handle's
//! permissions, and verify its full native identity and approved content before
//! calling `ValidatedHandle::delete`. This crate does not grant authorization.
//! Handle duplication is allowed; resolving a former pathname is not.
//!
//! The implementation is original code based on the documented Win32 ABI.
//! It does not copy dependency source or change the separately licensed file-id
//! patch. Unsafe code is limited to the private Windows FFI module.

#[cfg(windows)]
use std::{ffi::OsStr, fs::File, io};

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

/// Create one missing directory relative to an already retained ordinary parent.
///
/// The returned handle is the original creation handle. Existing names are
/// never opened or adopted. It excludes delete sharing and retains
/// traversal, enumeration, child-creation and permission-query rights.
/// The caller must check selected-root authority and application approval
/// before this effect, then check permissions on the returned handle.
#[cfg(windows)]
pub fn create_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    windows::validate(parent, true)?;
    windows::create_directory(parent, name)
}

/// Open one ordinary existing directory for a source-bound move, without effects.
///
/// The returned no-follow handle excludes write/delete sharing and has DELETE,
/// traversal, enumeration and permission-query access. The caller must retain
/// its selected parent chain and verify the full native identity, actual-handle
/// permissions and approved contents before renaming this exact handle.
#[cfg(windows)]
pub fn open_directory_for_move(parent: &File, name: &OsStr) -> io::Result<File> {
    windows::validate(parent, true)?;
    windows::open_directory_for_move(parent, name)
}

/// An owned ordinary Windows handle that passed structural deletion checks.
///
/// Construction has no filesystem effects. Dropping this value without calling
/// [`Self::delete`] only closes the handle. Each constructor rejects reparse
/// points and alternate streams; regular files must have exactly one hard link.
/// These checks do not replace the caller's root, ACL, identity, hash or lock
/// checks. Windows sharing exclusions must remain active through deletion.
#[cfg(windows)]
pub struct ValidatedHandle {
    file: File,
    directory: bool,
}

#[cfg(windows)]
impl ValidatedHandle {
    /// Move this exact verified directory to a missing name under a retained
    /// ordinary parent, then return its original, still-pinned handle.
    ///
    /// This does not reopen a checked source name, replace an existing target,
    /// change contents or grant approval. The caller must hold its application
    /// lock and saved move intent, validate root/parent authority and verify this
    /// same source handle before the call. Source DELETE access is required.
    /// An error is not proof of rollback; reconcile any uncertain native outcome.
    pub fn rename_directory_noreplace(self, parent: &File, name: &OsStr) -> io::Result<File> {
        if !self.directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a directory source is required",
            ));
        }
        windows::validate(&self.file, true)?;
        windows::validate(parent, true)?;
        windows::rename_directory_noreplace(&self.file, parent, name)?;
        Ok(self.file)
    }

    /// Validate and retain an already-open regular file without following paths.
    ///
    /// The caller must supply a no-follow handle opened without write/delete
    /// sharing. Read access is needed for validation; deletion also needs DELETE
    /// access. A read-only validation handle can be dropped without any effect.
    pub fn file(file: File) -> io::Result<Self> {
        windows::validate(&file, false)?;
        Ok(Self {
            file,
            directory: false,
        })
    }

    /// Validate and retain an already-open ordinary directory.
    ///
    /// The caller must supply a no-follow directory handle without write/delete
    /// sharing. Deletion also needs DELETE access. Only the empty directory
    /// itself can be deleted; Windows refuses a non-empty directory.
    pub fn directory(file: File) -> io::Result<Self> {
        windows::validate(&file, true)?;
        Ok(Self {
            file,
            directory: true,
        })
    }

    /// Revalidate, mark this exact owned handle for deletion, and close it.
    ///
    /// No pathname is opened, no contents or permissions are rewritten, and no
    /// recursive operation is performed. Errors leave the object unchanged.
    /// Read-only attributes are not cleared: Windows may refuse their deletion.
    /// Existing duplicate handles can defer final deletion until they close.
    ///
    /// The caller must verify authorization, full identity and content using
    /// the same native handle before this call, while holding its operation
    /// locks. A successful result is not proof of directory-entry durability;
    /// callers retain their platform-specific parent synchronization policy.
    pub fn delete(self) -> io::Result<()> {
        windows::validate(&self.file, self.directory)?;
        windows::delete(&self.file)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    fn fixture() -> io::Result<tempfile::TempDir> {
        tempfile::Builder::new()
            .prefix(".native-file-delete-test-")
            .tempdir_in(std::env::current_dir()?)
    }

    fn open(path: &Path, directory: bool, delete: bool) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .access_mode(0x8000_0000 | if delete { 0x0001_0000 } else { 0 })
            .share_mode(1)
            .custom_flags(0x0020_0000 | if directory { 0x0200_0000 } else { 0 })
            .open(path)
    }

    #[test]
    fn validated_handle_without_delete_has_no_effect() -> io::Result<()> {
        let root = fixture()?;
        let path = root.path().join("keep");
        fs::write(&path, b"unchanged")?;
        let file = open(&path, false, false)?;
        drop(ValidatedHandle::file(file)?);
        assert_eq!(fs::read(path)?, b"unchanged");
        Ok(())
    }

    #[test]
    fn new_directory_returns_the_original_pin_and_never_adopts_an_existing_name() -> io::Result<()>
    {
        let root = fixture()?;
        let parent = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .custom_flags(0x0220_0000)
            .open(root.path())?;
        let created = create_directory(&parent, OsStr::new("new"))
            .map_err(|error| io::Error::other(format!("create original directory: {error}")))?;
        let directory = root.path().join("new");
        fs::write(directory.join("keep"), b"generated")?;
        assert!(fs::rename(&directory, root.path().join("moved")).is_err());
        assert!(create_directory(&parent, OsStr::new("new")).is_err());
        assert_eq!(fs::read(directory.join("keep"))?, b"generated");
        drop(created);
        fs::rename(&directory, root.path().join("moved"))?;
        assert_eq!(
            fs::read(root.path().join("moved").join("keep"))?,
            b"generated"
        );
        Ok(())
    }

    #[test]
    fn new_directory_rejects_names_and_wrong_parent_without_effects() -> io::Result<()> {
        let root = fixture()?;
        let parent = open(root.path(), true, false)?;
        for name in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "a:stream",
            "trailing.",
            "trailing ",
        ] {
            assert!(create_directory(&parent, OsStr::new(name)).is_err());
        }
        assert!(create_directory(&parent, OsStr::new(&"a".repeat(256))).is_err());
        fs::write(root.path().join("file"), b"keep")?;
        let file = open(&root.path().join("file"), false, false)?;
        assert!(create_directory(&file, OsStr::new("child")).is_err());
        assert_eq!(fs::read_dir(root.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn native_move_keeps_the_verified_source_handle_and_refuses_existing_targets() -> io::Result<()>
    {
        let root = fixture()?;
        let parent = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .custom_flags(0x0220_0000)
            .open(root.path())?;
        let source = root.path().join("source");
        fs::create_dir(&source)?;
        fs::write(source.join("keep"), b"verified source")?;
        fs::write(root.path().join("collision"), b"existing")?;
        let held = open_directory_for_move(&parent, OsStr::new("source"))?;
        assert!(fs::rename(&source, root.path().join("swapped")).is_err());
        for name in ["collision", "../escape", "alternate:stream", "trailing."] {
            assert!(
                ValidatedHandle::directory(held.try_clone()?)?
                    .rename_directory_noreplace(&parent, OsStr::new(name))
                    .is_err()
            );
            assert_eq!(fs::read(source.join("keep"))?, b"verified source");
        }
        assert_eq!(fs::read(root.path().join("collision"))?, b"existing");
        let moved = ValidatedHandle::directory(held.try_clone()?)?
            .rename_directory_noreplace(&parent, OsStr::new("final"))?;
        assert!(!source.exists());
        assert_eq!(
            fs::read(root.path().join("final").join("keep"))?,
            b"verified source"
        );
        assert!(fs::rename(root.path().join("final"), &source).is_err());
        drop(moved);
        drop(held);
        fs::rename(root.path().join("final"), &source)?;
        assert!(
            ValidatedHandle::directory(open(&source, true, false)?)?
                .rename_directory_noreplace(&parent, OsStr::new("denied"))
                .is_err()
        );
        assert!(source.is_dir());
        assert!(!root.path().join("denied").exists());
        Ok(())
    }

    #[test]
    fn native_move_never_replaces_a_racing_file() -> io::Result<()> {
        use std::sync::{Arc, Barrier};
        for _ in 0..8 {
            let root = fixture()?;
            let parent = OpenOptions::new()
                .read(true)
                .share_mode(3)
                .custom_flags(0x0220_0000)
                .open(root.path())?;
            fs::create_dir(root.path().join("source"))?;
            fs::write(root.path().join("source").join("keep"), b"verified")?;
            let source = open_directory_for_move(&parent, OsStr::new("source"))?;
            let barrier = Arc::new(Barrier::new(2));
            let ready = barrier.clone();
            let target = root.path().join("target");
            let writer = std::thread::spawn(move || -> io::Result<bool> {
                ready.wait();
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                {
                    Ok(mut file) => {
                        std::io::Write::write_all(&mut file, b"racing")?;
                        Ok(true)
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::AlreadyExists
                            || (error.kind() == io::ErrorKind::PermissionDenied
                                && target.is_dir()) =>
                    {
                        Ok(false)
                    }
                    Err(error) => Err(error),
                }
            });
            barrier.wait();
            let moved = ValidatedHandle::directory(source)?
                .rename_directory_noreplace(&parent, OsStr::new("target"));
            if writer
                .join()
                .map_err(|_| io::Error::other("generated writer panicked"))??
            {
                assert!(moved.is_err());
                assert_eq!(fs::read(root.path().join("target"))?, b"racing");
                assert_eq!(
                    fs::read(root.path().join("source").join("keep"))?,
                    b"verified"
                );
            } else {
                drop(moved?);
                assert!(!root.path().join("source").exists());
                assert_eq!(
                    fs::read(root.path().join("target").join("keep"))?,
                    b"verified"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn delete_consumes_only_the_verified_file_handle() -> io::Result<()> {
        let root = fixture()?;
        let target = root.path().join("target");
        let keep = root.path().join("keep");
        fs::write(&target, b"verified")?;
        fs::write(&keep, b"keep")?;
        let mut file = open(&target, false, true)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        assert_eq!(content, b"verified");
        ValidatedHandle::file(file)?.delete()?;
        assert!(!target.exists());
        assert_eq!(fs::read(keep)?, b"keep");
        Ok(())
    }

    #[test]
    fn empty_directory_deletes_but_nonempty_directory_is_preserved() -> io::Result<()> {
        let root = fixture()?;
        let path = root.path().join("directory");
        fs::create_dir(&path)?;
        fs::write(path.join("keep"), b"keep")?;
        let directory = ValidatedHandle::directory(open(&path, true, true)?)?;
        assert!(directory.delete().is_err());
        assert_eq!(fs::read(path.join("keep"))?, b"keep");
        fs::remove_file(path.join("keep"))?;
        ValidatedHandle::directory(open(&path, true, true)?)?.delete()?;
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn hardlinks_and_alternate_streams_are_preserved() -> io::Result<()> {
        let root = fixture()?;
        let path = root.path().join("target");
        fs::write(&path, b"keep")?;
        fs::hard_link(&path, root.path().join("alias"))?;
        assert!(ValidatedHandle::file(open(&path, false, true)?).is_err());
        fs::remove_file(root.path().join("alias"))?;
        fs::write(root.path().join("target:hidden"), b"hidden")?;
        assert!(ValidatedHandle::file(open(&path, false, true)?).is_err());
        assert_eq!(fs::read(path)?, b"keep");
        Ok(())
    }

    #[test]
    fn wrong_kind_and_missing_delete_rights_fail_without_effects() -> io::Result<()> {
        let root = fixture()?;
        let path = root.path().join("target");
        fs::write(&path, b"keep")?;
        assert!(ValidatedHandle::directory(open(&path, false, true)?).is_err());
        assert!(ValidatedHandle::file(open(root.path(), true, false)?).is_err());
        assert!(
            ValidatedHandle::file(open(&path, false, false)?)?
                .delete()
                .is_err()
        );
        assert_eq!(fs::read(path)?, b"keep");
        Ok(())
    }
}
