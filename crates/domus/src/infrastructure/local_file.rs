use super::filesystem::{LibraryDirectory, require_local_disk};
use super::permissions::FilePermissions;
use crate::{Error, Result};
use std::io::Read;
use std::path::Path;

pub(crate) fn read_local(path: &Path, maximum: u64, sensitive: bool) -> Result<Vec<u8>> {
    if !path.is_absolute() {
        return Err(Error::InvalidInput(
            "service file paths must be absolute".into(),
        ));
    }
    require_local_disk(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("service file parent is missing".into()))?;
    let directory = LibraryDirectory::open(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::InvalidInput("service file name is missing".into()))?;
    let file = directory.open_file(name)?;
    FilePermissions::current()?.check(&file, sensitive)?;
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(Error::InvalidInput(
            "service file exceeds its size limit".into(),
        ));
    }
    Ok(bytes)
}

pub(crate) fn protect_files(store: &super::store::Store, files: &[&Path]) -> Result<()> {
    let parents = files
        .iter()
        .map(|path| {
            LibraryDirectory::open(
                path.parent()
                    .ok_or_else(|| Error::InvalidInput("service file parent is missing".into()))?,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let canonical = files
        .iter()
        .map(std::fs::canonicalize)
        .collect::<std::io::Result<Vec<_>>>()?;
    let mut after = 0;
    loop {
        let locations = store.locations(crate::domain::Page::new(after, 200)?)?;
        if locations.is_empty() {
            break;
        }
        for location in locations {
            after = location.id;
            if canonical
                .iter()
                .any(|path| path.starts_with(&location.root))
            {
                return Err(Error::InvalidInput(
                    "service configuration, requests and credentials must remain outside all media roots".into(),
                ));
            }
            if let Some(saved) = location.native_identity {
                if parents
                    .iter()
                    .any(|parent| parent.native_root().ancestors.contains(&saved))
                {
                    return Err(Error::InvalidInput(
                        "service input files overlap a media root identity".into(),
                    ));
                }
            } else {
                return Err(Error::Conflict(
                    "bind every configured media root before service startup".into(),
                ));
            }
        }
    }
    Ok(())
}
