//! Windows descriptor I/O for borrowed, already-open file handles.
//!
//! There are no path APIs or permission-policy decisions. Callers retain their
//! no-follow component handles, locks, identity checks and authorization.
//! The implementation uses the documented Win32 SDK ABI, not dependency source.
//! Unsafe code is limited to the private FFI module and its OS-owned buffers.

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

/// Read the owner and DACL of this exact borrowed handle as bounded SDDL.
///
/// READ_CONTROL access is required. The returned string owns its contents;
/// every SDK allocation is released before the handle borrow ends.
#[cfg(windows)]
pub fn descriptor(file: &std::fs::File) -> std::io::Result<String> {
    windows::descriptor(file)
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
