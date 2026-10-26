use super::filesystem::{
    LibraryDirectory, is_unsafe_cap_link, is_unsafe_link, sync_directory as sync,
};
use super::permissions::FilePermissions;
use super::store::{PublicationDestination, PublicationFile, Store};
use crate::domain::LibraryPath;
use crate::{Error, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

fn conflict() -> Error {
    Error::Conflict("publication destination changed, is incomplete or contains existing content; nothing was overwritten".into())
}

fn options(create: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(create)
        .follow(FollowSymlinks::No);
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    options
}

fn checked(directory: &Dir, permissions: &FilePermissions) -> Result<()> {
    if is_unsafe_cap_link(&directory.dir_metadata()?) {
        return Err(conflict());
    }
    permissions.check(&directory.try_clone()?.into_std_file(), false)?;
    Ok(())
}

fn check_case(directory: &Dir, name: &str) -> Result<()> {
    let key = name.to_uppercase();
    for entry in directory.entries()? {
        let candidate = entry?.file_name().into_string().map_err(|_| conflict())?;
        if candidate != name && candidate.to_uppercase() == key {
            return Err(conflict());
        }
    }
    Ok(())
}

pub(crate) struct PublicationDirectory {
    directory: Dir,
    identity: String,
    permissions: FilePermissions,
}

impl PublicationDirectory {
    /// Only create a new album. An existing unbound directory is never adopted,
    /// even if its bytes happen to match. Retain creation/receipt gaps for recovery.
    pub(crate) fn open(store: &Store, target: &PublicationDestination) -> Result<Self> {
        let root = LibraryDirectory::open(Path::new(&target.root))?;
        store.check_native_root(&target.root, "media", root.native_root())?;
        if root.native_root().identity != target.root_identity {
            return Err(conflict());
        }
        let path = LibraryPath::parse(&target.relative_folder)?;
        if path.as_str().is_empty() {
            return Err(conflict());
        }
        let permissions = FilePermissions::current()?;
        if target.album_identity.is_none() {
            let mut parts = path.as_str().split('/').peekable();
            let mut parent = root.into_directory();
            checked(&parent, &permissions)?;
            while let Some(name) = parts.next() {
                check_case(&parent, name)?;
                if parts.peek().is_none() {
                    parent.create_dir(name)?;
                    sync(&parent)?;
                } else {
                    match parent.create_dir(name) {
                        Ok(()) => sync(&parent)?,
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                parent = parent.open_dir_nofollow(name)?;
                checked(&parent, &permissions)?;
            }
        }
        let root = LibraryDirectory::open(Path::new(&target.root))?;
        store.check_native_root(&target.root, "media", root.native_root())?;
        let album = root.open_directory(&path)?;
        let identity = album.native_root().identity.clone();
        if target
            .album_identity
            .as_ref()
            .is_some_and(|saved| saved != &identity)
        {
            return Err(conflict());
        }
        let directory = album.into_directory();
        // Recheck case collisions after creation as well as before it.
        let mut parent = root.into_directory();
        for name in path.as_str().split('/') {
            check_case(&parent, name)?;
            parent = parent.open_dir_nofollow(name)?;
        }
        checked(&directory, &permissions)?;
        Ok(Self {
            directory,
            identity,
            permissions,
        })
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    pub(crate) fn check(&self, store: &Store, target: &PublicationDestination) -> Result<()> {
        let root = LibraryDirectory::open(Path::new(&target.root))?;
        store.check_native_root(&target.root, "media", root.native_root())?;
        if root.native_root().identity != target.root_identity {
            return Err(conflict());
        }
        let album = root.open_directory(&LibraryPath::parse(&target.relative_folder)?)?;
        if album.native_root().identity != self.identity
            || target.album_identity.as_deref() != Some(self.identity.as_str())
        {
            return Err(conflict());
        }
        checked(&self.directory, &self.permissions)
    }

    fn regular(&self, file: File) -> Result<File> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || is_unsafe_link(&metadata) {
            return Err(conflict());
        }
        self.permissions.check(&file, false)?;
        Ok(file)
    }

    /// Existing exact files can close a sync-before-receipt crash gap. Partial,
    /// different and receipted-but-missing files cannot be rewritten or truncated.
    pub(crate) fn copy(
        &self,
        entry: &PublicationFile,
        source: &mut File,
        check: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        check()?;
        LibraryPath::parse(&entry.name)?;
        if entry.name.is_empty() || entry.name.contains('/') || entry.bytes == 0 {
            return Err(conflict());
        }
        check_case(&self.directory, &entry.name)?;
        let (mut file, created) = if entry.verified_at.is_some() {
            (
                self.regular(
                    self.directory
                        .open_with(&entry.name, &options(false))?
                        .into_std(),
                )?,
                false,
            )
        } else {
            match self.directory.open_with(&entry.name, &options(true)) {
                Ok(file) => (self.regular(file.into_std())?, true),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
                    self.regular(
                        self.directory
                            .open_with(&entry.name, &options(false))?
                            .into_std(),
                    )?,
                    false,
                ),
                Err(error) => return Err(error.into()),
            }
        };
        if created {
            let mut remaining = entry.bytes;
            let mut buffer = [0; 65_536];
            while remaining > 0 {
                check()?;
                let count = remaining.min(buffer.len() as u64) as usize;
                source.read_exact(&mut buffer[..count])?;
                check()?;
                file.write_all(&buffer[..count])?;
                remaining -= count as u64;
            }
            if source.read(&mut [0; 1])? != 0 {
                return Err(conflict());
            }
        }
        check()?;
        file.sync_all()?;
        verify(&mut file, entry, check)?;
        sync(&self.directory)?;
        check()?;
        Ok(())
    }

    pub(crate) fn verify(
        &self,
        files: &[PublicationFile],
        check: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        let mut names = files
            .iter()
            .map(|file| file.name.as_str())
            .collect::<HashSet<_>>();
        for entry in self.directory.entries()? {
            check()?;
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| conflict())?;
            if !names.remove(name.as_str()) {
                return Err(conflict());
            }
            if is_unsafe_cap_link(&entry.metadata()?) {
                return Err(conflict());
            }
        }
        if !names.is_empty() {
            return Err(conflict());
        }
        for entry in files {
            check()?;
            let mut file = self.regular(
                self.directory
                    .open_with(&entry.name, &options(false))?
                    .into_std(),
            )?;
            verify(&mut file, entry, check)?;
        }
        sync(&self.directory)?;
        check()?;
        Ok(())
    }
}

fn verify(
    file: &mut File,
    entry: &PublicationFile,
    check: &dyn Fn() -> std::io::Result<()>,
) -> Result<()> {
    if file.metadata()?.len() != entry.bytes {
        return Err(conflict());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65_536];
    let mut remaining = entry.bytes;
    while remaining > 0 {
        check()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if file.read(&mut [0; 1])? != 0
        || file.metadata()?.len() != entry.bytes
        || format!("{:x}", hash.finalize()) != entry.sha256
    {
        return Err(conflict());
    }
    Ok(())
}
