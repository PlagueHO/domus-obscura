//! Private, bounded, file-based cache. No provider requests or approval evidence.
use super::filesystem::{LibraryDirectory, is_unsafe_link, require_local_disk, sync_directory};
use super::permissions::FilePermissions;
use super::store::Store;
use crate::application::services::cache::{Cache, CacheKey, CacheLookup, CachedContent};
use crate::{Error, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::fs::{File, TryLockError};
use std::io::{Read, Write};
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

const HEADER: usize = 96;
const MAGIC: &[u8; 8] = b"DOMCACHE";
const PENDING: &str = "pending.cache";

/// Includes entry headers and temporary replacement bytes, excluding owner.lock.
/// Reserve room for two maximum-size entries so replacement stays atomic.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CacheLimits {
    pub item_bytes: u32,
    pub total_bytes: u64,
    pub entries: u16,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            item_bytes: 9 * 1024 * 1024,
            total_bytes: 128 * 1024 * 1024,
            entries: 64,
        }
    }
}

/// Explicit operator configuration. Opening never creates the root or migrates
/// a database. Omission preserves the legacy provider-cache configuration.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfiguration {
    pub root: std::path::PathBuf,
    #[serde(default)]
    pub limits: CacheLimits,
}

impl CacheConfiguration {
    pub(crate) fn open(&self, store: &mut Store, protected: &[&Path]) -> Result<Arc<dyn Cache>> {
        if !self.root.is_absolute() {
            return Err(Error::InvalidInput("cache root must be absolute".into()));
        }
        let root = std::fs::canonicalize(&self.root)?;
        for path in protected {
            let path = std::fs::canonicalize(path)?;
            if path.starts_with(&root) || root.starts_with(&path) {
                return Err(Error::InvalidInput(
                    "cache root must not overlap configuration, secrets, TLS or browser assets"
                        .into(),
                ));
            }
        }
        Ok(Arc::new(FileCache::open(store, &self.root, self.limits)?))
    }
}

impl CacheLimits {
    fn validate(self) -> Result<Self> {
        if self.item_bytes == 0
            || self.item_bytes > 32 * 1024 * 1024
            || !(1..=4096).contains(&self.entries)
            || self.total_bytes < 2 * (u64::from(self.item_bytes) + HEADER as u64)
            || self.total_bytes > 1024 * 1024 * 1024
        {
            return Err(Error::InvalidInput("cache limits require 1 through 4096 entries, items up to 32 MiB, and a total from two encoded items through 1 GiB".into()));
        }
        Ok(self)
    }
}

struct CacheDirectory {
    directory: Dir,
    permissions: FilePermissions,
    limits: CacheLimits,
    _marker: File,
}

/// Clones share one serialized adapter. Each operation holds its own OS lock
/// through reads, atomic replacement and eviction, including across processes.
#[derive(Clone)]
pub struct FileCache {
    inner: Arc<Mutex<CacheDirectory>>,
}

struct CacheOperation<'a> {
    _owner: File,
    inner: MutexGuard<'a, CacheDirectory>,
}

impl Deref for CacheOperation<'_> {
    type Target = CacheDirectory;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

struct EntryHeader {
    key: [u8; 32],
    sha256: [u8; 32],
    stored_at: u64,
    expires_at: u64,
    bytes: u64,
}

struct InventoryEntry {
    name: String,
    header: EntryHeader,
    length: u64,
}

fn options(create: bool, write: bool, owner: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(write)
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
        options.share_mode(if owner {
            3
        } else if write {
            0
        } else {
            1
        });
    }
    #[cfg(not(windows))]
    let _ = owner;
    options
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn name(key: &CacheKey) -> String {
    format!("{}.entry", hex(key.digest()))
}

fn corruption() -> Error {
    Error::Conflict("cache entry is corrupt; explicitly invalidate its logical key".into())
}

impl EntryHeader {
    fn encode(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(HEADER);
        result.extend_from_slice(MAGIC);
        result.extend_from_slice(&self.key);
        result.extend_from_slice(&self.sha256);
        result.extend_from_slice(&self.stored_at.to_le_bytes());
        result.extend_from_slice(&self.expires_at.to_le_bytes());
        result.extend_from_slice(&self.bytes.to_le_bytes());
        result
    }

