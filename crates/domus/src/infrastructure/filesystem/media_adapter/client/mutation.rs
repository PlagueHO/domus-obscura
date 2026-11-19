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
    async fn create(&mut self, request: Request) -> Result<Response> {
        let mut checked = 0u64;
        loop {
            match self.execute(request.clone()).await? {
                Response::CheckingName { checked: progress }
                    if checked.checked_add(super::MAX_PAGE as u64) == Some(progress) =>
                {
                    checked = progress;
                }
                response @ Response::Created { .. } => return Ok(response),
                _ => return self.protocol_error(),
            }
        }
    }

    fn writable_parent(&self, parent: &MediaDirectory, name: &str) -> Result<()> {
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
        parent.observed = after.clone();
        let live = self
            .live
            .get_mut(&parent.reference.id)
            .ok_or(Error::Internal("media parent lease disappeared"))?;
        live.observed = after;
        live.complete = false;
        Ok(node)
    }

    /// Create only a missing child in an explicitly writable selected root.
    /// The caller must commit its approved application intent before this call.
    pub async fn create_directory(
        &mut self,
        parent: &mut MediaDirectory,
        name: &str,
    ) -> Result<MediaDirectory> {
        self.writable_parent(parent, name)?;
        let response = self
            .create(Request::CreateDirectory {
                parent: parent.reference.clone(),
                name: name.into(),
            })
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
        self.writable_parent(parent, name)?;
        if length == 0
            || length > i64::MAX as u64
            || sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::InvalidInput(
                "invalid bounded native output receipt".into(),
            ));
        }
        let response = self
            .create(Request::CreateFile {
                parent: parent.reference.clone(),
                name: name.into(),
                length,
                sha256: sha256.into(),
            })
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
        let live = self.require(&output.owner, &output.reference, Kind::File)?;
        let identity = live.observed.identity.clone();
        if output.written != output.length {
            return Err(Error::Conflict("native output stream is incomplete".into()));
        }
        let mut verified = 0;
        let observed = loop {
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
