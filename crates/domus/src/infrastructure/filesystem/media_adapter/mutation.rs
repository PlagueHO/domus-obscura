//! New-only native output leases. A failed or disconnected stream is retained,
//! never reopened for writing. The application journal remains authoritative.
use super::*;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use std::io::Write;

pub(super) struct NameCheck {
    name: String,
    cursor: Box<cap_std::fs::ReadDir>,
    checked: u64,
    failed: bool,
}

fn conflict() -> Error {
    Error::Conflict("native media output is changed, incomplete or no longer writable".into())
}

pub(super) fn component(name: &str) -> Result<()> {
    let path = LibraryPath::parse(name)?;
    if path.as_str().is_empty() || path.as_str().contains('/') || name.len() > 1024 {
        return Err(Error::InvalidInput(
            "media mutation requires one bounded component".into(),
        ));
    }
    Ok(())
}

impl Session {
    pub(super) fn mutation_parent(
        &self,
        adapter: &NativeAdapter,
        id: u64,
    ) -> Result<LibraryDirectory> {
        let entry = self.entries.get(&id).ok_or_else(conflict)?;
        let Object::Directory {
            directory,
            movable: false,
            ..
        } = &entry.object
        else {
            return Err(conflict());
        };
        if !entry.writable || entry.depth >= MAX_DEPTH {
            return Err(conflict());
        }
        if adapter.observe(&directory.directory_file()?, Kind::Directory)? != entry.observed {
            return Err(conflict());
        }
        Ok(directory.try_clone()?)
    }

    pub(super) fn changed_parent(
        &mut self,
        adapter: &NativeAdapter,
        parent_id: u64,
    ) -> Result<Observation> {
        let parent = self.entries.get_mut(&parent_id).ok_or_else(conflict)?;
        let Object::Directory {
            directory,
            cursor,
            failed,
            name_check,
            ..
        } = &mut parent.object
        else {
            return Err(conflict());
        };
        let current = adapter.observe(&directory.directory_file()?, Kind::Directory)?;
        if current.identity != parent.observed.identity {
            return Err(conflict());
        }
        parent.observed = current.clone();
        *cursor = None;
        *failed = false;
        *name_check = None;
        Ok(current)
    }

    fn created(
        &mut self,
        adapter: &NativeAdapter,
        parent_id: u64,
        mut child: Entry,
    ) -> Result<Response> {
        let current = self.changed_parent(adapter, parent_id)?;
        child.writable = self.entries.get(&parent_id).ok_or_else(conflict)?.writable;
        let Response::Opened {
            reference,
            observed,
        } = self.insert(child)?
        else {
            return Err(Error::Internal("native output lease is missing"));
        };
        Ok(Response::Created {
            reference,
            observed,
            parent: current,
        })
    }