    fn read(file: &mut File, limits: CacheLimits) -> Result<Option<Self>> {
        let mut bytes = [0; HEADER];
        match file.read_exact(&mut bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let key = bytes[8..40].try_into().map_err(|_| corruption())?;
        let sha256 = bytes[40..72].try_into().map_err(|_| corruption())?;
        let number = |start| -> Result<u64> {
            Ok(u64::from_le_bytes(
                bytes[start..start + 8]
                    .try_into()
                    .map_err(|_| corruption())?,
            ))
        };
        let header = Self {
            key,
            sha256,
            stored_at: number(72)?,
            expires_at: number(80)?,
            bytes: number(88)?,
        };
        if &bytes[..8] != MAGIC
            || header.bytes > u64::from(limits.item_bytes)
            || header.expires_at <= header.stored_at
            || file.metadata()?.len() != HEADER as u64 + header.bytes
        {
            return Ok(None);
        }
        Ok(Some(header))
    }
}

impl FileCache {
    /// Explicitly reserve an existing, empty private root on local disk.
    /// Existing roots must match both the native database binding and owner marker.
    /// No marker, copied marker or directory contents can replace a lost binding.
    pub fn open(store: &mut Store, root: &Path, limits: CacheLimits) -> Result<Self> {
        let limits = limits.validate()?;
        require_local_disk(root)?;
        let (root, opened) = LibraryDirectory::canonical(root)?;
        require_local_disk(&root)?;
        let native = opened.native_root().clone();
        let directory = opened.into_directory();
        let permissions = FilePermissions::current()?;
        permissions.check(&directory.try_clone()?.into_std_file(), true)?;
        let marker_exists = match directory.symlink_metadata("owner.lock") {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if !marker_exists && directory.entries()?.next().transpose()?.is_some() {
            return Err(Error::Conflict(
                "an unowned cache root must be empty".into(),
            ));
        }
        let fresh = store.reserve_cache_root(&root, &native)?;
        store.check_native_root(
            root.to_str()
                .ok_or_else(|| Error::InvalidInput("cache root must be Unicode".into()))?,
            "cache",
            &native,
        )?;
        if !marker_exists && !fresh {
            return Err(Error::Conflict(
                "reserved cache owner marker is missing; explicit recovery is required".into(),
            ));
        }
        let mut owner = directory
            .open_with("owner.lock", &options(!marker_exists, true, true))?
            .into_std();
        check_file(&owner, &permissions)?;
        match owner.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(Error::Busy("cache is owned by another live process"));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        let expected = format!(
            "domus-cache-v1\n{}\n{}\n",
            store.application_identity()?,
            native.identity
        );
        if marker_exists {
            let mut saved = Vec::new();
            (&mut owner).take(256).read_to_end(&mut saved)?;
            if saved != expected.as_bytes() {
                return Err(Error::Conflict(
                    "cache owner marker does not match its database and native root".into(),
                ));
            }
        } else {
            owner.write_all(expected.as_bytes())?;
            owner.sync_all()?;
            sync_directory(&directory)?;
        }
        let inner = CacheDirectory {
            directory,
            permissions,
            limits,
            _marker: owner,
        };
        inner.remove_pending()?;
        inner._marker.unlock()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    fn lock(&self) -> Result<CacheOperation<'_>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| Error::Internal("cache operation lock failed"))?;
        let owner = inner
            .directory
            .open_with("owner.lock", &options(false, true, true))?
            .into_std();
        check_file(&owner, &inner.permissions)?;
        match owner.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(Error::Busy(
                    "cache operation is owned by another live process",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        Ok(CacheOperation {
            _owner: owner,
            inner,
        })
    }
}

fn check_file(file: &File, permissions: &FilePermissions) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_unsafe_link(&metadata) {
        return Err(Error::InvalidInput(
            "cache files must be regular files, not links or reparse points".into(),
        ));
    }
    permissions.check(file, true)
}

impl CacheDirectory {
    fn open_file(&self, name: &str) -> Result<File> {
        let file = self
            .directory
            .open_with(name, &options(false, false, false))?
            .into_std();
        check_file(&file, &self.permissions)?;
        Ok(file)
    }

    fn remove_pending(&self) -> Result<()> {
        match self.open_file(PENDING) {
            Ok(file) => {
                drop(file);
                self.directory.remove_file(PENDING)?;
                sync_directory(&self.directory)?;
            }
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn inventory(&self) -> Result<Vec<InventoryEntry>> {
        let mut result = Vec::new();
        for (index, entry) in self.directory.entries()?.enumerate() {
            if index > usize::from(self.limits.entries) {
                return Err(Error::Conflict(
                    "cache entry count exceeds its configured bound".into(),
                ));
            }
            let entry = entry?;
            let entry_name = entry.file_name();
            let name = entry_name.to_str().ok_or_else(corruption)?;
            if name == "owner.lock" {
                continue;
            }
            let stem = name.strip_suffix(".entry").ok_or_else(corruption)?;
            if stem.len() != 64
                || !stem
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(corruption());
            }
            let mut file = self.open_file(name)?;
            let header = EntryHeader::read(&mut file, self.limits)?.ok_or_else(corruption)?;
            if hex(&header.key) != stem {
                return Err(corruption());
            }
            let length = file.metadata()?.len();
            result.push(InventoryEntry {
                name: name.into(),
                header,
                length,
            });
        }
        if result.len() > usize::from(self.limits.entries)
            || result.iter().map(|entry| entry.length).sum::<u64>() > self.limits.total_bytes
        {
            return Err(Error::Conflict(
                "cache exceeds its configured storage bound".into(),
            ));
        }
        Ok(result)
    }
}

impl Cache for FileCache {
    fn lookup(&self, key: &CacheKey, now: u64) -> Result<CacheLookup> {
        let inner = self.lock()?;
        let mut file = match inner.open_file(&name(key)) {
            Ok(file) => file,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheLookup::Miss);
            }
            Err(error) => return Err(error),
        };
        let Some(header) = EntryHeader::read(&mut file, inner.limits)? else {
            return Ok(CacheLookup::Corrupt);
        };
        if header.key != *key.digest() {
            return Ok(CacheLookup::Corrupt);
        }
        let mut bytes = Vec::new();
        file.take(u64::from(inner.limits.item_bytes) + 1)
            .read_to_end(&mut bytes)?;
        let sha256: [u8; 32] = Sha256::digest(&bytes).into();
        if bytes.len() as u64 != header.bytes || sha256 != header.sha256 {
            return Ok(CacheLookup::Corrupt);
        }
        if now < header.stored_at || now >= header.expires_at {
            return Ok(CacheLookup::Expired);
        }
        Ok(CacheLookup::Hit(CachedContent {
            bytes,
            sha256: header.sha256,
            stored_at: header.stored_at,
            expires_at: header.expires_at,
        }))
    }

    fn put(&self, key: &CacheKey, bytes: &[u8], now: u64, expires_at: u64) -> Result<[u8; 32]> {
        let inner = self.lock()?;
        if bytes.len() > inner.limits.item_bytes as usize || expires_at <= now {
            return Err(Error::InvalidInput(
                "cache item is too large or expiry is not after storage time".into(),
            ));
        }
        inner.remove_pending()?;
        let mut entries = inner.inventory()?;
        let destination = name(key);
        let replacement = entries.iter().any(|entry| entry.name == destination);
        let added = bytes.len() as u64 + HEADER as u64;
        let mut total: u64 = entries.iter().map(|entry| entry.length).sum();
        let mut count = entries.len() + usize::from(!replacement);
        entries.sort_by_key(|entry| {
            (
                entry.header.expires_at,
                entry.header.stored_at,
                entry.name.clone(),
            )
        });
        for entry in entries.iter().filter(|entry| entry.name != destination) {
            if count <= usize::from(inner.limits.entries)
                && total + added <= inner.limits.total_bytes
            {
                break;
            }
            inner.directory.remove_file(&entry.name)?;
            total -= entry.length;
            count -= 1;
        }
        if count > usize::from(inner.limits.entries) || total + added > inner.limits.total_bytes {
            return Err(Error::Conflict(
                "cache cannot reserve bounded space for atomic replacement".into(),
            ));
        }
        let sha256 = Sha256::digest(bytes).into();
        let header = EntryHeader {
            key: *key.digest(),
            sha256,
            stored_at: now,
            expires_at,
            bytes: bytes.len() as u64,
        };
        let mut pending = inner
            .directory
            .open_with(PENDING, &options(true, true, false))?
            .into_std();
        check_file(&pending, &inner.permissions)?;
        pending.write_all(&header.encode())?;
        pending.write_all(bytes)?;
        pending.sync_all()?;
        drop(pending);
        inner
            .directory
            .rename(PENDING, &inner.directory, &destination)?;
        sync_directory(&inner.directory)?;
        Ok(sha256)
    }

    fn invalidate(&self, key: &CacheKey) -> Result<bool> {
        let inner = self.lock()?;
        let name = name(key);
        match inner.open_file(&name) {
            Ok(file) => drop(file),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        inner.directory.remove_file(name)?;
        sync_directory(&inner.directory)?;
        Ok(true)
    }

    fn evict_expired(&self, now: u64, limit: u16) -> Result<u16> {
        let inner = self.lock()?;
        if limit == 0 || limit > inner.limits.entries {
            return Err(Error::InvalidInput(
                "cache eviction limit must be positive and within the entry bound".into(),
            ));
        }
        let mut entries = inner.inventory()?;
        entries.sort_by_key(|entry| (entry.header.expires_at, entry.name.clone()));
        let mut removed = 0;
        for entry in entries
            .iter()
            .filter(|entry| now < entry.header.stored_at || now >= entry.header.expires_at)
            .take(usize::from(limit))
        {
            inner.directory.remove_file(&entry.name)?;
            removed += 1;
        }
        sync_directory(&inner.directory)?;
        Ok(removed)
    }
}
