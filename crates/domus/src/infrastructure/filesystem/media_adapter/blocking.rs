use super::{
    Kind, ListedEntry, MAX_PAGE, MAX_READ, MediaClient, MediaDirectory, MediaFile, MediaNode,
    MediaPage,
};
use crate::domain::{LibraryPath, library::LibraryMetadata};
use crate::infrastructure::library_metadata::{FileEvidence, MetadataInput};
use crate::{Error, Result};
use std::io::{Read, Seek, SeekFrom};

/// One operation-owned session for synchronous use cases on blocking threads.
/// Live leases never enter SQLite. Drop retires the session after any failure.
pub(crate) struct BlockingMedia {
    pub(super) runtime: tokio::runtime::Runtime,
    pub(super) client: MediaClient,
    pub(super) root_id: String,
}

impl BlockingMedia {
    pub(crate) fn open_file(
        mut self,
        path: &LibraryPath,
        expected_parent: &[String],
    ) -> Result<super::BlockingFile> {
        let chain = self.open_directory(&LibraryPath::parse(path.parent())?, &mut || Ok(()))?;
        if Self::identities(&chain) != expected_parent {
            return Err(Error::Conflict("paired media parent changed".into()));
        }
        let parent = chain
            .last()
            .ok_or(Error::Internal("media ancestry is empty"))?;
        let name = path
            .as_str()
            .rsplit('/')
            .next()
            .ok_or(Error::Internal("media name is missing"))?;
        let entry = loop {
            let page = self.page(parent)?;
            if let Some(entry) = page.entries.into_iter().find(|entry| entry.name == name) {
                break entry;
            }
            if page.complete {
                return Err(Error::Conflict("paired media file is missing".into()));
            }
        };
        let file = match self
            .runtime
            .block_on(self.client.open_child(parent, &entry))?
        {
            MediaNode::File(file) => file,
            _ => return Err(Error::Conflict("paired media file changed type".into())),
        };
        Ok(super::BlockingFile {
            media: self,
            file,
            _chain: chain,
            position: 0,
        })
    }

    pub(crate) fn identities(chain: &[MediaDirectory]) -> Vec<String> {
        chain
            .iter()
            .map(|directory| directory.observation().identity.clone())
            .collect()
    }

    pub(crate) fn new(client: MediaClient, root_id: String) -> Result<Self> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
            client,
            root_id,
        })
    }
}

/// Owns the client and every parent through the last read. A new clone of the
/// public reader shares this retained object, never a checked-path reopen.
pub(crate) struct BlockingFile {
    media: BlockingMedia,
    file: MediaFile,
    _chain: Vec<MediaDirectory>,
    position: u64,
}

impl BlockingFile {
    pub(crate) fn evidence(&mut self) -> Result<super::Observation> {
        self.media
            .runtime
            .block_on(self.media.client.read(&self.file, 0, 1))?;
        Ok(self.file.observation().clone())
    }
}

impl Read for BlockingFile {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let bytes = self
            .media
            .runtime
            .block_on(self.media.client.read(
                &self.file,
                self.position.min(self.file.observation().length),
                output.len().min(MAX_READ),
            ))
            .map_err(std::io::Error::other)?;
        output[..bytes.len()].copy_from_slice(&bytes);
        self.position += bytes.len() as u64;
        Ok(bytes.len())
    }
}

impl Seek for BlockingFile {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.position = match position {
            SeekFrom::Start(value) => Some(value),
            SeekFrom::Current(value) => self.position.checked_add_signed(value),
            SeekFrom::End(value) => self.file.observation().length.checked_add_signed(value),
        }
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid media seek")
        })?;
        Ok(self.position)
    }
}

impl BlockingMedia {
    /// Keep every ancestor lease until the caller closes the first directory.
    /// Each descent uses an exact current parent-relative listed observation.
    pub(crate) fn open_directory(
        &mut self,
        path: &LibraryPath,
        checkpoint: &mut impl FnMut() -> Result<()>,
    ) -> Result<Vec<MediaDirectory>> {
        checkpoint()?;
        let root = self
            .runtime
            .block_on(self.client.open_root(&self.root_id))?;
        let mut chain = vec![root];
        for component in path.as_str().split('/').filter(|part| !part.is_empty()) {
            let parent = chain
                .last()
                .ok_or(Error::Internal("media ancestry is empty"))?;
            let entry = loop {
                checkpoint()?;
                let page = self.page(parent)?;
                if let Some(entry) = page
                    .entries
                    .into_iter()
                    .find(|entry| entry.name == component)
                {
                    break entry;
                }
                if page.complete {
                    return Err(Error::Conflict(
                        "media directory is no longer present".into(),
                    ));
                }
            };
            if entry.observed.kind != Kind::Directory {
                return Err(Error::Conflict("media directory changed type".into()));
            }
            checkpoint()?;
            match self
                .runtime
                .block_on(self.client.open_child(parent, &entry))?
            {
                MediaNode::Directory(directory) => chain.push(directory),
                MediaNode::File(_) => {
                    return Err(Error::Conflict("media directory changed type".into()));
                }
            }
        }
        Ok(chain)
    }