    pub(super) fn mutate(&mut self, adapter: &NativeAdapter, request: Request) -> Result<Response> {
        match request {
            Request::CreateDirectory { parent, name } => {
                component(&name)?;
                let id = self.require(&parent)?;
                self.mutation_capacity()?;
                let directory = self.mutation_parent(adapter, id)?;
                if let Some(checked) = self.check_new_name(id, &directory, &name)? {
                    return Ok(Response::CheckingName { checked });
                }
                let child = directory.create_child_directory(&name)?;
                super::super::sync_directory(&directory)?;
                let depth = self.entries.get(&id).ok_or_else(conflict)?.depth + 1;
                let entry = adapter.directory_entry(child, Some(id), depth)?;
                self.created(adapter, id, entry)
            }
            Request::CreateFile {
                parent,
                name,
                length,
                sha256,
            } => {
                component(&name)?;
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
                let id = self.require(&parent)?;
                self.mutation_capacity()?;
                let directory = self.mutation_parent(adapter, id)?;
                if let Some(checked) = self.check_new_name(id, &directory, &name)? {
                    return Ok(Response::CheckingName { checked });
                }
                let mut options = cap_std::fs::OpenOptions::new();
                options
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .follow(FollowSymlinks::No);
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.mode(0o644);
                }
                #[cfg(windows)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.share_mode(0);
                }
                let file = directory.open_with(&name, &options)?.into_std();
                let observed = adapter.observe(&file, Kind::File)?;
                if observed.links != 1 || observed.length != 0 {
                    return Err(conflict());
                }
                let entry = Entry {
                    writable: true,
                    parent: Some(id),
                    depth: self.entries.get(&id).ok_or_else(conflict)?.depth + 1,
                    observed,
                    object: Object::Writing {
                        file,
                        parent: directory,
                        length,
                        sha256,
                        written: 0,
                        verified: 0,
                        hash: Sha256::new(),
                        failed: false,
                    },
                };
                self.created(adapter, id, entry)
            }
            Request::Write {
                file,
                offset,
                bytes,
            } => {
                if bytes.is_empty() || bytes.len() > MAX_READ {
                    return Err(Error::InvalidInput(
                        "invalid bounded native output chunk".into(),
                    ));
                }
                let id = self.require(&file)?;
                let entry = self.entries.get_mut(&id).ok_or_else(conflict)?;
                let Object::Writing {
                    file,
                    length,
                    written,
                    failed,
                    ..
                } = &mut entry.object
                else {
                    return Err(conflict());
                };
                let end = offset
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(conflict)?;
                if !entry.writable || *failed || offset != *written || end > *length {
                    return Err(conflict());
                }
                if adapter.observe(file, Kind::File)? != entry.observed || entry.observed.links != 1
                {
                    return Err(conflict());
                }
                // A short native write has an uncertain outcome. Retire this output
                // before I/O, and never permit a later request to rewrite it.
                *failed = true;
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(&bytes)?;
                let observed = adapter.observe(file, Kind::File)?;
                if observed.identity != entry.observed.identity
                    || observed.links != 1
                    || observed.length != end
                {
                    return Err(conflict());
                }
                entry.observed = observed;
                *written = end;
                *failed = false;
                Ok(Response::Written { offset: end })
            }
            Request::Seal { file } => {
                let id = self.require(&file)?;
                let entry = self.entries.get_mut(&id).ok_or_else(conflict)?;
                let Object::Writing {
                    file,
                    parent,
                    length,
                    sha256,
                    written,
                    verified,
                    hash,
                    failed,
                } = &mut entry.object
                else {
                    return Err(conflict());
                };
                if !entry.writable || *failed || written != length {
                    return Err(conflict());
                }
                *failed = true;
                let before = adapter.observe(file, Kind::File)?;
                if before != entry.observed || before.links != 1 || before.length != *length {
                    return Err(conflict());
                }
                if *verified == 0 {
                    file.sync_all()?;
                }
                file.seek(SeekFrom::Start(*verified))?;
                let count = (*length - *verified).min(MAX_READ as u64) as usize;
                let mut buffer = [0; MAX_READ];
                file.read_exact(&mut buffer[..count])?;
                hash.update(&buffer[..count]);
                *verified += count as u64;
                if adapter.observe(file, Kind::File)? != before {
                    return Err(conflict());
                }
                if *verified < *length {
                    *failed = false;
                    return Ok(Response::Sealing {
                        verified: *verified,
                    });
                }
                if file.read(&mut [0; 1])? != 0
                    || format!("{:x}", hash.clone().finalize()) != *sha256
                    || adapter.observe(file, Kind::File)? != before
                {
                    return Err(conflict());
                }
                super::super::sync_directory(parent)?;
                let retained = file.try_clone()?;
                let parent = parent.try_clone()?;
                entry.object = Object::File {
                    file: retained,
                    _parent: parent,
                };
                entry.observed = before.clone();
                Ok(Response::Sealed { observed: before })
            }
            _ => Err(Error::Internal("not a native media mutation")),
        }
    }

    fn mutation_capacity(&self) -> Result<()> {
        if self.entries.len() >= MAX_LIVE || self.next.checked_add(1).is_none() {
            return Err(Error::Busy("native media capability limit reached"));
        }
        Ok(())
    }

    pub(super) fn check_new_name(
        &mut self,
        id: u64,
        directory: &LibraryDirectory,
        name: &str,
    ) -> Result<Option<u64>> {
        let entry = self.entries.get_mut(&id).ok_or_else(conflict)?;
        let Object::Directory { name_check, .. } = &mut entry.object else {
            return Err(conflict());
        };
        if name_check.as_ref().is_none_or(|check| check.name != name) {
            *name_check = Some(NameCheck {
                name: name.into(),
                cursor: Box::new(directory.entries()?),
                checked: 0,
                failed: false,
            });
        }
        let check = name_check.as_mut().ok_or_else(conflict)?;
        if check.failed {
            return Err(conflict());
        }
        check.failed = true;
        let key = name.to_uppercase();
        for _ in 0..MAX_PAGE {
            let Some(entry) = check.cursor.next() else {
                *name_check = None;
                return Ok(None);
            };
            let candidate = entry?.file_name().into_string().map_err(|_| conflict())?;
            if candidate.to_uppercase() == key {
                return Err(conflict());
            }
            check.checked = check.checked.checked_add(1).ok_or_else(conflict)?;
        }
        check.failed = false;
        Ok(Some(check.checked))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Fixture;
    use super::*;

    fn lease(response: Response) -> Result<Reference> {
        match response {
            Response::Opened { reference, .. } | Response::Created { reference, .. } => {
                Ok(reference)
            }
            _ => Err(Error::Internal("expected native mutation lease")),
        }
    }

    #[test]
    fn media_adapter_write_collision_checks_are_bounded_and_reject_changed_parents() -> Result<()> {
        let mut fixture = Fixture::new()?;
        fixture.selection.writable_roots.push("library".into());
        let media = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media");
        for index in 0..MAX_PAGE + 3 {
            std::fs::write(media.join(format!("entry-{index}.flac")), [5])?;
        }
        let adapter = NativeAdapter::new(fixture.selection.clone(), Vec::new())?;
        let mut session = Session::new()?;
        let root = lease(session.dispatch(&adapter, fixture.request()?)?)?;
        let request = Request::CreateDirectory {
            parent: root,
            name: "new-album".into(),
        };
        assert!(matches!(session.dispatch(&adapter, request.clone())?,
            Response::CheckingName { checked } if checked == MAX_PAGE as u64));
        std::fs::write(media.join("concurrent.flac"), [7])?;
        assert!(session.dispatch(&adapter, request).is_err());
        assert!(!media.join("new-album").exists());
        let root = lease(session.dispatch(&adapter, fixture.request()?)?)?;
        let request = Request::CreateDirectory {
            parent: root.clone(),
            name: "new-album".into(),
        };
        assert!(matches!(session.dispatch(&adapter, request.clone())?,
            Response::CheckingName { checked } if checked == MAX_PAGE as u64));
        assert!(matches!(
            session.dispatch(&adapter, request)?,
            Response::Created { .. }
        ));
        let collision = Request::CreateFile {
            parent: root,
            name: "ENTRY-66.FLAC".into(),
            length: 1,
            sha256: format!("{:x}", Sha256::digest([9])),
        };
        loop {
            match session.dispatch(&adapter, collision.clone()) {
                Ok(Response::CheckingName { .. }) => {}
                Err(Error::Conflict(_)) => break,
                _ => return Err(Error::Internal("native write adopted a case collision")),
            }
        }
        assert_eq!(std::fs::read(media.join("entry-66.flac"))?, [5]);
        Ok(())
    }

    #[test]
    fn media_adapter_write_defaults_deny_effects_and_never_adopt_existing_names() -> Result<()> {
        let mut fixture = Fixture::new()?;
        let adapter = NativeAdapter::new(fixture.selection.clone(), Vec::new())?;
        let mut session = Session::new()?;
        let root = lease(session.dispatch(&adapter, fixture.request()?)?)?;
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::CreateDirectory {
                        parent: root,
                        name: "unapproved".into(),
                    }
                )
                .is_err()
        );
        let media = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media");
        assert!(!media.join("unapproved").exists());
        drop(adapter);
        fixture.selection.writable_roots.push("library".into());
        let adapter = NativeAdapter::new(fixture.selection.clone(), Vec::new())?;
        let mut session = Session::new()?;
        let root = lease(session.dispatch(&adapter, fixture.request()?)?)?;
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::CreateDirectory {
                        parent: root.clone(),
                        name: "ALBUM".into(),
                    }
                )
                .is_err()
        );
        assert!(!media.join("ALBUM").exists() || cfg!(windows));
        for name in ["../escape", "file:stream", "/absolute"] {
            assert!(
                session
                    .dispatch(
                        &adapter,
                        Request::CreateDirectory {
                            parent: root.clone(),
                            name: name.into(),
                        }
                    )
                    .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn media_adapter_write_seals_exact_streams_and_never_rewrites_partial_outputs() -> Result<()> {
        let mut fixture = Fixture::new()?;
        fixture.selection.writable_roots.push("library".into());
        let adapter = NativeAdapter::new(fixture.selection.clone(), Vec::new())?;
        let mut session = Session::new()?;
        let root = lease(session.dispatch(&adapter, fixture.request()?)?)?;
        let directory = lease(session.dispatch(
            &adapter,
            Request::CreateDirectory {
                parent: root.clone(),
                name: ".domus-pending-fixture".into(),
            },
        )?)?;
        let bytes = vec![73; MAX_READ + 7];
        let file = lease(session.dispatch(
            &adapter,
            Request::CreateFile {
                parent: directory.clone(),
                name: "track.flac".into(),
                length: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            },
        )?)?;
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::Read {
                        file: file.clone(),
                        offset: 0,
                        count: 1,
                    }
                )
                .is_err()
        );
        assert!(
            session
                .dispatch(&adapter, Request::Seal { file: file.clone() })
                .is_err()
        );
        for (index, chunk) in bytes.chunks(MAX_READ).enumerate() {
            assert!(matches!(
                session.dispatch(
                    &adapter,
                    Request::Write {
                        file: file.clone(),
                        offset: (index * MAX_READ) as u64,
                        bytes: chunk.to_vec(),
                    }
                )?,
                Response::Written { .. }
            ));
        }
        assert!(matches!(
            session.dispatch(&adapter, Request::Seal { file: file.clone() })?,
            Response::Sealing { verified } if verified == MAX_READ as u64
        ));
        assert!(
            matches!(session.dispatch(&adapter, Request::Seal { file: file.clone() })?,
                    Response::Sealed { observed } if observed.length == bytes.len() as u64)
        );
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::Write {
                        file: file.clone(),
                        offset: 0,
                        bytes: vec![0],
                    }
                )
                .is_err()
        );
        assert!(matches!(session.dispatch(&adapter, Request::Read {
                    file, offset: MAX_READ as u64, count: MAX_READ,
                })?, Response::Read { bytes, complete: true } if bytes == vec![73; 7]));
        let partial = lease(session.dispatch(
            &adapter,
            Request::CreateFile {
                parent: directory.clone(),
                name: "partial.flac".into(),
                length: 10,
                sha256: format!("{:x}", Sha256::digest([73; 10])),
            },
        )?)?;
        session.dispatch(
            &adapter,
            Request::Write {
                file: partial,
                offset: 0,
                bytes: vec![73; 3],
            },
        )?;
        session.dispatch(
            &adapter,
            Request::Close {
                reference: directory,
            },
        )?;
        let Response::Page { entries, .. } = session.dispatch(
            &adapter,
            Request::Page {
                directory: root.clone(),
                limit: MAX_PAGE,
            },
        )?
        else {
            return Err(Error::Internal("expected output parent page"));
        };
        let expected = entries
            .into_iter()
            .find(|entry| entry.name == ".domus-pending-fixture")
            .ok_or(Error::Internal("missing output folder"))?
            .observed;
        let directory = lease(session.dispatch(
            &adapter,
            Request::OpenChild {
                parent: root,
                name: ".domus-pending-fixture".into(),
                expected,
            },
        )?)?;
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::CreateFile {
                        parent: directory,
                        name: "partial.flac".into(),
                        length: 10,
                        sha256: format!("{:x}", Sha256::digest([73; 10])),
                    }
                )
                .is_err()
        );
        let partial_path = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media")
            .join(".domus-pending-fixture")
            .join("partial.flac");
        assert_eq!(std::fs::read(partial_path)?, [73; 3]);
        Ok(())
    }
}
