//! Retained read-only media objects. Paired opens repeat exact parent-relative
//! admission and compare the retained chain; native locators never cross peers.
use super::media_adapter::{
    BlockingFile, BlockingMedia, Kind, MediaBinding, MediaDirectory, SelectedMedia,
};
use super::{DirectorySnapshot, LibraryDirectory};
use crate::domain::LibraryPath;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

pub(super) fn link_count(file: &File) -> Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink())
    }
    #[cfg(windows)]
    {
        use cap_fs_ext::MetadataExt;
        Ok(cap_std::fs::File::from_std(file.try_clone()?)
            .metadata()?
            .nlink())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum ReadSnapshot {
    Native(DirectorySnapshot),
    Paired {
        paired_root: MediaBinding,
        identities: Vec<String>,
    },
}

impl ReadSnapshot {
    pub(crate) fn identity(&self) -> Result<String> {
        match self {
            Self::Native(snapshot) => snapshot.identity(),
            Self::Paired { .. } => Ok(format!(
                "paired-directory-v1:{:x}",
                Sha256::digest(serde_json::to_vec(self)?)
            )),
        }
    }
}

pub(crate) enum ReadDirectory {
    Native(Box<LibraryDirectory>),
    Paired(Box<PairedDirectory>),
}

pub(crate) struct PairedDirectory {
    selected: Arc<SelectedMedia>,
    binding: MediaBinding,
    relative: LibraryPath,
    snapshot: ReadSnapshot,
    media: Mutex<BlockingMedia>,
    chain: Vec<MediaDirectory>,
}

impl ReadDirectory {
    pub(crate) fn native_identity(&self) -> Result<String> {
        match self {
            Self::Native(directory) => Ok(directory.native_root().identity.clone()),
            Self::Paired(directory) => directory
                .chain
                .last()
                .map(|directory| directory.observation().identity.clone())
                .ok_or(Error::Internal("media ancestry is empty")),
        }
    }

    pub(crate) fn paired(
        selected: Arc<SelectedMedia>,
        binding: MediaBinding,
        relative: &LibraryPath,
    ) -> Result<Self> {
        let mut media = selected.open(&binding)?;
        let chain = media.open_directory(relative, &mut || Ok(()))?;
        let snapshot = ReadSnapshot::Paired {
            paired_root: binding.clone(),
            identities: BlockingMedia::identities(&chain),
        };
        Ok(Self::Paired(Box::new(PairedDirectory {
            selected,
            binding,
            relative: relative.clone(),
            snapshot,
            media: Mutex::new(media),
            chain,
        })))
    }

    pub(crate) fn snapshot(&self) -> ReadSnapshot {
        match self {
            Self::Native(directory) => ReadSnapshot::Native(directory.snapshot()),
            Self::Paired(directory) => directory.snapshot.clone(),
        }
    }

    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Native(left), Self::Native(right)) => {
                left.native_root().overlaps(right.native_root())
            }
            (Self::Paired(left), Self::Paired(right)) => {
                let (
                    ReadSnapshot::Paired {
                        identities: left, ..
                    },
                    ReadSnapshot::Paired {
                        identities: right, ..
                    },
                ) = (&left.snapshot, &right.snapshot)
                else {
                    return false;
                };
                left.last().is_some_and(|id| right.contains(id))
                    || right.last().is_some_and(|id| left.contains(id))
            }
            _ => false, // Exact selected cross-provider topology is checked before admission.
        }
    }

    pub(crate) fn open_directory(&self, relative: &LibraryPath) -> Result<Self> {
        match self {
            Self::Native(directory) => {
                Ok(Self::Native(Box::new(directory.open_directory(relative)?)))
            }
            Self::Paired(directory) => {
                let path = join(&directory.relative, relative.as_str())?;
                let opened =
                    Self::paired(directory.selected.clone(), directory.binding.clone(), &path)?;
                let ReadSnapshot::Paired {
                    identities: before, ..
                } = &directory.snapshot
                else {
                    return Err(Error::Internal("paired snapshot is missing"));
                };
                let ReadSnapshot::Paired {
                    identities: after, ..
                } = opened.snapshot()
                else {
                    return Err(Error::Internal("paired snapshot is missing"));
                };
                if !after.starts_with(before) {
                    return Err(Error::Conflict("paired media parent changed".into()));
                }
                Ok(opened)
            }
        }
    }

    pub(crate) fn open_file(&self, name: &OsStr) -> Result<ReadFile> {
        match self {
            Self::Native(directory) => Ok(ReadFile {
                inner: ReadFileInner::Native(directory.open_file(name)?),
            }),
            Self::Paired(directory) => {
                let name = name
                    .to_str()
                    .ok_or_else(|| Error::InvalidInput("media name is not Unicode".into()))?;
                let component = LibraryPath::parse(name)?;
                if component.as_str().is_empty() || component.as_str().contains('/') {
                    return Err(Error::InvalidInput(
                        "media file requires one component".into(),
                    ));
                }
                let ReadSnapshot::Paired { identities, .. } = &directory.snapshot else {
                    return Err(Error::Internal("paired snapshot is missing"));
                };
                let file = directory
                    .selected
                    .open(&directory.binding)?
                    .open_file(&join(&directory.relative, name)?, identities)?;
                Ok(ReadFile {
                    inner: ReadFileInner::Paired(Arc::new(Mutex::new(file))),
                })
            }
        }
    }

    pub(crate) fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<ReadEntry>> + '_>> {
        match self {
            Self::Native(directory) => Ok(Box::new(directory.entries()?.map(|entry| {
                let entry = entry?;
                let metadata = entry.metadata()?;
                Ok(ReadEntry {
                    name: entry
                        .file_name()
                        .into_string()
                        .map_err(|_| Error::InvalidInput("shared name is not Unicode".into()))?,
                    directory: metadata.is_dir(),
                    file: metadata.is_file(),
                    unsafe_link: super::is_unsafe_cap_link(&metadata),
                })
            }))),
            Self::Paired(_) => {
                let Self::Paired(directory) = self.open_directory(&LibraryPath::parse("")?)? else {
                    return Err(Error::Internal("paired directory is missing"));
                };
                Ok(Box::new(PairedEntries {
                    directory,
                    pending: VecDeque::new(),
                    complete: false,
                }))
            }
        }
    }
}

