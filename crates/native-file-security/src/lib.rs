//! Native security and filesystem facts for borrowed, already-open handles.
//!
//! There are no path APIs or permission-policy decisions. Callers retain their
//! no-follow component handles, locks, identity checks and authorization.
//! Implementations use documented operating-system interfaces, not dependency
//! source. Unsafe code is limited to private FFI modules and bounded OS buffers.

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux;

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

/// Opaque Linux exportfs identity within the caller's trusted provider scope.
///
/// The filesystem identifier is native ABI data, not a physical-device identity.
/// Mount IDs are deliberately excluded because they are transient observations.
/// These bytes do not prove locality, ancestry or separation from another root.
#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemHandle {
    pub filesystem_id: [u8; 8],
    pub handle_type: i32,
    pub bytes: Vec<u8>,
}

/// Identify this exact borrowed regular file or directory without a pathname.
///
/// Uses `name_to_handle_at` with `AT_EMPTY_PATH`, never `open_by_handle_at`.
/// Unsupported filesystems, unavailable identifiers and syscall restrictions
/// return errors. There is no inode-only or former-path fallback.
#[cfg(target_os = "linux")]
pub fn filesystem_handle(file: &std::fs::File) -> std::io::Result<FilesystemHandle> {
    linux::filesystem_handle(file)
}

/// Filesystem facts collected from an existing file or directory handle.
///
/// These describe the logical Windows filesystem, not its physical backing.
/// A remote filesystem does not have a name from the local-volume API.
/// Callers, not this adapter, decide which filesystem profiles are supported.
#[cfg(windows)]
#[derive(Debug, Eq, PartialEq)]
pub struct FilesystemInformation {
    pub device_type: u32,
    pub remote: bool,
    pub name: Option<String>,
}

/// Query the actual filesystem without resolving the handle's former path.
///
/// An independent synchronous handle keeps query completion and temporary
/// output buffers confined to this call. The original handle remains borrowed.
#[cfg(windows)]
pub fn filesystem_information(file: &std::fs::File) -> std::io::Result<FilesystemInformation> {
    windows::filesystem_information(file)
}

/// Read the owner and DACL of this exact borrowed handle as bounded SDDL.
///
/// READ_CONTROL access is required. The returned string owns its contents;
/// every SDK allocation is released before the handle borrow ends.
#[cfg(windows)]
pub fn descriptor(file: &std::fs::File) -> std::io::Result<String> {
    windows::descriptor(file)
}

/// Read bounded owner/DACL information from a borrowed Windows pipe handle.
/// This does not close the handle or change its access policy.
#[cfg(windows)]
pub fn pipe_descriptor(pipe: &impl std::os::windows::io::AsRawHandle) -> std::io::Result<String> {
    windows::pipe_descriptor(pipe)
}

/// Create a remote-refusing asynchronous pipe with the caller's explicit ACL.
/// The native descriptor remains owned through synchronous handle creation.
#[cfg(windows)]
pub fn create_named_pipe(
    path: &std::path::Path,
    sddl: &str,
    first: bool,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    windows::create_named_pipe(path, sddl, first)
}

/// Copy only the DACL between these exact borrowed handles and protect it.
///
/// The caller must validate source policy and authorize the target mutation.
/// WRITE_DAC is required on the target. Owner and SACL remain unchanged.
/// A missing or null DACL is an error, never an unrestricted grant.
#[cfg(windows)]
pub fn copy_dacl(source: &std::fs::File, target: &std::fs::File) -> std::io::Result<()> {
    windows::copy_dacl(source, target)
}

/// Set a protected, non-null DACL from SDDL on an already-open handle.
///
/// Callers must authorize the mutation and supply WRITE_DAC access. The owner
/// and SACL remain unchanged. Directory ACE inheritance follows Windows rules;
/// fixture callers must use only fresh generated directories.
#[cfg(windows)]
pub fn set_dacl(file: &std::fs::File, sddl: &str) -> std::io::Result<()> {
    windows::set_dacl(file, sddl)
}

