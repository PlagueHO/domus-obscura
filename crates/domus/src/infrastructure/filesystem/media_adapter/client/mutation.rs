use super::super::mutation::receipt;
use super::*;
use crate::domain::LibraryPath;

/// One new output on its original connection. It cannot be read, cloned,
/// serialized or reopened for writing. Seal verifies and syncs its exact body.
pub struct MediaOutput {
    owner: String,
    reference: Reference,
    length: u64,
    written: u64,
}

impl MediaClient {
    pub(super) async fn named_mutation(
        &mut self,
        request: Request,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Response> {
        let mut checked = 0u64;
        loop {
            checkpoint()?;
            match self.execute(request.clone()).await? {
                Response::CheckingName { checked: progress }
                    if checked.checked_add(super::MAX_PAGE as u64) == Some(progress) =>
                {
                    checked = progress;
                }
                response @ (Response::Created { .. } | Response::Renamed { .. }) => {
                    return Ok(response);
                }
                _ => return self.protocol_error(),
            }
        }
    }

    pub(super) fn writable_parent(&self, parent: &MediaDirectory, name: &str) -> Result<()> {
        self.capacity()?;
        let live = self.require(&parent.owner, &parent.reference, Kind::Directory)?;
        let path = LibraryPath::parse(name)?;
        if !live.writable {
            return Err(Error::Conflict("media root is selected read-only".into()));
        }
        if live.depth >= MAX_DEPTH
            || path.as_str().is_empty()
            || path.as_str().contains('/')
            || name.len() > 1024
        {
            return Err(Error::InvalidInput(
                "media mutation requires one bounded component".into(),
            ));
        }
        Ok(())
    }

    fn created_node(
        &mut self,
        parent: &mut MediaDirectory,
        response: Response,
        kind: Kind,
    ) -> Result<MediaNode> {
        let Response::Created {
            reference,
            observed,
            parent: after,
        } = response
        else {
            return self.protocol_error();
        };
        if observed.kind != kind
            || self.check_observation(&after).is_err()
            || after.kind != Kind::Directory
            || after.identity != parent.observed.identity
            || (kind == Kind::File && (observed.length != 0 || observed.links != 1))
        {
            return self.protocol_error();
        }
        let node = self.insert(
            Response::Opened {
                reference,
                observed,
            },
            Some(parent.reference.id),
        )?;
        self.update_parent(parent, after)?;
        Ok(node)
    }

    pub(super) fn update_parent(
        &mut self,
        parent: &mut MediaDirectory,
        after: Observation,
    ) -> Result<()> {
        if self.check_observation(&after).is_err()
            || after.kind != Kind::Directory
            || after.identity != parent.observed.identity
        {
            return self.protocol_error();
        }
        parent.observed = after.clone();
        let live = self
            .live
            .get_mut(&parent.reference.id)
            .ok_or(Error::Internal("media parent lease disappeared"))?;
        live.observed = after;
        live.complete = false;
        Ok(())
    }

    /// Create only a missing child in an explicitly writable selected root.
    /// The caller must commit its approved application intent before this call.
    pub async fn create_directory(
        &mut self,
        parent: &mut MediaDirectory,
        name: &str,
    ) -> Result<MediaDirectory> {
        self.create_directory_checked(parent, name, &mut || Ok(()))
            .await
    }

    /// Check the caller's application claim before every bounded name page and
    /// before creation. A denied checkpoint does not create the child.
    pub async fn create_directory_checked(
        &mut self,
        parent: &mut MediaDirectory,
        name: &str,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<MediaDirectory> {
        self.writable_parent(parent, name)?;
        let response = self
            .named_mutation(
                Request::CreateDirectory {
                    parent: parent.reference.clone(),
                    name: name.into(),
                },
                checkpoint,
            )
            .await?;
        match self.created_node(parent, response, Kind::Directory)? {
            MediaNode::Directory(directory) => Ok(directory),
            _ => self.protocol_error(),
        }
    }

    /// Start a missing file only. A complete stream must match its frozen
    /// length and SHA-256 before sealing. Existing and interrupted files stay unchanged.
    pub async fn create_file(
        &mut self,
        parent: &mut MediaDirectory,
        name: &str,
        length: u64,
        sha256: &str,
    ) -> Result<MediaOutput> {
        self.create_file_checked(parent, name, length, sha256, &mut || Ok(()))
            .await
    }

    /// The checkpoint also runs between name-check pages, not only before the
    /// first request. Existing journal and approval checks belong there.
    pub async fn create_file_checked(
        &mut self,
        parent: &mut MediaDirectory,
        name: &str,
        length: u64,
        sha256: &str,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<MediaOutput> {
        self.writable_parent(parent, name)?;
        receipt(length, sha256)?;
        if length == 0 {
            return Err(Error::InvalidInput(
                "invalid bounded native output receipt".into(),
            ));
        }
        let response = self
            .named_mutation(
                Request::CreateFile {
                    parent: parent.reference.clone(),
                    name: name.into(),
                    length,
                    sha256: sha256.into(),
                },
                checkpoint,
            )
            .await?;
        match self.created_node(parent, response, Kind::File)? {
            MediaNode::File(file) => Ok(MediaOutput {
                owner: file.owner,
                reference: file.reference,
                length,
                written: 0,
            }),
            _ => self.protocol_error(),
        }
    }

    /// Sync and verify an exact existing file without creating a write lease.
    /// Partial, changed and different files are never repaired or truncated.
    pub async fn verify_file(
        &mut self,
        parent: &MediaDirectory,
        entry: &ListedEntry,
        length: u64,
        sha256: &str,
    ) -> Result<MediaFile> {
        self.verify_file_checked(parent, entry, length, sha256, &mut || Ok(()))
            .await
    }

    /// Recheck the application claim before admission and each bounded hash
    /// request. The actual Windows handle has sync rights, never a write API.
    pub async fn verify_file_checked(
        &mut self,
        parent: &MediaDirectory,
        entry: &ListedEntry,
        length: u64,
        sha256: &str,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<MediaFile> {
        self.verify_existing(parent, entry, length, sha256, false, checkpoint)
            .await
    }

    pub(super) async fn verify_existing(
        &mut self,
        parent: &MediaDirectory,
        entry: &ListedEntry,
        length: u64,
        sha256: &str,
        removal: bool,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<MediaFile> {
        self.writable_parent(parent, &entry.name)?;
        receipt(length, sha256)?;
        self.check_observation(&entry.observed)?;
        if entry.observed.kind != Kind::File
            || entry.observed.links != 1
            || entry.observed.length != length
        {
            return Err(Error::Conflict(
                "existing media file differs from its receipt".into(),
            ));
        }
        checkpoint()?;
        let request = if removal {
            Request::PrepareRemoval {
                parent: parent.reference.clone(),
                name: entry.name.clone(),
                expected: entry.observed.clone(),
                length,
                sha256: sha256.into(),
            }
        } else {
            Request::VerifyFile {
                parent: parent.reference.clone(),
                name: entry.name.clone(),
                expected: entry.observed.clone(),
                length,
                sha256: sha256.into(),
            }
        };
        let response = self.execute(request).await?;
        if !matches!(&response, Response::Opened { observed, .. } if observed == &entry.observed) {
            return self.protocol_error();
        }
        let MediaNode::File(file) = self.insert(response, Some(parent.reference.id))? else {
            return self.protocol_error();
        };
        self.seal_checked(
            MediaOutput {
                owner: file.owner,
                reference: file.reference,
                length,
                written: length,
            },
            checkpoint,
        )
        .await
    }

    /// Append one bounded chunk. A lost response retires the session; never
    /// infer that no bytes were written or resend the chunk on a fresh connection.
    pub async fn write(&mut self, output: &mut MediaOutput, bytes: &[u8]) -> Result<()> {
        self.require(&output.owner, &output.reference, Kind::File)?;
        let end = output
            .written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| Error::InvalidInput("native output length overflow".into()))?;
        if bytes.is_empty() || bytes.len() > MAX_READ || end > output.length {
            return Err(Error::InvalidInput(
                "invalid bounded native output chunk".into(),
            ));
        }
        let response = self
            .execute(Request::Write {
                file: output.reference.clone(),
                offset: output.written,
                bytes: bytes.to_vec(),
            })
            .await?;
        match response {
            Response::Written { offset } if offset == end => {
                output.written = end;
                Ok(())
            }
            _ => self.protocol_error(),
        }
    }

    /// Sync and independently hash the complete output before obtaining a read
    /// lease. This is I/O evidence, not an application publication receipt.
    pub async fn seal(&mut self, output: MediaOutput) -> Result<MediaFile> {
        self.seal_checked(output, &mut || Ok(())).await
    }

    /// Renew the application claim and check stop between bounded hash steps.
    /// A denied checkpoint leaves the incomplete or verified bytes unchanged.
    pub async fn seal_checked(
        &mut self,
        output: MediaOutput,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<MediaFile> {
        let live = self.require(&output.owner, &output.reference, Kind::File)?;
        let identity = live.observed.identity.clone();
        if output.written != output.length {
            return Err(Error::Conflict("native output stream is incomplete".into()));
        }
        let mut verified = 0;
        let observed = loop {
            checkpoint()?;
            let end = verified + (output.length - verified).min(MAX_READ as u64);
            match self
                .execute(Request::Seal {
                    file: output.reference.clone(),
                })
                .await?
            {
                Response::Sealing { verified: progress }
                    if progress == end && end < output.length =>
                {
                    verified = progress;
                }
                Response::Sealed { observed } if end == output.length => break observed,
                _ => return self.protocol_error(),
            }
        };
        if self.check_observation(&observed).is_err()
            || observed.kind != Kind::File
            || observed.identity != identity
            || observed.length != output.length
            || observed.links != 1
        {
            return self.protocol_error();
        }
        self.live
            .get_mut(&output.reference.id)
            .ok_or(Error::Internal("native output lease disappeared"))?
            .observed = observed.clone();
        Ok(MediaFile {
            owner: output.owner,
            reference: output.reference,
            observed,
        })
    }

    /// Release an unfinished output without removing or changing its bytes.
    pub async fn close_output(&mut self, output: MediaOutput) -> Result<()> {
        self.require(&output.owner, &output.reference, Kind::File)?;
        self.close(output.reference).await
    }
}
