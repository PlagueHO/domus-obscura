use crate::domain::LibraryPath;
use crate::{Error, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

mod binding;
mod identity;
pub mod media_adapter;
mod read_only;
mod selection;
mod state;
pub use binding::{DirectoryExpectation, DirectoryIdentityProfile, RetainedDirectory};
pub(crate) use identity::NativeRoot;
pub(crate) use identity::file_id as native_file_identity;
pub use read_only::ReadFile;
pub(crate) use read_only::{ReadDirectory, ReadSnapshot};
pub(crate) use selection::DirectorySnapshot;
#[cfg(test)]
pub(crate) use selection::fixture_storage;
pub use selection::{
    FilesystemClient, LogicalRootBinding, ProviderMapping, RootRevision, StorageConfiguration,
    StorageLease, StorageRole, StorageRoot, TopologyAssurance,
};
pub(crate) use state::initialize_selection;
pub(crate) use state::sqlite_context;

/// Read the native identity from the actual no-follow directory handle.
/// This is an identity comparison value, not proof against ID reuse or cloning.
pub fn native_root_identity(path: &Path) -> Result<String> {
    Ok(LibraryDirectory::canonical(path)?.1.native.identity)
}

/// Read a bounded operator-selected local file through no-follow handles.
/// Sensitive files require private permissions on the actual opened file.
pub fn read_local_file(path: &Path, maximum: u64, sensitive: bool) -> Result<Vec<u8>> {
    read_local_file_for(None, path, maximum, sensitive)
}

pub(crate) fn read_local_file_for(
    filesystem: Option<&std::sync::Arc<FilesystemClient>>,
    path: &Path,
    maximum: u64,
    sensitive: bool,
) -> Result<Vec<u8>> {
    require_local_path(path)?;
    validate_native_reference(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("service file parent is missing".into()))?;
    let directory = LibraryDirectory::open(parent)?;
    if let Some(filesystem) = filesystem {
        filesystem.require_private_input(&directory)?;
    }
    let name = path
        .file_name()
        .ok_or_else(|| Error::InvalidInput("service file name is missing".into()))?;
    read_file_in(&directory, name, maximum, sensitive)
}

pub(crate) fn read_file_in(
    directory: &LibraryDirectory,
    name: &OsStr,
    maximum: u64,
    sensitive: bool,
) -> Result<Vec<u8>> {
    read_checked_file_in(directory, name, maximum, sensitive, require_local_file)
}

/// Installed browser bytes can use a read-only Linux image layer. This does
/// not admit writable state, secrets or a selected storage root on overlayfs.
pub(crate) fn read_browser_file_in(
    directory: &LibraryDirectory,
    name: &OsStr,
    maximum: u64,
) -> Result<Vec<u8>> {
    read_checked_file_in(directory, name, maximum, false, require_browser_filesystem)
}

fn read_checked_file_in(
    directory: &LibraryDirectory,
    name: &OsStr,
    maximum: u64,
    sensitive: bool,
    check_filesystem: fn(&std::fs::File) -> Result<()>,
) -> Result<Vec<u8>> {
    let limit = maximum
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("local file size limit is too large".into()))?;
    check_filesystem(&directory.directory_file()?)?;
    let file = directory.open_file(name)?;
    check_filesystem(&file)?;
    super::permissions::FilePermissions::current()?.check(&file, sensitive)?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(Error::InvalidInput(
            "service file exceeds its size limit".into(),
        ));
    }
    Ok(bytes)
}

