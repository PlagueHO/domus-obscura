use super::*;
use crate::infrastructure::filesystem::{is_unsafe_link, sync_directory};
use crate::infrastructure::store::{PublicationFile, ReplacementFile};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

fn conflict() -> Error {
    Error::Conflict(
        "publication file differs from its approved receipt; nothing was overwritten".into(),
    )
}

fn folder_conflict() -> Error {
    Error::Conflict(
        "media folder changed or differs from the expected files; nothing was overwritten".into(),
    )
}

fn sort_manifest(files: &mut [ReplacementFile]) {
    files.sort_by(|left, right| {
        left.name
            .to_uppercase()
            .cmp(&right.name.to_uppercase())
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn options(create: bool) -> cap_std::fs::OpenOptions {
    let mut options = cap_std::fs::OpenOptions::new();
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

fn regular(file: File) -> Result<File> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || is_unsafe_link(&metadata)
        || crate::infrastructure::filesystem::read_only::link_count(&file)? != 1
    {
        return Err(conflict());
    }
    FilePermissions::current()?.check(&file, false)?;
    Ok(file)
}

fn validate(entry: &PublicationFile) -> Result<()> {
    let path = LibraryPath::parse(&entry.name)?;
    super::super::mutation::receipt(entry.bytes, &entry.sha256)?;
    if path.as_str().is_empty() || path.as_str().contains('/') {
        return Err(conflict());
    }
    Ok(())
}

fn verify_native(
    file: &mut File,
    entry: &PublicationFile,
    checkpoint: &dyn Fn() -> std::io::Result<()>,
) -> Result<()> {
    let before = ReadFile::from_native(file.try_clone()?).evidence()?;
    if before.bytes != entry.bytes || before.links != 1 {
        return Err(conflict());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65_536];
    let mut remaining = entry.bytes;
    while remaining > 0 {
        checkpoint()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if file.read(&mut [0; 1])? != 0
        || ReadFile::from_native(file.try_clone()?).evidence()? != before
        || format!("{:x}", hash.finalize()) != entry.sha256
    {
        return Err(conflict());
    }
    Ok(())
}

impl MediaDirectoryIo {
    pub(crate) fn replacement_snapshot(
        &self,
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<(String, Vec<ReplacementFile>)> {
        checkpoint()?;
        self.check()?;
        let identity = self.identity().to_owned();
        let mut files = Vec::new();
        let mut names = HashSet::new();
        for entry in self.entries_checked(1001, checkpoint)? {
            checkpoint()?;
            let relative = LibraryPath::parse(&entry.name)?;
            if entry.directory
                || relative.as_str().is_empty()
                || relative.as_str().contains('/')
                || !names.insert(entry.name.to_uppercase())
            {
                return Err(folder_conflict());
            }
            let mut file = self.open_file(&entry)?;
            let before = file.evidence()?;
            if before.bytes > i64::MAX as u64 || before.links != 1 {
                return Err(folder_conflict());
            }
            let mut hash = Sha256::new();
            let mut buffer = [0; 65_536];
            let mut total = 0u64;
            loop {
                checkpoint()?;
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                total = total
                    .checked_add(count as u64)
                    .ok_or_else(folder_conflict)?;
                if total > before.bytes {
                    return Err(folder_conflict());
                }
                hash.update(&buffer[..count]);
            }
            if total != before.bytes || file.evidence()? != before {
                return Err(folder_conflict());
            }
            file.close()?;
            files.push(ReplacementFile {
                name: entry.name,
                bytes: total,
                sha256: format!("{:x}", hash.finalize()),
            });
        }
        checkpoint()?;
        self.check()?;
        sort_manifest(&mut files);
        Ok((identity, files))
    }

    pub(crate) fn verify_replacement(
        &self,
        expected: &[ReplacementFile],
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        if expected.len() > 1001 {
            return Err(folder_conflict());
        }
        let (_, actual) = self.replacement_snapshot(checkpoint)?;
        let mut expected = expected.to_vec();
        sort_manifest(&mut expected);
        if actual != expected {
            return Err(folder_conflict());
        }
        Ok(())
    }

    /// Verify through the actual move-capable source, then rename that same
    /// handle without replacement. Any failure retires this operation; a lost
    /// reply never authorizes a pathname retry. The caller owns durable intent.
    pub(crate) fn rename_directory(
        &self,
        from: &str,
        to: &str,
        identity: &str,
        expected: &[ReplacementFile],
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        super::super::mutation::component(from)?;
        super::super::mutation::component(to)?;
        if expected.len() > 1001 {
            return Err(folder_conflict());
        }
        checkpoint()?;
        self.check()?;
        let source = match self {
            Self::Native(parent) => {
                Self::check_native_case(parent, to, checkpoint)?;
                checkpoint()?;
                Self::native(parent.open_child_for_move(from)?)?
            }
            Self::Paired(parent) => {
                let entry =
                    Self::find_checked(parent, from, checkpoint)?.ok_or_else(folder_conflict)?;
                checkpoint()?;
                let source = parent.with(|runtime, client, directory| {
                    runtime.block_on(client.prepare_move(directory, &entry))
                })?;
                Self::Paired(Arc::new(PairedDirectoryIo {
                    identity: source.directory().observation().identity.clone(),
                    media: parent.media.clone(),
                    directory: Mutex::new(Some(DirectoryLease::Move(source))),
                    _parent: Some(parent.clone()),
                }))
            }
        };
        if source.identity() != identity {
            return Err(folder_conflict());
        }
        source.verify_replacement(expected, checkpoint)?;
        checkpoint()?;
        match (self, source) {
            (Self::Native(parent), Self::Native(source)) => {
                parent.rename_retained_child(&source, to)?;
                sync_directory(parent)?;
            }
            (Self::Paired(parent), Self::Paired(source)) => {
                if Arc::strong_count(&source) != 1 {
                    return Err(Error::Conflict("move source still has child leases".into()));
                }
                let lease = source
                    .directory
                    .lock()
                    .map_err(|_| Error::Internal("media directory lock failed"))?
                    .take();
                let Some(DirectoryLease::Move(source)) = lease else {
                    return Err(Error::Internal("reviewed move source disappeared"));
                };
                parent.with_mut(|runtime, client, directory| {
                    runtime.block_on(client.rename_directory_checked(
                        directory,
                        source,
                        to,
                        &mut || checkpoint().map_err(Error::from),
                    ))
                })?;
            }
            _ => return Err(Error::Internal("move source changed I/O profile")),
        }
        Ok(())
    }

    /// Copy only a missing output, or sync and verify an exact existing output.
    /// The caller owns durable intent and approval. Checkpoints may access local
    /// claims and staging, never this media session or another async runtime.
    pub(crate) fn copy_publication_file(
        &self,
        entry: &PublicationFile,
        source: &mut File,
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        checkpoint()?;
        validate(entry)?;
        if entry.bytes == 0 {
            return Err(conflict());
        }
        self.check()?;
        let mut existing = None;
        for candidate in self.entries_checked(1001, checkpoint)? {
            if candidate.name.to_uppercase() == entry.name.to_uppercase() {
                if candidate.name != entry.name || candidate.directory || existing.is_some() {
                    return Err(conflict());
                }
                existing = Some(candidate);
            }
        }
        if entry.verified_at.is_some() && existing.is_none() {
            return Err(conflict());
        }
        match self {
            Self::Native(directory) => {
                let (mut file, created) = if existing.is_some() {
                    (
                        regular(directory.open_with(&entry.name, &options(false))?.into_std())?,
                        false,
                    )
                } else {
                    match directory.open_with(&entry.name, &options(true)) {
                        Ok(file) => (regular(file.into_std())?, true),
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
                            regular(directory.open_with(&entry.name, &options(false))?.into_std())?,
                            false,
                        ),
                        Err(error) => return Err(error.into()),
                    }
                };
                if created {
                    let mut remaining = entry.bytes;
                    let mut buffer = [0; 65_536];
                    while remaining > 0 {
                        checkpoint()?;
                        let count = remaining.min(buffer.len() as u64) as usize;
                        source.read_exact(&mut buffer[..count])?;
                        checkpoint()?;
                        file.write_all(&buffer[..count])?;
                        #[cfg(test)]
                        if entry.ordinal > 0 {
                            crate::crash_tests::checkpoint("publication_writing")?;
                        }
                        remaining -= count as u64;
                    }
                    if source.read(&mut [0; 1])? != 0 {
                        return Err(conflict());
                    }
                }
                checkpoint()?;
                file.sync_all()?;
                verify_native(&mut file, entry, checkpoint)?;
                sync_directory(directory)?;
            }
            Self::Paired(directory) => directory.with_mut(|runtime, client, parent| {
                runtime.block_on(async {
                    let mut check = || checkpoint().map_err(Error::from);
                    if let Some(existing) = existing {
                        let listed = ListedEntry {
                            name: existing.name,
                            observed: existing
                                .observed
                                .ok_or(Error::Internal("paired publication observation missing"))?,
                        };
                        let file = client
                            .verify_file_checked(
                                parent,
                                &listed,
                                entry.bytes,
                                &entry.sha256,
                                &mut check,
                            )
                            .await?;
                        return client.close_file(file).await;
                    }
                    let mut output = client
                        .create_file_checked(
                            parent,
                            &entry.name,
                            entry.bytes,
                            &entry.sha256,
                            &mut check,
                        )
                        .await?;
                    let copied: Result<()> = async {
                        let mut remaining = entry.bytes;
                        let mut buffer = [0; MAX_READ];
                        while remaining > 0 {
                            checkpoint()?;
                            let count = remaining.min(buffer.len() as u64) as usize;
                            source.read_exact(&mut buffer[..count])?;
                            checkpoint()?;
                            client.write(&mut output, &buffer[..count]).await?;
                            #[cfg(test)]
                            if entry.ordinal > 0 {
                                crate::crash_tests::checkpoint("publication_writing")?;
                            }
                            remaining -= count as u64;
                        }
                        if source.read(&mut [0; 1])? != 0 {
                            return Err(conflict());
                        }
                        Ok(())
                    }
                    .await;
                    if let Err(error) = copied {
                        return match client.close_output(output).await {
                            Ok(()) => Err(error),
                            Err(close) => Err(Error::Conflict(format!(
                                "{error}; unfinished publication output could not be released: {close}"
                            ))),
                        };
                    }
                    let file = client.seal_checked(output, &mut check).await?;
                    client.close_file(file).await
                })
            })?,
        }
        self.check()?;
        checkpoint()?;
        #[cfg(test)]
        crate::crash_tests::checkpoint("publication_synced")?;
        Ok(())
    }

    pub(crate) fn verify_publication_files(
        &self,
        files: &[PublicationFile],
        checkpoint: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        checkpoint()?;
        self.check()?;
        let mut names = files
            .iter()
            .map(|file| file.name.as_str())
            .collect::<HashSet<_>>();
        let entries = self.entries_checked(files.len(), checkpoint)?;
        for entry in &entries {
            if entry.directory || !names.remove(entry.name.as_str()) {
                return Err(conflict());
            }
        }
        if !names.is_empty() || files.len() != entries.len() {
            return Err(conflict());
        }
        for entry in files {
            checkpoint()?;
            validate(entry)?;
            match self {
                Self::Native(directory) => {
                    let mut file = regular(
                        directory
                            .open_with(&entry.name, &options(false))?
                            .into_std(),
                    )?;
                    verify_native(&mut file, entry, checkpoint)?;
                }
                Self::Paired(directory) => {
                    let existing = entries
                        .iter()
                        .find(|listed| listed.name == entry.name)
                        .ok_or_else(conflict)?;
                    let listed = ListedEntry {
                        name: existing.name.clone(),
                        observed: existing
                            .observed
                            .clone()
                            .ok_or(Error::Internal("paired publication observation missing"))?,
                    };
                    directory.with(|runtime, client, parent| {
                        runtime.block_on(async {
                            let file = client
                                .verify_file_checked(
                                    parent,
                                    &listed,
                                    entry.bytes,
                                    &entry.sha256,
                                    &mut || checkpoint().map_err(Error::from),
                                )
                                .await?;
                            client.close_file(file).await
                        })
                    })?;
                }
            }
        }
        if let Self::Native(directory) = self {
            sync_directory(directory)?;
        }
        self.check()?;
        checkpoint()?;
        Ok(())
    }
}