#[cfg(all(test, windows))]
mod native_security {
    use super::*;
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    fn fixture() -> io::Result<tempfile::TempDir> {
        tempfile::Builder::new()
            .prefix(".native-file-security-test-")
            .tempdir_in(std::env::current_dir()?)
    }

    fn open(path: &Path, access: u32) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .access_mode(access)
            .custom_flags(0x0020_0000)
            .open(path)
    }

    fn owner(sddl: &str) -> io::Result<&str> {
        sddl.strip_prefix("O:")
            .and_then(|value| value.split_once("D:"))
            .map(|(owner, _)| owner)
            .ok_or_else(|| io::Error::other("Generated fixture has no owner"))
    }

    #[test]
    fn filesystem_query_keeps_the_borrowed_handle_and_ignores_path_replacement() -> io::Result<()> {
        let root = fixture()?;
        let path = root.path().join("original");
        fs::write(&path, b"retained")?;
        let file = open(&path, 0x8000_0000)?;
        let before = filesystem_information(&file)?;
        assert!(!before.remote);
        assert_eq!(before.device_type, 7);
        assert!(before.name.as_ref().is_some_and(|name| !name.is_empty()));
        fs::rename(&path, root.path().join("moved"))?;
        fs::write(&path, b"replacement")?;
        assert_eq!(filesystem_information(&file)?, before);
        assert_eq!(file.metadata()?.len(), 8);
        assert_eq!(fs::read(path)?, b"replacement");
        Ok(())
    }

    #[test]
    fn filesystem_query_reopens_async_directory_handles_after_rename() -> io::Result<()> {
        let root = fixture()?;
        let path = root.path().join("original");
        fs::create_dir(&path)?;
        let directory = OpenOptions::new()
            .access_mode(0x0010_0080)
            .share_mode(7)
            .custom_flags(0x4220_0000)
            .open(&path)?;
        let before = filesystem_information(&directory)?;
        assert!(!before.remote);
        assert_eq!(before.device_type, 7);
        assert!(before.name.as_ref().is_some_and(|name| !name.is_empty()));
        fs::rename(&path, root.path().join("moved"))?;
        fs::write(&path, b"not the retained directory")?;
        assert_eq!(filesystem_information(&directory)?, before);
        assert!(directory.metadata()?.is_dir());
        assert_eq!(fs::read(path)?, b"not the retained directory");
        Ok(())
    }

    #[test]
    fn copy_changes_only_the_protected_dacl_and_keeps_both_file_contents() -> io::Result<()> {
        let root = fixture()?;
        let source_path = root.path().join("source");
        let target_path = root.path().join("target");
        fs::write(&source_path, b"source content")?;
        fs::write(&target_path, b"target content")?;
        let source = open(&source_path, 0x8004_0000)?;
        let target = open(&target_path, 0x8004_0000)?;
        let source_before = descriptor(&source)?;
        let target_before = descriptor(&target)?;
        set_dacl(
            &source,
            &format!("O:SYG:SYD:P(A;;FA;;;{})", owner(&source_before)?),
        )?;
        let source_after = descriptor(&source)?;
        assert_eq!(owner(&source_before)?, owner(&source_after)?);
        copy_dacl(&source, &target)?;
        let target_after = descriptor(&target)?;
        assert_eq!(owner(&target_before)?, owner(&target_after)?);
        assert_eq!(
            source_after.split_once("D:").map(|(_, dacl)| dacl),
            target_after.split_once("D:").map(|(_, dacl)| dacl)
        );
        assert!(
            target_after
                .split_once("D:")
                .is_some_and(|(_, dacl)| dacl.starts_with('P') && dacl.contains('(')),
            "{target_after}"
        );
        drop((source, target));
        assert_eq!(fs::read(source_path)?, b"source content");
        assert_eq!(fs::read(target_path)?, b"target content");
        Ok(())
    }

    #[test]
    fn malformed_missing_null_and_oversized_dacls_leave_the_target_unchanged() -> io::Result<()> {
        let root = fixture()?;
        let path = root.path().join("keep");
        fs::write(&path, b"keep")?;
        let file = open(&path, 0x8004_0000)?;
        let before = descriptor(&file)?;
        for invalid in [
            "not SDDL",
            "O:SY",
            "D:NO_ACCESS_CONTROL",
            "D:P(A;;FA;;;SY)\0D:NO_ACCESS_CONTROL",
            &"D".repeat(131_073),
        ] {
            assert!(set_dacl(&file, invalid).is_err());
            assert_eq!(descriptor(&file)?, before);
        }
        drop(file);
        assert_eq!(fs::read(path)?, b"keep");
        Ok(())
    }

    #[test]
    fn empty_dacl_is_preserved_instead_of_becoming_an_unrestricted_grant() -> io::Result<()> {
        let root = fixture()?;
        let source_path = root.path().join("source");
        let target_path = root.path().join("target");
        fs::write(&source_path, b"source")?;
        fs::write(&target_path, b"target")?;
        let source = open(&source_path, 0x8004_0000)?;
        let target = open(&target_path, 0x8004_0000)?;
        let source_restore = format!("D:P(A;;FA;;;{})", owner(&descriptor(&source)?)?);
        let target_restore = format!("D:P(A;;FA;;;{})", owner(&descriptor(&target)?)?);
        set_dacl(&source, "D:P")?;
        let copied = copy_dacl(&source, &target);
        let source_checked = descriptor(&source);
        let target_checked = descriptor(&target);
        // Restore access before assertions or filesystem-based fixture cleanup.
        set_dacl(&source, &source_restore)?;
        set_dacl(&target, &target_restore)?;
        copied?;
        for checked in [source_checked?, target_checked?] {
            assert!(
                checked
                    .split_once("D:")
                    .is_some_and(|(_, dacl)| dacl.starts_with('P') && !dacl.contains('(')),
                "{checked}"
            );
        }
        Ok(())
    }

    #[test]
    fn descriptor_and_mutation_require_the_rights_on_the_actual_handle() -> io::Result<()> {
        let root = fixture()?;
        let path = root.path().join("rights");
        fs::write(&path, b"keep")?;
        let readable = open(&path, 0x8000_0000)?;
        let no_control = open(&path, 0x80)?;
        let before = descriptor(&readable)?;
        let acl = format!("D:P(A;;FA;;;{})", owner(&before)?);
        assert!(descriptor(&no_control).is_err());
        assert!(set_dacl(&readable, &acl).is_err());
        assert!(copy_dacl(&readable, &readable).is_err());
        assert_eq!(descriptor(&readable)?, before);
        Ok(())
    }

    #[test]
    fn renamed_handles_never_resolve_their_former_paths() -> io::Result<()> {
        let root = fixture()?;
        let old = root.path().join("old");
        let moved = root.path().join("moved");
        fs::write(&old, b"original")?;
        let file = open(&old, 0x8004_0000)?;
        let before = descriptor(&file)?;
        fs::rename(&old, &moved)?;
        fs::write(&old, b"replacement")?;
        let replacement = open(&old, 0x8000_0000)?;
        let replacement_before = descriptor(&replacement)?;
        set_dacl(&file, &format!("D:P(A;;FA;;;{})", owner(&before)?))?;
        assert!(
            descriptor(&file)?
                .split_once("D:")
                .is_some_and(|(_, dacl)| dacl.starts_with('P') && dacl.contains('('))
        );
        assert_eq!(descriptor(&replacement)?, replacement_before);
        drop((file, replacement));
        assert_eq!(fs::read(moved)?, b"original");
        assert_eq!(fs::read(old)?, b"replacement");
        Ok(())
    }
}