fn require_browser_filesystem(file: &std::fs::File) -> Result<()> {
    if filesystem_class(file)? == FilesystemClass::Local {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let filesystem = rustix::fs::fstatfs(file).map_err(std::io::Error::from)?;
        let mount = rustix::fs::fstatvfs(file).map_err(std::io::Error::from)?;
        if filesystem.f_type as u64 == 0x794c_7630
            && mount.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY)
        {
            return Ok(());
        }
    }
    Err(Error::InvalidInput(
        "installed browser assets require supported local storage or a read-only Linux overlay filesystem".into(),
    ))
}

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub(crate) fn require_local_disk(path: &Path) -> Result<()> {
    require_local_path(path)?;
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = || {
        path.parent()
            .ok_or(Error::InvalidInput("storage path has no parent".into()))
    };
    let (root, existing_file) = match fs::symlink_metadata(&path) {
        Ok(metadata) if is_unsafe_link(&metadata) => {
            return Err(Error::InvalidInput(
                "local storage must not use a symlink or reparse point".into(),
            ));
        }
        Ok(metadata) if metadata.is_dir() => (path.as_path(), false),
        Ok(metadata) if metadata.is_file() => (parent()?, true),
        Ok(_) => {
            return Err(Error::InvalidInput(
                "local storage requires a regular file or directory".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (parent()?, false),
        Err(error) => return Err(error.into()),
    };
    let directory = LibraryDirectory::open(root)?;
    directory.require_local()?;
    if existing_file {
        let name = path
            .file_name()
            .ok_or(Error::InvalidInput("storage file has no name".into()))?;
        require_local_file(&directory.open_file(name)?)?;
    }
    Ok(())
}

/// Validate spelling without opening a reference that can be inactive or not
/// created yet. Active access must also check the actual opened filesystem.
pub(crate) fn require_local_path(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let remote = matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if !matches!(
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

pub(crate) fn validate_native_reference(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.components().any(|component| {
        matches!(component, Component::Normal(name) if name.to_str().is_none_or(|name| name.contains(':') || name.chars().any(char::is_control)))
    }) {
        return Err(Error::InvalidInput(
            "storage references require absolute paths without alternate streams".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FilesystemClass {
    Local,
    Remote,
    Unknown,
}

/// Inspect the opened filesystem, never its path spelling or physical disks.
pub(crate) fn filesystem_class(file: &std::fs::File) -> Result<FilesystemClass> {
    #[cfg(windows)]
    {
        let information = native_file_security::filesystem_information(file)?;
        Ok(if information.remote {
            FilesystemClass::Remote
        } else if information.device_type == 7 && information.name.as_deref() == Some("NTFS") {
            FilesystemClass::Local
        } else {
            FilesystemClass::Unknown
        })
    }
    #[cfg(target_os = "linux")]
    {
        let information = rustix::fs::fstatfs(file).map_err(std::io::Error::from)?;
        Ok(match information.f_type as u64 {
            0xef53 => FilesystemClass::Local,
            0x6969 | 0xff53_4d42 | 0xfe53_4d42 => FilesystemClass::Remote,
            _ => FilesystemClass::Unknown,
        })
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = file;
        Ok(FilesystemClass::Unknown)
    }
}

pub(crate) fn require_local_file(file: &std::fs::File) -> Result<()> {
    if filesystem_class(file)? != FilesystemClass::Local {
        return Err(Error::InvalidInput(
            "application state and secrets require a verified supported local filesystem".into(),
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
    parents: Vec<Dir>,
    names: Vec<std::ffi::OsString>,
    selection: Option<selection::DirectorySelection>,
}

impl std::ops::Deref for LibraryDirectory {
    type Target = Dir;

    fn deref(&self) -> &Dir {
        &self.directory
    }
}

impl LibraryDirectory {
    /// Retain the original Windows creation handle. On Unix, as with other
    /// publication effects, external writers must respect the operation lock.
    pub(crate) fn create_child_directory(&self, name: &str) -> Result<Self> {
        let relative = LibraryPath::parse(name)?;
        if relative.as_str().is_empty() || relative.as_str().contains('/') {
            return Err(Error::InvalidInput(
                "directory creation requires one component".into(),
            ));
        }
        #[cfg(unix)]
        {
            self.directory.create_dir(name)?;
            self.open_directory(&relative)
        }
        #[cfg(windows)]
        {
            let directory = Dir::from_std_file(native_file_delete::create_directory(
                &self.directory_file()?,
                OsStr::new(name),
            )?);
            self.retain_child_directory(name, directory)
        }
    }

    #[cfg(windows)]
    fn retain_child_directory(&self, name: &str, directory: Dir) -> Result<Self> {
        Self::check_directory(&directory)?;
        let mut parents = self
            .parents
            .iter()
            .map(Dir::try_clone)
            .collect::<std::io::Result<Vec<_>>>()?;
        parents.push(self.directory.try_clone()?);
        let mut selection = self.selection.clone();
        if let Some(selected) = &mut selection {
            selected.descend(name, &directory.try_clone()?.into_std_file())?;
        }
        parents.push(directory);
        let native = NativeRoot::read(&parents)?;
        let directory = parents
            .pop()
            .ok_or(Error::Internal("retained child is missing"))?;
        let mut names = self.names.clone();
        names.push(name.into());
        Ok(Self {
            directory,
            native,
            parents,
            names,
            selection,
        })
    }

    /// Windows retains the original no-follow DELETE-capable source. No checked
    /// source pathname is reopened when the subsequent rename is performed.
    pub(crate) fn open_child_for_move(&self, name: &str) -> Result<Self> {
        let relative = LibraryPath::parse(name)?;
        if relative.as_str().is_empty() || relative.as_str().contains('/') {
            return Err(Error::InvalidInput(
                "directory move requires one component".into(),
            ));
        }
        #[cfg(unix)]
        {
            self.open_directory(&relative)
        }
        #[cfg(windows)]
        {
            self.retain_child_directory(
                name,
                Dir::from_std_file(native_file_delete::open_directory_for_move(
                    &self.directory_file()?,
                    OsStr::new(name),
                )?),
            )
        }
    }

    /// The caller must own the operation lock, saved intent and exact source
    /// approval. Unix requires cooperative writers, as other renameat effects do.
    pub(crate) fn rename_retained_child(&self, source: &Self, name: &str) -> Result<()> {
        let relative = LibraryPath::parse(name)?;
        if relative.as_str().is_empty()
            || relative.as_str().contains('/')
            || NativeRoot::read(&source.parents)? != self.native
        {
            return Err(Error::Conflict(
                "move source is not a direct retained child".into(),
            ));
        }
        #[cfg(windows)]
        {
            drop(
                native_file_delete::ValidatedHandle::directory(source.directory_file()?)?
                    .rename_directory_noreplace(&self.directory_file()?, OsStr::new(name))?,
            );
            Ok(())
        }
        #[cfg(unix)]
        {
            let from = source
                .names
                .last()
                .and_then(|name| name.to_str())
                .ok_or(Error::Internal("move source name is missing"))?;
            rename_no_replace(&self.directory, from, name)
        }
    }

    pub(crate) fn require_local(&self) -> Result<()> {
        require_local_file(&self.directory_file()?)
    }

    pub(crate) fn canonical(path: &Path) -> Result<(PathBuf, Self)> {
        let opened = Self::open(path)?;
        let root = fs::canonicalize(path)?;
        let verified = Self::open(&root)?;
        if opened.native.identity != verified.native.identity {
            return Err(Error::Conflict(
                "library root changed while resolving its canonical name".into(),
            ));
        }
        Ok((root, verified))
    }

    pub(crate) fn native_root(&self) -> &NativeRoot {
        &self.native
    }

    pub(crate) fn snapshot(&self) -> DirectorySnapshot {
        match &self.selection {
            Some(selection) => selection.snapshot(),
            None => DirectorySnapshot::Native(self.native.clone()),
        }
    }

    pub(crate) fn reopen_at(&self, path: &Path) -> Result<Self> {
        let current = match &self.selection {
            Some(selection) => selection.reopen()?,
            None => Self::open(path)?,
        };
        if current.native_root() != self.native_root() {
            return Err(Error::Conflict("retained directory binding changed".into()));
        }
        Ok(current)
    }

    /// Clone a metadata/permission query handle. Retain this directory as the
    /// authority while using the returned file; it does not carry ancestor pins.
    pub(crate) fn directory_file(&self) -> std::io::Result<std::fs::File> {
        Ok(self.directory.try_clone()?.into_std_file())
    }

    pub(crate) fn try_clone(&self) -> std::io::Result<Self> {
        Ok(Self {
            directory: self.directory.try_clone()?,
            native: self.native.clone(),
            parents: self
                .parents
                .iter()
                .map(Dir::try_clone)
                .collect::<std::io::Result<Vec<_>>>()?,
            names: self.names.clone(),
            selection: self.selection.clone(),
        })
    }

    /// Keep the complete retained chain when existing capability-based
    /// consumers descend into application-controlled child directories.
    pub(crate) fn open_dir_nofollow(&self, path: impl AsRef<Path>) -> std::io::Result<Self> {
        let relative = path
            .as_ref()
            .to_str()
            .ok_or_else(|| std::io::Error::other("directory identifier must be Unicode"))?;
        let opened = LibraryPath::parse(relative).and_then(|path| self.open_directory(&path));
        opened.map_err(|error| match error {
            Error::Io(error) => error,
            error => std::io::Error::other(error),
        })
    }

    pub(crate) fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::InvalidInput("library root must be absolute".into()));
        }
        validate_native_reference(path)?;

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
        let mut names = Vec::new();
        for component in components {
            match component {
                Component::Normal(name) => {
                    let parent = ancestors.last().ok_or_else(|| {
                        Error::InvalidInput("library root has no filesystem anchor".into())
                    })?;
                    ancestors.push(Self::open_child(parent, Path::new(name))?);
                    names.push(name.to_owned());
                }
                Component::ParentDir => {
                    if ancestors.len() > 1 {
                        ancestors.pop();
                        names.pop();
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
        Ok(Self {
            directory,
            native,
            parents: ancestors,
            names,
            selection: None,
        })
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
        let mut names = self.names.clone();
        let mut selection = self.selection.clone();
        let mut ancestors = self
            .parents
            .iter()
            .map(Dir::try_clone)
            .collect::<std::io::Result<Vec<_>>>()?;
        ancestors.push(self.directory.try_clone()?);
        if !relative.as_str().is_empty() {
            for component in relative.as_str().split('/') {
                let parent = ancestors
                    .last()
                    .ok_or(Error::Internal("directory has no retained parent"))?;
                ancestors.push(Self::open_child(parent, Path::new(component))?);
                if let Some(selection) = &mut selection {
                    let child = ancestors
                        .last()
                        .ok_or(Error::Internal("directory has no retained child"))?;
                    selection.descend(component, &child.try_clone()?.into_std_file())?;
                }
                names.push(component.into());
            }
        }
        let native = NativeRoot::read(&ancestors)?;
        let directory = ancestors
            .pop()
            .ok_or(Error::Internal("directory has no retained handle"))?;
        Ok(Self {
            directory,
            native,
            parents: ancestors,
            names,
            selection,
        })
    }

    pub(crate) fn sqlite_chain(&self) -> Result<native_sqlite::DirectoryChain> {
        if self.names.len() != self.parents.len() {
            return Err(Error::Internal("retained directory chain is incomplete"));
        }
        let anchor = self.parents.first().unwrap_or(&self.directory);
        let components = self
            .names
            .iter()
            .zip(
                self.parents
                    .iter()
                    .skip(1)
                    .chain((!self.names.is_empty()).then_some(&self.directory)),
            )
            .map(|(name, directory)| Ok((name.clone(), directory.try_clone()?.into_std_file())))
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(native_sqlite::DirectoryChain::new(
            anchor.try_clone()?.into_std_file(),
            components,
        )?)
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

#[cfg(test)]
#[path = "filesystem/boundary_prototype.rs"]
mod boundary_prototype;

#[cfg(test)]
#[path = "filesystem/logical_binding_prototype.rs"]
mod logical_binding_prototype;

#[cfg(test)]
#[path = "filesystem/topology_prototype.rs"]
mod topology_prototype;
