use super::*;

/// One full-stream-verified source retained for exact file removal. This is
/// neither application approval nor a readable or writable file capability.
pub struct MediaRemoval {
    file: MediaFile,
    name: String,
}

impl MediaRemoval {
    pub fn observation(&self) -> &Observation {
        &self.file.observed
    }
}

impl MediaClient {
    pub async fn prepare_removal(
        &mut self,
        parent: &MediaDirectory,
        entry: &ListedEntry,
        length: u64,
        sha256: &str,
    ) -> Result<MediaRemoval> {
        self.prepare_removal_checked(parent, entry, length, sha256, &mut || Ok(()))
            .await
    }

    /// Full hash verification uses the actual DELETE-capable Windows handle.
    /// The caller must renew its claim and check current approval at each step.
    pub async fn prepare_removal_checked(
        &mut self,
        parent: &MediaDirectory,
        entry: &ListedEntry,
        length: u64,
        sha256: &str,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<MediaRemoval> {
        let file = self
            .verify_existing(parent, entry, length, sha256, true, checkpoint)
            .await?;
        Ok(MediaRemoval {
            file,
            name: entry.name.clone(),
        })
    }

    /// Commit exact per-file deletion intent before this irreversible effect.
    /// A failed or lost response requires explicit journal reconciliation.
    pub async fn remove_file(
        &mut self,
        parent: &mut MediaDirectory,
        source: MediaRemoval,
    ) -> Result<()> {
        self.remove_file_checked(parent, source, &mut || Ok(()))
            .await
    }

    pub async fn remove_file_checked(
        &mut self,
        parent: &mut MediaDirectory,
        source: MediaRemoval,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.writable_parent(parent, &source.name)?;
        self.remove_reference(
            parent,
            &source.file.owner,
            source.file.reference,
            Kind::File,
            checkpoint,
        )
        .await
    }

    /// Remove only one exact empty directory through its retained move lease.
    /// Nonempty directories and selected roots are never removed.
    pub async fn remove_directory(
        &mut self,
        parent: &mut MediaDirectory,
        source: MediaMove,
    ) -> Result<()> {
        self.remove_directory_checked(parent, source, &mut || Ok(()))
            .await
    }

    pub async fn remove_directory_checked(
        &mut self,
        parent: &mut MediaDirectory,
        source: MediaMove,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let source = source.directory;
        self.remove_reference(
            parent,
            &source.owner,
            source.reference,
            Kind::Directory,
            checkpoint,
        )
        .await
    }

    async fn remove_reference(
        &mut self,
        parent: &mut MediaDirectory,
        owner: &str,
        reference: Reference,
        kind: Kind,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let root = self.require(&parent.owner, &parent.reference, Kind::Directory)?;
        if !root.writable {
            return Err(Error::Conflict("media root is selected read-only".into()));
        }
        let source = self.require(owner, &reference, kind)?;
        if source.parent != Some(parent.reference.id)
            || self
                .live
                .values()
                .any(|entry| entry.parent == Some(reference.id))
        {
            return Err(Error::Conflict(
                "removal requires the original parent and closed child leases".into(),
            ));
        }
        checkpoint()?;
        let id = reference.id;
        let request = match kind {
            Kind::Directory => Request::RemoveDirectory {
                directory: reference,
            },
            Kind::File => Request::RemoveFile { file: reference },
        };
        let Response::Removed { parent: after } = self.execute(request).await? else {
            return self.protocol_error();
        };
        self.update_parent(parent, after)?;
        self.live.remove(&id);
        Ok(())
    }

    pub async fn close_removal(&mut self, source: MediaRemoval) -> Result<()> {
        self.close_file(source.file).await
    }
}
