//! Internal Windows deletion boundary for already-open, validated file handles.
//!
//! There are no native path APIs. Callers must open each component without
//! following links, exclude write/delete sharing, check the actual handle's
//! permissions, and verify its full native identity and approved content before
//! calling `ValidatedHandle::delete`. This crate does not grant authorization.
//! Handle duplication is allowed; resolving a former pathname is not.
//!
//! The implementation is original code based on the documented Win32 ABI.
//! It does not copy dependency source or change the separately licensed file-id
//! patch. Unsafe code is limited to the private Windows FFI module.

#[cfg(windows)]
use std::{fs::File, io};

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

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
