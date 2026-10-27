//! One explicitly selected folder request over a short-lived outgoing P
//! connection. No share enumeration, listeners, transfers, or hidden retries.
use super::protocol::{expanded, file};
use super::*;
use crate::domain::sources::{
    MAX_FOLDER_DIRECTORIES, MAX_FOLDER_FILES, SourceFolder, remote_identifier,
};
use std::collections::HashSet;

fn parse_response(body: &[u8], peer: &str, folder: &str, token: u32) -> Result<SourceFolder> {
    let expanded = expanded(body)?;
    let mut fields = Fields(&expanded);
    if fields.u32()? != token
        || remote_identifier(fields.text(4096)?).map_err(|_| protocol())? != folder
    {
        return Err(Error::Remote(
            "Soulseek folder response differs from the selected folder or token",
        ));
    }
    let count = usize::try_from(fields.u32()?).map_err(|_| protocol())?;
    if count == 0 || count > MAX_FOLDER_DIRECTORIES {
        return Err(protocol());
    }
    let mut directories = HashSet::new();
    let mut files = Vec::new();
    let prefix = format!("{folder}\\");
    for _ in 0..count {
        let directory = remote_identifier(fields.text(4096)?).map_err(|_| protocol())?;
        if (directory != folder && !directory.starts_with(&prefix))
            || !directories.insert(directory.to_uppercase())
        {
            return Err(protocol());
        }
        let file_count = usize::try_from(fields.u32()?).map_err(|_| protocol())?;
        if file_count > MAX_FOLDER_FILES - files.len() {
            return Err(protocol());
        }
        for _ in 0..file_count {
            let mut entry = file(&mut fields)?;
            // Folder responses supply bare file names, not native or absolute
            // paths. Do not permit a child to escape its advertised directory.
            if entry.remote_path.contains(['\\', '/']) {
                return Err(protocol());
            }
            entry.remote_path = format!("{directory}\\{}", entry.remote_path);
            files.push(entry);
        }
    }
    fields.end()?;
    if !directories.contains(&folder.to_uppercase()) {
        return Err(protocol());
    }
    let listing = SourceFolder {
        peer: peer.into(),
        folder: folder.into(),
        directories: count as u16,
        files,
    };
    listing.validate().map_err(|_| protocol())?;
    Ok(listing)
}

impl PeerControl<'_> {
    /// Consume this control channel to prevent stale replies from entering
    /// another operation. Correlate the sole response by token and folder.
    /// A persistent concurrent client must instead use bounded typed routing.
    pub fn folder_contents(mut self, folder: &str, token: u32) -> Result<SourceFolder> {
        let folder = remote_identifier(folder)?;
        if token == 0 || self.pending.is_some() || self.accepted.is_some() {
            return Err(Error::Conflict(
                "folder query needs a nonzero token and an idle peer channel".into(),
            ));
        }
        let mut stream = self
            .stream
            .take()
            .ok_or_else(|| Error::Conflict("Soulseek peer channel is closed".into()))?;
        let mut body = token.to_le_bytes().to_vec();
        string(&mut body, &folder);
        stream.send(36, &body)?;
        match stream.receive(Instant::now() + OPERATION_TIMEOUT, &mut || Ok(()), |_| true)? {
            Event::Peer(PeerEvent::Folder(body)) => {
                parse_response(&body, self.username(), &folder, token)
            }
            _ => Err(protocol()),
        }
    }
}

#[cfg(test)]
mod tests;
