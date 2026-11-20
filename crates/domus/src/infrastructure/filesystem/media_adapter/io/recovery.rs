use super::*;
use crate::infrastructure::recovery_files;
use crate::infrastructure::store::RecoveryFile;

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
