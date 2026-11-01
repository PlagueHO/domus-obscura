use std::path::Path;

pub fn tempdir() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    directory(root.path())?;
    Ok(root)
}

pub fn directory(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use winsafe::co;

        let token = winsafe::HPROCESS::GetCurrentProcess()
            .OpenProcessToken(co::TOKEN::QUERY)
            .map_err(std::io::Error::other)?;
        let information = token
            .GetTokenInformation(co::TOKEN_INFORMATION_CLASS::User)
            .map_err(std::io::Error::other)?;
        let winsafe::TokenInfo::User(user) = information else {
            return Err("fixture process user is absent".into());
        };
        let sid = user.User.Sid().ok_or("fixture process SID is absent")?;
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .access_mode(0x0006_0000)
            .custom_flags(0x0200_0000)
            .open(path)?;
        native_file_security::set_dacl(&handle, &format!("D:P(A;OICI;FA;;;{sid})"))?;
    }
    Ok(())
}
