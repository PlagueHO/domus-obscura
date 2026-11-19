//! Exact nonrecursive effects. The application must commit its approved
//! per-object removal intent before submission; this is not a second journal.
use super::*;

impl Session {
    pub(super) fn remove_object(
        &mut self,
        adapter: &NativeAdapter,
        request: Request,
    ) -> Result<Response> {
        let (reference, directory) = match request {
            Request::RemoveFile { file } => (file, false),
            Request::RemoveDirectory { directory } => (directory, true),
            _ => return Err(Error::Internal("not a native removal request")),
        };
        let id = self.require(&reference)?;
        let entry = self
            .entries
            .get(&id)
            .ok_or(Error::Internal("removal source disappeared"))?;
        if !entry.writable || self.entries.values().any(|entry| entry.parent == Some(id)) {
            return Err(Error::Conflict(
                "removal source is read-only or has child leases".into(),
            ));
        }
        let parent_id = entry
            .parent
            .ok_or_else(|| Error::Conflict("selected roots cannot be removed".into()))?;
        let parent = self.mutation_parent(adapter, parent_id)?;
        let observed = match &entry.object {
            Object::File {
                file,
                removal_name: Some(_),
                ..
            } if !directory => adapter.observe(file, Kind::File)?,
            Object::Directory {
                directory: source,
                movable: true,
                ..
            } if directory => adapter.observe(&source.directory_file()?, Kind::Directory)?,
            _ => {
                return Err(Error::Conflict(
                    "removal requires a verified source capability".into(),
                ));
            }
        };
        if observed != entry.observed {
            return Err(Error::Conflict("retained removal source changed".into()));
        }
        let entry = self
            .entries
            .remove(&id)
            .ok_or(Error::Internal("removal source disappeared"))?;
        match entry.object {
            Object::File {
                file,
                removal_name: Some(name),
                ..
            } => parent.remove_child_file(&name, file)?,
            Object::Directory {
                directory,
                cursor,
                name_check,
                ..
            } => {
                drop(cursor);
                drop(name_check);
                parent.remove_child_directory(directory)?;
            }
            _ => return Err(Error::Internal("validated removal source changed kind")),
        }
        super::super::sync_directory(&parent)?;
        Ok(Response::Removed {
            parent: self.changed_parent(adapter, parent_id)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Fixture;
    use super::*;

    fn lease(response: Response) -> Result<Reference> {
        match response {
            Response::Opened { reference, .. } => Ok(reference),
            _ => Err(Error::Internal("expected generated removal lease")),
        }
    }

    #[test]
    fn media_adapter_removal_refuses_read_only_stale_different_and_hard_linked_sources()
    -> Result<()> {
        for writable in [false, true] {
            let mut fixture = Fixture::new()?;
            if writable {
                fixture.selection.writable_roots.push("library".into());
            }
            let media = fixture
                .selection
                .provider
                .boundary
                .join("parent")
                .join("media");
            let path = media.join("album").join("track.flac");
            let bytes = std::fs::read(&path)?;
            let adapter = NativeAdapter::new(fixture.selection.clone(), Vec::new())?;
            let mut session = Session::new()?;
            let root = lease(session.dispatch(&adapter, fixture.request()?)?)?;
            let parent = adapter
                .roots
                .get("library")
                .ok_or(Error::Internal("generated root missing"))?;
            let expected = adapter.observe(
                &parent
                    .open_directory(&LibraryPath::parse("album")?)?
                    .directory_file()?,
                Kind::Directory,
            )?;
            let album = lease(session.dispatch(
                &adapter,
                Request::OpenChild {
                    parent: root,
                    name: "album".into(),
                    expected,
                },
            )?)?;
            let Response::Page { entries, .. } = session.dispatch(
                &adapter,
                Request::Page {
                    directory: album.clone(),
                    limit: MAX_PAGE,
                },
            )?
            else {
                return Err(Error::Internal("generated removal listing missing"));
            };
            let listed = entries
                .first()
                .ok_or(Error::Internal("generated file missing"))?;
            let request = Request::PrepareRemoval {
                parent: album.clone(),
                name: listed.name.clone(),
                expected: listed.observed.clone(),
                length: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            };
            if !writable {
                assert!(session.dispatch(&adapter, request).is_err());
                assert_eq!(std::fs::read(&path)?, bytes);
                continue;
            }
            let mut stale = request.clone();
            if let Request::PrepareRemoval {
                expected, length, ..
            } = &mut stale
            {
                expected.length += 1;
                *length += 1;
            }
            assert!(session.dispatch(&adapter, stale).is_err());
            let mut different = request.clone();
            if let Request::PrepareRemoval { sha256, .. } = &mut different {
                *sha256 = "0".repeat(64);
            }
            let file = lease(session.dispatch(&adapter, different)?)?;
            assert!(
                session
                    .dispatch(&adapter, Request::Seal { file: file.clone() })
                    .is_err()
            );
            assert!(
                session
                    .dispatch(&adapter, Request::RemoveFile { file: file.clone() })
                    .is_err()
            );
            session.dispatch(&adapter, Request::Close { reference: file })?;
            std::fs::hard_link(&path, media.join("alias.flac"))?;
            assert!(session.dispatch(&adapter, request).is_err());
            assert_eq!(std::fs::read(&path)?, bytes);
            assert_eq!(std::fs::read(media.join("alias.flac"))?, bytes);
        }
        Ok(())
    }

    #[test]
    fn media_adapter_removal_requires_full_verification_and_never_recurses_or_replays() -> Result<()>
    {
        let mut fixture = Fixture::new()?;
        fixture.selection.writable_roots.push("library".into());
        let media = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media");
        let bytes = vec![37; MAX_READ + 3];
        std::fs::write(media.join("album").join("track.flac"), &bytes)?;
        std::fs::create_dir(media.join("foreign"))?;
        std::fs::write(media.join("foreign").join("keep"), b"keep")?;
        let adapter = NativeAdapter::new(fixture.selection.clone(), Vec::new())?;
        let mut session = Session::new()?;
        let root = lease(session.dispatch(&adapter, fixture.request()?)?)?;
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::RemoveDirectory {
                        directory: root.clone()
                    }
                )
                .is_err()
        );
        let parent = adapter
            .roots
            .get("library")
            .ok_or(Error::Internal("generated root missing"))?;
        let observed = adapter.observe(
            &parent
                .open_directory(&LibraryPath::parse("album")?)?
                .directory_file()?,
            Kind::Directory,
        )?;
        let album = lease(session.dispatch(
            &adapter,
            Request::OpenChild {
                parent: root.clone(),
                name: "album".into(),
                expected: observed,
            },
        )?)?;
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::RemoveDirectory {
                        directory: album.clone()
                    }
                )
                .is_err()
        );
        let Response::Page { entries, .. } = session.dispatch(
            &adapter,
            Request::Page {
                directory: album.clone(),
                limit: MAX_PAGE,
            },
        )?
        else {
            return Err(Error::Internal("generated removal listing missing"));
        };
        let listed = entries
            .first()
            .ok_or(Error::Internal("generated file missing"))?;
        let ordinary = lease(session.dispatch(
            &adapter,
            Request::OpenChild {
                parent: album.clone(),
                name: listed.name.clone(),
                expected: listed.observed.clone(),
            },
        )?)?;
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::RemoveFile {
                        file: ordinary.clone()
                    }
                )
                .is_err()
        );
        session.dispatch(
            &adapter,
            Request::Close {
                reference: ordinary,
            },
        )?;
        let file = lease(session.dispatch(
            &adapter,
            Request::PrepareRemoval {
                parent: album.clone(),
                name: listed.name.clone(),
                expected: listed.observed.clone(),
                length: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            },
        )?)?;
        let remove = Request::RemoveFile { file: file.clone() };
        assert!(session.dispatch(&adapter, remove.clone()).is_err());
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::Write {
                        file: file.clone(),
                        offset: 0,
                        bytes: vec![99]
                    }
                )
                .is_err()
        );
        assert!(matches!(
            session.dispatch(&adapter, Request::Seal { file: file.clone() })?,
            Response::Sealing { .. }
        ));
        assert!(session.dispatch(&adapter, remove.clone()).is_err());
        assert!(matches!(
            session.dispatch(&adapter, Request::Seal { file })?,
            Response::Sealed { .. }
        ));
        assert!(matches!(
            session.dispatch(&adapter, remove.clone())?,
            Response::Removed { .. }
        ));
        assert!(session.dispatch(&adapter, remove).is_err());
        assert!(!media.join("album").join("track.flac").exists());
        session.dispatch(&adapter, Request::Close { reference: album })?;
        for (name, empty) in [("foreign", false), ("album", true)] {
            let expected = adapter.observe(
                &parent
                    .open_directory(&LibraryPath::parse(name)?)?
                    .directory_file()?,
                Kind::Directory,
            )?;
            let directory = lease(session.dispatch(
                &adapter,
                Request::PrepareMove {
                    parent: root.clone(),
                    name: name.into(),
                    expected,
                },
            )?)?;
            let remove = Request::RemoveDirectory { directory };
            if empty {
                assert!(matches!(
                    session.dispatch(&adapter, remove.clone())?,
                    Response::Removed { .. }
                ));
                assert!(!media.join(name).exists());
            } else {
                assert!(session.dispatch(&adapter, remove.clone()).is_err());
                assert_eq!(std::fs::read(media.join(name).join("keep"))?, b"keep");
            }
            assert!(session.dispatch(&adapter, remove).is_err());
        }
        Ok(())
    }
}
