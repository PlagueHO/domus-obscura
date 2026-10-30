use crate::domain::LibraryPath;
use crate::infrastructure::filesystem::{LibraryDirectory, canonical_root, is_unsafe_cap_link};
use crate::infrastructure::store::{Observation, ScanClaim, Store};
use crate::{Error, Result};
use std::path::Path;
use std::time::UNIX_EPOCH;

pub mod catalog;
mod normalization;
pub mod services;
pub mod sharing;
pub use normalization::normalize_album;
mod acquisition;
pub(crate) use acquisition::reconcile_local_inputs;
pub use acquisition::{acquire_album, acquire_transfer, negotiate_transfer, receive_transfer_file};
mod playback;
pub use playback::export_playback_album;
mod publication;
pub use publication::{preview_album_replacements, publish_new_album, restore_album_replacement};

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
    run_scan(store, claim, Path::new(&location.root))
}

/// Refresh one directory only. This never renews whole-location coverage.
pub fn refresh_library_folder(
    store: &mut Store,
    location_id: i64,
    folder: &LibraryPath,
) -> Result<i64> {
    if folder.as_str().split('/').any(|part| {
        let part = part.to_ascii_lowercase();
        part.starts_with(".domus-pending-")
            || part.starts_with(".domus-retained-")
            || part.starts_with(".domus-restore-")
    }) {
        return Err(Error::InvalidInput(
            "publication recovery folders cannot be scanned".into(),
        ));
    }
    let location = store.location(location_id)?;
    let claim = store.begin_folder_scan(location_id, folder)?;
    run_scan(store, claim, Path::new(&location.root))
}

fn run_scan(store: &mut Store, claim: ScanClaim, root: &Path) -> Result<i64> {
    if let Err(error) = observe(store, &claim, root) {
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

/// Read a saved JPEG asset by entry identity. No raw bytes are served until the
/// caller passes them through the bounded image service.
pub fn library_jpeg_input(store: &Store, location: i64, entry: i64) -> Result<Vec<u8>> {
    use crate::infrastructure::{filesystem::native_file_identity, library_metadata::modified_ns};
    use std::io::Read;
    let saved = store.library_file(location, entry)?;
    let path = LibraryPath::parse(&saved.relative_path)?;
    let extension = saved
        .relative_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !saved.present
        || saved.metadata.status != "asset"
        || !matches!(extension.as_str(), "jpg" | "jpeg")
    {
        return Err(Error::InvalidInput(
            "preview requires a current indexed JPEG asset".into(),
        ));
    }
    let location = store.location(location)?;
    let root = LibraryDirectory::open(Path::new(&location.root))?;
    store.check_native_root(&location.root, "media", root.native_root())?;
    let parent = root.open_directory(&LibraryPath::parse(path.parent())?)?;
    let name = path
        .as_str()
        .rsplit('/')
        .next()
        .ok_or_else(|| Error::InvalidInput("missing asset name".into()))?;
    let mut file = parent.open_file(std::ffi::OsStr::new(name))?;
    let matches = |file: &std::fs::File| -> Result<bool> {
        Ok(
            Some(native_file_identity(file)?) == saved.metadata.native_identity
                && modified_ns(file)? == saved.metadata.modified_ns
                && i64::try_from(file.metadata()?.len()).ok() == saved.metadata.size,
        )
    };
    if !matches(&file)? {
        return Err(Error::Conflict(
            "JPEG asset changed; refresh its folder before preview".into(),
        ));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(Error::InvalidInput(
            "JPEG preview exceeds compressed byte limit".into(),
        ));
    }
    if !matches(&file)? || !bytes.starts_with(&[0xff, 0xd8]) {
        return Err(Error::Conflict(
            "JPEG asset changed or has an invalid signature".into(),
        ));
    }
    Ok(bytes)
}

struct OwnedObservation {
    path: LibraryPath,
    kind: &'static str,
    size: Option<i64>,
    modified_at: Option<i64>,
    metadata: Option<crate::domain::library::LibraryMetadata>,
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
    let metadata = batch
        .iter()
        .filter_map(|item| {
            item.metadata
                .as_ref()
                .map(|metadata| (&item.path, metadata))
        })
        .collect::<Vec<_>>();
    store.record_library_batch(claim, &observations, &metadata)?;
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
        let folded_name = name.to_ascii_lowercase();
        if folded_name.starts_with(".domus-pending-")
            || folded_name.starts_with(".domus-retained-")
            || folded_name.starts_with(".domus-restore-")
        {
            continue;
        }
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
        let metadata = if kind == "file" {
            store.check_worker_stop()?;
            store.renew_scan(claim)?;
            Some(match opened.open_file(&entry.file_name()) {
                Ok(mut file) => crate::infrastructure::library_metadata::inspect(&mut file, name),
                Err(error) => crate::domain::library::LibraryMetadata {
                    status: "unreadable".into(),
                    error: Some(error.to_string()),
                    ..Default::default()
                },
            })
        } else {
            None
        };
        batch.push(OwnedObservation {
            path,
            kind,
            size: metadata.as_ref().and_then(|item| item.size).or(size),
            modified_at: metadata
                .as_ref()
                .and_then(|item| item.modified_ns.as_ref())
                .and_then(|value| value.parse::<u128>().ok())
                .and_then(|value| i64::try_from(value / 1_000_000_000).ok())
                .or(modified_at),
            metadata,
        });
        if batch.len() == 128 {
            flush(store, claim, batch)?;
            store.check_worker_stop()?;
        }
    }
    Ok(())
}
