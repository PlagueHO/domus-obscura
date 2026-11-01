use super::filesystem::{
    LibraryDirectory, is_unsafe_cap_link, is_unsafe_link, sync_directory as sync,
};
use super::permissions::FilePermissions;
use super::store::{
    AlbumReplacementPreview, AlbumReplacementRestore, AlbumReplacementRestoreDecision,
    AlbumRestorePreview, PublicationDestination, PublicationFile, PublicationPreparation,
    ReplacementFile, Store, replacement_hash,
};
use crate::domain::LibraryPath;
use crate::{Error, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

fn conflict() -> Error {
    Error::Conflict("publication destination changed, is incomplete or contains existing content; nothing was overwritten".into())
}

fn options(create: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(create)
        .follow(FollowSymlinks::No);
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    options
}

fn read_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options
}

fn checked(directory: &Dir, permissions: &FilePermissions) -> Result<()> {
    if is_unsafe_cap_link(&directory.dir_metadata()?) {
        return Err(conflict());
    }
    permissions.check(&directory.try_clone()?.into_std_file(), false)?;
    Ok(())
}

fn check_case(directory: &Dir, name: &str) -> Result<()> {
    let key = name.to_uppercase();
    for entry in directory.entries()? {
        let candidate = entry?.file_name().into_string().map_err(|_| conflict())?;
        if candidate != name && candidate.to_uppercase() == key {
            return Err(conflict());
        }
    }
    Ok(())
}

pub(crate) fn album_replacement_previews(
    store: &Store,
    job: i64,
    version: i64,
) -> Result<Vec<AlbumReplacementPreview>> {
    album_replacement_previews_for_location(store, job, version, None)
}

pub(crate) fn album_replacement_preview_for_location(
    store: &Store,
    job: i64,
    version: i64,
    location_id: i64,
) -> Result<AlbumReplacementPreview> {
    album_replacement_previews_for_location(store, job, version, Some(location_id))?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Conflict("replacement destination is no longer planned".into()))
}

pub(crate) fn restore_album_replacement(
    store: &mut Store,
    job: i64,
    version: i64,
    location_id: i64,
    decision: AlbumReplacementRestoreDecision,
) -> Result<()> {
    let mut restore =
        store.prepare_album_replacement_restore(job, version, location_id, Some(&decision))?;
    let result = (|| {
        let preview = inspect_album_restore(store, job, version, location_id, &restore)?;
        if !preview.verified {
            return Err(Error::Conflict(format!(
                "restore cannot verify this destination: {}",
                preview
                    .unavailable_reason
                    .as_deref()
                    .unwrap_or("file state is unavailable"),
            )));
        }
        if decision
            .expected_preview_sha256
            .as_ref()
            .is_some_and(|expected| preview.preview_sha256.as_ref() != Some(expected))
            || ((restore.state == "published" || restore.approved_preview.is_some())
                && decision.expected_preview_sha256.is_none())
        {
            return Err(Error::Conflict(
                "restore preview changed or is unavailable; review this destination again".into(),
            ));
        }
        if restore.state == "published" {
            store.approve_album_restore(&restore, &preview)?;
            restore.replacement_album_identity = preview.current_album_identity;
            restore.replacement_files = preview.current_files;
            restore.state = "restore_prepared".into();
        }
        run_album_restore(store, &restore)
    })();
    match result {
        Ok(()) => store.album_restore_result(restore.destination_id, None).map_err(|error| {
            Error::Conflict(format!(
                "restore files were verified but final recovery status could not be saved: {error}; inspect the journal before retry"
            ))
        }),
        Err(error) => {
            store
                .album_restore_result(restore.destination_id, Some(&error.to_string()))
                .map_err(|record| {
                    Error::Conflict(format!(
                        "{error}; restore status could not be saved: {record}; inspect saved folders before retry"
                    ))
                })?;
            Err(error)
        }
    }
}

pub(crate) fn preview_album_restore(
    store: &mut Store,
    job: i64,
    version: i64,
    location_id: i64,
) -> Result<AlbumRestorePreview> {
    let restore = store.prepare_album_replacement_restore(job, version, location_id, None)?;
    inspect_album_restore(store, job, version, location_id, &restore)
}

