//! Original, bounded Win32 ABI bindings. Only borrowed live handles cross FFI.
//!
//! ABI sources:
//! - https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle
//! - https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex
//! - https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_standard_info
//! - https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_stream_info
//! - https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_disposition_info
//! - https://learn.microsoft.com/windows/win32/api/winternl/nf-winternl-ntcreatefile
//! - https://learn.microsoft.com/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information
//! - https://learn.microsoft.com/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntsetinformationfile

use std::ffi::{OsStr, c_void};
use std::fs::File;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};

const FILE_STANDARD_INFO: i32 = 1;
const FILE_DISPOSITION_INFO: i32 = 4;
const FILE_STREAM_INFO: i32 = 7;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: *mut c_void,
    name: *mut UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_qos: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct IoStatus {
    status: usize,
    information: usize,
}

#[repr(C)]
struct RenameInfo {
    replace: u32,
    root: *mut c_void,
    length: u32,
    name: [u16; 1],
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        file: *mut *mut c_void,
        access: u32,
        attributes: *mut ObjectAttributes,
        status: *mut IoStatus,
        allocation: *const i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        extended_attributes: *const c_void,
        extended_bytes: u32,
    ) -> i32;
    fn NtSetInformationFile(
        file: *mut c_void,
        status: *mut IoStatus,
        information: *const c_void,
        bytes: u32,
        class: i32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

fn single_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let name = name.encode_wide().take(256).collect::<Vec<_>>();
    if name.is_empty()
        || name.len() > 255
        || name == [46]
        || name == [46, 46]
        || name.last().is_some_and(|unit| [32, 46].contains(unit))
        || name
            .iter()
            .any(|unit| *unit < 32 || [34, 42, 47, 58, 60, 62, 63, 92, 124].contains(unit))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "one bounded native name is required",
        ));
    }
    Ok(name)
}

pub(super) fn create_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    directory_handle(parent, name, true)
}

pub(super) fn open_directory_for_move(parent: &File, name: &OsStr) -> io::Result<File> {
    directory_handle(parent, name, false)
}

fn directory_handle(parent: &File, name: &OsStr, create: bool) -> io::Result<File> {
    let mut name = single_name(name)?;
    let length = (name.len() * 2) as u16;
    let mut text = UnicodeString {
        length,
        maximum_length: length,
        buffer: name.as_mut_ptr(),
    };
    let mut attributes = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root: parent.as_raw_handle(),
        name: &mut text,
        attributes: 0x1040, // OBJ_DONT_REPARSE | OBJ_CASE_INSENSITIVE.
        security_descriptor: std::ptr::null_mut(),
        security_qos: std::ptr::null_mut(),
    };
    let mut status = IoStatus::default();
    let mut handle = std::ptr::null_mut();
    // SAFETY: All ABI structures and the bounded UTF-16 name remain live through
    // synchronous opening. FILE_CREATE cannot open an existing object.
    // DIRECTORY_FILE and OPEN_REPARSE_POINT prohibit traversal through a link.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            if create { 0x0012_00a7 } else { 0x0013_00a1 },
            &mut attributes,
            &mut status,
            std::ptr::null(),
            0,
            if create { 3 } else { 1 },
            if create { 2 } else { 1 },
            0x0020_0021,
            std::ptr::null(),
            0,
        )
    };
    if result < 0 {
        // SAFETY: This pure conversion takes no pointers or owned resources.
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    if handle.is_null() || handle as isize == -1 {
        return Err(io::Error::other(
            "native directory operation returned no handle",
        ));
    }
    // SAFETY: A successful synchronous NtCreateFile transfers one owned handle.
    let file = unsafe { File::from_raw_handle(handle) };
    if result != 0 || status.information != if create { 2 } else { 1 } {
        return Err(io::Error::other(
            "native directory operation outcome is uncertain",
        ));
    }
    validate(&file, true)?;
    Ok(file)
}

pub(super) fn rename_directory_noreplace(
    source: &File,
    parent: &File,
    name: &OsStr,
) -> io::Result<()> {
    let name = single_name(name)?;
    let mut storage = [0u64; 128];
    let bytes = std::mem::size_of::<RenameInfo>() + name.len() * 2;
    let mut status = IoStatus::default();
    // SAFETY: Fixed storage is eight-byte aligned and larger than the maximum
    // header plus 255 UTF-16 units. All padding and ReplaceIfExists stay zero.
    // Both handles, the status block and storage outlive the synchronous call.
    let result = unsafe {
        let info = storage.as_mut_ptr().cast::<RenameInfo>();
        std::ptr::addr_of_mut!((*info).root).write(parent.as_raw_handle());
        std::ptr::addr_of_mut!((*info).length).write((name.len() * 2) as u32);
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).name).cast::<u16>(),
            name.len(),
        );
        NtSetInformationFile(
            source.as_raw_handle(),
            &mut status,
            info.cast(),
            bytes as u32,
            10,
        )
    };
    if result < 0 {
        // SAFETY: This pure conversion takes no pointers or owned resources.
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    if result != 0 {
        return Err(io::Error::other(
            "native directory rename outcome is uncertain",
        ));
    }
    Ok(())
}

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
    fn nt_create_structures_match_the_documented_pointer_width_layout() {
        let pointer = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<UnicodeString>(), pointer * 2);
        assert_eq!(std::mem::offset_of!(UnicodeString, buffer), pointer);
        assert_eq!(std::mem::size_of::<ObjectAttributes>(), pointer * 6);
        assert_eq!(std::mem::offset_of!(ObjectAttributes, root), pointer);
        assert_eq!(std::mem::size_of::<IoStatus>(), pointer * 2);
        assert_eq!(std::mem::offset_of!(RenameInfo, root), pointer);
        assert_eq!(std::mem::offset_of!(RenameInfo, name), pointer * 2 + 4);
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
