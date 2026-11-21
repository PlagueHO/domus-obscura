//! A move lease pins its reviewed source through the actual no-replace effect.
//! Application approval and durable intent remain the caller's responsibility.
use super::mutation::component;
use super::*;

impl Session {
    pub(super) fn move_directory(
        &mut self,
        adapter: &NativeAdapter,
        request: Request,
    ) -> Result<Response> {
        match request {
            Request::PrepareMove {
                parent,
                name,
                expected,
            } => {
                component(&name)?;
                let id = self.require(&parent)?;
                if expected.kind != Kind::Directory || self.entries.len() >= MAX_LIVE {
                    return Err(Error::Conflict(
                        "move requires an available directory lease".into(),
                    ));
                }
                if self
                    .entries
                    .values()
                    .any(|entry| entry.observed.identity == expected.identity)
                {
                    return Err(Error::Conflict(
                        "close existing source leases before preparing a move".into(),
                    ));
                }
                let parent = self.mutation_parent(adapter, id)?;
                let source = parent.open_child_for_move(&name)?;
                let depth = self
                    .entries
                    .get(&id)
                    .ok_or(Error::Internal("move parent disappeared"))?
                    .depth
                    + 1;
                let mut entry = adapter.directory_entry(source, Some(id), depth)?;
                if entry.observed != expected {
                    return Err(Error::Conflict(
                        "move source differs from its reviewed observation".into(),
                    ));
                }
                let Object::Directory { movable, .. } = &mut entry.object else {
                    return Err(Error::Internal("move source is not a directory"));
                };
                *movable = true;
                entry.writable = true;
                self.insert(entry)
            }
            Request::Rename { directory, name } => {
                component(&name)?;
                let id = self.require(&directory)?;
                let entry = self
                    .entries
                    .get(&id)
                    .ok_or(Error::Internal("move source disappeared"))?;
                let Object::Directory {
                    directory: source,
                    movable: true,
                    ..
                } = &entry.object
                else {
                    return Err(Error::Conflict(
                        "rename requires the retained move source".into(),
                    ));
                };
                if !entry.writable
                    || self.entries.values().any(|entry| entry.parent == Some(id))
                    || adapter.observe(&source.directory_file()?, Kind::Directory)?
                        != entry.observed
                {
                    return Err(Error::Conflict(
                        "move source changed or still has child leases".into(),
                    ));
                }
                let source = source.try_clone()?;
                let parent_id = entry
                    .parent
                    .ok_or_else(|| Error::Conflict("selected roots cannot be moved".into()))?;
                let parent = self.mutation_parent(adapter, parent_id)?;
                if let Some(checked) = self.check_new_name(parent_id, &parent, &name)? {
                    return Ok(Response::CheckingName { checked });
                }
                parent.rename_retained_child(&source, &name)?;
                super::super::sync_directory(&parent)?;
                let observed = adapter.observe(&source.directory_file()?, Kind::Directory)?;
                let parent = self.changed_parent(adapter, parent_id)?;
                self.entries.remove(&id);
                Ok(Response::Renamed { observed, parent })
            }
            _ => Err(Error::Internal("unexpected directory move request")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Fixture;
    use super::*;

    fn lease(response: Response) -> Result<Reference> {
        match response {
            Response::Opened { reference, .. } => Ok(reference),
            _ => Err(Error::Internal("expected generated move lease")),
        }
    }

    #[test]
    fn media_adapter_move_rejects_readonly_roots_and_unverified_ordinary_sources() -> Result<()> {
        for writable in [false, true] {
            let mut fixture = Fixture::new()?;
            let media = fixture
                .selection
                .provider
                .boundary
                .join("parent")
                .join("media");
            std::fs::write(media.join("COLLISION"), b"existing")?;
            if writable {
                fixture.selection.writable_roots.push("library".into());
            }
            let adapter = NativeAdapter::new(fixture.selection.clone(), Vec::new())?;
            let mut session = Session::new()?;
            let root = lease(session.dispatch(&adapter, fixture.request()?)?)?;
            let Response::Page { entries, .. } = session.dispatch(
                &adapter,
                Request::Page {
                    directory: root.clone(),
                    limit: MAX_PAGE,
                },
            )?
            else {
                return Err(Error::Internal("expected generated move listing"));
            };
            let listed = entries
                .iter()
                .find(|entry| entry.name == "album")
                .ok_or(Error::Internal("generated source missing"))?;
            let prepare = Request::PrepareMove {
                parent: root.clone(),
                name: listed.name.clone(),
                expected: listed.observed.clone(),
            };
            if !writable {
                assert!(matches!(
                    session.dispatch(&adapter, prepare),
                    Err(Error::Conflict(_))
                ));
                continue;
            }
            let ordinary = lease(session.dispatch(
                &adapter,
                Request::OpenChild {
                    parent: root.clone(),
                    name: listed.name.clone(),
                    expected: listed.observed.clone(),
                },
            )?)?;
            assert!(matches!(
                session.dispatch(
                    &adapter,
                    Request::Rename {
                        directory: ordinary.clone(),
                        name: "unapproved".into()
                    }
                ),
                Err(Error::Conflict(_))
            ));
            assert!(matches!(
                session.dispatch(&adapter, prepare.clone()),
                Err(Error::Conflict(_))
            ));
            session.dispatch(
                &adapter,
                Request::Close {
                    reference: ordinary,
                },
            )?;
            let source = lease(session.dispatch(&adapter, prepare)?)?;
            assert!(matches!(
                session.dispatch(
                    &adapter,
                    Request::Rename {
                        directory: source.clone(),
                        name: "collision".into(),
                    }
                ),
                Err(Error::Conflict(_))
            ));
            assert_eq!(std::fs::read(media.join("COLLISION"))?, b"existing");
            assert!(matches!(
                session.dispatch(
                    &adapter,
                    Request::CreateDirectory {
                        parent: source.clone(),
                        name: "unapproved".into()
                    }
                ),
                Err(Error::Conflict(_))
            ));
            let Response::Page { entries, .. } = session.dispatch(
                &adapter,
                Request::Page {
                    directory: source.clone(),
                    limit: MAX_PAGE,
                },
            )?
            else {
                return Err(Error::Internal("expected generated source listing"));
            };
            let file = entries
                .first()
                .ok_or(Error::Internal("generated source file missing"))?;
            let file = lease(session.dispatch(
                &adapter,
                Request::OpenChild {
                    parent: source.clone(),
                    name: file.name.clone(),
                    expected: file.observed.clone(),
                },
            )?)?;
            let rename = Request::Rename {
                directory: source.clone(),
                name: "final".into(),
            };
            assert!(matches!(
                session.dispatch(&adapter, rename.clone()),
                Err(Error::Conflict(_))
            ));
            session.dispatch(&adapter, Request::Close { reference: file })?;
            let Response::Renamed { observed, .. } = session.dispatch(&adapter, rename)? else {
                return Err(Error::Internal("expected exact source move"));
            };
            assert_eq!(observed.identity, listed.observed.identity);
            assert!(
                session
                    .dispatch(
                        &adapter,
                        Request::Rename {
                            directory: source,
                            name: "replayed".into()
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
            assert!(!media.join("album").exists());
            assert!(media.join("final").join("track.flac").is_file());
            assert!(!media.join("replayed").exists());
        }
        Ok(())
    }

    #[test]
    fn media_adapter_move_name_checks_reject_a_changed_parent_without_effect() -> Result<()> {
        let mut fixture = Fixture::new()?;
        fixture.selection.writable_roots.push("library".into());
        let media = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media");
        for index in 0..MAX_PAGE + 3 {
            std::fs::write(media.join(format!("entry-{index}")), [3])?;
        }
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
        let source = lease(session.dispatch(
            &adapter,
            Request::PrepareMove {
                parent: root,
                name: "album".into(),
                expected,
            },
        )?)?;
        let rename = Request::Rename {
            directory: source,
            name: "final".into(),
        };
        assert!(
            matches!(session.dispatch(&adapter, rename.clone())?, Response::CheckingName { checked } if checked == MAX_PAGE as u64)
        );
        std::fs::write(media.join("concurrent"), [8])?;
        assert!(session.dispatch(&adapter, rename).is_err());
        assert!(media.join("album").join("track.flac").is_file());
        assert!(!media.join("final").exists());
        Ok(())
    }
}