fn inspect_album_restore(
    store: &Store,
    job: i64,
    version: i64,
    location_id: i64,
    restore: &AlbumReplacementRestore,
) -> Result<AlbumRestorePreview> {
    let destination = store
        .publication_destinations(job, version)?
        .into_iter()
        .find(|destination| destination.id == restore.destination_id)
        .ok_or_else(conflict)?;
    let mut preview = AlbumRestorePreview {
        job_id: job,
        plan_version: version,
        location_id,
        destination_id: restore.destination_id,
        review_id: restore.review_id,
        root: restore.root.clone(),
        root_identity: restore.root_identity.clone(),
        original_folder: restore.original_folder.clone(),
        retained_folder: restore.retained_folder.clone(),
        restore_folder: restore.restore_folder.clone(),
        state: restore.state.clone(),
        retained_album_identity: restore.original_album_identity.clone(),
        current_album_identity: restore.replacement_album_identity.clone(),
        retained_files: restore.original_files.clone(),
        current_files: restore.replacement_files.clone(),
        verified: false,
        unavailable_reason: None,
        preview_sha256: None,
        status: destination
            .replacement
            .and_then(|replacement| replacement.restore_status),
    };
    let inspected: Result<()> = (|| {
        let folders = RestoreFolders::open(store, restore)?;
        let old = (
            restore.original_album_identity.as_str(),
            restore.original_files.as_slice(),
        );
        if restore.state == "published" && restore.approved_preview.is_none() {
            let current =
                open_child_directory(&folders.parent, &folders.final_name)?.ok_or_else(conflict)?;
            let (identity, files) = replacement_snapshot(&current, &folders.permissions)?;
            if identity == restore.original_album_identity
                || folder_version(
                    &folders.parent,
                    &folders.retained_name,
                    old,
                    (&identity, &files),
                    &folders.permissions,
                )? != Some(FolderVersion::Old)
                || open_child_directory(&folders.parent, &folders.temporary_name)?.is_some()
            {
                return Err(conflict());
            }
            verify_replacement(&current, &files)?;
            preview.current_album_identity = identity;
            preview.current_files = files;
        } else {
            let new = (
                restore.replacement_album_identity.as_str(),
                restore.replacement_files.as_slice(),
            );
            let layout = (
                folder_version(
                    &folders.parent,
                    &folders.final_name,
                    old,
                    new,
                    &folders.permissions,
                )?,
                folder_version(
                    &folders.parent,
                    &folders.retained_name,
                    old,
                    new,
                    &folders.permissions,
                )?,
                folder_version(
                    &folders.parent,
                    &folders.temporary_name,
                    old,
                    new,
                    &folders.permissions,
                )?,
            );
            restore_step(&restore.state, layout.0, layout.1, layout.2)?;
        }
        Ok(())
    })();
    match inspected {
        Ok(()) => {
            preview.verified = true;
            preview.preview_sha256 = Some(preview.digest()?);
        }
        Err(error) => preview.unavailable_reason = Some(error.to_string()),
    }
    Ok(preview)
}

struct RestoreFolders {
    parent: Dir,
    permissions: FilePermissions,
    final_name: String,
    retained_name: String,
    temporary_name: String,
}

impl RestoreFolders {
    fn open(store: &Store, restore: &AlbumReplacementRestore) -> Result<Self> {
        let root = LibraryDirectory::open(Path::new(&restore.root))?;
        store.check_native_root(&restore.root, "media", root.native_root())?;
        if root.native_root().identity != restore.root_identity {
            return Err(conflict());
        }

        let original = LibraryPath::parse(&restore.original_folder)?;
        let retained = LibraryPath::parse(&restore.retained_folder)?;
        let temporary = LibraryPath::parse(&restore.restore_folder)?;
        let parent_path = original.parent();
        let final_name = original.as_str().rsplit('/').next().ok_or_else(conflict)?;
        let retained_name = retained.as_str().rsplit('/').next().ok_or_else(conflict)?;
        let temporary_name = temporary.as_str().rsplit('/').next().ok_or_else(conflict)?;
        if original.as_str().is_empty()
            || retained.parent() != parent_path
            || temporary.parent() != parent_path
            || !retained_name.starts_with(".domus-retained-")
            || !temporary_name.starts_with(".domus-restore-")
            || restore.original_album_identity == restore.replacement_album_identity
        {
            return Err(conflict());
        }

        let permissions = FilePermissions::current()?;
        let mut parent = root.into_directory();
        checked(&parent, &permissions)?;
        for name in parent_path.split('/').filter(|part| !part.is_empty()) {
            check_case(&parent, name)?;
            parent = parent.open_dir_nofollow(name)?;
            checked(&parent, &permissions)?;
        }
        for name in [final_name, retained_name, temporary_name] {
            check_case(&parent, name)?;
        }
        Ok(Self {
            parent,
            permissions,
            final_name: final_name.into(),
            retained_name: retained_name.into(),
            temporary_name: temporary_name.into(),
        })
    }
}

