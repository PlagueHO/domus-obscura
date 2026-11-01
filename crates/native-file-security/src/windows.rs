use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::ptr::{self, NonNull};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, OBJECT_SECURITY_INFORMATION,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
};

const MAX_SDDL_UNITS: usize = 131_072;

struct LocalAllocation(NonNull<c_void>);

impl LocalAllocation {
    fn from_sdk(pointer: *mut c_void) -> io::Result<Self> {
        NonNull::new(pointer)
            .map(Self)
            .ok_or_else(|| invalid("Windows returned an absent security buffer"))
    }
}

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: successful SDK calls transfer one LocalFree allocation here.
        // Its interior pointers never outlive this unique allocation guard.
        if !unsafe { LocalFree(self.0.as_ptr()) }.is_null() {
            eprintln!("Windows security buffer could not be released.");
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn status(code: u32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

fn read(file: &File, information: OBJECT_SECURITY_INFORMATION) -> io::Result<LocalAllocation> {
    let mut descriptor = ptr::null_mut();
    // SAFETY: the File borrow retains the handle. All optional outputs are null;
    // the SDK initializes the descriptor allocation on success.
    status(unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            information,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    })?;
    LocalAllocation::from_sdk(descriptor)
}

pub(super) fn descriptor(file: &File) -> io::Result<String> {
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let descriptor = read(file, information)?;
    let mut text = ptr::null_mut();
    // SAFETY: the descriptor is SDK-owned and remains live through conversion.
    // The SDK supplies a NUL-terminated UTF-16 allocation. Its optional buffer
    // size is not used as a string length.
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0.as_ptr(),
            SDDL_REVISION_1,
            information,
            &mut text,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let text = LocalAllocation::from_sdk(text.cast())?;
    let pointer = text.0.as_ptr().cast::<u16>();
    decode_sddl(
        (0..=MAX_SDDL_UNITS).map(|index| {
            // SAFETY: the SDK initializes the string through its terminator
            // within its allocation. decode_sddl stops there, or at our bound
            // while the remaining SDK string is still initialized.
            unsafe { pointer.add(index).read() }
        }),
        MAX_SDDL_UNITS as u32 + 1,
    )
}

fn decode_sddl(units: impl Iterator<Item = u16>, buffer_len: u32) -> io::Result<String> {
    if buffer_len == 0 || buffer_len as usize > MAX_SDDL_UNITS + 1 {
        return Err(invalid("Windows security string exceeds its bound"));
    }
    let mut value = Vec::new();
    for unit in units.take(buffer_len as usize) {
        if unit == 0 {
            return String::from_utf16(&value)
                .map_err(|_| invalid("Windows security string is not valid UTF-16"));
        }
        value.push(unit);
    }
    Err(invalid(
        "Windows security string exceeds its bound or has no terminator",
    ))
}

fn apply(file: &File, descriptor: &LocalAllocation) -> io::Result<()> {
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: the descriptor was returned by the SDK and all outputs are valid.
    // The DACL remains an interior borrow of descriptor through SetSecurityInfo.
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor.0.as_ptr(),
            &mut present,
            &mut dacl,
            &mut defaulted,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if present == 0 || dacl.is_null() {
        return Err(invalid("A missing or null DACL cannot be applied"));
    }
    // SAFETY: both the borrowed handle and SDK-owned DACL remain live. Only
    // DACL flags are selected; null owner/group/SACL outputs cannot change them.
    status(unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    })
}

pub(super) fn copy_dacl(source: &File, target: &File) -> io::Result<()> {
    let descriptor = read(source, DACL_SECURITY_INFORMATION)?;
    apply(target, &descriptor)
}

pub(super) fn set_dacl(file: &File, sddl: &str) -> io::Result<()> {
    if sddl.len() > MAX_SDDL_UNITS || sddl.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid SDDL input",
        ));
    }
    let text = sddl.encode_utf16().chain([0]).collect::<Vec<_>>();
    let mut descriptor = ptr::null_mut();
    // SAFETY: text is bounded and NUL-terminated; the SDK validates its contents
    // and initializes a LocalFree-owned descriptor on success.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = LocalAllocation::from_sdk(descriptor)?;
    apply(file, &descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_security_decoding_is_strict_and_stops_at_the_terminator() -> io::Result<()> {
        assert_eq!(decode_sddl([b'O' as u16, 0, 0xd800].into_iter(), 3)?, "O");
        assert_eq!(
            decode_sddl([0xd800, 0].into_iter(), 2)
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        assert!(decode_sddl([65].into_iter(), 1).is_err());
        Ok(())
    }

    #[test]
    fn native_security_decoding_checks_the_bound_before_reading() {
        for length in [0, MAX_SDDL_UNITS as u32 + 2, u32::MAX] {
            let never_read = std::iter::from_fn(|| panic!("Invalid length must not read memory"));
            assert!(decode_sddl(never_read, length).is_err());
        }
        let maximum = std::iter::repeat_n(65, MAX_SDDL_UNITS).chain([0]);
        assert_eq!(
            decode_sddl(maximum, MAX_SDDL_UNITS as u32 + 1)
                .map(|value| value.len())
                .ok(),
            Some(MAX_SDDL_UNITS)
        );
    }
}
