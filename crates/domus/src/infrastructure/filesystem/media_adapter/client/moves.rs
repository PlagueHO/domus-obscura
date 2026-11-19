use super::*;

/// A reviewed directory retained for a same-parent, no-replace move. Close all
/// child leases before moving it. Its contents can be inspected but not written.
pub struct MediaMove {
    directory: MediaDirectory,
}

impl MediaMove {
    pub fn directory(&self) -> &MediaDirectory {
        &self.directory
    }
}

impl MediaClient {
    /// Open the exact listed source with native move rights. Existing leases
    /// for this source must first be closed; selected roots cannot be sources.
    pub async fn prepare_move(
        &mut self,
        parent: &MediaDirectory,
        entry: &ListedEntry,
    ) -> Result<MediaMove> {
        self.writable_parent(parent, &entry.name)?;
        self.check_observation(&entry.observed)?;
        if entry.observed.kind != Kind::Directory {
            return Err(Error::InvalidInput(
                "move source must be a directory".into(),
            ));
        }
        let response = self
            .execute(Request::PrepareMove {
                parent: parent.reference.clone(),
                name: entry.name.clone(),
                expected: entry.observed.clone(),
            })
            .await?;
        if !matches!(&response, Response::Opened { observed, .. } if observed == &entry.observed) {
            return self.protocol_error();
        }
        match self.insert(response, Some(parent.reference.id))? {
            MediaNode::Directory(directory) => {
                self.live
                    .get_mut(&directory.reference.id)
                    .ok_or(Error::Internal("move lease disappeared"))?
                    .writable = false;
                Ok(MediaMove { directory })
            }
            _ => self.protocol_error(),
        }
    }

    /// Commit the caller's current approved intent before invoking this effect.
    /// A lost reply is uncertain, not permission to retry against a pathname.
    pub async fn rename_directory(
        &mut self,
        parent: &mut MediaDirectory,
        source: MediaMove,
        name: &str,
    ) -> Result<Observation> {
        self.rename_directory_checked(parent, source, name, &mut || Ok(()))
            .await
    }

    /// Recheck the application claim and approval before each bounded native
    /// request, including the final effect after a multi-page name check.
    pub async fn rename_directory_checked(
        &mut self,
        parent: &mut MediaDirectory,
        source: MediaMove,
        name: &str,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Observation> {
        self.writable_parent(parent, name)?;
        let source = source.directory;
        let live = self.require(&source.owner, &source.reference, Kind::Directory)?;
        if live.parent != Some(parent.reference.id)
            || self
                .live
                .values()
                .any(|entry| entry.parent == Some(source.reference.id))
        {
            return Err(Error::Conflict(
                "move requires its original parent and closed child leases".into(),
            ));
        }
        let response = self
            .named_mutation(
                Request::Rename {
                    directory: source.reference.clone(),
                    name: name.into(),
                },
                checkpoint,
            )
            .await?;
        let Response::Renamed {
            observed,
            parent: after,
        } = response
        else {
            return self.protocol_error();
        };
        if observed.identity != source.observed.identity
            || observed.kind != Kind::Directory
            || self.check_observation(&observed).is_err()
        {
            return self.protocol_error();
        }
        self.update_parent(parent, after)?;
        self.live.remove(&source.reference.id);
        Ok(observed)
    }

    pub async fn close_move(&mut self, source: MediaMove) -> Result<()> {
        self.close_directory(source.directory).await
    }
}