fn run_album_restore(store: &mut Store, restore: &AlbumReplacementRestore) -> Result<()> {
    let RestoreFolders {
        parent,
        permissions,
        final_name,
        retained_name,
        temporary_name,
    } = RestoreFolders::open(store, restore)?;
    let final_name = final_name.as_str();
    let retained_name = retained_name.as_str();
    let temporary_name = temporary_name.as_str();
    let old = (
        restore.original_album_identity.as_str(),
        restore.original_files.as_slice(),
    );
    let new = (
        restore.replacement_album_identity.as_str(),
        restore.replacement_files.as_slice(),
    );

    let mut state = restore.state.as_str();
    loop {
        let final_version = folder_version(&parent, final_name, old, new, &permissions)?;
        let retained_version = folder_version(&parent, retained_name, old, new, &permissions)?;
        let temporary_version = folder_version(&parent, temporary_name, old, new, &permissions)?;
        let rename = match restore_step(state, final_version, retained_version, temporary_version)?
        {
            RestoreStep::MoveCurrent => (final_name, temporary_name),
            RestoreStep::MoveOriginal => (retained_name, final_name),
            RestoreStep::RetainCurrent => (temporary_name, retained_name),
            RestoreStep::Record(next) => {
                sync(&parent)?;
                store.record_album_replacement_restore_state(
                    restore.destination_id,
                    state,
                    next,
                )?;
                state = next;
                continue;
            }
            RestoreStep::Complete => return Ok(()),
        };
        rename_no_replace(&parent, rename.0, rename.1)?;
        sync(&parent)?;
    }
}

