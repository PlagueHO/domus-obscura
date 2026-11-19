//! Read-only native media capabilities, selected-root availability and scanning.
//! This is not remote private storage or permission to mutate media.
//!
//! The transport owns one table per authenticated TLS connection. Frozen
//! logical selections survive reconnection; live references never do.

use super::{LibraryDirectory, ProviderMapping, StorageRole, StorageRoot};
use crate::domain::LibraryPath;
use crate::infrastructure::permissions::FilePermissions;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::UNIX_EPOCH;

mod transport;
pub use transport::{Client, PreparedAdapter};
mod client;
pub use client::{MediaClient, MediaDirectory, MediaFile, MediaNode, MediaPage};
mod selected;
pub(crate) use selected::{MediaBinding, MediaPairing, SelectedMedia};
mod blocking;
pub(crate) use blocking::{BlockingFile, BlockingMedia};
#[cfg(test)]
mod tests;

const MAX_ROOTS: usize = 16;
const MAX_LIVE: usize = 32;
const MAX_DEPTH: usize = 32;
const MAX_PAGE: usize = 64;
const MAX_READ: usize = 16384;
const MAX_FRAME: usize = 262144;

/// Only this host maps provider-relative identifiers to Windows/native paths.
/// The private root reserves the adapter's own configuration and credentials.
/// No database location or private-file read operation crosses the channel.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub revision: u64,
    pub provider: ProviderMapping,
    pub media: Vec<StorageRoot>,
    pub private: StorageRoot,
}

impl Selection {
    pub(crate) fn validate(&self) -> Result<()> {
        self.logical().validate()?;
        for root in self.media.iter().chain(std::iter::once(&self.private)) {
            root.expected
                .validate(&LibraryPath::parse(&root.relative)?)?;
        }
        Ok(())
    }

    /// Frozen peer-visible meaning. Native path locators never cross the wire.
    pub fn logical(&self) -> MediaSelection {
        MediaSelection {
            revision: self.revision,
            provider_scope: self.provider.scope.clone(),
            media: self.media.clone(),
            private: self.private.clone(),
        }
    }

    /// Stable logical meaning excludes the host-specific boundary locator.
    pub fn digest(&self) -> Result<String> {
        self.logical().digest()
    }
}

/// A remote media declaration is not authority for local filesystem access.
/// The paired native peer must verify its full chain before issuing a lease.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaSelection {
    pub revision: u64,
    pub provider_scope: String,
    pub media: Vec<StorageRoot>,
    pub private: StorageRoot,
}

