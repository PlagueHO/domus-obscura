//! Operation-owned native/paired media I/O. Paired children share one connection
//! and retain their actual parents. Policy,
//! approval, durable intent and receipts remain in their existing use cases.
use super::{
    BlockingMedia, Kind, ListedEntry, MAX_PAGE, MAX_READ, MediaDirectory, MediaNode, Observation,
};
use crate::domain::LibraryPath;
use crate::infrastructure::filesystem::read_only::ReadEvidence;
use crate::infrastructure::filesystem::{
    LibraryDirectory, ReadFile, is_unsafe_cap_link, sync_directory,
};
use crate::infrastructure::permissions::FilePermissions;
use crate::{Error, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use std::io::Read;
use std::sync::{Arc, Mutex};

mod output;

pub(crate) enum MediaDirectoryIo {
    Native(Box<LibraryDirectory>),
    Paired(Arc<PairedDirectoryIo>),
}

pub(crate) struct PairedDirectoryIo {
    media: Arc<Mutex<BlockingMedia>>,
    directory: Mutex<Option<DirectoryLease>>,
    _parent: Option<Arc<PairedDirectoryIo>>,
    identity: String,
}

enum DirectoryLease {
    Opened(MediaDirectory),
    Move(super::MediaMove),
}

impl DirectoryLease {
    fn directory(&self) -> &MediaDirectory {
        match self {
            Self::Opened(directory) => directory,
            Self::Move(source) => source.directory(),
        }
    }
}

impl PairedDirectoryIo {
    fn with_mut<T>(
        &self,
        action: impl FnOnce(
            &tokio::runtime::Runtime,
            &mut super::MediaClient,
            &mut MediaDirectory,
        ) -> Result<T>,
    ) -> Result<T> {
        let mut directory = self
            .directory
            .lock()
            .map_err(|_| Error::Internal("media directory lock failed"))?;
        let directory = directory
            .as_mut()
            .ok_or(Error::Internal("media directory is closed"))?;
        let DirectoryLease::Opened(directory) = directory else {
            return Err(Error::Conflict("reviewed move source is read-only".into()));
        };
        let mut media = self
            .media
            .lock()
            .map_err(|_| Error::Internal("media session lock failed"))?;
        let BlockingMedia {
            runtime, client, ..
        } = &mut *media;
        action(runtime, client, directory)
    }

    fn with<T>(
        &self,
        action: impl FnOnce(
            &tokio::runtime::Runtime,
            &mut super::MediaClient,
            &MediaDirectory,
        ) -> Result<T>,
    ) -> Result<T> {
        let directory = self
            .directory
            .lock()
            .map_err(|_| Error::Internal("media directory lock failed"))?;
        let directory = directory
            .as_ref()
            .ok_or(Error::Internal("media directory is closed"))?;
        let mut media = self
            .media
            .lock()
            .map_err(|_| Error::Internal("media session lock failed"))?;
        let BlockingMedia {
            runtime, client, ..
        } = &mut *media;
        action(runtime, client, directory.directory())
    }

    fn child(self: &Arc<Self>, directory: MediaDirectory) -> Self {
        Self {
            identity: directory.observation().identity.clone(),
            media: self.media.clone(),
            directory: Mutex::new(Some(DirectoryLease::Opened(directory))),
            _parent: Some(self.clone()),
        }
    }
}

impl Drop for PairedDirectoryIo {
    fn drop(&mut self) {
        let release = (|| -> Result<()> {
            let directory = self
                .directory
                .get_mut()
                .map_err(|_| Error::Internal("media directory lock failed"))?
                .take();
            if let Some(directory) = directory {
                let mut media = self
                    .media
                    .lock()
                    .map_err(|_| Error::Internal("media session lock failed"))?;
                let BlockingMedia {
                    runtime, client, ..
                } = &mut *media;
                match directory {
                    DirectoryLease::Opened(directory) => {
                        runtime.block_on(client.close_directory(directory))?;
                    }
                    DirectoryLease::Move(source) => {
                        runtime.block_on(client.close_move(source))?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = release {
            eprintln!("media directory release failed; retire this operation: {error}");
        }
    }
}

pub(crate) struct MediaEntryIo {
    pub name: String,
    pub directory: bool,
    observed: Option<Observation>,
}

impl MediaDirectoryIo {
    pub(crate) fn native(directory: LibraryDirectory) -> Result<Self> {
        FilePermissions::current()?.check(&directory.directory_file()?, false)?;
        Ok(Self::Native(Box::new(directory)))
    }

    pub(crate) fn paired(mut media: BlockingMedia) -> Result<Self> {
        let directory = media
            .runtime
            .block_on(media.client.open_root(&media.root_id))?;
        Ok(Self::Paired(Arc::new(PairedDirectoryIo {
            identity: directory.observation().identity.clone(),
            media: Arc::new(Mutex::new(media)),
            directory: Mutex::new(Some(DirectoryLease::Opened(directory))),
            _parent: None,
        })))
    }

    pub(crate) fn identity(&self) -> &str {
        match self {
            Self::Native(directory) => &directory.native_root().identity,
            Self::Paired(directory) => &directory.identity,
        }
    }

    pub(crate) fn check(&self) -> Result<()> {
        match self {
            Self::Native(directory) => {
                FilePermissions::current()?.check(&directory.directory_file()?, false)
            }
            Self::Paired(directory) => directory.with(|runtime, client, directory| {
                runtime.block_on(client.check_directory(directory))
            }),
        }
    }

    pub(crate) fn open_directory(&self, relative: &LibraryPath) -> Result<Option<Self>> {
        let Self::Paired(root) = self else {
            let Self::Native(root) = self else {
                return Err(Error::Internal("media directory kind changed"));
            };
            return match root.open_directory(relative) {
                Ok(directory) => Self::native(directory).map(Some),
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            };
        };
        let mut directory = root.clone();
        for component in relative.as_str().split('/').filter(|part| !part.is_empty()) {
            let Some(entry) = Self::find(&directory, component)? else {
                return Ok(None);
            };
            if entry.observed.kind != Kind::Directory {
                return Err(Error::Conflict("media directory changed type".into()));
            }
            let child = directory.with(|runtime, client, parent| {
                runtime.block_on(client.open_child(parent, &entry))
            })?;
            let MediaNode::Directory(child) = child else {
                return Err(Error::Conflict("media directory changed type".into()));
            };
            directory = Arc::new(directory.child(child));
        }
        Ok(Some(Self::Paired(directory)))
    }

    /// Create a missing child and retain its original Windows creation handle.
    /// Existing names are never adopted. The caller owns durable intent; paired
    /// checkpoints must not reenter this session or another async runtime.
    pub(crate) fn create_directory(
        &self,
        name: &str,
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<Self> {
        super::mutation::component(name)?;
        checkpoint()?;
        self.check()?;
        match self {
            Self::Native(parent) => {
                Self::check_native_case(parent, name, checkpoint)?;
                checkpoint()?;
                let directory = Self::native(parent.create_child_directory(name)?)?;
                sync_directory(parent)?;
                Ok(directory)
            }
            Self::Paired(parent) => {
                let directory = parent.with_mut(|runtime, client, directory| {
                    runtime.block_on(client.create_directory_checked(directory, name, &mut || {
                        checkpoint().map_err(Error::from)
                    }))
                })?;
                Ok(Self::Paired(Arc::new(parent.child(directory))))
            }
        }
    }

    fn check_native_case(
        parent: &LibraryDirectory,
        name: &str,
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        let key = name.to_uppercase();
        for entry in parent.entries()? {
            checkpoint()?;
            let candidate = entry?
                .file_name()
                .into_string()
                .map_err(|_| Error::InvalidInput("media name is not Unicode".into()))?;
            if candidate != name && candidate.to_uppercase() == key {
                return Err(Error::Conflict("media directory name collides".into()));
            }
        }
        Ok(())
    }

    fn find(directory: &PairedDirectoryIo, name: &str) -> Result<Option<ListedEntry>> {
        Self::find_checked(directory, name, &|| Ok(()))
    }

    fn find_checked(
        directory: &PairedDirectoryIo,
        name: &str,
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<Option<ListedEntry>> {
        checkpoint()?;
        directory
            .with(|runtime, client, parent| runtime.block_on(client.rewind_directory(parent)))?;
        loop {
            checkpoint()?;
            let page = directory
                .with(|runtime, client, parent| runtime.block_on(client.page(parent, MAX_PAGE)))?;
            if let Some(entry) = page.entries.into_iter().find(|entry| entry.name == name) {
                return Ok(Some(entry));
            }
            if page.complete {
                return Ok(None);
            }
        }
    }

    fn entries_checked(
        &self,
        maximum: usize,
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<Vec<MediaEntryIo>> {
        let mut entries = Vec::new();
        match self {
            Self::Native(directory) => {
                for entry in directory.entries()? {
                    checkpoint()?;
                    if entries.len() == maximum {
                        return Err(Error::Conflict(
                            "media folder exceeds its file limit".into(),
                        ));
                    }
                    let entry = entry?;
                    let metadata = entry.metadata()?;
                    if is_unsafe_cap_link(&metadata) || (!metadata.is_file() && !metadata.is_dir())
                    {
                        return Err(Error::Conflict(
                            "media folder contains an unsafe object".into(),
                        ));
                    }
                    entries.push(MediaEntryIo {
                        name: entry
                            .file_name()
                            .into_string()
                            .map_err(|_| Error::InvalidInput("media name is not Unicode".into()))?,
                        directory: metadata.is_dir(),
                        observed: None,
                    });
                }
            }
            Self::Paired(directory) => {
                checkpoint()?;
                directory.with(|runtime, client, parent| {
                    runtime.block_on(client.rewind_directory(parent))
                })?;
                loop {
                    checkpoint()?;
                    let page = directory.with(|runtime, client, parent| {
                        runtime.block_on(client.page(parent, MAX_PAGE))
                    })?;
                    for entry in page.entries {
                        if entries.len() == maximum {
                            return Err(Error::Conflict(
                                "media folder exceeds its file limit".into(),
                            ));
                        }
                        entries.push(MediaEntryIo {
                            name: entry.name,
                            directory: entry.observed.kind == Kind::Directory,
                            observed: Some(entry.observed),
                        });
                    }
                    if page.complete {
                        break;
                    }
                }
            }
        }
        Ok(entries)
    }

    pub(crate) fn open_file(&self, entry: &MediaEntryIo) -> Result<MediaFileIo> {
        let relative = LibraryPath::parse(&entry.name)?;
        if entry.directory || relative.as_str().is_empty() || relative.as_str().contains('/') {
            return Err(Error::InvalidInput(
                "media file requires one regular child".into(),
            ));
        }
        match (self, &entry.observed) {
            (Self::Native(directory), None) => {
                let mut options = cap_std::fs::OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No);
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
                }
                #[cfg(windows)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.share_mode(0);
                }
                let file = directory.open_with(&entry.name, &options)?.into_std();
                FilePermissions::current()?.check(&file, false)?;
                Ok(MediaFileIo::Native(ReadFile::from_native(file)))
            }
            (Self::Paired(directory), Some(observed)) => {
                let entry = ListedEntry {
                    name: entry.name.clone(),
                    observed: observed.clone(),
                };
                let file = directory.with(|runtime, client, parent| {
                    runtime.block_on(client.open_child(parent, &entry))
                })?;
                let MediaNode::File(file) = file else {
                    return Err(Error::Conflict("media file changed type".into()));
                };
                Ok(MediaFileIo::Paired {
                    parent: directory.clone(),
                    file,
                    position: 0,
                })
            }
            _ => Err(Error::Conflict(
                "media entry belongs to a different I/O profile".into(),
            )),
        }
    }
}

pub(crate) enum MediaFileIo {
    Native(ReadFile),
    Paired {
        parent: Arc<PairedDirectoryIo>,
        file: super::MediaFile,
        position: u64,
    },
}

impl MediaFileIo {
    pub(crate) fn evidence(&self) -> Result<ReadEvidence> {
        match self {
            Self::Native(file) => file.evidence(),
            Self::Paired { parent, file, .. } => {
                parent.with(|runtime, client, _| runtime.block_on(client.read(file, 0, 1)))?;
                let observed = file.observation();
                Ok(ReadEvidence {
                    identity: observed.identity.clone(),
                    bytes: observed.length,
                    links: observed.links,
                    modified: (u128::from(observed.modified_seconds) * 1_000_000_000
                        + u128::from(observed.modified_nanoseconds))
                    .to_string(),
                })
            }
        }
    }

    pub(crate) fn close(self) -> Result<()> {
        match self {
            Self::Native(_) => Ok(()),
            Self::Paired { parent, file, .. } => {
                parent.with(|runtime, client, _| runtime.block_on(client.close_file(file)))
            }
        }
    }
}

impl Read for MediaFileIo {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Native(file) => file.read(output),
            Self::Paired {
                parent,
                file,
                position,
            } => {
                if output.is_empty() {
                    return Ok(0);
                }
                let bytes = parent
                    .with(|runtime, client, _| {
                        runtime.block_on(client.read(file, *position, output.len().min(MAX_READ)))
                    })
                    .map_err(std::io::Error::other)?;
                output[..bytes.len()].copy_from_slice(&bytes);
                *position += bytes.len() as u64;
                Ok(bytes.len())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{NativeAdapter, Request, Response, Session, tests::Fixture};
    use super::*;

    #[test]
    fn media_adapter_directory_rewind_never_clears_failed_coverage() -> Result<()> {
        let fixture = Fixture::new()?;
        let boundary = &fixture.selection.provider.boundary;
        let input = boundary.join("private").join("input");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&input)?;
        std::fs::hard_link(
            &input,
            boundary
                .join("parent")
                .join("media")
                .join("album")
                .join("alias"),
        )?;
        let adapter = NativeAdapter::new(fixture.selection.clone(), vec![file])?;
        let mut session = Session::new()?;
        let Response::Opened {
            reference: root, ..
        } = session.dispatch(&adapter, fixture.request()?)?
        else {
            return Err(Error::Internal("generated root missing"));
        };
        let Response::Page { entries, .. } = session.dispatch(
            &adapter,
            Request::Page {
                directory: root.clone(),
                limit: MAX_PAGE,
            },
        )?
        else {
            return Err(Error::Internal("generated root listing missing"));
        };
        let entry = entries
            .first()
            .ok_or(Error::Internal("generated album missing"))?;
        let Response::Opened {
            reference: album, ..
        } = session.dispatch(
            &adapter,
            Request::OpenChild {
                parent: root,
                name: entry.name.clone(),
                expected: entry.observed.clone(),
            },
        )?
        else {
            return Err(Error::Internal("generated album missing"));
        };
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::Page {
                        directory: album.clone(),
                        limit: MAX_PAGE
                    }
                )
                .is_err()
        );
        for rewind in [false, true] {
            assert!(
                session
                    .dispatch(
                        &adapter,
                        Request::DirectoryStatus {
                            directory: album.clone(),
                            rewind
                        }
                    )
                    .is_err()
            );
        }
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::Page {
                        directory: album,
                        limit: MAX_PAGE
                    }
                )
                .is_err()
        );
        Ok(())
    }
}