fn join(parent: &LibraryPath, child: &str) -> Result<LibraryPath> {
    LibraryPath::parse(&if parent.as_str().is_empty() {
        child.to_owned()
    } else if child.is_empty() {
        parent.as_str().to_owned()
    } else {
        format!("{}/{child}", parent.as_str())
    })
}

pub(crate) struct ReadEntry {
    pub name: String,
    pub directory: bool,
    pub file: bool,
    pub unsafe_link: bool,
}

struct PairedEntries {
    directory: Box<PairedDirectory>,
    pending: VecDeque<ReadEntry>,
    complete: bool,
}

impl Iterator for PairedEntries {
    type Item = Result<ReadEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(entry) = self.pending.pop_front() {
                return Some(Ok(entry));
            }
            if self.complete {
                return None;
            }
            let page = (|| {
                let parent = self
                    .directory
                    .chain
                    .last()
                    .ok_or(Error::Internal("media ancestry is empty"))?;
                self.directory
                    .media
                    .lock()
                    .map_err(|_| Error::Internal("media session lock failed"))?
                    .page(parent)
            })();
            match page {
                Ok(page) => {
                    self.complete = page.complete;
                    self.pending
                        .extend(page.entries.into_iter().map(|entry| ReadEntry {
                            name: entry.name,
                            directory: entry.observed.kind == Kind::Directory,
                            file: entry.observed.kind == Kind::File,
                            unsafe_link: false,
                        }));
                }
                Err(error) => {
                    self.complete = true;
                    return Some(Err(error));
                }
            }
        }
    }
}

/// Read-only native or paired retained file. Clones share the same open object
/// and cursor, matching File::try_clone. No write operation is exposed.
pub struct ReadFile {
    inner: ReadFileInner,
}
enum ReadFileInner {
    Native(File),
    Paired(Arc<Mutex<BlockingFile>>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadEvidence {
    pub identity: String,
    pub bytes: u64,
    pub modified: String,
    pub links: u64,
}

impl ReadFile {
    pub fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            inner: match &self.inner {
                ReadFileInner::Native(file) => ReadFileInner::Native(file.try_clone()?),
                ReadFileInner::Paired(file) => ReadFileInner::Paired(file.clone()),
            },
        })
    }

    pub(crate) fn evidence(&self) -> Result<ReadEvidence> {
        match &self.inner {
            ReadFileInner::Native(file) => {
                let metadata = file.metadata()?;
                if !metadata.is_file() || super::is_unsafe_link(&metadata) {
                    return Err(Error::Conflict("media file changed type".into()));
                }
                let modified = crate::infrastructure::library_metadata::modified_ns(file)?
                    .ok_or(Error::Internal("media modification time is missing"))?;
                Ok(ReadEvidence {
                    identity: super::native_file_identity(file)?,
                    bytes: metadata.len(),
                    modified,
                    links: link_count(file)?,
                })
            }
            ReadFileInner::Paired(file) => {
                let observed = file
                    .lock()
                    .map_err(|_| Error::Internal("media file lock failed"))?
                    .evidence()?;
                Ok(ReadEvidence {
                    identity: observed.identity,
                    bytes: observed.length,
                    links: observed.links,
                    modified: (u128::from(observed.modified_seconds) * 1_000_000_000
                        + u128::from(observed.modified_nanoseconds))
                    .to_string(),
                })
            }
        }
    }
}

impl Read for ReadFile {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            ReadFileInner::Native(file) => file.read(bytes),
            ReadFileInner::Paired(file) => file
                .lock()
                .map_err(|_| std::io::Error::other("media file lock failed"))?
                .read(bytes),
        }
    }
}

impl Seek for ReadFile {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        match &mut self.inner {
            ReadFileInner::Native(file) => file.seek(position),
            ReadFileInner::Paired(file) => file
                .lock()
                .map_err(|_| std::io::Error::other("media file lock failed"))?
                .seek(position),
        }
    }
}
