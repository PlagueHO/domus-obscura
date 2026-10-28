//! Bounded, read-only sharing over explicitly bound native library directories.
//! A catalog is evidence, not open authority. Every read resolves from its binding
//! again and retains the cross-process publication lock for the protected work.
use super::filesystem::{LibraryDirectory, NativeRoot, is_unsafe_cap_link, native_file_identity};
use super::local_file::ProcessLock;
use super::store::Store;
use crate::application::sharing::selected_directory;
use crate::domain::LibraryPath;
use crate::domain::sharing::{SharePolicy, eligible_extension};
use crate::{Error, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::UNIX_EPOCH;

const MAX_CATALOG_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_DEPTH: usize = 128;
const BATCH: usize = 128;

fn stale() -> Error {
    Error::Conflict(
        "share catalog is stale or incomplete; rebuild it after inspecting the bound folders"
            .into(),
    )
}

fn generation(store: &Store) -> Result<i64> {
    Ok(store.sharing_connection().query_row(
        "SELECT generation FROM share_visibility WHERE id=1",
        [],
        |row| row.get(0),
    )?)
}

/// Nonblocking admission: publication retries at its existing job boundary
/// rather than waiting indefinitely for a slow response or upload.
pub(crate) fn publication_visibility_write(store: &Store) -> Result<ProcessLock> {
    ProcessLock::acquire(store.database_path(), "share-visibility")
}

#[derive(Clone)]
struct Binding {
    id: i64,
    location: i64,
    relative: String,
    native: NativeRoot,
}

fn binding(store: &Store, id: i64) -> Result<Binding> {
    let saved = store
        .sharing_connection()
        .query_row(
            "SELECT location_id,relative_path,native_body FROM share_bindings WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(stale)?;
    Ok(Binding {
        id,
        location: saved.0,
        relative: saved.1,
        native: serde_json::from_str(&saved.2)?,
    })
}

fn bound_directory(store: &Store, binding: &Binding) -> Result<LibraryDirectory> {
    let opened = selected_directory(
        store,
        binding.location,
        &LibraryPath::parse(&binding.relative)?,
    )?;
    if opened.native_root() != &binding.native {
        return Err(stale());
    }
    Ok(opened)
}

/// Retain this guard from the first browse page through the final response byte.
/// `check` rejects a stopped worker, changed root, or invalidated generation.
pub struct ShareVisibilityGuard {
    _lock: ProcessLock,
    database: PathBuf,
    generation: i64,
    bindings: Vec<Binding>,
    stop: Arc<AtomicBool>,
    catalog: i64,
}

impl ShareVisibilityGuard {
    pub fn check(&self) -> Result<()> {
        if self.stop.load(Ordering::Acquire) {
            return Err(Error::WorkerStopping);
        }
        let store = Store::open_read_current(&self.database)?;
        if generation(&store)? != self.generation {
            return Err(stale());
        }
        let current: bool = store.sharing_connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM share_catalogs WHERE id=?1 AND complete=1 AND last_error IS NULL)",
            [self.catalog], |row| row.get(0),
        )?;
        if !current {
            return Err(stale());
        }
        for binding in &self.bindings {
            bound_directory(&store, binding)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Evidence {
    identity: String,
    bytes: u64,
    modified: String,
    directory: String,
}

fn single_link(file: &File) -> Result<()> {
    #[cfg(unix)]
    let links = {
        use std::os::unix::fs::MetadataExt;
        file.metadata()?.nlink()
    };
    #[cfg(windows)]
    let links = {
        use cap_fs_ext::MetadataExt;
        let borrowed = cap_std::fs::File::from_std(file.try_clone()?);
        borrowed.metadata()?.nlink()
    };
    if links != 1 {
        return Err(Error::InvalidInput(
            "shared files must have exactly one native hard link".into(),
        ));
    }
    Ok(())
}

fn evidence(file: &File, directory: &LibraryDirectory) -> Result<Evidence> {
    single_link(file)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || super::filesystem::is_unsafe_link(&metadata) {
        return Err(stale());
    }
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidInput("shared file timestamp predates Unix epoch".into()))?;
    Ok(Evidence {
        identity: native_file_identity(file)?,
        bytes: metadata.len(),
        modified: modified.as_nanos().to_string(),
        directory: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(directory.native_root())?)
        ),
    })
}