enum RestoreStep {
    MoveCurrent,
    MoveOriginal,
    RetainCurrent,
    Record(&'static str),
    Complete,
}

fn restore_step(
    state: &str,
    visible: Option<FolderVersion>,
    retained: Option<FolderVersion>,
    temporary: Option<FolderVersion>,
) -> Result<RestoreStep> {
    use FolderVersion::{New, Old};
    match (state, visible, retained, temporary) {
        ("restore_prepared", Some(New), Some(Old), None) => Ok(RestoreStep::MoveCurrent),
        ("restore_prepared", None, Some(Old), Some(New)) => {
            Ok(RestoreStep::Record("restore_current_moved"))
        }
        ("restore_current_moved", None, Some(Old), Some(New)) => Ok(RestoreStep::MoveOriginal),
        ("restore_current_moved", Some(Old), None, Some(New)) => {
            Ok(RestoreStep::Record("restore_original_visible"))
        }
        ("restore_original_visible", Some(Old), None, Some(New)) => Ok(RestoreStep::RetainCurrent),
        ("restore_original_visible", Some(Old), Some(New), None) => {
            Ok(RestoreStep::Record("restored"))
        }
        ("restored", Some(Old), Some(New), None) => Ok(RestoreStep::Complete),
        _ => Err(conflict()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderVersion {
    Old,
    New,
}

fn folder_version(
    parent: &Dir,
    name: &str,
    old: (&str, &[ReplacementFile]),
    new: (&str, &[ReplacementFile]),
    permissions: &FilePermissions,
) -> Result<Option<FolderVersion>> {
    let Some(directory) = open_child_directory(parent, name)? else {
        return Ok(None);
    };
    checked(&directory, permissions)?;
    let identity =
        super::filesystem::native_file_identity(&directory.try_clone()?.into_std_file())?;
    let (version, files) = if identity == old.0 {
        (FolderVersion::Old, old.1)
    } else if identity == new.0 {
        (FolderVersion::New, new.1)
    } else {
        return Err(conflict());
    };
    verify_replacement(&directory, files)?;
    Ok(Some(version))
}

fn replacement_snapshot(
    directory: &Dir,
    permissions: &FilePermissions,
) -> Result<(String, Vec<ReplacementFile>)> {
    checked(directory, permissions)?;
    let identity =
        super::filesystem::native_file_identity(&directory.try_clone()?.into_std_file())?;
    let mut files = Vec::new();
    let mut names = HashSet::new();
    for entry in directory.entries()? {
        if files.len() == 1001 {
            return Err(Error::Conflict(
                "existing album exceeds the replacement file limit".into(),
            ));
        }
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| conflict())?;
        LibraryPath::parse(&name)?;
        let metadata = entry.metadata()?;
        if name.is_empty()
            || name.contains('/')
            || !metadata.is_file()
            || is_unsafe_cap_link(&metadata)
            || !names.insert(name.to_uppercase())
        {
            return Err(conflict());
        }
        let mut file = directory.open_with(&name, &read_options())?.into_std();
        let metadata = file.metadata()?;
        if !metadata.is_file() || is_unsafe_link(&metadata) || metadata.len() > i64::MAX as u64 {
            return Err(conflict());
        }
        let bytes = metadata.len();
        let mut hash = Sha256::new();
        let mut buffer = [0; 65_536];
        let mut total = 0u64;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total.checked_add(count as u64).ok_or_else(conflict)?;
            if total > bytes {
                return Err(conflict());
            }
            hash.update(&buffer[..count]);
        }
        if total != bytes || file.metadata()?.len() != bytes {
            return Err(conflict());
        }
        files.push(ReplacementFile {
            name,
            bytes,
            sha256: format!("{:x}", hash.finalize()),
        });
    }
    files.sort_by(|left, right| {
        left.name
            .to_uppercase()
            .cmp(&right.name.to_uppercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok((identity, files))
}

fn album_replacement_previews_for_location(
    store: &Store,
    job: i64,
    version: i64,
    location_id: Option<i64>,
) -> Result<Vec<AlbumReplacementPreview>> {
    let plan = store.plan(job, version)?;
    let folder = plan
        .plan
        .outputs
        .first()
        .ok_or(Error::Internal("publication plan has no outputs"))?
        .relative_folder
        .clone();
    let relative = LibraryPath::parse(&folder)?;
    let mut output_files = vec!["folder.jpg".to_owned()];
    output_files.extend(
        plan.plan
            .outputs
            .iter()
            .map(|output| output.filename.clone()),
    );
    if output_files.len() > 1001 {
        return Err(Error::Conflict(
            "replacement preview exceeds its file limit".into(),
        ));
    }

    let mut previews = Vec::with_capacity(plan.plan.destinations.len());
    let saved_reviews = store.album_replacement_reviews(job, version)?;
    for target in plan
        .plan
        .destinations
        .into_iter()
        .filter(|target| location_id.is_none_or(|location| target.location_id == location))
    {
        let root = match super::filesystem::LibraryDirectory::open(Path::new(&target.root)) {
            Ok(root) => root,
            Err(Error::Io(error)) => {
                let mut preview = AlbumReplacementPreview {
                    job_id: job,
                    plan_version: version,
                    location_id: target.location_id,
                    root: target.root,
                    root_identity: String::new(),
                    kind: target.kind,
                    relative_folder: folder.clone(),
                    output_files: output_files.clone(),
                    destination_available: false,
                    unavailable_reason: Some(error.to_string()),
                    album_identity: None,
                    manifest_sha256: String::new(),
                    existing_files: Vec::new(),
                    decision_saved: false,
                    saved_action: None,
                };
                preview.manifest_sha256 = replacement_hash(&preview)?;
                previews.push(preview);
                continue;
            }
            Err(error) => return Err(error),
        };
        store.check_native_root(&target.root, "media", root.native_root())?;
        let album = match root.open_directory(&relative) {
            Ok(album) => Some(album),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let (album_identity, existing_files) = if let Some(album) = album {
            let (identity, files) =
                replacement_snapshot(&album.into_directory(), &FilePermissions::current()?)?;
            (Some(identity), files)
        } else {
            (None, Vec::new())
        };
        let mut preview = AlbumReplacementPreview {
            job_id: job,
            plan_version: version,
            location_id: target.location_id,
            root: target.root,
            root_identity: root.native_root().identity.clone(),
            kind: target.kind,
            relative_folder: folder.clone(),
            output_files: output_files.clone(),
            destination_available: true,
            unavailable_reason: None,
            album_identity,
            manifest_sha256: String::new(),
            existing_files,
            decision_saved: false,
            saved_action: None,
        };
        preview.manifest_sha256 = replacement_hash(&preview)?;
        if let Some(review) = saved_reviews.iter().find(|review| {
            review.location_id == preview.location_id
                && review.manifest_sha256 == preview.manifest_sha256
        }) {
            preview.decision_saved = true;
            preview.saved_action = Some(review.action);
        }
        previews.push(preview);
    }
    Ok(previews)
}

pub(crate) struct PublicationDirectory {
    directory: Dir,
    parent: Dir,
    identity: String,
    permissions: FilePermissions,
    prepared_folder: String,
    prepared_name: String,
    final_folder: String,
    final_name: String,
    retained_name: Option<String>,
    visible: bool,
}

impl PublicationDirectory {
    /// Prepare a complete album in a hidden sibling folder. An unbound folder is
    /// never adopted, even if its bytes happen to match.
    pub(crate) fn open(
        store: &Store,
        target: &PublicationDestination,
        preparation: &PublicationPreparation,
    ) -> Result<Option<Self>> {
        let root = LibraryDirectory::open(Path::new(&target.root))?;
        store.check_native_root(&target.root, "media", root.native_root())?;
        if root.native_root().identity != target.root_identity {
            return Err(conflict());
        }
        let final_path = LibraryPath::parse(&target.relative_folder)?;
        let prepared_path = LibraryPath::parse(&preparation.folder)?;
        let final_parent = final_path.parent();
        if final_path.as_str().is_empty()
            || prepared_path.parent() != final_parent
            || !prepared_path
                .as_str()
                .rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with(".domus-pending-"))
        {
            return Err(conflict());
        }
        let permissions = FilePermissions::current()?;
        let mut parent = root.into_directory();
        checked(&parent, &permissions)?;
        let mut parts = final_parent.split('/').filter(|part| !part.is_empty());
        for name in parts.by_ref() {
            check_case(&parent, name)?;
            match parent.create_dir(name) {
                Ok(()) => sync(&parent)?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            parent = parent.open_dir_nofollow(name)?;
            checked(&parent, &permissions)?;
        }
        let final_name = final_path
            .as_str()
            .rsplit('/')
            .next()
            .ok_or_else(conflict)?;
        let prepared_name = prepared_path
            .as_str()
            .rsplit('/')
            .next()
            .ok_or_else(conflict)?;
        check_case(&parent, final_name)?;
        check_case(&parent, prepared_name)?;
        let mut final_directory = open_child_directory(&parent, final_name)?;
        let mut prepared_directory = open_child_directory(&parent, prepared_name)?;
        let mut retained_name = None;
        let mut visible = preparation.published_at.is_some();
        let directory = if let Some(replacement) = target.replacement.as_ref() {
            let retained_path = LibraryPath::parse(&replacement.retained_folder)?;
            if replacement.original_folder != target.relative_folder
                || retained_path.parent() != final_parent
                || !retained_path
                    .as_str()
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.starts_with(".domus-retained-"))
            {
                return Err(conflict());
            }
            let name = retained_path
                .as_str()
                .rsplit('/')
                .next()
                .ok_or_else(conflict)?;
            check_case(&parent, name)?;
            let retained_directory = open_child_directory(&parent, name)?;
            let original_at = |directory: &Dir| -> Result<bool> {
                let identity = super::filesystem::native_file_identity(
                    &directory.try_clone()?.into_std_file(),
                )?;
                if identity != replacement.original_album_identity {
                    return Ok(false);
                }
                verify_replacement(directory, &replacement.original_files)?;
                Ok(true)
            };
            if let Some(directory) = retained_directory.as_ref()
                && !original_at(directory)?
            {
                return Err(conflict());
            }
            let final_is_original = final_directory
                .as_ref()
                .map(original_at)
                .transpose()?
                .unwrap_or(false);
            let final_is_new = if let Some(directory) = final_directory.as_ref() {
                let identity = super::filesystem::native_file_identity(
                    &directory.try_clone()?.into_std_file(),
                )?;
                target.album_identity.as_deref() == Some(identity.as_str())
            } else {
                false
            };
            if final_directory.is_some() && !final_is_original && !final_is_new {
                return Err(conflict());
            }
            if final_is_original && retained_directory.is_some() {
                return Err(conflict());
            }
            if final_is_new {
                if retained_directory.is_none() || prepared_directory.is_some() {
                    return Err(conflict());
                }
                visible = true;
                retained_name = Some(name.to_owned());
                final_directory.take().ok_or_else(conflict)?
            } else {
                if preparation.published_at.is_some() {
                    return Err(conflict());
                }
                if final_directory.is_none() && retained_directory.is_none() {
                    return Err(conflict());
                }
                if final_directory.is_none() && prepared_directory.is_none() {
                    return Err(conflict());
                }
                if final_is_original {
                    verify_replacement(
                        final_directory.as_ref().ok_or_else(conflict)?,
                        &replacement.original_files,
                    )?;
                }
                retained_name = Some(name.to_owned());
                if let Some(directory) = prepared_directory.take() {
                    directory
                } else {
                    match parent.create_dir(prepared_name) {
                        Ok(()) => {
                            sync(&parent)?;
                            parent.open_dir_nofollow(prepared_name)?
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            return Ok(None);
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        } else {
            if final_directory.is_some() && prepared_directory.is_some() {
                return Err(conflict());
            }
            if visible {
                if prepared_directory.is_some() {
                    return Err(conflict());
                }
                final_directory.take().ok_or_else(conflict)?
            } else if let Some(directory) = prepared_directory.take() {
                directory
            } else if let Some(directory) = final_directory.take() {
                let identity = super::filesystem::native_file_identity(
                    &directory.try_clone()?.into_std_file(),
                )?;
                if target.album_identity.as_deref() != Some(identity.as_str()) {
                    return Err(conflict());
                }
                if preparation.atomic {
                    visible = true;
                    directory
                } else {
                    drop(directory);
                    rename_no_replace(&parent, final_name, prepared_name)?;
                    sync(&parent)?;
                    parent.open_dir_nofollow(prepared_name)?
                }
            } else if target.album_identity.is_some() {
                return Err(conflict());
            } else {
                match parent.create_dir(prepared_name) {
                    Ok(()) => {
                        sync(&parent)?;
                        parent.open_dir_nofollow(prepared_name)?
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Ok(None);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        };
        let identity =
            super::filesystem::native_file_identity(&directory.try_clone()?.into_std_file())?;
        if target
            .album_identity
            .as_ref()
            .is_some_and(|saved| saved != &identity)
        {
            return Err(conflict());
        }
        checked(&directory, &permissions)?;
        Ok(Some(Self {
            directory,
            parent,
            identity,
            permissions,
            prepared_folder: preparation.folder.clone(),
            prepared_name: prepared_name.to_owned(),
            final_folder: target.relative_folder.clone(),
            final_name: final_name.to_owned(),
            retained_name,
            visible,
        }))
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    pub(crate) fn check(&self, store: &Store, target: &PublicationDestination) -> Result<()> {
        let root = LibraryDirectory::open(Path::new(&target.root))?;
        store.check_native_root(&target.root, "media", root.native_root())?;
        if root.native_root().identity != target.root_identity {
            return Err(conflict());
        }
        let folder = if self.visible {
            &self.final_folder
        } else {
            &self.prepared_folder
        };
        let album = root.open_directory(&LibraryPath::parse(folder)?)?;
        if album.native_root().identity != self.identity
            || target.album_identity.as_deref() != Some(self.identity.as_str())
        {
            return Err(conflict());
        }
        checked(&self.directory, &self.permissions)
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn publish(
        &mut self,
        store: &mut Store,
        claim: &crate::infrastructure::store::JobClaim,
        target: &PublicationDestination,
    ) -> Result<()> {
        self.check(store, target)?;
        if let Some(replacement) = target.replacement.as_ref() {
            if self.visible {
                if replacement.state == "planned" {
                    store.record_publication_replacement_state(claim, target.id, "retained")?;
                }
                store.record_publication_replacement_state(claim, target.id, "published")?;
                return self.check(store, target);
            }
            let retained_name = self.retained_name.as_deref().ok_or_else(conflict)?;
            if let Some(retained) = open_child_directory(&self.parent, retained_name)? {
                let identity = super::filesystem::native_file_identity(
                    &retained.try_clone()?.into_std_file(),
                )?;
                if identity != replacement.original_album_identity {
                    return Err(conflict());
                }
                verify_replacement(&retained, &replacement.original_files)?;
                if self.target_original_at_final(target)? {
                    return Err(conflict());
                }
            } else {
                let original =
                    open_child_directory(&self.parent, &self.final_name)?.ok_or_else(conflict)?;
                let identity = super::filesystem::native_file_identity(
                    &original.try_clone()?.into_std_file(),
                )?;
                if identity != replacement.original_album_identity {
                    return Err(conflict());
                }
                verify_replacement(&original, &replacement.original_files)?;
                drop(original);
                rename_no_replace(&self.parent, &self.final_name, retained_name)?;
                sync(&self.parent)?;
            }
            store.record_publication_replacement_state(claim, target.id, "retained")?;
            if open_child_directory(&self.parent, &self.final_name)?.is_some() {
                return Err(conflict());
            }
            let directory = std::mem::replace(&mut self.directory, self.parent.try_clone()?);
            drop(directory);
            if let Err(error) =
                rename_no_replace(&self.parent, &self.prepared_name, &self.final_name)
            {
                self.directory = self.parent.open_dir_nofollow(&self.prepared_name)?;
                return Err(error);
            }
            sync(&self.parent)?;
            self.directory = self.parent.open_dir_nofollow(&self.final_name)?;
            self.visible = true;
            store.record_publication_replacement_state(claim, target.id, "published")?;
            return self.check(store, target);
        }
        check_case(&self.parent, &self.final_name)?;
        if open_child_directory(&self.parent, &self.final_name)?.is_some()
            && (target.replacement.is_none() || !self.visible)
        {
            return Err(conflict());
        }
        let directory = std::mem::replace(&mut self.directory, self.parent.try_clone()?);
        drop(directory);
        let result = rename_no_replace(&self.parent, &self.prepared_name, &self.final_name);
        if let Err(error) = result {
            self.directory = self.parent.open_dir_nofollow(&self.prepared_name)?;
            return Err(error);
        }
        sync(&self.parent)?;
        self.directory = self.parent.open_dir_nofollow(&self.final_name)?;
        self.visible = true;
        self.check(store, target)
    }

    fn target_original_at_final(&self, target: &PublicationDestination) -> Result<bool> {
        let Some(replacement) = target.replacement.as_ref() else {
            return Ok(false);
        };
        let Some(directory) = open_child_directory(&self.parent, &self.final_name)? else {
            return Ok(false);
        };
        let identity =
            super::filesystem::native_file_identity(&directory.try_clone()?.into_std_file())?;
        if identity != replacement.original_album_identity {
            return Err(conflict());
        }
        verify_replacement(&directory, &replacement.original_files)?;
        Ok(true)
    }

    fn regular(&self, file: File) -> Result<File> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || is_unsafe_link(&metadata) {
            return Err(conflict());
        }
        self.permissions.check(&file, false)?;
        Ok(file)
    }

    /// Existing exact files can close a sync-before-receipt crash gap. Partial,
    /// different and receipted-but-missing files cannot be rewritten or truncated.
    pub(crate) fn copy(
        &self,
        entry: &PublicationFile,
        source: &mut File,
        check: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        check()?;
        LibraryPath::parse(&entry.name)?;
        if entry.name.is_empty() || entry.name.contains('/') || entry.bytes == 0 {
            return Err(conflict());
        }
        check_case(&self.directory, &entry.name)?;
        let (mut file, created) = if entry.verified_at.is_some() {
            (
                self.regular(
                    self.directory
                        .open_with(&entry.name, &options(false))?
                        .into_std(),
                )?,
                false,
            )
        } else {
            match self.directory.open_with(&entry.name, &options(true)) {
                Ok(file) => (self.regular(file.into_std())?, true),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
                    self.regular(
                        self.directory
                            .open_with(&entry.name, &options(false))?
                            .into_std(),
                    )?,
                    false,
                ),
                Err(error) => return Err(error.into()),
            }
        };
        if created {
            let mut remaining = entry.bytes;
            let mut buffer = [0; 65_536];
            while remaining > 0 {
                check()?;
                let count = remaining.min(buffer.len() as u64) as usize;
                source.read_exact(&mut buffer[..count])?;
                check()?;
                file.write_all(&buffer[..count])?;
                remaining -= count as u64;
            }
            if source.read(&mut [0; 1])? != 0 {
                return Err(conflict());
            }
        }
        check()?;
        file.sync_all()?;
        verify(&mut file, entry, check)?;
        sync(&self.directory)?;
        check()?;
        Ok(())
    }

    pub(crate) fn verify(
        &self,
        files: &[PublicationFile],
        check: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        let mut names = files
            .iter()
            .map(|file| file.name.as_str())
            .collect::<HashSet<_>>();
        for entry in self.directory.entries()? {
            check()?;
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| conflict())?;
            if !names.remove(name.as_str()) {
                return Err(conflict());
            }
            if is_unsafe_cap_link(&entry.metadata()?) {
                return Err(conflict());
            }
        }
        if !names.is_empty() {
            return Err(conflict());
        }
        for entry in files {
            check()?;
            let mut file = self.regular(
                self.directory
                    .open_with(&entry.name, &options(false))?
                    .into_std(),
            )?;
            verify(&mut file, entry, check)?;
        }
        sync(&self.directory)?;
        check()?;
        Ok(())
    }
}

fn open_child_directory(parent: &Dir, name: &str) -> Result<Option<Dir>> {
    match parent.open_dir_nofollow(name) {
        Ok(directory) => Ok(Some(directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn verify_replacement(directory: &Dir, files: &[ReplacementFile]) -> Result<()> {
    let mut names = files
        .iter()
        .map(|file| file.name.as_str())
        .collect::<HashSet<_>>();
    for entry in directory.entries()? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| conflict())?;
        if !names.remove(name.as_str()) || is_unsafe_cap_link(&entry.metadata()?) {
            return Err(conflict());
        }
    }
    if !names.is_empty() {
        return Err(conflict());
    }
    for entry in files {
        LibraryPath::parse(&entry.name)?;
        if entry.name.is_empty() || entry.name.contains('/') || entry.bytes > i64::MAX as u64 {
            return Err(conflict());
        }
        let mut file = directory
            .open_with(&entry.name, &read_options())?
            .into_std();
        let metadata = file.metadata()?;
        if !metadata.is_file() || is_unsafe_link(&metadata) || metadata.len() != entry.bytes {
            return Err(conflict());
        }
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0; 65_536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total.checked_add(count as u64).ok_or_else(conflict)?;
            if total > entry.bytes {
                return Err(conflict());
            }
            hash.update(&buffer[..count]);
        }
        if total != entry.bytes
            || file.metadata()?.len() != entry.bytes
            || format!("{:x}", hash.finalize()) != entry.sha256
        {
            return Err(conflict());
        }
    }
    Ok(())
}

fn rename_no_replace(directory: &Dir, from: &str, to: &str) -> Result<()> {
    super::filesystem::rename_no_replace(directory, from, to)
}

fn verify(
    file: &mut File,
    entry: &PublicationFile,
    check: &dyn Fn() -> std::io::Result<()>,
) -> Result<()> {
    if file.metadata()?.len() != entry.bytes {
        return Err(conflict());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65_536];
    let mut remaining = entry.bytes;
    while remaining > 0 {
        check()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if file.read(&mut [0; 1])? != 0
        || file.metadata()?.len() != entry.bytes
        || format!("{:x}", hash.finalize()) != entry.sha256
    {
        return Err(conflict());
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::rename_no_replace;
    use cap_std::fs::Dir;
    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::sync::{Arc, Barrier};

    #[test]
    fn windows_directory_rename_does_not_replace_a_racing_file() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let parent = Dir::open_ambient_dir(root.path(), cap_std::ambient_authority())?;

        for _ in 0..32 {
            let prepared = root.path().join("prepared");
            let final_path = root.path().join("album");
            fs::create_dir(&prepared)?;
            fs::write(prepared.join("track.flac"), b"album")?;

            let barrier = Arc::new(Barrier::new(2));
            let writer_barrier = Arc::clone(&barrier);
            let writer_path = final_path.clone();
            let writer = std::thread::Builder::new().spawn(move || {
                writer_barrier.wait();
                let result = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&writer_path);
                match result {
                    Ok(mut file) => {
                        file.write_all(b"racing file")?;
                        Ok(true)
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
                    Err(error)
                        if error.kind() == io::ErrorKind::PermissionDenied
                            && writer_path.is_dir() =>
                    {
                        Ok(false)
                    }
                    Err(error) => Err(error),
                }
            })?;

            barrier.wait();
            let rename = rename_no_replace(&parent, "prepared", "album");
            let writer_created_file = writer
                .join()
                .map_err(|_| io::Error::other("destination writer panicked"))??;

            if writer_created_file {
                assert!(rename.is_err());
                assert!(prepared.is_dir());
                assert_eq!(fs::read(&final_path)?, b"racing file");
                fs::remove_file(final_path)?;
                fs::remove_dir_all(prepared)?;
            } else {
                assert!(rename.is_ok());
                assert!(!prepared.exists());
                assert_eq!(fs::read(final_path.join("track.flac"))?, b"album");
                fs::remove_dir_all(final_path)?;
            }
        }

        Ok(())
    }
}
