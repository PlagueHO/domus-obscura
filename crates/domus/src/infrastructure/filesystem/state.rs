use super::{LibraryDirectory, require_local_path, sync_directory, validate_native_reference};
use crate::infrastructure::permissions::FilePermissions;
use crate::{Error, Result};
use std::path::{Path, PathBuf};

pub(super) fn database_leaf(path: &Path) -> Result<&str> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::InvalidInput("database requires a named Unicode component".into()))?;
    if name.eq_ignore_ascii_case(native_sqlite::SELECTION_FILE) {
        return Err(Error::InvalidInput(
            "database component is reserved for storage coordination".into(),
        ));
    }
    Ok(name)
}

pub(crate) fn initialize_selection(directory: &LibraryDirectory) -> Result<()> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use std::io::Write;
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = directory
        .open_with(native_sqlite::SELECTION_FILE, &options)?
        .into_std();
    FilePermissions::current()?.check(&file, true)?;
    file.write_all(&native_sqlite::SelectionFence::initial_record())?;
    file.sync_all()?;
    sync_directory(directory)
}

pub(super) fn inspect_selection(directory: &LibraryDirectory) -> Result<Option<String>> {
    match directory.open_file(std::ffi::OsStr::new(native_sqlite::SELECTION_FILE)) {
        Ok(file) => {
            super::require_local_file(&file)?;
            FilePermissions::current()?.check(&file, true)?;
            Ok(Some(super::binding::persistent_file_identity(&file)?))
        }
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn selection_fence(
    directory: &LibraryDirectory,
    expected: &str,
    selection: &str,
) -> Result<native_sqlite::SelectionFence> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).write(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.share_mode(3);
    }
    let file = directory
        .open_with(native_sqlite::SELECTION_FILE, &options)?
        .into_std();
    if !file.metadata()?.is_file() || super::is_unsafe_link(&file.metadata()?) {
        return Err(Error::Conflict(
            "state coordination file must be a regular no-follow file".into(),
        ));
    }
    super::require_local_file(&file)?;
    FilePermissions::current()?.check(&file, true)?;
    if super::binding::persistent_file_identity(&file)? != expected {
        return Err(Error::Conflict(
            "state coordination file identity changed; preserve it for explicit recovery".into(),
        ));
    }
    Ok(native_sqlite::SelectionFence::acquire(file, selection)?)
}

pub(crate) fn sqlite_context(
    path: &Path,
    create: bool,
) -> Result<(
    native_sqlite::Context,
    PathBuf,
    std::sync::Arc<LibraryDirectory>,
)> {
    require_local_path(path)?;
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    validate_native_reference(&path)?;
    database_leaf(&path)?;
    let parent = path.parent().ok_or_else(|| {
        Error::InvalidInput("database requires a selected state directory".into())
    })?;
    let (_, directory) = LibraryDirectory::canonical(parent)?;
    directory.require_local()?;
    FilePermissions::current()?.check(&directory.directory_file()?, true)?;
    if create {
        let name = path
            .file_name()
            .ok_or(Error::Internal("database name is missing"))?;
        let missing = match directory.symlink_metadata(name) {
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => return Err(error.into()),
        };
        if missing && inspect_selection(&directory)?.is_none() {
            initialize_selection(&directory)?;
        }
    }
    sqlite_context_in(&directory, &path, create, None)
}

pub(super) fn sqlite_context_in(
    directory: &LibraryDirectory,
    path: &Path,
    create: bool,
    fence: Option<native_sqlite::SelectionFence>,
) -> Result<(
    native_sqlite::Context,
    PathBuf,
    std::sync::Arc<LibraryDirectory>,
)> {
    require_local_path(path)?;
    validate_native_reference(path)?;
    directory.require_local()?;
    let permissions = FilePermissions::current()?;
    permissions.check(&directory.directory_file()?, true)?;
    let name = database_leaf(path)?;
    if create {
        let mut builder = cap_std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        #[cfg(not(unix))]
        let _ = &mut builder;
        match directory.create_dir_with("sqlite-tmp", &builder) {
            Ok(()) => sync_directory(directory)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let temporary = match directory.open_dir_nofollow("sqlite-tmp") {
        Ok(temporary) => temporary,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::Conflict(
                "private SQLite temporary directory is missing; explicit fresh setup is required"
                    .into(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    temporary.require_local()?;
    permissions.check(&temporary.directory_file()?, true)?;
    let parent = path
        .parent()
        .ok_or(Error::Internal("database parent is missing"))?;
    let resolved = std::fs::canonicalize(parent)?.join(name);
    let context = match fence {
        Some(fence) => native_sqlite::Context::new_fenced(
            directory.sqlite_chain()?,
            temporary.directory_file()?,
            name,
            fence,
        )?,
        None => native_sqlite::Context::new(
            directory.sqlite_chain()?,
            temporary.directory_file()?,
            name,
        )?,
    };
    context.check()?;
    Ok((
        context,
        resolved,
        std::sync::Arc::new(directory.try_clone()?),
    ))
}
