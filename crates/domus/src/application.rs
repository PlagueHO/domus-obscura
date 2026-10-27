use crate::domain::LibraryPath;
use crate::infrastructure::filesystem::{LibraryDirectory, canonical_root, is_unsafe_cap_link};
use crate::infrastructure::store::{Observation, ScanClaim, Store};
use crate::{Error, Result};
use std::path::Path;
use std::time::UNIX_EPOCH;

pub mod catalog;
mod normalization;
pub mod services;
pub use normalization::normalize_album;
mod acquisition;
pub(crate) use acquisition::reconcile_local_inputs;
pub use acquisition::{acquire_album, acquire_transfer, negotiate_transfer, receive_transfer_file};
mod playback;
pub use playback::export_playback_album;
mod publication;
pub use publication::publish_new_album;

pub fn add_location(store: &Store, root: &Path, kind: &str) -> Result<i64> {
    let root = canonical_root(root)?;
    let root = root
        .to_str()
        .ok_or_else(|| Error::InvalidInput("library root is not valid Unicode".into()))?;
    store.add_location(root, kind)
}

pub fn scan_location(store: &mut Store, location_id: i64) -> Result<i64> {
    let location = store.location(location_id)?;
    let claim = store.begin_scan(location_id)?;
    if let Err(error) = observe(store, &claim, Path::new(&location.root)) {
        if matches!(error, Error::WorkerStopping) {
            store.release_scan(&claim)?;
            return Err(error);
        }
        store.scan_error(&claim, &error.to_string())?;
    }
    if !store.finish_scan(&claim)? {
        return Err(Error::Conflict(
            "scan is incomplete; inspect the location error before retrying".into(),
        ));
    }
    Ok(claim.id)
}

/// Open only the configured root and verify its saved native binding. This is
/// a point-in-time availability check, not a scan or authority for later I/O.
/// Keep cached coverage and entries unchanged when the root is unavailable.
pub fn check_location_availability(store: &Store, location_id: i64) -> Result<()> {
    let location = store.location(location_id)?;
    let opened = LibraryDirectory::open(Path::new(&location.root))?;
    store.check_native_root(&location.root, "media", opened.native_root())
}

struct OwnedObservation {
    path: LibraryPath,
    kind: &'static str,
    size: Option<i64>,
    modified_at: Option<i64>,
}

fn flush(store: &mut Store, claim: &ScanClaim, batch: &mut Vec<OwnedObservation>) -> Result<()> {
    let observations = batch
        .iter()
        .map(|item| Observation {
            path: &item.path,
            kind: item.kind,
            size: item.size,
            modified_at: item.modified_at,
        })
        .collect::<Vec<_>>();
    store.record_batch(claim, &observations)?;
    batch.clear();
    Ok(())
}

/// The persistent directory frontier resumes after a crash. Only an unfinished
/// directory is revisited. File batches and directory completion are separate
/// short transactions; no transaction spans filesystem I/O.
fn observe(store: &mut Store, claim: &ScanClaim, root: &Path) -> Result<()> {
    let root = LibraryDirectory::open(root)?;
    let location = store.location(claim.location)?;
    store.check_native_root(&location.root, "media", root.native_root())?;
    let mut batch = Vec::with_capacity(128);
    while let Some(directory) = store.next_directory(claim)? {
        store.check_worker_stop()?;
        let result = observe_directory(store, claim, &root, &directory, &mut batch);
        if let Err(error) = result {
            if matches!(error, Error::WorkerStopping) {
                return Err(error);
            }
            store.scan_error(claim, &error.to_string())?;
        }
        flush(store, claim, &mut batch)?;
        store.complete_directory(claim, &directory)?;
    }
    Ok(())
}

fn observe_directory(
    store: &mut Store,
    claim: &ScanClaim,
    root: &LibraryDirectory,
    directory: &LibraryPath,
    batch: &mut Vec<OwnedObservation>,
) -> Result<()> {
    let opened = root.open_directory(directory)?;
    for entry in opened.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| Error::InvalidInput("file path is not valid Unicode".into()))?;
        let relative = if directory.as_str().is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", directory.as_str())
        };
        let path = LibraryPath::parse(&relative)?;
        let metadata = entry.metadata()?;
        let kind = if is_unsafe_cap_link(&metadata) {
            "unsafe_link"
        } else if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        let size = if metadata.is_file() {
            Some(
                i64::try_from(metadata.len())
                    .map_err(|_| Error::InvalidInput("file size exceeds SQLite range".into()))?,
            )
        } else {
            None
        };
        let modified_at =
            match metadata.modified()?.into_std().duration_since(UNIX_EPOCH) {
                Ok(duration) => Some(i64::try_from(duration.as_secs()).map_err(|_| {
                    Error::InvalidInput("file timestamp exceeds SQLite range".into())
                })?),
                Err(_) => None,
            };
        batch.push(OwnedObservation {
            path,
            kind,
            size,
            modified_at,
        });
        if batch.len() == 128 {
            flush(store, claim, batch)?;
            store.check_worker_stop()?;
        }
    }
    Ok(())
}
