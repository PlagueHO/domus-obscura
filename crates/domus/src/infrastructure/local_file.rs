use super::filesystem::{LibraryDirectory, require_local_disk};
use super::permissions::FilePermissions;
use crate::{Error, Result};
use std::io::Read;
use std::path::Path;

/// Stable no-follow lock files are retained across exits, never replaced.
pub(crate) struct ProcessLock {
    _file: std::fs::File,
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        // A concurrent fork can retain the open file description after close.
        if let Err(error) = self._file.unlock() {
            eprintln!("failed to release process lock: {error}");
        }
    }
}

impl ProcessLock {
    /// Verify that the pathname still names the locked object. Replacing a
    /// stable lock file must not create a second owner for the same operation.
    pub(crate) fn verify(&self, database: &Path, purpose: &str) -> Result<()> {
        let directory = LibraryDirectory::open(
            database
                .parent()
                .ok_or(Error::Internal("process lock parent is missing"))?,
        )?;
        let name = format!(".domus-{purpose}.lock");
        let current = directory.open_file(std::ffi::OsStr::new(&name))?;
        if super::filesystem::native_file_identity(&current)?
            != super::filesystem::native_file_identity(&self._file)?
        {
            return Err(Error::Conflict("process lock identity changed".into()));
        }
        Ok(())
    }

    pub(crate) fn acquire(database: &Path, purpose: &str) -> Result<Self> {
        Self::acquire_mode(database, purpose, false)
    }

    pub(crate) fn acquire_shared(database: &Path, purpose: &str) -> Result<Self> {
        Self::acquire_mode(database, purpose, true)
    }

    fn acquire_mode(database: &Path, purpose: &str, shared: bool) -> Result<Self> {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        use cap_std::fs::OpenOptions;
        if purpose.is_empty()
            || !purpose
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(Error::InvalidInput("invalid process lock purpose".into()));
        }
        let directory = LibraryDirectory::open(
            database
                .parent()
                .ok_or(Error::Internal("database parent is missing"))?,
        )?
        .into_directory();
        FilePermissions::current()?.check(&directory.try_clone()?.into_std_file(), false)?;
        let name = format!(".domus-{purpose}.lock");
        let options = |create| {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .write(true)
                .create_new(create)
                .follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            #[cfg(windows)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.access_mode(0xc004_0000).share_mode(3);
            }
            options
        };
        let file = match directory.open_with(&name, &options(true)) {
            Ok(file) => file.into_std(),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                directory.open_with(&name, &options(false))?.into_std()
            }
            Err(error) => return Err(error.into()),
        };
        if !file.metadata()?.is_file() || super::filesystem::is_unsafe_link(&file.metadata()?) {
            return Err(Error::InvalidInput(
                "process lock must be a regular file".into(),
            ));
        }
        FilePermissions::current()?.check(&file, true)?;
        let locked = if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        };
        match locked {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                Err(Error::Busy("another process owns this worker or account"))
            }
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

pub(crate) fn validate_reference(path: &Path) -> Result<()> {
    require_local_disk(path)?;
    if !path.is_absolute() || path.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name.to_str().is_none_or(|name| name.contains(':') || name.chars().any(char::is_control)))
    }) {
        return Err(Error::InvalidInput("service references require absolute local paths without alternate streams".into()));
    }
    Ok(())
}

pub(crate) fn read_local(path: &Path, maximum: u64, sensitive: bool) -> Result<Vec<u8>> {
    validate_reference(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("service file parent is missing".into()))?;
    let directory = LibraryDirectory::open(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::InvalidInput("service file name is missing".into()))?;
    let file = directory.open_file(name)?;
    FilePermissions::current()?.check(&file, sensitive)?;
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(Error::InvalidInput(
            "service file exceeds its size limit".into(),
        ));
    }
    Ok(bytes)
}

pub(crate) fn protect_files(store: &super::store::Store, files: &[&Path]) -> Result<()> {
    let parents = files
        .iter()
        .map(|path| {
            LibraryDirectory::open(
                path.parent()
                    .ok_or_else(|| Error::InvalidInput("service file parent is missing".into()))?,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let canonical = files
        .iter()
        .map(std::fs::canonicalize)
        .collect::<std::io::Result<Vec<_>>>()?;
    let mut after = 0;
    loop {
        let locations = store.locations(crate::domain::Page::new(after, 200)?)?;
        if locations.is_empty() {
            break;
        }
        for location in locations {
            after = location.id;
            if canonical
                .iter()
                .any(|path| path.starts_with(&location.root))
            {
                return Err(Error::InvalidInput(
                    "service configuration, requests and credentials must remain outside all media roots".into(),
                ));
            }
            if let Some(saved) = location.native_identity {
                if parents
                    .iter()
                    .any(|parent| parent.native_root().ancestors.contains(&saved))
                {
                    return Err(Error::InvalidInput(
                        "service input files overlap a media root identity".into(),
                    ));
                }
            } else {
                return Err(Error::Conflict(
                    "bind every configured media root before service startup".into(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::permissions::private_test_directory;

    #[test]
    fn process_lock_releases_exclusive_ownership_with_a_retained_duplicate()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = private_test_directory()?;
        let database = directory.path().join("state.db");
        let owner = ProcessLock::acquire(&database, "test")?;
        let inherited = owner._file.try_clone()?;
        assert!(matches!(
            ProcessLock::acquire(&database, "test"),
            Err(Error::Busy(_))
        ));
        assert!(matches!(
            ProcessLock::acquire_shared(&database, "test"),
            Err(Error::Busy(_))
        ));
        drop(owner);
        let next = ProcessLock::acquire(&database, "test")?;
        drop(next);
        let reader = ProcessLock::acquire_shared(&database, "test")?;
        drop(reader);
        drop(inherited);
        Ok(())
    }

    #[test]
    fn process_lock_releases_only_the_dropped_reader_with_a_retained_duplicate()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = private_test_directory()?;
        let database = directory.path().join("state.db");
        let first = ProcessLock::acquire_shared(&database, "test")?;
        let inherited = first._file.try_clone()?;
        let second = ProcessLock::acquire_shared(&database, "test")?;
        drop(first);
        assert!(matches!(
            ProcessLock::acquire(&database, "test"),
            Err(Error::Busy(_))
        ));
        drop(second);
        let writer = ProcessLock::acquire(&database, "test")?;
        assert!(matches!(
            ProcessLock::acquire_shared(&database, "test"),
            Err(Error::Busy(_))
        ));
        drop(writer);
        drop(inherited);
        Ok(())
    }
}
