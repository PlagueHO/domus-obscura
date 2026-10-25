use crate::{Error, Result};
use std::fs::File;

/// Permission checks use the opened object, never a second pathname lookup.
/// Administrators/root remain trusted operating-system authorities.
pub(crate) struct FilePermissions {
    #[cfg(windows)]
    user: String,
    #[cfg(unix)]
    user: u32,
}

impl FilePermissions {
    pub(crate) fn current() -> Result<Self> {
        #[cfg(windows)]
        {
            use winsafe::co;
            let token = winsafe::HPROCESS::GetCurrentProcess()
                .OpenProcessToken(co::TOKEN::QUERY)
                .map_err(std::io::Error::other)?;
            let information = token
                .GetTokenInformation(co::TOKEN_INFORMATION_CLASS::User)
                .map_err(std::io::Error::other)?;
            let winsafe::TokenInfo::User(user) = information else {
                return Err(Error::Internal(
                    "Windows did not return the process token user",
                ));
            };
            let user = user
                .User
                .Sid()
                .ok_or(Error::Internal("Windows token user SID is absent"))?;
            Ok(Self {
                user: user.to_string(),
            })
        }
        #[cfg(unix)]
        {
            Ok(Self {
                user: rustix::process::geteuid().as_raw(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(Error::InvalidInput(
                "file permissions require Windows or Unix".into(),
            ))
        }
    }

    pub(crate) fn check(&self, file: &File, private: bool) -> Result<()> {
        #[cfg(windows)]
        {
            use windows_permissions::constants::{SeObjectType, SecurityInformation};
            use windows_permissions::wrappers::{
                ConvertSecurityDescriptorToStringSecurityDescriptor, GetSecurityInfo,
            };
            let information = SecurityInformation::Owner | SecurityInformation::Dacl;
            let descriptor = GetSecurityInfo(file, SeObjectType::SE_FILE_OBJECT, information)?;
            let sddl =
                ConvertSecurityDescriptorToStringSecurityDescriptor(&descriptor, information)?;
            let sddl = sddl.to_str().ok_or_else(permission_error)?;
            check_sddl(sddl, &self.user, private)
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata()?;
            let denied = if private { 0o077 } else { 0o022 };
            if ![0, self.user].contains(&metadata.uid()) || metadata.mode() & denied != 0 {
                return Err(permission_error());
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (file, private);
            Err(permission_error())
        }
    }
}

fn permission_error() -> Error {
    Error::InvalidInput("file ownership or permissions allow untrusted access; private files require owner-only access".into())
}

/// Accept only a small, explicit subset of the OS-generated SDDL. This is an
/// allow-list, not an effective-access evaluator. Unknown ACL forms fail closed.
#[cfg(windows)]
fn check_sddl(sddl: &str, user: &str, private: bool) -> Result<()> {
    let (owner, dacl) = sddl
        .strip_prefix("O:")
        .and_then(|value| value.split_once("D:"))
        .ok_or_else(permission_error)?;
    let trusted = |sid: &str| {
        sid == user
            || matches!(sid, "SY" | "BA" | "S-1-5-18" | "S-1-5-32-544")
            || (sid == "LS" && user == "S-1-5-19")
            || (sid == "NS" && user == "S-1-5-20")
    };
    if !trusted(owner) || sddl.len() > 131_072 {
        return Err(permission_error());
    }
    let (flags, entries) = dacl.split_once('(').ok_or_else(permission_error)?;
    let mut flags = flags;
    while !flags.is_empty() {
        flags = flags
            .strip_prefix('P')
            .or_else(|| flags.strip_prefix("AI"))
            .or_else(|| flags.strip_prefix("AR"))
            .ok_or_else(permission_error)?;
    }
    let mut entries = entries;
    let mut count = 0;
    loop {
        let (entry, rest) = entries.split_once(')').ok_or_else(permission_error)?;
        count += 1;
        if count > 2048 {
            return Err(permission_error());
        }
        let fields = entry.split(';').collect::<Vec<_>>();
        if fields.len() != 6
            || !fields[3].is_empty()
            || !fields[4].is_empty()
            || !matches!(fields[0], "A" | "D")
        {
            return Err(permission_error());
        }
        let mut flags = fields[1];
        while !flags.is_empty() {
            let (flag, remaining) = flags.split_at_checked(2).ok_or_else(permission_error)?;
            if !matches!(flag, "OI" | "CI" | "IO" | "ID" | "NP") {
                return Err(permission_error());
            }
            flags = remaining;
        }
        if fields[0] == "A" && !trusted(fields[5]) && (private || !read_only_rights(fields[2])) {
            return Err(permission_error());
        }
        if rest.is_empty() {
            return Ok(());
        }
        entries = rest.strip_prefix('(').ok_or_else(permission_error)?;
    }
}

#[cfg(windows)]
fn read_only_rights(rights: &str) -> bool {
    if let Some(hex) = rights.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).is_ok_and(|mask| mask & !0xa012_00a9 == 0);
    }
    !rights.is_empty()
        && rights
            .as_bytes()
            .chunks(2)
            .all(|part| matches!(part, b"FR" | b"FX" | b"GR" | b"GX" | b"RC"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    const USER: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn private_permissions_accept_only_trusted_owners_and_grants() {
        let private = format!("O:{USER}D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{USER})");
        assert!(check_sddl(&private, USER, true).is_ok());
        for bad in [
            format!("O:WDD:(A;;FA;;;{USER})"),
            format!("O:{USER}D:NO_ACCESS_CONTROL"),
            format!("O:{USER}D:"),
            format!("O:{USER}D:(A;;FR;;;WD)"),
            format!("O:{USER}D:(A;OIIO;FA;;;WD)"),
            format!("O:{USER}D:(XA;;FA;;;{USER};(@User.foo == 1))"),
            format!("O:{USER}D:(OA;;FA;object;;{USER})"),
            format!("O:{USER}D:(A;ZZ;FA;;;{USER})"),
            format!("O:{USER}D:(A;;FA;;;{USER})trailing"),
        ] {
            assert!(check_sddl(&bad, USER, true).is_err(), "{bad}");
        }
        assert!(check_sddl("O:LSD:(A;;FA;;;LS)", "S-1-5-19", true).is_ok());
        assert!(check_sddl("O:LSD:(A;;FA;;;LS)", USER, true).is_err());
    }

    #[test]
    fn readable_files_never_accept_untrusted_write_or_security_rights() {
        for rights in ["FR", "FX", "GR", "GX", "FRFX", "RC", "0x1", "0x1200a9"] {
            assert!(
                check_sddl(&format!("O:{USER}D:(A;;{rights};;;WD)"), USER, false).is_ok(),
                "{rights}"
            );
        }
        for rights in [
            "FA", "FW", "GA", "GW", "SD", "WD", "WO", "0x2", "0x4", "0x40", "0x100", "0x40000",
            "0x80000", "0x10000",
        ] {
            assert!(
                check_sddl(&format!("O:{USER}D:(A;;{rights};;;WD)"), USER, false).is_err(),
                "{rights}"
            );
        }
    }

    #[test]
    fn actual_file_permissions_are_checked_through_the_same_open_handle()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_permissions::constants::{SeObjectType, SecurityInformation};
        use windows_permissions::{LocalBox, SecurityDescriptor, wrappers::SetSecurityInfo};
        let directory = tempfile::tempdir()?;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .access_mode(0xc004_0000)
            .open(directory.path().join("permissions.fixture"))?;
        let permissions = FilePermissions::current()?;
        permissions.check(&file, true)?;
        for rights in ["FR", "FW"] {
            let descriptor: LocalBox<SecurityDescriptor> =
                format!("D:P(A;;FA;;;{})(A;;{rights};;;WD)", permissions.user).parse()?;
            SetSecurityInfo(
                &mut file,
                SeObjectType::SE_FILE_OBJECT,
                SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
                None,
                None,
                descriptor.dacl(),
                None,
            )?;
            assert!(permissions.check(&file, true).is_err());
            assert_eq!(permissions.check(&file, false).is_ok(), rights == "FR");
        }
        Ok(())
    }
}