    pub(crate) fn page(&mut self, directory: &MediaDirectory) -> Result<MediaPage> {
        self.runtime.block_on(self.client.page(directory, MAX_PAGE))
    }

    pub(crate) fn close_directory(&mut self, directory: MediaDirectory) -> Result<()> {
        self.runtime
            .block_on(self.client.close_directory(directory))
    }

    pub(crate) fn inspect_file(
        &mut self,
        parent: &MediaDirectory,
        entry: &ListedEntry,
        checkpoint: &mut impl FnMut() -> Result<()>,
    ) -> Result<LibraryMetadata> {
        checkpoint()?;
        let file = match self
            .runtime
            .block_on(self.client.open_child(parent, entry))?
        {
            MediaNode::File(file) => file,
            MediaNode::Directory(_) => {
                return Err(Error::Conflict("media file changed type".into()));
            }
        };
        let mut reader = MediaReader {
            runtime: &self.runtime,
            client: &mut self.client,
            file: &file,
            position: 0,
            checkpoint,
            failure: None,
        };
        let metadata = crate::infrastructure::library_metadata::inspect(&mut reader, &entry.name);
        // Parsing errors are file diagnostics. Transport, authority and claim
        // failures are scan failures and must not become successful coverage.
        if let Some(error) = reader.failure {
            return Err(error);
        }
        self.runtime.block_on(self.client.close_file(file))?;
        Ok(metadata)
    }
}

struct MediaReader<'a, F> {
    runtime: &'a tokio::runtime::Runtime,
    client: &'a mut MediaClient,
    file: &'a MediaFile,
    position: u64,
    checkpoint: &'a mut F,
    failure: Option<Error>,
}

impl<F: FnMut() -> Result<()>> MediaReader<'_, F> {
    fn read_at(&mut self, offset: u64, count: usize) -> Result<Vec<u8>> {
        (self.checkpoint)()?;
        self.runtime
            .block_on(self.client.read(self.file, offset, count))
    }

    fn failed(&mut self, error: Error) -> std::io::Error {
        let diagnostic = std::io::Error::other(error.to_string());
        self.failure = Some(error);
        diagnostic
    }
}

impl<F: FnMut() -> Result<()>> MetadataInput for MediaReader<'_, F> {
    fn evidence(&mut self) -> Result<FileEvidence> {
        if let Err(error) = self.read_at(0, 1) {
            let diagnostic = self.failed(error);
            return Err(Error::Io(diagnostic));
        }
        let observed = self.file.observation();
        Ok(FileEvidence {
            identity: observed.identity.clone(),
            length: observed.length,
            modified_ns: Some(
                (u128::from(observed.modified_seconds) * 1_000_000_000
                    + u128::from(observed.modified_nanoseconds))
                .to_string(),
            ),
        })
    }
}

impl<F: FnMut() -> Result<()>> Read for MediaReader<'_, F> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let bytes = self
            .read_at(
                self.position.min(self.file.observation().length),
                output.len().min(MAX_READ),
            )
            .map_err(|error| self.failed(error))?;
        output[..bytes.len()].copy_from_slice(&bytes);
        self.position += bytes.len() as u64;
        Ok(bytes.len())
    }
}

impl<F: FnMut() -> Result<()>> Seek for MediaReader<'_, F> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let position = match position {
            SeekFrom::Start(value) => Some(value),
            SeekFrom::Current(value) => self.position.checked_add_signed(value),
            SeekFrom::End(value) => self.file.observation().length.checked_add_signed(value),
        }
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid media seek")
        })?;
        self.position = position;
        Ok(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::filesystem::media_adapter::tests::Fixture;

    #[test]
    fn media_adapter_metadata_propagates_stop_and_changed_authority_instead_of_file_diagnostics()
    -> Result<()> {
        let fixture = Fixture::new()?;
        for changed in [false, true] {
            let client = MediaClient::local(fixture.selection.clone(), Vec::new())?;
            let mut media = BlockingMedia::new(client, "library".into())?;
            let chain = media.open_directory(&LibraryPath::parse("album")?, &mut || Ok(()))?;
            let parent = chain.last().ok_or(Error::Internal("missing parent"))?;
            let entry = media
                .page(parent)?
                .entries
                .into_iter()
                .next()
                .ok_or(Error::Internal("missing file"))?;
            let file = fixture
                .selection
                .provider
                .boundary
                .join("parent")
                .join("media")
                .join("album")
                .join("track.flac");
            let mut calls = 0;
            let result = media.inspect_file(parent, &entry, &mut || {
                calls += 1;
                if calls == 2 {
                    if changed {
                        std::fs::write(&file, b"changed generated media with a different length")?;
                    } else {
                        return Err(Error::WorkerStopping);
                    }
                }
                Ok(())
            });
            if changed {
                assert!(matches!(result, Err(Error::Conflict(_))));
            } else {
                assert!(matches!(result, Err(Error::WorkerStopping)));
            }
        }
        Ok(())
    }
}
