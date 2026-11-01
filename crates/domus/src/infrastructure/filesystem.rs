use crate::domain::LibraryPath;
use crate::{Error, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::path::{Component, Path, PathBuf};

mod identity;
pub(crate) use identity::NativeRoot;
pub(crate) use identity::file_id as native_file_identity;

/// Read the native identity from the actual no-follow directory handle.
/// This is an identity comparison value, not proof against ID reuse or cloning.
pub fn native_root_identity(path: &Path) -> Result<String> {
    Ok(LibraryDirectory::canonical(path)?.1.native.identity)
}

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub(crate) fn require_local_disk(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let remote = matches!(
        path.components().next(),
        Some(std::path::Component::Prefix(prefix)) if !matches!(
            prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
        )
    );
    #[cfg(not(windows))]
    let remote = path.to_string_lossy().starts_with("\\\\");
    if remote {
        return Err(Error::InvalidInput(
            "application state and secrets must use local disk paths".into(),
        ));
    }
    Ok(())
}

pub fn is_unsafe_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub fn canonical_root(path: &Path) -> Result<PathBuf> {
    Ok(LibraryDirectory::canonical(path)?.0)
}

pub(crate) fn sync_directory(directory: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        // Traversal handles can use O_PATH, which cannot be synced. Reopen the
        // same directory for reading without resolving its former native path.
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        directory.open_with(".", &options)?.sync_all()?;
    }
    // Windows flushes each writable file. Directory flushing and power-loss
    // acceptance remain separate from this adapter.
    #[cfg(windows)]
    let _ = directory;
    Ok(())
}

/// Publish a new child without replacing a concurrent writer's destination.
pub(crate) fn rename_no_replace(directory: &Dir, from: &str, to: &str) -> Result<()> {
    #[cfg(unix)]
    {
        let directory = directory.try_clone()?.into_std_file();
        rustix::fs::renameat_with(
            &directory,
            from,
            &directory,
            to,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
    }
    #[cfg(windows)]
    {
        let directory = fence_windows::DirectoryHandle::from_directory_file(
            directory.try_clone()?.into_std_file(),
        )
        .map_err(std::io::Error::other)?;
        directory
            .rename_child_noreplace(OsStr::new(from), OsStr::new(to))
            .map_err(std::io::Error::other)?;
    }
    Ok(())
}

pub(crate) fn is_unsafe_cap_link(metadata: &cap_std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_symlink()
    }
}

/// The handle, not a checked pathname, is the authority for subsequent reads.
/// Open each component separately so intermediate links cannot be followed.
pub(crate) struct LibraryDirectory {
    directory: Dir,
    native: NativeRoot,
}

impl LibraryDirectory {
    pub(crate) fn canonical(path: &Path) -> Result<(PathBuf, Self)> {
        let mut opened = Self::open(path)?;
        let root = fs::canonicalize(path)?;
        let verified = Self::open(&root)?;
        if opened.native.identity != verified.native.identity {
            return Err(Error::Conflict(
                "library root changed while resolving its canonical name".into(),
            ));
        }
        opened.native = verified.native;
        Ok((root, opened))
    }

    pub(crate) fn native_root(&self) -> &NativeRoot {
        &self.native
    }

    pub(crate) fn into_directory(self) -> Dir {
        self.directory
    }

    pub(crate) fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::InvalidInput("library root must be absolute".into()));
        }

        let mut components = path.components().peekable();
        let mut anchor = PathBuf::new();
        while let Some(component @ (Component::Prefix(_) | Component::RootDir)) =
            components.peek().copied()
        {
            anchor.push(component.as_os_str());
            components.next();
        }
        let directory = Dir::open_ambient_dir(anchor, cap_std::ambient_authority())?;
        Self::check_directory(&directory)?;
        let mut ancestors = vec![directory];
        for component in components {
            match component {
                Component::Normal(name) => {
                    let parent = ancestors.last().ok_or_else(|| {
                        Error::InvalidInput("library root has no filesystem anchor".into())
                    })?;
                    ancestors.push(Self::open_child(parent, Path::new(name))?);
                }
                Component::ParentDir => {
                    if ancestors.len() > 1 {
                        ancestors.pop();
                    }
                }
                Component::CurDir => {}
                _ => {
                    return Err(Error::InvalidInput(
                        "library root has an invalid filesystem anchor".into(),
                    ));
                }
            }
        }
        let native = NativeRoot::read(&ancestors)?;
        let directory = ancestors
            .pop()
            .ok_or_else(|| Error::InvalidInput("library root has no filesystem anchor".into()))?;
        Ok(Self { directory, native })
    }

    fn check_directory(directory: &Dir) -> Result<()> {
        let metadata = directory.dir_metadata()?;
        if is_unsafe_cap_link(&metadata) || !metadata.is_dir() {
            return Err(Error::InvalidInput(
                "library directory must not be a symlink or reparse point".into(),
            ));
        }
        Ok(())
    }

    fn open_child(parent: &Dir, name: &Path) -> Result<Dir> {
        let directory = parent.open_dir_nofollow(name)?;
        Self::check_directory(&directory)?;
        Ok(directory)
    }

    pub(crate) fn open_directory(&self, relative: &LibraryPath) -> Result<Self> {
        let mut directory = self.directory.try_clone()?;
        let mut native = self.native.clone();
        if !relative.as_str().is_empty() {
            for component in relative.as_str().split('/') {
                directory = Self::open_child(&directory, Path::new(component))?;
                native.child(&directory)?;
            }
        }
        Ok(Self { directory, native })
    }

    pub(crate) fn entries(&self) -> std::io::Result<cap_std::fs::ReadDir> {
        self.directory.entries()
    }

    pub(crate) fn open_file(&self, name: &OsStr) -> Result<std::fs::File> {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
        }
        let file = self
            .directory
            .open_with(Path::new(name), &options)?
            .into_std();
        let metadata = file.metadata()?;
        if !metadata.is_file() || is_unsafe_link(&metadata) {
            return Err(Error::InvalidInput(
                "file must be regular and must not be a symlink or reparse point".into(),
            ));
        }
        Ok(file)
    }
}

#[cfg(test)]
mod tests;
