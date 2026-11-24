use super::*;
use std::fs;

pub(super) struct Fixture {
    pub(super) directory: tempfile::TempDir,
    pub(super) selection: Selection,
}

impl Fixture {
    pub(super) fn new() -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix(".media-adapter-")
            .tempdir_in(std::env::current_dir()?)?;
        crate::infrastructure::permissions::private_fixture::directory(directory.path())?;
        let boundary = directory.path().join("boundary");
        fs::create_dir_all(boundary.join("parent").join("media").join("album"))?;
        fs::create_dir(boundary.join("private"))?;
        crate::infrastructure::permissions::private_fixture::directory(&boundary.join("private"))?;
        fs::write(
            boundary
                .join("parent")
                .join("media")
                .join("album")
                .join("track.flac"),
            b"generated media bytes",
        )?;
        let inspect = |id, role, relative| {
            StorageRoot::inspect(
                id,
                1,
                role,
                "windows-media",
                &boundary,
                &LibraryPath::parse(relative)?,
            )
        };
        let selection = Selection {
            revision: 1,
            provider: ProviderMapping {
                scope: "windows-media".into(),
                boundary: boundary.clone(),
            },
            media: vec![inspect("library", StorageRole::Media, "parent/media")?],
            private: inspect("adapter-private", StorageRole::Application, "private")?,
            writable_roots: Vec::new(),
        };
        Ok(Self {
            directory,
            selection,
        })
    }

    fn adapter(&self) -> Result<NativeAdapter> {
        NativeAdapter::new(self.selection.clone(), Vec::new())
    }

    pub(super) fn request(&self) -> Result<Request> {
        Ok(Request::OpenRoot {
            selection: self.selection.digest()?,
            root: self.selection.media[0].clone(),
        })
    }
}

fn opened(response: Response) -> Result<(Reference, Observation)> {
    match response {
        Response::Opened {
            reference,
            observed,
        } => Ok((reference, observed)),
        _ => Err(Error::Internal("expected an opened native media reference")),
    }
}

fn entries(response: Response) -> Result<(Vec<ListedEntry>, bool)> {
    match response {
        Response::Page { entries, complete } => Ok((entries, complete)),
        _ => Err(Error::Internal("expected a native media page")),
    }
}

fn child(
    session: &mut Session,
    adapter: &NativeAdapter,
    parent: Reference,
) -> Result<(Reference, Observation)> {
    let (page, _) = entries(session.dispatch(
        adapter,
        Request::Page {
            directory: parent.clone(),
            limit: MAX_PAGE,
        },
    )?)?;
    assert_eq!(page.len(), 1);
    let entry = page
        .into_iter()
        .next()
        .ok_or(Error::Internal("fixture child is absent"))?;
    opened(session.dispatch(
        adapter,
        Request::OpenChild {
            parent,
            name: entry.name,
            expected: entry.observed,
        },
    )?)
}