impl MediaSelection {
    pub(crate) fn validate(&self) -> Result<()> {
        let name = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value.trim() == value
                && !value.chars().any(char::is_control)
        };
        if self.revision == 0
            || !name(&self.provider_scope)
            || self.media.is_empty()
            || self.media.len() > MAX_ROOTS
            || self.private.role != StorageRole::Application
        {
            return Err(Error::InvalidInput("invalid native media selection".into()));
        }
        let mut ids = BTreeSet::new();
        for root in self.media.iter().chain(std::iter::once(&self.private)) {
            let relative = LibraryPath::parse(&root.relative)?;
            if !name(&root.root_id)
                || root.binding_revision == 0
                || root.provider_scope != self.provider_scope
                || root.expected.components.len() > MAX_DEPTH
                || !ids.insert(&root.root_id)
                || root.expected.profile != self.private.expected.profile
                || root.expected.components.first() != self.private.expected.components.first()
            {
                return Err(Error::InvalidInput("invalid native media root".into()));
            }
            root.expected.validate_record(&relative)?;
        }
        if self
            .media
            .iter()
            .any(|root| root.role != StorageRole::Media)
        {
            return Err(Error::InvalidInput(
                "only media roots can be exported".into(),
            ));
        }
        let roots = self
            .media
            .iter()
            .chain(std::iter::once(&self.private))
            .collect::<Vec<_>>();
        for (index, root) in roots.iter().enumerate() {
            for other in roots.iter().skip(index + 1) {
                let chain = &root.expected.components;
                let other = &other.expected.components;
                if chain.starts_with(other)
                    || other.starts_with(chain)
                    || chain.last() == other.last()
                {
                    return Err(Error::Conflict(
                        "selected native media roots overlap".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Uses the same digest as the native declaration, without its locator.
    pub fn digest(&self) -> Result<String> {
        let mut media = self.media.clone();
        media.sort_by(|left, right| left.root_id.cmp(&right.root_id));
        let mut hash = Sha256::new();
        hash.update(b"domus-native-media-selection-v1\0");
        hash.update(serde_json::to_vec(&(
            self.revision,
            &self.provider_scope,
            media,
            &self.private,
        ))?);
        Ok(format!("{:x}", hash.finalize()))
    }

    /// Exact logical topology review for one local selection and one paired
    /// media declaration. Locators and TLS secrets are not part of this value.
    pub fn storage_digest(&self, local: &super::StorageConfiguration) -> Result<String> {
        self.validate()?;
        let mut hash = Sha256::new();
        hash.update(b"domus-paired-media-storage-v1\0");
        hash.update(serde_json::to_vec(&(
            local.selection_digest()?,
            self.digest()?,
        ))?);
        Ok(format!("{:x}", hash.finalize()))
    }
}

/// These values are meaningful only on their originating live connection.
/// They are neither saved bindings nor reusable bearer credentials.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    connection: String,
    id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Directory,
    File,
}

/// Metadata comes from the same retained object used for later I/O.
/// This observation is not a content hash or full-stream verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub identity: String,
    pub kind: Kind,
    pub length: u64,
    pub modified_seconds: u64,
    pub modified_nanoseconds: u32,
    pub links: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListedEntry {
    pub name: String,
    pub observed: Observation,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    OpenRoot {
        selection: String,
        root: StorageRoot,
    },
    OpenChild {
        parent: Reference,
        name: String,
        expected: Observation,
    },
    Page {
        directory: Reference,
        limit: usize,
    },
    Read {
        file: Reference,
        offset: u64,
        count: usize,
    },
    Close {
        reference: Reference,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Opened {
        reference: Reference,
        observed: Observation,
    },
    Page {
        entries: Vec<ListedEntry>,
        complete: bool,
    },
    Read {
        bytes: Vec<u8>,
        complete: bool,
    },
    Closed,
}

struct NativeAdapter {
    selection: Selection,
    digest: String,
    private: LibraryDirectory,
    roots: BTreeMap<String, LibraryDirectory>,
    protected_files: BTreeSet<String>,
    _protected_inputs: Vec<File>,
}

impl NativeAdapter {
    fn new(selection: Selection, protected_files: Vec<File>) -> Result<Self> {
        selection.validate()?;
        if protected_files.len() > MAX_ROOTS
            || selection.provider.boundary.components().count() > 64
        {
            return Err(Error::InvalidInput("invalid native media selection".into()));
        }
        let open = |root: &StorageRoot| -> Result<LibraryDirectory> {
            let relative = LibraryPath::parse(&root.relative)?;
            Ok(root
                .expected
                .open(&selection.provider.boundary, &relative)?
                .directory)
        };
        let permissions = FilePermissions::current()?;
        let private = open(&selection.private)?;
        private.require_local()?;
        permissions.check(&private.directory_file()?, true)?;
        let mut roots: BTreeMap<String, LibraryDirectory> = BTreeMap::new();
        for root in &selection.media {
            if root.role != StorageRole::Media || root.root_id == selection.private.root_id {
                return Err(Error::InvalidInput(
                    "only media roots can be exported".into(),
                ));
            }
            let directory = open(root)?;
            permissions.check(&directory.directory_file()?, false)?;
            if directory.native_root().overlaps(private.native_root())
                || roots
                    .values()
                    .any(|other| directory.native_root().overlaps(other.native_root()))
                || roots.contains_key(&root.root_id)
            {
                return Err(Error::Conflict("native media roots overlap".into()));
            }
            roots.insert(root.root_id.clone(), directory);
        }
        let protected_identities = protected_files
            .iter()
            .map(|file| {
                if !file.metadata()?.is_file() {
                    return Err(Error::InvalidInput(
                        "protected adapter input is not a regular file".into(),
                    ));
                }
                super::require_local_file(file)?;
                permissions.check(file, true)?;
                super::binding::persistent_file_identity(file)
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            digest: selection.digest()?,
            selection,
            private,
            roots,
            protected_files: protected_identities,
            _protected_inputs: protected_files,
        })
    }

    fn observe(&self, file: &File, kind: Kind) -> Result<Observation> {
        FilePermissions::current()?.check(file, false)?;
        let metadata = file.metadata()?;
        if super::is_unsafe_link(&metadata)
            || (kind == Kind::Directory && !metadata.is_dir())
            || (kind == Kind::File && !metadata.is_file())
        {
            return Err(Error::Conflict("native media object type changed".into()));
        }
        let identity = super::binding::persistent_file_identity(file)?;
        let links = super::read_only::link_count(file)?;
        if self.protected_files.contains(&identity) {
            return Err(Error::Conflict(
                "media aliases a protected adapter input".into(),
            ));
        }
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::InvalidInput("unsupported media modification time".into()))?;
        Ok(Observation {
            identity,
            kind,
            length: metadata.len(),
            modified_seconds: modified.as_secs(),
            modified_nanoseconds: modified.subsec_nanos(),
            links,
        })
    }

    fn open_root(&self, selection: &str, root: &StorageRoot) -> Result<Entry> {
        if selection != self.digest || !self.selection.media.contains(root) {
            return Err(Error::Conflict(
                "native media selection is stale or unapproved".into(),
            ));
        }
        let current = root
            .expected
            .open(
                &self.selection.provider.boundary,
                &LibraryPath::parse(&root.relative)?,
            )?
            .directory;
        if self
            .roots
            .get(&root.root_id)
            .map(LibraryDirectory::native_root)
            != Some(current.native_root())
        {
            return Err(Error::Conflict(
                "native media runtime ancestry changed".into(),
            ));
        }
        self.directory_entry(current, None, 0)
    }

    fn directory_entry(
        &self,
        directory: LibraryDirectory,
        parent: Option<u64>,
        depth: usize,
    ) -> Result<Entry> {
        if directory.native_root().overlaps(self.private.native_root()) {
            return Err(Error::Conflict(
                "media crosses the adapter private root".into(),
            ));
        }
        let observed = self.observe(&directory.directory_file()?, Kind::Directory)?;
        Ok(Entry {
            parent,
            depth,
            observed,
            object: Object::Directory {
                directory,
                cursor: None,
                failed: false,
            },
        })
    }

    fn child(&self, parent_id: u64, parent: &Entry, name: &str, kind: Kind) -> Result<Entry> {
        let relative = LibraryPath::parse(name)?;
        if name.is_empty() || name.contains('/') || name.len() > 1024 || parent.depth >= MAX_DEPTH {
            return Err(Error::InvalidInput(
                "media operation requires one bounded component".into(),
            ));
        }
        let Object::Directory { directory, .. } = &parent.object else {
            return Err(Error::InvalidInput(
                "media parent is not a directory".into(),
            ));
        };
        match kind {
            Kind::Directory => self.directory_entry(
                directory.open_directory(&relative)?,
                Some(parent_id),
                parent.depth + 1,
            ),
            Kind::File => {
                let file = directory.open_file(OsStr::new(name))?;
                let observed = self.observe(&file, Kind::File)?;
                Ok(Entry {
                    parent: Some(parent_id),
                    depth: parent.depth + 1,
                    observed,
                    object: Object::File {
                        file,
                        _parent: directory.try_clone()?,
                    },
                })
            }
        }
    }
}

enum Object {
    Directory {
        directory: LibraryDirectory,
        cursor: Option<Box<cap_std::fs::ReadDir>>,
        failed: bool,
    },
    File {
        file: File,
        _parent: LibraryDirectory,
    },
}

struct Entry {
    parent: Option<u64>,
    depth: usize,
    observed: Observation,
    object: Object,
}

struct Session {
    connection: String,
    next: u64,
    entries: BTreeMap<u64, Entry>,
}

impl Session {
    fn new() -> Result<Self> {
        let mut nonce = [0; 32];
        rustls::crypto::ring::default_provider()
            .secure_random
            .fill(&mut nonce)
            .map_err(|_| Error::Internal("native media connection generation failed"))?;
        Ok(Self {
            connection: nonce.iter().map(|byte| format!("{byte:02x}")).collect(),
            next: 1,
            entries: BTreeMap::new(),
        })
    }

    fn require(&self, reference: &Reference) -> Result<u64> {
        if reference.connection != self.connection || !self.entries.contains_key(&reference.id) {
            return Err(Error::Conflict(
                "native media reference is stale or belongs to another connection".into(),
            ));
        }
        Ok(reference.id)
    }

    fn insert(&mut self, entry: Entry) -> Result<Response> {
        if self.entries.len() >= MAX_LIVE {
            return Err(Error::Busy("native media capability limit reached"));
        }
        let id = self.next;
        self.next = id
            .checked_add(1)
            .ok_or(Error::Internal("native media reference space exhausted"))?;
        let observed = entry.observed.clone();
        self.entries.insert(id, entry);
        Ok(Response::Opened {
            reference: Reference {
                connection: self.connection.clone(),
                id,
            },
            observed,
        })
    }

    fn dispatch(&mut self, adapter: &NativeAdapter, request: Request) -> Result<Response> {
        match request {
            Request::OpenRoot { selection, root } => {
                if self.entries.len() >= MAX_LIVE {
                    return Err(Error::Busy("native media capability limit reached"));
                }
                self.insert(adapter.open_root(&selection, &root)?)
            }
            Request::OpenChild {
                parent,
                name,
                expected,
            } => {
                let id = self.require(&parent)?;
                if self.entries.len() >= MAX_LIVE {
                    return Err(Error::Busy("native media capability limit reached"));
                }
                let parent = self
                    .entries
                    .get(&id)
                    .ok_or(Error::Internal("media parent disappeared"))?;
                let entry = adapter.child(id, parent, &name, expected.kind)?;
                if entry.observed != expected {
                    return Err(Error::Conflict(
                        "media differs from its listed observation".into(),
                    ));
                }
                self.insert(entry)
            }
            Request::Page { directory, limit } => {
                let id = self.require(&directory)?;
                self.page(adapter, id, limit)
            }
            Request::Read {
                file,
                offset,
                count,
            } => {
                let id = self.require(&file)?;
                let entry = self
                    .entries
                    .get_mut(&id)
                    .ok_or(Error::Internal("media file disappeared"))?;
                let Object::File { file, .. } = &mut entry.object else {
                    return Err(Error::InvalidInput("media reference is not a file".into()));
                };
                if !(1..=MAX_READ).contains(&count) || offset > entry.observed.length {
                    return Err(Error::InvalidInput("invalid bounded media read".into()));
                }
                if adapter.observe(file, Kind::File)? != entry.observed {
                    return Err(Error::Conflict("retained media file changed".into()));
                }
                let length = count.min(
                    usize::try_from((entry.observed.length - offset).min(MAX_READ as u64))
                        .map_err(|_| Error::Internal("bounded read length is invalid"))?,
                );
                let mut bytes = vec![0; length];
                file.seek(SeekFrom::Start(offset))?;
                file.read_exact(&mut bytes)?;
                if adapter.observe(file, Kind::File)? != entry.observed {
                    return Err(Error::Conflict(
                        "retained media file changed during read".into(),
                    ));
                }
                Ok(Response::Read {
                    complete: offset + length as u64 == entry.observed.length,
                    bytes,
                })
            }
            Request::Close { reference } => {
                let id = self.require(&reference)?;
                let mut closed = BTreeSet::from([id]);
                loop {
                    let before = closed.len();
                    for (id, entry) in &self.entries {
                        if entry.parent.is_some_and(|parent| closed.contains(&parent)) {
                            closed.insert(*id);
                        }
                    }
                    if before == closed.len() {
                        break;
                    }
                }
                self.entries.retain(|id, _| !closed.contains(id));
                Ok(Response::Closed)
            }
        }
    }

    fn page(&mut self, adapter: &NativeAdapter, id: u64, limit: usize) -> Result<Response> {
        if !(1..=MAX_PAGE).contains(&limit) {
            return Err(Error::InvalidInput(
                "invalid media directory page limit".into(),
            ));
        }
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or(Error::Internal("media directory disappeared"))?;
        let Object::Directory {
            directory,
            cursor,
            failed,
        } = &mut entry.object
        else {
            return Err(Error::InvalidInput(
                "media reference is not a directory".into(),
            ));
        };
        if *failed {
            return Err(Error::Conflict(
                "directory enumeration failed; open a new reference".into(),
            ));
        }
        *failed = true;
        if adapter.observe(&directory.directory_file()?, Kind::Directory)? != entry.observed {
            return Err(Error::Conflict(
                "media directory changed before enumeration".into(),
            ));
        }
        let mut iteration = match cursor.take() {
            Some(iteration) => iteration,
            None => Box::new(directory.entries()?),
        };
        let mut entries = Vec::with_capacity(limit);
        let mut complete = false;
        for _ in 0..limit {
            let next = iteration.next();
            let Some(next) = next else {
                complete = true;
                break;
            };
            let next = next?;
            let name = next
                .file_name()
                .into_string()
                .map_err(|_| Error::InvalidInput("media filename is not Unicode".into()))?;
            let kind = if next.file_type()?.is_dir() {
                Kind::Directory
            } else {
                Kind::File
            };
            let child = adapter.child(id, entry, &name, kind)?;
            entries.push(ListedEntry {
                name,
                observed: child.observed,
            });
        }
        let Object::Directory {
            directory,
            cursor,
            failed,
        } = &mut entry.object
        else {
            return Err(Error::Internal("media directory type changed"));
        };
        if adapter.observe(&directory.directory_file()?, Kind::Directory)? != entry.observed {
            return Err(Error::Conflict(
                "media directory changed during enumeration".into(),
            ));
        }
        *cursor = Some(iteration);
        *failed = false;
        Ok(Response::Page { entries, complete })
    }
}
