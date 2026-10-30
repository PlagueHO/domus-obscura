//! Original, bounded Win32 ABI bindings. Only borrowed live handles cross FFI.
//!
//! ABI sources:
//! - https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle
//! - https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex
//! - https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_standard_info
//! - https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_stream_info
//! - https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_disposition_info

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;

const FILE_STANDARD_INFO: i32 = 1;
const FILE_DISPOSITION_INFO: i32 = 4;
const FILE_STREAM_INFO: i32 = 7;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandleEx(
        file: *mut c_void,
        class: i32,
        information: *mut c_void,
        bytes: u32,
    ) -> i32;
    fn SetFileInformationByHandle(
        file: *mut c_void,
        class: i32,
        information: *const c_void,
        bytes: u32,
    ) -> i32;
}

#[repr(C)]
#[derive(Default)]
struct StandardInfo {
    allocation_size: i64,
    end_of_file: i64,
    number_of_links: u32,
    delete_pending: u8,
    directory: u8,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "deletion requires an ordinary handle without reparse points, hardlinks or alternate streams",
    )
}

pub(super) fn validate(file: &File, directory: bool) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.file_attributes() & 0x400 != 0
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err(invalid());
    }
    let mut info = StandardInfo::default();
    // SAFETY: StandardInfo has FILE_STANDARD_INFO's documented C layout.
    // The initialized writable storage and borrowed handle outlive this
    // synchronous call, which retains no pointers.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_STANDARD_INFO,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of::<StandardInfo>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if (!directory && info.number_of_links != 1)
        || info.delete_pending != 0
        || (info.directory != 0) != directory
    {
        return Err(invalid());
    }
    check_streams(file, directory)
}

fn check_streams(file: &File, directory: bool) -> io::Result<()> {
    // A fixed 64 KiB, eight-byte aligned buffer bounds native enumeration.
    // A larger response fails closed instead of allocating unbounded memory.
    let mut storage = [0u64; 8192];
    // SAFETY: The live buffer is aligned and writable for exactly 65536 bytes.
    // The borrowed handle and buffer outlive the synchronous call.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_STREAM_INFO,
            storage.as_mut_ptr().cast(),
            std::mem::size_of_val(&storage) as u32,
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        // An ordinary directory can have no data streams at all.
        if directory && error.raw_os_error() == Some(38) {
            return Ok(());
        }
        return Err(error);
    }
    // SAFETY: All bytes of this integer-array storage are initialized. The
    // immutable view borrows the same live allocation without alignment needs.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            storage.as_ptr().cast::<u8>(),
            std::mem::size_of_val(&storage),
        )
    };
    validate_stream_record(bytes)
}

fn validate_stream_record(bytes: &[u8]) -> io::Result<()> {
    // FILE_STREAM_INFO's fixed header is 24 bytes, then UTF-16 stream text.
    let fixed = bytes.get(..38).ok_or_else(invalid)?;
    let next = u32::from_ne_bytes(fixed[0..4].try_into().map_err(|_| invalid())?);
    let length = u32::from_ne_bytes(fixed[4..8].try_into().map_err(|_| invalid())?);
    let default = [b':' as u16, b':' as u16, b'$' as u16, 68, 65, 84, 65];
    if next != 0
        || length != 14
        || fixed[24..38]
            .as_chunks::<2>()
            .0
            .iter()
            .zip(default)
            .any(|(bytes, unit)| u16::from_ne_bytes(*bytes) != unit)
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn delete(file: &File) -> io::Result<()> {
    // FILE_DISPOSITION_INFO contains one BOOLEAN, not a four-byte BOOL.
    let delete_file: u8 = 1;
    // SAFETY: The initialized one-byte FILE_DISPOSITION_INFO and borrowed
    // handle remain live. Missing DELETE rights produce an OS error. This
    // synchronous call retains no pointer and never opens a pathname.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FILE_DISPOSITION_INFO,
            std::ptr::from_ref(&delete_file).cast(),
            1,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, next: u32) -> Vec<u8> {
        let name = name.encode_utf16().collect::<Vec<_>>();
        let mut bytes = vec![0; 24];
        bytes[0..4].copy_from_slice(&next.to_ne_bytes());
        bytes[4..8].copy_from_slice(&((name.len() * 2) as u32).to_ne_bytes());
        for unit in name {
            bytes.extend_from_slice(&unit.to_ne_bytes());
        }
        bytes
    }

    #[test]
    fn streams_require_one_complete_default_record() {
        assert!(validate_stream_record(&record("::$DATA", 0)).is_ok());
        assert!(validate_stream_record(&record("::$DATA", 40)).is_err());
        assert!(validate_stream_record(&record(":hidden:$DATA", 0)).is_err());
        for length in 0..38 {
            assert!(validate_stream_record(&vec![0; length]).is_err());
        }
    }

    #[test]
    fn standard_info_matches_the_documented_windows_layout() {
        assert_eq!(std::mem::size_of::<StandardInfo>(), 24);
        assert_eq!(std::mem::offset_of!(StandardInfo, number_of_links), 16);
        assert_eq!(std::mem::offset_of!(StandardInfo, delete_pending), 20);
        assert_eq!(std::mem::offset_of!(StandardInfo, directory), 21);
    }
}