#[test]
fn media_adapter_write_policy_is_explicit_media_only_and_bound_to_selection() -> Result<()> {
    let fixture = Fixture::new()?;
    let original = fixture.selection.logical();
    assert!(!original.permits_writes("library")?);
    assert!(original.permits_writes("adapter-private").is_err());
    assert!(original.permits_writes("unknown").is_err());
    let legacy = serde_json::to_value(&fixture.selection)?;
    assert!(legacy.get("writable_roots").is_none());
    let restored: Selection = serde_json::from_value(legacy)?;
    assert!(!restored.logical().permits_writes("library")?);
    assert_eq!(restored.digest()?, original.digest()?);
    for roots in [
        vec!["adapter-private"],
        vec!["unknown"],
        vec!["library", "library"],
        vec![" library"],
    ] {
        let mut selection = original.clone();
        selection.writable_roots = roots.into_iter().map(str::to_owned).collect();
        assert!(selection.validate().is_err());
        assert!(selection.permits_writes("library").is_err());
    }
    let mut writable = fixture.selection.clone();
    writable.writable_roots = vec!["library".into()];
    assert!(writable.logical().permits_writes("library")?);
    assert_ne!(writable.digest()?, original.digest()?);
    let adapter = NativeAdapter::new(writable.clone(), Vec::new())?;
    let mut session = Session::new()?;
    assert!(session.dispatch(&adapter, fixture.request()?).is_err());
    let root = writable.media[0].clone();
    let response = session.dispatch(
        &adapter,
        Request::OpenRoot {
            selection: writable.digest()?,
            root: root.clone(),
        },
    )?;
    let (_, observed) = opened(response)?;
    assert_eq!(
        observed.identity,
        *root
            .expected
            .components
            .last()
            .ok_or(Error::Internal("missing selected identity"))?
    );
    let read_only_adapter = fixture.adapter()?;
    assert!(
        session
            .dispatch(
                &read_only_adapter,
                Request::OpenRoot {
                    selection: writable.digest()?,
                    root,
                }
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn media_adapter_write_policy_order_does_not_change_logical_review() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let second = fixture.selection.provider.boundary.join("second");
    fs::create_dir(&second)?;
    fixture.selection.media.push(StorageRoot::inspect(
        "backup",
        1,
        StorageRole::Media,
        "windows-media",
        &fixture.selection.provider.boundary,
        &LibraryPath::parse("second")?,
    )?);
    fixture.selection.writable_roots = vec!["library".into(), "backup".into()];
    fixture.selection.validate()?;
    let before = fixture.selection.digest()?;
    fixture.selection.writable_roots.reverse();
    assert_eq!(before, fixture.selection.digest()?);
    fixture.selection.writable_roots.pop();
    assert_ne!(before, fixture.selection.digest()?);
    Ok(())
}

#[test]
fn media_adapter_retains_read_authority_and_invalidates_descendants() -> Result<()> {
    let fixture = Fixture::new()?;
    let adapter = fixture.adapter()?;
    let mut session = Session::new()?;
    let (root, _) = opened(session.dispatch(&adapter, fixture.request()?)?)?;
    let (album, _) = child(&mut session, &adapter, root.clone())?;
    let (file, observed) = child(&mut session, &adapter, album)?;
    assert_eq!(observed.kind, Kind::File);
    match session.dispatch(
        &adapter,
        Request::Read {
            file: file.clone(),
            offset: 10,
            count: MAX_READ,
        },
    )? {
        Response::Read { bytes, complete } => {
            assert_eq!(bytes, b"media bytes");
            assert!(complete);
        }
        _ => return Err(Error::Internal("expected retained media bytes")),
    }
    let mut other_connection = Session::new()?;
    assert!(
        other_connection
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
    session.dispatch(&adapter, Request::Close { reference: root })?;
    assert!(session.entries.is_empty());
    assert!(
        session
            .dispatch(
                &adapter,
                Request::Read {
                    file,
                    offset: 0,
                    count: 1
                }
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn media_adapter_rejects_stale_meaning_and_bounds_live_handles_and_pages() -> Result<()> {
    let fixture = Fixture::new()?;
    let adapter = fixture.adapter()?;
    let mut session = Session::new()?;
    let mut root = fixture.selection.media[0].clone();
    root.binding_revision += 1;
    assert!(
        session
            .dispatch(
                &adapter,
                Request::OpenRoot {
                    selection: fixture.selection.digest()?,
                    root,
                }
            )
            .is_err()
    );
    let mut root = fixture.selection.media[0].clone();
    root.expected.components.pop();
    assert!(
        session
            .dispatch(
                &adapter,
                Request::OpenRoot {
                    selection: fixture.selection.digest()?,
                    root,
                }
            )
            .is_err()
    );
    assert!(
        session
            .dispatch(
                &adapter,
                Request::OpenRoot {
                    selection: "stale".into(),
                    root: fixture.selection.media[0].clone(),
                }
            )
            .is_err()
    );
    let (root, _) = opened(session.dispatch(&adapter, fixture.request()?)?)?;
    for limit in [0, MAX_PAGE + 1, usize::MAX] {
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::Page {
                        directory: root.clone(),
                        limit
                    }
                )
                .is_err()
        );
    }
    for _ in 1..MAX_LIVE {
        session.dispatch(&adapter, fixture.request()?)?;
    }
    assert!(matches!(
        session.dispatch(&adapter, fixture.request()?),
        Err(Error::Busy(_))
    ));
    assert_eq!(session.entries.len(), MAX_LIVE);
    Ok(())
}

#[test]
fn media_adapter_checks_exact_listing_evidence_and_single_components() -> Result<()> {
    let fixture = Fixture::new()?;
    let adapter = fixture.adapter()?;
    let mut session = Session::new()?;
    let (root, _) = opened(session.dispatch(&adapter, fixture.request()?)?)?;
    let (album, _) = child(&mut session, &adapter, root)?;
    let (page, complete) = entries(session.dispatch(
        &adapter,
        Request::Page {
            directory: album.clone(),
            limit: 1,
        },
    )?)?;
    assert!(!complete);
    let listed = page
        .first()
        .ok_or(Error::Internal("fixture file is absent"))?;
    for name in [
        "",
        "../private",
        "C:/private",
        "track.flac:secret",
        "album/track.flac",
        "a\\b",
    ] {
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::OpenChild {
                        parent: album.clone(),
                        name: name.into(),
                        expected: listed.observed.clone(),
                    }
                )
                .is_err()
        );
    }
    let path = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media")
        .join("album")
        .join("track.flac");
    fs::rename(&path, path.with_extension("old"))?;
    fs::write(&path, b"generated media bytes")?;
    assert!(
        session
            .dispatch(
                &adapter,
                Request::OpenChild {
                    parent: album.clone(),
                    name: listed.name.clone(),
                    expected: listed.observed.clone(),
                }
            )
            .is_err()
    );
    // A changed directory cannot turn its interrupted old cursor into absence.
    assert!(
        session
            .dispatch(
                &adapter,
                Request::Page {
                    directory: album.clone(),
                    limit: 1
                }
            )
            .is_err()
    );
    assert!(
        session
            .dispatch(
                &adapter,
                Request::Page {
                    directory: album,
                    limit: 1
                }
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn media_adapter_rejects_changed_boundaries_parents_and_leaves_after_restart() -> Result<()> {
    for component in ["boundary", "parent", "media"] {
        let fixture = Fixture::new()?;
        drop(fixture.adapter()?);
        let boundary = &fixture.selection.provider.boundary;
        match component {
            "boundary" => {
                let saved = fixture.directory.path().join("old-boundary");
                fs::rename(boundary, &saved)?;
                fs::create_dir(boundary)?;
                for child in ["parent", "private"] {
                    fs::rename(saved.join(child), boundary.join(child))?;
                }
            }
            "parent" => {
                let saved = boundary.join("old-parent");
                fs::rename(boundary.join("parent"), &saved)?;
                fs::create_dir(boundary.join("parent"))?;
                fs::rename(saved.join("media"), boundary.join("parent").join("media"))?;
            }
            _ => {
                fs::rename(
                    boundary.join("parent").join("media"),
                    boundary.join("old-media"),
                )?;
                fs::create_dir(boundary.join("parent").join("media"))?;
            }
        }
        assert!(fixture.adapter().is_err(), "{component}");
    }
    Ok(())
}

#[test]
fn media_adapter_rejects_private_nested_and_duplicate_native_roots() -> Result<()> {
    let fixture = Fixture::new()?;
    for relative in ["", "private", "parent/media", "parent/media/album"] {
        let mut selection = fixture.selection.clone();
        selection.media.push(StorageRoot::inspect(
            "other-library",
            1,
            StorageRole::Media,
            "windows-media",
            &selection.provider.boundary,
            &LibraryPath::parse(relative)?,
        )?);
        assert!(
            NativeAdapter::new(selection, Vec::new()).is_err(),
            "{relative}"
        );
    }
    let mut selection = fixture.selection.clone();
    selection.media[0].role = StorageRole::State;
    assert!(NativeAdapter::new(selection, Vec::new()).is_err());
    Ok(())
}

#[test]
fn media_adapter_rejects_hard_linked_private_inputs() -> Result<()> {
    let fixture = Fixture::new()?;
    let private = fixture
        .selection
        .provider
        .boundary
        .join("private")
        .join("pairing-key");
    fs::write(&private, b"generated private fixture")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600))?;
    }
    fs::hard_link(
        &private,
        fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media")
            .join("alias"),
    )?;
    let adapter = NativeAdapter::new(fixture.selection.clone(), vec![File::open(private)?])?;
    let mut session = Session::new()?;
    let (root, _) = opened(session.dispatch(&adapter, fixture.request()?)?)?;
    assert!(
        session
            .dispatch(
                &adapter,
                Request::Page {
                    directory: root.clone(),
                    limit: MAX_PAGE
                }
            )
            .is_err()
    );
    assert!(
        session
            .dispatch(
                &adapter,
                Request::Page {
                    directory: root,
                    limit: MAX_PAGE
                }
            )
            .is_err()
    );
    Ok(())
}

pub(super) fn directory_alias(alias: &std::path::Path, target: &std::path::Path) -> Result<()> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(alias)
            .arg(target)
            .output()?;
        assert!(
            output.status.success(),
            "generated junction creation failed"
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, alias)?;
    Ok(())
}

#[test]
fn media_adapter_rejects_reparse_or_symlink_entries_without_confirming_absence() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.selection.writable_roots.push("library".into());
    let alias = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media")
        .join("alias");
    let target = fixture.selection.provider.boundary.join("private");
    directory_alias(&alias, &target)?;
    let adapter = fixture.adapter()?;
    let mut session = Session::new()?;
    let (root, _) = opened(session.dispatch(&adapter, fixture.request()?)?)?;
    assert!(
        session
            .dispatch(
                &adapter,
                Request::Page {
                    directory: root.clone(),
                    limit: MAX_PAGE
                }
            )
            .is_err()
    );
    assert!(
        session
            .dispatch(
                &adapter,
                Request::SyncDirectory {
                    directory: root.clone(),
                },
            )
            .is_err()
    );
    assert!(
        session
            .dispatch(
                &adapter,
                Request::Page {
                    directory: root,
                    limit: MAX_PAGE
                }
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn media_adapter_rechecks_file_observations_and_read_limits() -> Result<()> {
    let fixture = Fixture::new()?;
    let adapter = fixture.adapter()?;
    let mut session = Session::new()?;
    let (root, _) = opened(session.dispatch(&adapter, fixture.request()?)?)?;
    let (album, _) = child(&mut session, &adapter, root)?;
    let (file, observed) = child(&mut session, &adapter, album)?;
    for (offset, count) in [(0, 0), (0, MAX_READ + 1), (observed.length + 1, 1)] {
        assert!(
            session
                .dispatch(
                    &adapter,
                    Request::Read {
                        file: file.clone(),
                        offset,
                        count
                    }
                )
                .is_err()
        );
    }
    let path = fixture
        .selection
        .provider
        .boundary
        .join("parent")
        .join("media")
        .join("album")
        .join("track.flac");
    fs::write(path, b"changed")?;
    assert!(matches!(
        session.dispatch(
            &adapter,
            Request::Read {
                file,
                offset: 0,
                count: 1,
            }
        ),
        Err(Error::Conflict(_))
    ));
    Ok(())
}

#[test]
fn media_adapter_reads_exact_bounded_chunks_until_observed_eof() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut bytes = vec![b'a'; MAX_READ * 2 + 7];
    bytes[MAX_READ..MAX_READ * 2].fill(b'b');
    bytes[MAX_READ * 2..].fill(b'c');
    fs::write(
        fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media")
            .join("album")
            .join("track.flac"),
        &bytes,
    )?;
    let adapter = fixture.adapter()?;
    let mut session = Session::new()?;
    let (root, _) = opened(session.dispatch(&adapter, fixture.request()?)?)?;
    let (album, _) = child(&mut session, &adapter, root)?;
    let (file, _) = child(&mut session, &adapter, album)?;
    for (offset, length, eof) in [
        (0, MAX_READ, false),
        (MAX_READ, MAX_READ, false),
        (MAX_READ * 2, 7, true),
    ] {
        let Response::Read {
            bytes: actual,
            complete,
        } = session.dispatch(
            &adapter,
            Request::Read {
                file: file.clone(),
                offset: offset as u64,
                count: MAX_READ,
            },
        )?
        else {
            return Err(Error::Internal("expected a bounded file chunk"));
        };
        assert_eq!(actual, bytes[offset..offset + length]);
        assert_eq!(complete, eof);
    }
    Ok(())
}
