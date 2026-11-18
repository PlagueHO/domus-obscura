use crate::FilesystemHandle;
use std::fs::File;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::fd::AsRawFd;

const CAPACITY: usize = 128;
const _: () = assert!(size_of::<libc::fsid_t>() == 8);

#[repr(C)]
struct HandleBuffer {
    handle_bytes: u32,
    handle_type: i32,
    bytes: [u8; CAPACITY],
}

pub(super) fn filesystem_handle(file: &File) -> io::Result<FilesystemHandle> {
    let metadata = file.metadata()?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem identity requires an opened regular file or directory",
        ));
    }

    let mut information = MaybeUninit::<libc::statfs>::zeroed();
    // The syscall initializes this libc ABI structure while the file stays borrowed.
    let status = unsafe { libc::fstatfs(file.as_raw_fd(), information.as_mut_ptr()) };
    if status == -1 {
        return Err(io::Error::last_os_error());
    }
    if status != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "filesystem query returned an unexpected status",
        ));
    }
    let information = unsafe { information.assume_init_ref() };
    // Linux fsid_t is the UAPI's pair of 32-bit integers. Keep its bytes opaque.
    let filesystem_id =
        unsafe { std::ptr::read_unaligned((&raw const information.f_fsid).cast::<[u8; 8]>()) };
    if filesystem_id == [0; 8] {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem did not supply an identifier",
        ));
    }

    let mut handle = HandleBuffer {
        handle_bytes: CAPACITY as u32,
        handle_type: 0,
        bytes: [0; CAPACITY],
    };
    let mut mount_id: libc::c_int = 0;
    // Empty-path lookup names only the borrowed object. All output storage is
    // initialized, correctly aligned and live until the synchronous call ends.
    let status = unsafe {
        libc::syscall(
            libc::SYS_name_to_handle_at,
            file.as_raw_fd(),
            c"".as_ptr(),
            &raw mut handle,
            &raw mut mount_id,
            libc::AT_EMPTY_PATH,
        )
    };
    if status == -1 {
        return Err(io::Error::last_os_error());
    }
    if status != 0 || handle.handle_bytes == 0 || handle.handle_bytes as usize > CAPACITY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "filesystem returned an invalid opaque handle",
        ));
    }
    Ok(FilesystemHandle {
        filesystem_id,
        handle_type: handle.handle_type,
        bytes: handle.bytes[..handle.handle_bytes as usize].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    fn open(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
    }

    #[test]
    fn filesystem_handle_keeps_borrowed_objects_distinct_from_replacements() -> io::Result<()> {
        let fixture = tempfile::Builder::new()
            .prefix(".native-filesystem-handle-")
            .tempdir_in(std::env::current_dir()?)?;
        for directory in [false, true] {
            let path = fixture.path().join("original");
            let retained = fixture.path().join("retained");
            if directory {
                fs::create_dir(&path)?;
                fs::write(path.join("owner.lock"), b"copied marker")?;
            } else {
                fs::write(&path, b"copied marker")?;
            }
            let file = open(&path)?;
            let expected = filesystem_handle(&file)?;
            fs::rename(&path, &retained)?;
            if directory {
                fs::create_dir(&path)?;
                fs::write(path.join("owner.lock"), b"copied marker")?;
            } else {
                fs::write(&path, b"copied marker")?;
            }
            assert_eq!(filesystem_handle(&file)?, expected);
            assert_ne!(filesystem_handle(&open(&path)?)?, expected);
            drop(file);
            assert_eq!(filesystem_handle(&open(&retained)?)?, expected);
            if directory {
                assert_eq!(fs::read(path.join("owner.lock"))?, b"copied marker");
                assert_eq!(fs::read(retained.join("owner.lock"))?, b"copied marker");
            } else {
                assert_eq!(fs::read(&path)?, b"copied marker");
                assert_eq!(fs::read(&retained)?, b"copied marker");
                fs::remove_file(path)?;
                fs::remove_file(retained)?;
            }
        }
        Ok(())
    }

    #[test]
    fn filesystem_handle_refuses_a_borrowed_symlink_without_following_it() -> io::Result<()> {
        let fixture = tempfile::Builder::new()
            .prefix(".native-filesystem-handle-")
            .tempdir_in(std::env::current_dir()?)?;
        let target = fixture.path().join("target");
        fs::write(&target, b"unchanged")?;
        let link = fixture.path().join("link");
        std::os::unix::fs::symlink(&target, &link)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(link)?;
        assert_eq!(
            filesystem_handle(&file).map_err(|error| error.kind()),
            Err(io::ErrorKind::InvalidInput)
        );
        assert_eq!(fs::read(target)?, b"unchanged");
        Ok(())
    }
}
