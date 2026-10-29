use super::filesystem::{
    LibraryDirectory, is_unsafe_cap_link, is_unsafe_link, sync_directory as sync,
};
use super::permissions::FilePermissions;
use super::store::{PublicationDestination, PublicationFile, PublicationPreparation, Store};
use crate::domain::LibraryPath;
use crate::{Error, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
#[cfg(windows)]
use std::ffi::OsStr;
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
    parent: Dir,
    identity: String,
    permissions: FilePermissions,
    prepared_folder: String,
    prepared_name: String,
    final_folder: String,
    final_name: String,
    visible: bool,
}

impl PublicationDirectory {
    /// Prepare a complete album in a hidden sibling folder. An unbound folder is
    /// never adopted, even if its bytes happen to match.
    pub(crate) fn open(
        store: &Store,
        target: &PublicationDestination,
        preparation: &PublicationPreparation,
    ) -> Result<Option<Self>> {
        let root = LibraryDirectory::open(Path::new(&target.root))?;
        store.check_native_root(&target.root, "media", root.native_root())?;
        if root.native_root().identity != target.root_identity {
            return Err(conflict());
        }
        let final_path = LibraryPath::parse(&target.relative_folder)?;
        let prepared_path = LibraryPath::parse(&preparation.folder)?;
        let final_parent = final_path.parent();
        if final_path.as_str().is_empty()
            || prepared_path.parent() != final_parent
            || !prepared_path
                .as_str()
                .rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with(".domus-pending-"))
        {
            return Err(conflict());
        }
        let permissions = FilePermissions::current()?;
        let mut parent = root.into_directory();
        checked(&parent, &permissions)?;
        let mut parts = final_parent.split('/').filter(|part| !part.is_empty());
        for name in parts.by_ref() {
            check_case(&parent, name)?;
            match parent.create_dir(name) {
                Ok(()) => sync(&parent)?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            parent = parent.open_dir_nofollow(name)?;
            checked(&parent, &permissions)?;
        }
        let final_name = final_path
            .as_str()
            .rsplit('/')
            .next()
            .ok_or_else(conflict)?;
        let prepared_name = prepared_path
            .as_str()
            .rsplit('/')
            .next()
            .ok_or_else(conflict)?;
        check_case(&parent, final_name)?;
        check_case(&parent, prepared_name)?;
        let mut final_directory = open_child_directory(&parent, final_name)?;
        let mut prepared_directory = open_child_directory(&parent, prepared_name)?;
        if final_directory.is_some() && prepared_directory.is_some() {
            return Err(conflict());
        }
        let mut visible = preparation.published_at.is_some();
        let directory = if visible {
            if prepared_directory.is_some() {
                return Err(conflict());
            }
            final_directory.take().ok_or_else(conflict)?
        } else if let Some(directory) = prepared_directory.take() {
            directory
        } else if let Some(directory) = final_directory.take() {
            let identity =
                super::filesystem::native_file_identity(&directory.try_clone()?.into_std_file())?;
            if target.album_identity.as_deref() != Some(identity.as_str()) {
                return Err(conflict());
            }
            if preparation.atomic {
                visible = true;
                directory
            } else {
                drop(directory);
                rename_no_replace(&parent, final_name, prepared_name)?;
                sync(&parent)?;
                parent.open_dir_nofollow(prepared_name)?
            }
        } else if target.album_identity.is_some() {
            return Err(conflict());
        } else {
            match parent.create_dir(prepared_name) {
                Ok(()) => {
                    sync(&parent)?;
                    parent.open_dir_nofollow(prepared_name)?
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            }
        };
        let identity =
            super::filesystem::native_file_identity(&directory.try_clone()?.into_std_file())?;
        if target
            .album_identity
            .as_ref()
            .is_some_and(|saved| saved != &identity)
        {
            return Err(conflict());
        }
        checked(&directory, &permissions)?;
        Ok(Some(Self {
            directory,
            parent,
            identity,
            permissions,
            prepared_folder: preparation.folder.clone(),
            prepared_name: prepared_name.to_owned(),
            final_folder: target.relative_folder.clone(),
            final_name: final_name.to_owned(),
            visible,
        }))
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
        let folder = if self.visible {
            &self.final_folder
        } else {
            &self.prepared_folder
        };
        let album = root.open_directory(&LibraryPath::parse(folder)?)?;
        if album.native_root().identity != self.identity
            || target.album_identity.as_deref() != Some(self.identity.as_str())
        {
            return Err(conflict());
        }
        checked(&self.directory, &self.permissions)
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn publish(&mut self, store: &Store, target: &PublicationDestination) -> Result<()> {
        self.check(store, target)?;
        check_case(&self.parent, &self.final_name)?;
        if open_child_directory(&self.parent, &self.final_name)?.is_some() {
            return Err(conflict());
        }
        let directory = std::mem::replace(&mut self.directory, self.parent.try_clone()?);
        drop(directory);
        let result = rename_no_replace(&self.parent, &self.prepared_name, &self.final_name);
        if let Err(error) = result {
            self.directory = self.parent.open_dir_nofollow(&self.prepared_name)?;
            return Err(error);
        }
        sync(&self.parent)?;
        self.directory = self.parent.open_dir_nofollow(&self.final_name)?;
        self.visible = true;
        self.check(store, target)
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

fn open_child_directory(parent: &Dir, name: &str) -> Result<Option<Dir>> {
    match parent.open_dir_nofollow(name) {
        Ok(directory) => Ok(Some(directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn rename_no_replace(directory: &Dir, from: &str, to: &str) -> Result<()> {
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

#[cfg(all(test, windows))]
mod tests {
    use super::rename_no_replace;
    use cap_std::fs::Dir;
    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::sync::{Arc, Barrier};

    #[test]
    fn windows_directory_rename_does_not_replace_a_racing_file() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let parent = Dir::open_ambient_dir(root.path(), cap_std::ambient_authority())?;

        for _ in 0..32 {
            let prepared = root.path().join("prepared");
            let final_path = root.path().join("album");
            fs::create_dir(&prepared)?;
            fs::write(prepared.join("track.flac"), b"album")?;

            let barrier = Arc::new(Barrier::new(2));
            let writer_barrier = Arc::clone(&barrier);
            let writer_path = final_path.clone();
            let writer = std::thread::Builder::new().spawn(move || {
                writer_barrier.wait();
                let result = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&writer_path);
                match result {
                    Ok(mut file) => {
                        file.write_all(b"racing file")?;
                        Ok(true)
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
                    Err(error)
                        if error.kind() == io::ErrorKind::PermissionDenied
                            && writer_path.is_dir() =>
                    {
                        Ok(false)
                    }
                    Err(error) => Err(error),
                }
            })?;

            barrier.wait();
            let rename = rename_no_replace(&parent, "prepared", "album");
            let writer_created_file = writer
                .join()
                .map_err(|_| io::Error::other("destination writer panicked"))??;

            if writer_created_file {
                assert!(rename.is_err());
                assert!(prepared.is_dir());
                assert_eq!(fs::read(&final_path)?, b"racing file");
                fs::remove_file(final_path)?;
                fs::remove_dir_all(prepared)?;
            } else {
                assert!(rename.is_ok());
                assert!(!prepared.exists());
                assert_eq!(fs::read(final_path.join("track.flac"))?, b"album");
                fs::remove_dir_all(final_path)?;
            }
        }

        Ok(())
    }
}