#[derive(Clone, Debug)]
pub struct SharedFile {
    pub id: i64,
    pub wire_path: String,
    pub bytes: u64,
    catalog: i64,
    binding: i64,
    relative: String,
    evidence: Evidence,
}

#[derive(Clone, Debug)]
pub struct SharedDirectory {
    pub wire_path: String,
    pub files: u32,
}

struct CatalogEntry {
    relative: String,
    wire: String,
    bytes: u64,
    evidence: Option<String>,
    directory_body: Option<String>,
}

/// The file handle is read-only. Keep the guard alive through the final byte.
/// Call `check` before each bounded socket write and once at completion.
pub struct SharedRead {
    pub file: File,
    pub guard: ShareVisibilityGuard,
    evidence: Evidence,
}

impl SharedRead {
    pub fn check(&self) -> Result<()> {
        self.guard.check()?;
        single_link(&self.file)?;
        let metadata = self.file.metadata()?;
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| stale())?
            .as_nanos()
            .to_string();
        if native_file_identity(&self.file)? != self.evidence.identity
            || metadata.len() != self.evidence.bytes
            || modified != self.evidence.modified
        {
            return Err(stale());
        }
        Ok(())
    }
}

fn joined(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn portable(path: &str) -> Result<LibraryPath> {
    let path = LibraryPath::parse(path)?;
    if path.as_str().split('/').count() > MAX_DEPTH
        || path.as_str().split('/').any(|name| {
            name.len() > 255
                || name.ends_with(['.', ' '])
                || name.contains(['*', '?', '"', '<', '>', '|'])
        })
    {
        return Err(Error::InvalidInput(
            "shared name cannot be represented safely by peers".into(),
        ));
    }
    Ok(path)
}

fn excluded(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with('.')
        || matches!(
            name.as_str(),
            "system volume information" | "$recycle.bin" | "lost+found"
        )
}

fn check_private_ancestry(store: &Store, directory: &LibraryDirectory) -> Result<()> {
    let mut statement = store
        .sharing_connection()
        .prepare("SELECT body FROM filesystem_roots WHERE kind<>'media'")?;
    for row in statement.query_map([], |row| row.get::<_, String>(0))? {
        let private: NativeRoot = serde_json::from_str(&row?)?;
        if directory.native_root().overlaps(&private) {
            return Err(Error::InvalidInput(
                "shared traversal intersects a private native directory".into(),
            ));
        }
    }
    Ok(())
}

/// One rule for browse and open. Retained destination intents hide incomplete
/// copies after restart. A primary receipt does not depend on a backup receipt.
fn publication_eligible(
    store: &Store,
    binding: &Binding,
    relative: &str,
    directory: bool,
    file: Option<&mut File>,
    stop: &AtomicBool,
) -> Result<bool> {
    let path = joined(&binding.relative, relative);
    let mut statement = store.sharing_connection().prepare(
        "SELECT id,relative_folder,root_identity,album_identity,verified_at,last_error
         FROM publication_destinations WHERE location_id=?1
         AND folder_key=?2",
    )?;
    // Probe exact ancestor keys through the unique location/folder index. A
    // million files must not each scan every publication destination.
    let mut ancestor = path.to_uppercase();
    let mut matches = Vec::with_capacity(2);
    loop {
        let saved = statement
            .query_row(params![binding.location, ancestor], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .optional()?;
        if let Some(saved) = saved {
            matches.push(saved);
        }
        if matches.len() == 2 {
            return Ok(false);
        }
        let Some((parent, _)) = ancestor.rsplit_once('/') else {
            break;
        };
        ancestor = parent.to_owned();
    }
    if matches.is_empty() {
        return Ok(true);
    }
    if matches.len() != 1 {
        return Ok(false);
    }
    let (id, folder, root_identity, album_identity, verified, error) = &matches[0];
    if verified.is_none() || error.is_some() {
        return Ok(false);
    }
    let location = store.location(binding.location)?;
    let root = LibraryDirectory::open(Path::new(&location.root))?;
    store.check_native_root(&location.root, "media", root.native_root())?;
    let album = root.open_directory(&LibraryPath::parse(folder)?)?;
    if &root.native_root().identity != root_identity
        || album.native_root().identity != *album_identity.as_ref().ok_or_else(stale)?
    {
        return Err(stale());
    }
    if directory {
        let mut statement = store.sharing_connection().prepare(
            "SELECT name,bytes,verified_at FROM publication_files WHERE destination_id=?1 LIMIT 1002",
        )?;
        let receipts = statement
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if receipts.is_empty() || receipts.len() > 1001 {
            return Err(stale());
        }
        let mut names = HashSet::with_capacity(receipts.len());
        for (name, bytes, receipt) in receipts {
            let relative = portable(&name)?;
            if receipt.is_none() || relative.as_str().contains('/') {
                return Err(stale());
            }
            let opened = album.open_file(OsStr::new(&name))?;
            single_link(&opened)?;
            if opened.metadata()?.len() != u64::try_from(bytes).map_err(|_| stale())? {
                return Err(stale());
            }
            names.insert(name);
        }
        for entry in album.entries()? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| stale())?;
            if !names.remove(&name) {
                return Err(stale());
            }
        }
        if !names.is_empty() {
            return Err(stale());
        }
        return Ok(path == *folder);
    }
    let Some((parent, name)) = path.rsplit_once('/') else {
        return Ok(false);
    };
    if parent != folder {
        return Ok(false);
    }
    let receipt: Option<(i64, String)> = store.sharing_connection().query_row(
        "SELECT bytes,sha256 FROM publication_files WHERE destination_id=?1 AND name=?2 AND verified_at IS NOT NULL",
        params![id, name], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let Some((bytes, digest)) = receipt else {
        return Err(stale());
    };
    let Some(file) = file else {
        return Err(stale());
    };
    if file.metadata()?.len() != u64::try_from(bytes).map_err(|_| stale())? {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65_536];
    loop {
        if stop.load(Ordering::Acquire) {
            return Err(Error::WorkerStopping);
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0))?;
    if format!("{:x}", hash.finalize()) != digest {
        return Err(stale());
    }
    Ok(true)
}

/// Owns a complete immutable generation in local SQLite. No username policy is
/// stored here: the live session authorizes every operation separately.
pub struct ShareCatalog {
    database: PathBuf,
    id: i64,
    visibility: i64,
    bindings: Vec<Binding>,
    stop: Arc<AtomicBool>,
}

impl ShareCatalog {
    /// Rebuild after a publication or explicit scan invalidates this generation.
    /// Drop all response/read guards first. Admission is nonblocking; Busy means
    /// another protected operation still owns the visibility boundary.
    pub fn refresh(&mut self, policy: &SharePolicy) -> Result<()> {
        *self = Self::build(&self.database, policy, self.stop.clone())?;
        Ok(())
    }

    pub fn build(database: &Path, policy: &SharePolicy, stop: Arc<AtomicBool>) -> Result<Self> {
        policy.validate()?;
        let store = Store::open_current_existing(database)?;
        let _builder = ProcessLock::acquire(store.database_path(), "share-catalog")?;
        let _lock = ProcessLock::acquire(store.database_path(), "share-visibility")?;
        if stop.load(Ordering::Acquire) {
            return Err(Error::WorkerStopping);
        }
        let visibility = generation(&store)?;
        let bindings = policy
            .roots
            .iter()
            .map(|root| binding(&store, root.binding_id))
            .collect::<Result<Vec<_>>>()?;
        for (index, selected) in bindings.iter().enumerate() {
            bound_directory(&store, selected)?;
            if bindings[..index]
                .iter()
                .any(|other| other.native.overlaps(&selected.native))
            {
                return Err(Error::InvalidInput(
                    "selected share folders overlap through native ancestry".into(),
                ));
            }
        }
        // A single builder and current-generation handles keep retained disk use
        // finite. Rebuilding deliberately invalidates all older catalog objects.
        store
            .sharing_connection()
            .execute("DELETE FROM share_catalogs", [])?;
        store.sharing_connection().execute(
            "INSERT INTO share_catalogs(visibility) VALUES(?1)",
            [visibility],
        )?;
        let id = store.sharing_connection().last_insert_rowid();
        let catalog = Self {
            database: store.database_path().to_owned(),
            id,
            visibility,
            bindings,
            stop,
        };
        let built = catalog.populate(&store, policy);
        match built {
            Ok(()) => {
                store
                    .sharing_connection()
                    .execute("UPDATE share_catalogs SET complete=1 WHERE id=?1", [id])?;
                Ok(catalog)
            }
            Err(error) => {
                let message = error.to_string().chars().take(2048).collect::<String>();
                store.sharing_connection().execute(
                    "UPDATE share_catalogs SET last_error=?2 WHERE id=?1",
                    params![id, message],
                )?;
                Err(error)
            }
        }
    }

    fn populate(&self, store: &Store, policy: &SharePolicy) -> Result<()> {
        for (root, binding) in policy.roots.iter().zip(&self.bindings) {
            portable(&root.alias)?;
            store.sharing_connection().execute(
                "INSERT INTO share_catalog_paths(catalog_id,binding_id,relative_path,wire_path,wire_key,parent_wire,kind,directory_body)
                 VALUES(?1,?2,'',?3,?4,'','directory',?5)",
                params![self.id, binding.id, root.alias, root.alias.to_uppercase(), serde_json::to_string(&binding.native)?],
            )?;
        }
        let mut files = 0u32;
        let mut paths = self.bindings.len() as u32;
        let mut stored_bytes = 0u64;
        let catalog_bytes = u64::from(policy.limits.catalog_bytes).min(MAX_CATALOG_BYTES);
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(Error::WorkerStopping);
            }
            let next: Option<(i64, i64, String, String, String)> = store
                .sharing_connection()
                .query_row(
                    "SELECT id,binding_id,relative_path,wire_path,directory_body FROM share_catalog_paths
                 WHERE catalog_id=?1 AND kind='directory' AND scanned=0 ORDER BY id LIMIT 1",
                    [self.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()?;
            let Some((id, binding_id, relative, wire, native_body)) = next else {
                break;
            };
            let binding = self
                .bindings
                .iter()
                .find(|binding| binding.id == binding_id)
                .ok_or_else(stale)?;
            if !publication_eligible(store, binding, &relative, true, None, &self.stop)? {
                store
                    .sharing_connection()
                    .execute("DELETE FROM share_catalog_paths WHERE id=?1", [id])?;
                continue;
            }
            let root = bound_directory(store, binding)?;
            let directory = root.open_directory(&portable(&relative)?)?;
            if *directory.native_root() != serde_json::from_str::<NativeRoot>(&native_body)? {
                return Err(stale());
            }
            check_private_ancestry(store, &directory)?;
            let mut batch = Vec::with_capacity(BATCH);
            for entry in directory.entries()? {
                if self.stop.load(Ordering::Acquire) {
                    return Err(Error::WorkerStopping);
                }
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::InvalidInput("shared name is not Unicode".into()))?;
                if excluded(&name) {
                    continue;
                }
                let metadata = entry.metadata()?;
                if is_unsafe_cap_link(&metadata) {
                    return Err(Error::InvalidInput(
                        "share folder contains a symlink or reparse point".into(),
                    ));
                }
                if !metadata.is_dir() && (!metadata.is_file() || !eligible_extension(&name)) {
                    continue;
                }
                let child = portable(&joined(&relative, &name))?;
                let child_wire = joined(&wire, &name);
                portable(&child_wire)?;
                let mut directory_body = None;
                let proof = if metadata.is_file() {
                    let mut file = directory.open_file(OsStr::new(&name))?;
                    let before = evidence(&file, &directory)?;
                    if !publication_eligible(
                        store,
                        binding,
                        child.as_str(),
                        false,
                        Some(&mut file),
                        &self.stop,
                    )? {
                        continue;
                    }
                    if evidence(&file, &directory)? != before {
                        return Err(stale());
                    }
                    files = files.checked_add(1).ok_or_else(stale)?;
                    if files > policy.limits.catalog_files {
                        return Err(Error::InvalidInput(
                            "share catalog file limit exceeded; no partial catalog was activated"
                                .into(),
                        ));
                    }
                    Some(before)
                } else {
                    if !publication_eligible(
                        store,
                        binding,
                        child.as_str(),
                        true,
                        None,
                        &self.stop,
                    )? {
                        continue;
                    }
                    // Open from the retained parent; never authorize using dirent metadata.
                    let child = directory.open_directory(&portable(&name)?)?;
                    directory_body = Some(serde_json::to_string(child.native_root())?);
                    None
                };
                paths = paths.checked_add(1).ok_or_else(stale)?;
                let proof_json = proof.as_ref().map(serde_json::to_string).transpose()?;
                stored_bytes += (child.as_str().len()
                    + child_wire.len() * 2
                    + proof_json.as_ref().map_or(0, String::len)
                    + directory_body.as_ref().map_or(0, String::len)
                    + 128) as u64;
                if paths
                    > policy
                        .limits
                        .catalog_files
                        .saturating_mul(2)
                        .saturating_add(64)
                    || stored_bytes > catalog_bytes
                {
                    return Err(Error::InvalidInput(
                        "share catalog directory or disk budget exceeded".into(),
                    ));
                }
                batch.push(CatalogEntry {
                    relative: child.as_str().to_owned(),
                    wire: child_wire,
                    bytes: proof.as_ref().map_or(0, |proof| proof.bytes),
                    evidence: proof_json,
                    directory_body,
                });
                if batch.len() == BATCH {
                    self.flush(store, binding_id, &mut batch)?;
                }
            }
            self.flush(store, binding_id, &mut batch)?;
            if directory.native_root() != root.open_directory(&portable(&relative)?)?.native_root()
            {
                return Err(stale());
            }
            store
                .sharing_connection()
                .execute("UPDATE share_catalog_paths SET scanned=1 WHERE id=?1", [id])?;
        }
        if generation(store)? != self.visibility {
            return Err(stale());
        }
        for binding in &self.bindings {
            bound_directory(store, binding)?;
        }
        Ok(())
    }

    fn flush(&self, store: &Store, binding: i64, rows: &mut Vec<CatalogEntry>) -> Result<()> {
        let transaction = store.sharing_connection().unchecked_transaction()?;
        {
            let mut insert = transaction.prepare(
                "INSERT INTO share_catalog_paths(catalog_id,binding_id,relative_path,wire_path,wire_key,parent_wire,kind,bytes,evidence,directory_body)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )?;
            for entry in rows.iter() {
                insert.execute(params![
                    self.id,
                    binding,
                    entry.relative,
                    entry.wire,
                    entry.wire.to_uppercase(),
                    entry.wire.rsplit_once('/').map_or("", |(parent, _)| parent),
                    if entry.evidence.is_some() {
                        "file"
                    } else {
                        "directory"
                    },
                    i64::try_from(entry.bytes).map_err(|_| stale())?,
                    entry.evidence,
                    entry.directory_body
                ])?;
            }
        }
        transaction.commit()?;
        rows.clear();
        Ok(())
    }

    /// Begin a protected response. An old generation can never create a new guard.
    pub fn check(&self) -> Result<ShareVisibilityGuard> {
        let lock = ProcessLock::acquire_shared(&self.database, "share-visibility")?;
        let guard = ShareVisibilityGuard {
            _lock: lock,
            database: self.database.clone(),
            generation: self.visibility,
            bindings: self.bindings.clone(),
            stop: self.stop.clone(),
            catalog: self.id,
        };
        guard.check()?;
        Ok(guard)
    }

    /// Owned and Send: this guard can outlive a MutexGuard<ShareCatalog>.
    /// Retain it through the final browse/search response byte, and call
    /// `ShareVisibilityGuard::check` before every bounded network chunk.
    pub fn response_guard(&self) -> Result<ShareVisibilityGuard> {
        self.check()
    }

    fn current(&self, store: &Store) -> Result<()> {
        let complete: bool = store.sharing_connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM share_catalogs WHERE id=?1 AND visibility=?2 AND complete=1 AND last_error IS NULL)",
            params![self.id, self.visibility], |row| row.get(0),
        )?;
        if !complete {
            return Err(stale());
        }
        Ok(())
    }

    pub fn counts(&self) -> Result<(u32, u32)> {
        let _guard = self.check()?;
        let store = Store::open_read_current(&self.database)?;
        self.current(&store)?;
        Ok(store.sharing_connection().query_row(
            "SELECT (SELECT count(*) FROM share_catalog_paths WHERE catalog_id=?1 AND kind='directory'),
                    (SELECT count(*) FROM share_catalog_paths WHERE catalog_id=?1 AND kind='file')",
            [self.id], |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    /// Keyset pages are at most 200 files. Folder selects exact directory files.
    /// Folder uses the public backslash path;
    /// query is a literal case-insensitive substring, never SQL or a native path.
    pub fn page(
        &self,
        after: i64,
        limit: u16,
        folder: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<SharedFile>> {
        crate::domain::Page::new(after, limit)?;
        let _guard = self.check()?;
        let store = Store::open_read_current(&self.database)?;
        self.current(&store)?;
        let folder = folder.map(|folder| folder.replace('\\', "/"));
        if let Some(folder) = &folder {
            portable(folder)?;
        }
        if query.is_some_and(|query| query.len() > 256 || query.chars().any(char::is_control)) {
            return Err(Error::InvalidInput(
                "shared search text exceeds bounds".into(),
            ));
        }
        let mut statement = store.sharing_connection().prepare(
            "SELECT id,wire_path,bytes,binding_id,relative_path,evidence FROM share_catalog_paths
             WHERE catalog_id=?1 AND kind='file' AND id>?2
             AND (?4 IS NULL OR parent_wire=?4)
             AND (?5 IS NULL OR instr(lower(wire_path),lower(?5))>0)
             ORDER BY id LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![self.id, after, limit, folder, query], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, wire, bytes, binding, relative, proof)| {
                let shared = SharedFile {
                    id,
                    wire_path: wire.replace('/', "\\"),
                    bytes: u64::try_from(bytes).map_err(|_| stale())?,
                    catalog: self.id,
                    binding,
                    relative,
                    evidence: serde_json::from_str(&proof)?,
                };
                self.check_entry(&store, &shared)?;
                Ok(shared)
            })
            .collect()
    }

    pub fn directory_files(
        &self,
        directory: &str,
        after: i64,
        limit: u16,
    ) -> Result<Vec<SharedFile>> {
        self.page(after, limit, Some(directory), None)
    }

    fn check_entry(&self, store: &Store, shared: &SharedFile) -> Result<()> {
        if self.stop.load(Ordering::Acquire) {
            return Err(Error::WorkerStopping);
        }
        let binding = self
            .bindings
            .iter()
            .find(|binding| binding.id == shared.binding)
            .ok_or_else(stale)?;
        let root = bound_directory(store, binding)?;
        let relative = portable(&shared.relative)?;
        let directory = root.open_directory(&LibraryPath::parse(relative.parent())?)?;
        check_private_ancestry(store, &directory)?;
        let name = relative.as_str().rsplit('/').next().ok_or_else(stale)?;
        let file = directory.open_file(OsStr::new(name))?;
        if evidence(&file, &directory)? != shared.evidence {
            return Err(stale());
        }
        Ok(())
    }

    /// Complete folder enumeration, including empty folders, in bounded keyset
    /// pages. Pass the last returned wire_path as `after` without changing it.
    /// `within` includes the selected directory and its descendants.
    pub fn directory_page(
        &self,
        after: &str,
        limit: u16,
        within: Option<&str>,
    ) -> Result<Vec<SharedDirectory>> {
        crate::domain::Page::new(0, limit)?;
        let after = after.replace('\\', "/");
        portable(&after)?;
        let within = within.map(|path| path.replace('\\', "/"));
        if let Some(path) = &within {
            portable(path)?;
        }
        let _guard = self.check()?;
        let store = Store::open_read_current(&self.database)?;
        self.current(&store)?;
        let mut statement = store.sharing_connection().prepare(
            "SELECT d.wire_path,(SELECT count(*) FROM share_catalog_paths f
               WHERE f.catalog_id=d.catalog_id AND f.kind='file' AND f.parent_wire=d.wire_path),
               d.binding_id,d.relative_path,d.directory_body
             FROM share_catalog_paths d WHERE d.catalog_id=?1 AND d.kind='directory'
               AND d.wire_path>?2
               AND (?4 IS NULL OR d.wire_path=?4 OR substr(d.wire_path,1,length(?4)+1)=?4||'/')
             ORDER BY d.wire_path LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![self.id, after, limit, within], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(wire_path, files, binding_id, relative, native_body)| {
                let binding = self
                    .bindings
                    .iter()
                    .find(|binding| binding.id == binding_id)
                    .ok_or_else(stale)?;
                let root = bound_directory(&store, binding)?;
                let directory = root.open_directory(&portable(&relative)?)?;
                check_private_ancestry(&store, &directory)?;
                if *directory.native_root() != serde_json::from_str::<NativeRoot>(&native_body)? {
                    return Err(stale());
                }
                Ok(SharedDirectory {
                    wire_path: wire_path.replace('/', "\\"),
                    files,
                })
            })
            .collect()
    }

    pub fn file(&self, wire_path: &str) -> Result<SharedFile> {
        let path = wire_path.replace('\\', "/");
        portable(&path)?;
        let _guard = self.check()?;
        let store = Store::open_read_current(&self.database)?;
        self.current(&store)?;
        let row: Option<(i64, i64, i64, String, String)> = store
            .sharing_connection()
            .query_row(
                "SELECT id,bytes,binding_id,relative_path,evidence FROM share_catalog_paths
             WHERE catalog_id=?1 AND kind='file' AND wire_path=?2",
                params![self.id, path],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let (id, bytes, binding, relative, proof) = row.ok_or_else(stale)?;
        Ok(SharedFile {
            id,
            wire_path: path.replace('/', "\\"),
            bytes: u64::try_from(bytes).map_err(|_| stale())?,
            catalog: self.id,
            binding,
            relative,
            evidence: serde_json::from_str(&proof)?,
        })
    }

    pub fn open(&self, shared: &SharedFile) -> Result<SharedRead> {
        let guard = self.check()?;
        let store = Store::open_read_current(&self.database)?;
        self.current(&store)?;
        if shared.catalog != self.id {
            return Err(stale());
        }
        let exact: bool = store.sharing_connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM share_catalog_paths WHERE catalog_id=?1 AND id=?2
             AND binding_id=?3 AND wire_path=?4 AND relative_path=?5 AND bytes=?6 AND evidence=?7)",
            params![
                self.id,
                shared.id,
                shared.binding,
                shared.wire_path.replace('\\', "/"),
                shared.relative,
                i64::try_from(shared.bytes).map_err(|_| stale())?,
                serde_json::to_string(&shared.evidence)?
            ],
            |row| row.get(0),
        )?;
        if !exact {
            return Err(stale());
        }
        let binding = self
            .bindings
            .iter()
            .find(|binding| binding.id == shared.binding)
            .ok_or_else(stale)?;
        let root = bound_directory(&store, binding)?;
        let path = portable(&shared.relative)?;
        let directory = root.open_directory(&LibraryPath::parse(path.parent())?)?;
        check_private_ancestry(&store, &directory)?;
        let name = path.as_str().rsplit('/').next().ok_or_else(stale)?;
        let mut file = directory.open_file(OsStr::new(name))?;
        if evidence(&file, &directory)? != shared.evidence
            || !publication_eligible(
                &store,
                binding,
                path.as_str(),
                false,
                Some(&mut file),
                &self.stop,
            )?
            || evidence(&file, &directory)? != shared.evidence
        {
            return Err(stale());
        }
        let read = SharedRead {
            file,
            guard,
            evidence: shared.evidence.clone(),
        };
        read.check()?;
        Ok(read)
    }
}

#[cfg(test)]
mod tests;
