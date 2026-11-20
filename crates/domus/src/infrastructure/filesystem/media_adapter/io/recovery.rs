use super::*;
use crate::infrastructure::recovery_files;
use crate::infrastructure::store::{RecoveryFile, ReplacementFile};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

const MAX_FILES: usize = 1002;

fn conflict() -> Error {
    Error::Conflict("recovery source changed or remains present".into())
}

fn require_absent(parent: &LibraryDirectory, name: &str) -> Result<()> {
    match parent.symlink_metadata(name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(conflict()),
    }
}

impl MediaDirectoryIo {
    /// Confirm a name on the retained parent. Only a complete, successful lookup
    /// can report absence; inaccessible or unsafe objects remain errors.
    pub(crate) fn recovery_entry(&self, name: &str) -> Result<Option<MediaEntryIo>> {
        super::super::mutation::component(name)?;
        self.check()?;
        match self {
            Self::Native(parent) => match parent.symlink_metadata(name) {
                Ok(metadata) => {
                    if is_unsafe_cap_link(&metadata) || (!metadata.is_dir() && !metadata.is_file())
                    {
                        return Err(conflict());
                    }
                    Ok(Some(MediaEntryIo {
                        name: name.into(),
                        directory: metadata.is_dir(),
                        observed: None,
                    }))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            },
            Self::Paired(parent) => Ok(Self::find(parent, name)?.map(|entry| MediaEntryIo {
                name: entry.name,
                directory: entry.observed.kind == Kind::Directory,
                observed: Some(entry.observed),
            })),
        }
    }

    pub(crate) fn recovery_names(&self) -> Result<Vec<String>> {
        self.check_named_directory(&|| Ok(()))?;
        let mut names = Vec::new();
        let mut folded = HashSet::new();
        for entry in self.entries_checked(MAX_FILES, &|| Ok(()))? {
            super::super::mutation::component(&entry.name)?;
            if !folded.insert(entry.name.to_uppercase()) {
                return Err(conflict());
            }
            names.push(entry.name);
        }
        names.sort();
        Ok(names)
    }

    /// Hash one exact regular file through its retained lease. Native recovery
    /// keeps its existing actual-handle validation; paired IDs stay opaque.
    pub(crate) fn recovery_file(&self, name: &str) -> Result<RecoveryFile> {
        if let Self::Native(parent) = self {
            return recovery_files::file(parent, name);
        }
        let entry = self.recovery_entry(name)?.ok_or_else(conflict)?;
        let mut file = self.open_file(&entry)?;
        let before = file.evidence()?;
        if before.links != 1 || before.bytes > i64::MAX as u64 {
            return Err(conflict());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 65_536];
        let mut total = 0u64;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total.checked_add(count as u64).ok_or_else(conflict)?;
            if total > before.bytes {
                return Err(conflict());
            }
            hash.update(&buffer[..count]);
        }
        if total != before.bytes || file.evidence()? != before {
            return Err(conflict());
        }
        file.close()?;
        Ok(RecoveryFile {
            name: name.into(),
            bytes: total,
            sha256: format!("{:x}", hash.finalize()),
            identity: before.identity,
        })
    }

    /// Verify the complete flat recovery manifest, including recipe envelopes.
    /// Return identities from this operation, never translated native IDs.
    pub(crate) fn inspect_recovery(
        &self,
        expected: &[ReplacementFile],
    ) -> Result<(String, Vec<RecoveryFile>)> {
        self.check_named_directory(&|| Ok(()))?;
        if let Self::Native(directory) = self {
            return recovery_files::inspect(directory, expected);
        }
        if expected.len() > MAX_FILES {
            return Err(conflict());
        }
        let mut manifest = BTreeMap::new();
        let mut folded = HashSet::new();
        for entry in expected {
            super::super::mutation::component(&entry.name)?;
            if !folded.insert(entry.name.to_uppercase())
                || manifest.insert(entry.name.as_str(), entry).is_some()
            {
                return Err(conflict());
            }
        }
        let listing = self.recovery_names()?;
        if listing.len() != manifest.len()
            || listing
                .iter()
                .any(|name| !manifest.contains_key(name.as_str()))
        {
            return Err(conflict());
        }
        let mut files = Vec::with_capacity(listing.len());
        for name in &listing {
            let expected = manifest.get(name.as_str()).ok_or_else(conflict)?;
            let file = self.recovery_file(name)?;
            if file.bytes != expected.bytes || file.sha256 != expected.sha256 {
                return Err(conflict());
            }
            files.push(file);
        }
        if self.recovery_names()? != listing {
            return Err(conflict());
        }
        self.check_named_directory(&|| Ok(()))?;
        Ok((self.identity().into(), files))
    }

    /// Remove only the exact full-stream-verified source. Commit per-file intent
    /// first. Checkpoints may use local state, never this media session.
    pub(crate) fn remove_recovery_file(
        &self,
        expected: &RecoveryFile,
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        super::super::mutation::component(&expected.name)?;
        checkpoint()?;
        self.check()?;
        match self {
            Self::Native(parent) => {
                recovery_files::delete_file_checked(parent, expected, checkpoint)?;
                require_absent(parent, &expected.name)
            }
            Self::Paired(parent) => {
                let entry =
                    Self::find_checked(parent, &expected.name, checkpoint)?.ok_or_else(conflict)?;
                if entry.observed.kind != Kind::File
                    || entry.observed.identity != expected.identity
                    || entry.observed.length != expected.bytes
                {
                    return Err(conflict());
                }
                checkpoint()?;
                let source = parent.with(|runtime, client, directory| {
                    runtime.block_on(client.prepare_removal_checked(
                        directory,
                        &entry,
                        expected.bytes,
                        &expected.sha256,
                        &mut || checkpoint().map_err(Error::from),
                    ))
                })?;
                parent.with_mut(|runtime, client, directory| {
                    runtime.block_on(client.remove_file_checked(directory, source, &mut || {
                        checkpoint().map_err(Error::from)
                    }))
                })
            }
        }
    }

    /// Remove one exact empty directory through the actual retained source.
    /// Close ordinary source handles first. Never recurse or replay an
    /// uncertain effect without the caller's existing recovery journal.
    pub(crate) fn remove_recovery_directory(
        &self,
        name: &str,
        identity: &str,
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        super::super::mutation::component(name)?;
        checkpoint()?;
        self.check()?;
        match self {
            Self::Native(parent) => {
                recovery_files::remove_directory_checked(parent, name, identity, checkpoint)?;
                require_absent(parent, name)
            }
            Self::Paired(parent) => {
                let entry = Self::find_checked(parent, name, checkpoint)?.ok_or_else(conflict)?;
                if entry.observed.kind != Kind::Directory || entry.observed.identity != identity {
                    return Err(conflict());
                }
                checkpoint()?;
                let source = parent.with(|runtime, client, directory| {
                    runtime.block_on(client.prepare_move(directory, &entry))
                })?;
                parent.with_mut(|runtime, client, directory| {
                    runtime.block_on(client.remove_directory_checked(
                        directory,
                        source,
                        &mut || checkpoint().map_err(Error::from),
                    ))
                })
            }
        }
    }
}
