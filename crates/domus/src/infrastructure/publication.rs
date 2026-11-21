#[cfg(test)]
use super::filesystem::LibraryDirectory;
use super::filesystem::media_adapter::MediaDirectoryIo;
use super::store::{
    AlbumReplacementPreview, AlbumReplacementRestore, AlbumReplacementRestoreDecision,
    AlbumRestorePreview, PublicationDestination, PublicationFile, PublicationPreparation,
    ReplacementFile, Store, replacement_hash,
};
use crate::domain::LibraryPath;
use crate::{Error, Result};
use std::fs::File;

fn conflict() -> Error {
    Error::Conflict("publication destination changed, is incomplete or contains existing content; nothing was overwritten".into())
}

fn publication_checkpoint(
    store: &Store,
    claim: &super::store::JobClaim,
    target: &PublicationDestination,
) -> std::io::Result<()> {
    store
        .renew_claim(claim, 300)
        .map_err(std::io::Error::other)?;
    store
        .check_publication(claim, target.id)
        .map(|_| ())
        .map_err(std::io::Error::other)
}

fn publication_manifest(files: &[PublicationFile]) -> Vec<ReplacementFile> {
    files
        .iter()
        .map(|file| ReplacementFile {
            name: file.name.clone(),
            bytes: file.bytes,
            sha256: file.sha256.clone(),
        })
        .collect()
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
            #[cfg(test)]
            crate::crash_tests::checkpoint("restore_intent")?;
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
                open_media_child(&folders.parent, &folders.final_name)?.ok_or_else(conflict)?;
            let (identity, files) = current.replacement_snapshot(&|| Ok(()))?;
            if identity == restore.original_album_identity
                || folder_version(
                    &folders.parent,
                    &folders.retained_name,
                    old,
                    (&identity, &files),
                )? != Some(FolderVersion::Old)
                || open_media_child(&folders.parent, &folders.temporary_name)?.is_some()
            {
                return Err(conflict());
            }
            current.verify_replacement(&files, &|| Ok(()))?;
            preview.current_album_identity = identity;
            preview.current_files = files;
        } else {
            let new = (
                restore.replacement_album_identity.as_str(),
                restore.replacement_files.as_slice(),
            );
            let layout = (
                folder_version(&folders.parent, &folders.final_name, old, new)?,
                folder_version(&folders.parent, &folders.retained_name, old, new)?,
                folder_version(&folders.parent, &folders.temporary_name, old, new)?,
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
    parent: MediaDirectoryIo,
    final_name: String,
    retained_name: String,
    temporary_name: String,
}

impl RestoreFolders {
    fn open(store: &Store, restore: &AlbumReplacementRestore) -> Result<Self> {
        let root = store.media_directory_io(&restore.root)?;
        if store.root_binding_identity(&restore.root, "media")? != restore.root_identity {
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

        let mut parent = root;
        parent.check_named_directory(&|| Ok(()))?;
        for name in parent_path.split('/').filter(|part| !part.is_empty()) {
            parent.check_case(name, &|| Ok(()))?;
            parent = open_media_child(&parent, name)?.ok_or_else(conflict)?;
            parent.check_named_directory(&|| Ok(()))?;
        }
        for name in [final_name, retained_name, temporary_name] {
            parent.check_case(name, &|| Ok(()))?;
        }
        Ok(Self {
            parent,
            final_name: final_name.into(),
            retained_name: retained_name.into(),
            temporary_name: temporary_name.into(),
        })
    }
}

fn run_album_restore(store: &mut Store, restore: &AlbumReplacementRestore) -> Result<()> {
    let RestoreFolders {
        parent,
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
        parent.check_named_directory(&|| Ok(()))?;
        let final_version = folder_version(&parent, final_name, old, new)?;
        let retained_version = folder_version(&parent, retained_name, old, new)?;
        let temporary_version = folder_version(&parent, temporary_name, old, new)?;
        let rename = match restore_step(state, final_version, retained_version, temporary_version)?
        {
            RestoreStep::MoveCurrent => (final_name, temporary_name, new),
            RestoreStep::MoveOriginal => (retained_name, final_name, old),
            RestoreStep::RetainCurrent => (temporary_name, retained_name, new),
            RestoreStep::Record(next) => {
                parent.sync_checked(&|| Ok(()))?;
                store.record_album_replacement_restore_state(
                    restore.destination_id,
                    state,
                    next,
                )?;
                state = next;
                #[cfg(test)]
                crate::crash_tests::checkpoint(next)?;
                continue;
            }
            RestoreStep::Complete => return Ok(()),
        };
        parent.rename_directory(rename.0, rename.1, rename.2.0, rename.2.1, &|| Ok(()))?;
        parent.sync_checked(&|| Ok(()))?;
        #[cfg(test)]
        crate::crash_tests::checkpoint(match state {
            "restore_prepared" => "restore_current_renamed",
            "restore_current_moved" => "restore_original_renamed",
            "restore_original_visible" => "restore_displaced_renamed",
            _ => return Err(conflict()),
        })?;
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
    parent: &MediaDirectoryIo,
    name: &str,
    old: (&str, &[ReplacementFile]),
    new: (&str, &[ReplacementFile]),
) -> Result<Option<FolderVersion>> {
    let Some(directory) = open_media_child(parent, name)? else {
        return Ok(None);
    };
    directory.check_named_directory(&|| Ok(()))?;
    let identity = directory.identity();
    let (version, files) = if identity == old.0 {
        (FolderVersion::Old, old.1)
    } else if identity == new.0 {
        (FolderVersion::New, new.1)
    } else {
        return Err(conflict());
    };
    directory.verify_replacement(files, &|| Ok(()))?;
    Ok(Some(version))
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
        let root = match store.media_directory_io(&target.root) {
            Ok(root) => root,
            Err(error @ (Error::Io(_) | Error::Remote(_))) => {
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
        let album = root.open_directory(&relative)?;
        let (album_identity, existing_files) = if let Some(album) = album {
            let (identity, files) = media_replacement_snapshot(&album)?;
            (Some(identity), files)
        } else {
            (None, Vec::new())
        };
        let mut preview = AlbumReplacementPreview {
            job_id: job,
            plan_version: version,
            location_id: target.location_id,
            root_identity: store.root_binding_identity(&target.root, "media")?,
            root: target.root,
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

pub(crate) fn media_replacement_snapshot(
    directory: &MediaDirectoryIo,
) -> Result<(String, Vec<ReplacementFile>)> {
    directory.replacement_snapshot(&|| Ok(()))
}

pub(crate) struct PublicationDirectory {
    directory: Option<MediaDirectoryIo>,
    parent: MediaDirectoryIo,
    identity: String,
    prepared_name: String,
    final_name: String,
    retained_name: Option<String>,
    visible: bool,
}

impl PublicationDirectory {
    /// Prepare a complete album in a hidden sibling folder. An unbound folder is
    /// never adopted, even if its bytes happen to match.
    pub(crate) fn open(
        store: &Store,
        claim: &super::store::JobClaim,
        target: &PublicationDestination,
        preparation: &PublicationPreparation,
    ) -> Result<Option<Self>> {
        // Keep native admission until restore and recovery own this same I/O.
        let root = store.media_directory_io(&target.root)?;
        if store.root_binding_identity(&target.root, "media")? != target.root_identity {
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
        let checkpoint = || publication_checkpoint(store, claim, target);
        let mut parent = root;
        parent.check_named_directory(&|| Ok(()))?;
        let mut parts = final_parent.split('/').filter(|part| !part.is_empty());
        for name in parts.by_ref() {
            parent.check_case(name, &checkpoint)?;
            parent = match open_media_child(&parent, name)? {
                Some(directory) => directory,
                None => match parent.create_directory(name, &checkpoint) {
                    Ok(directory) => directory,
                    Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        parent.check_case(name, &checkpoint)?;
                        open_media_child(&parent, name)?.ok_or_else(conflict)?
                    }
                    Err(error) => return Err(error),
                },
            };
            parent.check_named_directory(&|| Ok(()))?;
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
        parent.check_case(final_name, &checkpoint)?;
        parent.check_case(prepared_name, &checkpoint)?;
        let mut final_directory = open_media_child(&parent, final_name)?;
        let mut prepared_directory = open_media_child(&parent, prepared_name)?;
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
            parent.check_case(name, &checkpoint)?;
            let retained_directory = open_media_child(&parent, name)?;
            let original_at = |directory: &MediaDirectoryIo| -> Result<bool> {
                if directory.identity() != replacement.original_album_identity {
                    return Ok(false);
                }
                directory.verify_replacement(&replacement.original_files, &checkpoint)?;
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
                target.album_identity.as_deref() == Some(directory.identity())
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
                    final_directory
                        .as_ref()
                        .ok_or_else(conflict)?
                        .verify_replacement(&replacement.original_files, &checkpoint)?;
                }
                retained_name = Some(name.to_owned());
                if let Some(directory) = prepared_directory.take() {
                    directory
                } else {
                    match parent.create_directory(prepared_name, &checkpoint) {
                        Ok(directory) => directory,
                        Err(Error::Io(error))
                            if error.kind() == std::io::ErrorKind::AlreadyExists =>
                        {
                            return Ok(None);
                        }
                        Err(error) => return Err(error),
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
                let identity = directory.identity().to_owned();
                if target.album_identity.as_deref() != Some(identity.as_str()) {
                    return Err(conflict());
                }
                if preparation.atomic {
                    visible = true;
                    directory
                } else {
                    drop(directory);
                    let files = publication_manifest(&store.publication_files(claim, target.id)?);
                    parent.rename_directory(
                        final_name,
                        prepared_name,
                        &identity,
                        &files,
                        &checkpoint,
                    )?;
                    parent.sync_checked(&checkpoint)?;
                    open_media_child(&parent, prepared_name)?.ok_or_else(conflict)?
                }
            } else if target.album_identity.is_some() {
                return Err(conflict());
            } else {
                match parent.create_directory(prepared_name, &checkpoint) {
                    Ok(directory) => directory,
                    Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Ok(None);
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        let identity = directory.identity().to_owned();
        if target
            .album_identity
            .as_ref()
            .is_some_and(|saved| saved != &identity)
        {
            return Err(conflict());
        }
        directory.check_named_directory(&|| Ok(()))?;
        Ok(Some(Self {
            directory: Some(directory),
            parent,
            identity,
            prepared_name: prepared_name.to_owned(),
            final_name: final_name.to_owned(),
            retained_name,
            visible,
        }))
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    fn directory(&self) -> Result<&MediaDirectoryIo> {
        let directory = self.directory.as_ref().ok_or(Error::Conflict(
            "publication move failed; reopen the operation".into(),
        ))?;
        if directory.identity() != self.identity {
            return Err(conflict());
        }
        Ok(directory)
    }

    pub(crate) fn check(&self, store: &Store, target: &PublicationDestination) -> Result<()> {
        if store.root_binding_identity(&target.root, "media")? != target.root_identity {
            return Err(conflict());
        }
        if target.album_identity.as_deref() != Some(self.identity.as_str()) {
            return Err(conflict());
        }
        self.directory()?.check_named_directory(&|| Ok(()))
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
            if let Some(retained) = open_media_child(&self.parent, retained_name)? {
                let identity = retained.identity();
                if identity != replacement.original_album_identity {
                    return Err(conflict());
                }
                retained.verify_replacement(&replacement.original_files, &|| {
                    publication_checkpoint(store, claim, target)
                })?;
                if self.target_original_at_final(target)? {
                    return Err(conflict());
                }
            } else {
                let original =
                    open_media_child(&self.parent, &self.final_name)?.ok_or_else(conflict)?;
                let identity = original.identity();
                if identity != replacement.original_album_identity {
                    return Err(conflict());
                }
                original.verify_replacement(&replacement.original_files, &|| {
                    publication_checkpoint(store, claim, target)
                })?;
                drop(original);
                self.parent.rename_directory(
                    &self.final_name,
                    retained_name,
                    &replacement.original_album_identity,
                    &replacement.original_files,
                    &|| publication_checkpoint(store, claim, target),
                )?;
                self.parent
                    .sync_checked(&|| publication_checkpoint(store, claim, target))?;
                #[cfg(test)]
                crate::crash_tests::checkpoint("replacement_retained")?;
            }
            store.record_publication_replacement_state(claim, target.id, "retained")?;
            if open_media_child(&self.parent, &self.final_name)?.is_some() {
                return Err(conflict());
            }
            let files = publication_manifest(&store.publication_files(claim, target.id)?);
            drop(self.directory.take());
            self.parent.rename_directory(
                &self.prepared_name,
                &self.final_name,
                &self.identity,
                &files,
                &|| publication_checkpoint(store, claim, target),
            )?;
            self.parent
                .sync_checked(&|| publication_checkpoint(store, claim, target))?;
            self.directory =
                Some(open_media_child(&self.parent, &self.final_name)?.ok_or_else(conflict)?);
            self.visible = true;
            #[cfg(test)]
            crate::crash_tests::checkpoint("replacement_visible")?;
            store.record_publication_replacement_state(claim, target.id, "published")?;
            return self.check(store, target);
        }
        self.parent.check_case(&self.final_name, &|| {
            publication_checkpoint(store, claim, target)
        })?;
        if open_media_child(&self.parent, &self.final_name)?.is_some()
            && (target.replacement.is_none() || !self.visible)
        {
            return Err(conflict());
        }
        let files = publication_manifest(&store.publication_files(claim, target.id)?);
        drop(self.directory.take());
        self.parent.rename_directory(
            &self.prepared_name,
            &self.final_name,
            &self.identity,
            &files,
            &|| publication_checkpoint(store, claim, target),
        )?;
        self.parent
            .sync_checked(&|| publication_checkpoint(store, claim, target))?;
        self.directory =
            Some(open_media_child(&self.parent, &self.final_name)?.ok_or_else(conflict)?);
        self.visible = true;
        #[cfg(test)]
        crate::crash_tests::checkpoint("publication_visible")?;
        self.check(store, target)
    }

    fn target_original_at_final(&self, target: &PublicationDestination) -> Result<bool> {
        let Some(replacement) = target.replacement.as_ref() else {
            return Ok(false);
        };
        let Some(directory) = open_media_child(&self.parent, &self.final_name)? else {
            return Ok(false);
        };
        let identity = directory.identity();
        if identity != replacement.original_album_identity {
            return Err(conflict());
        }
        directory.verify_replacement(&replacement.original_files, &|| Ok(()))?;
        Ok(true)
    }

    /// Existing exact files can close a sync-before-receipt crash gap. Partial,
    /// different and receipted-but-missing files cannot be rewritten or truncated.
    pub(crate) fn copy(
        &self,
        entry: &PublicationFile,
        source: &mut File,
        check: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        self.directory()?
            .copy_publication_file(entry, source, check)
    }

    pub(crate) fn verify(
        &self,
        files: &[PublicationFile],
        check: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        self.directory()?.verify_publication_files(files, check)
    }
}

fn open_media_child(parent: &MediaDirectoryIo, name: &str) -> Result<Option<MediaDirectoryIo>> {
    parent.open_directory(&LibraryPath::parse(name)?)
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[test]
    fn publication_ownership_rejects_a_different_retained_folder_with_identical_bytes() -> Result<()>
    {
        let fixture = tempfile::tempdir()?;
        std::fs::create_dir(fixture.path().join("prepared"))?;
        std::fs::create_dir(fixture.path().join("other"))?;
        let parent = LibraryDirectory::open(fixture.path())?;
        let prepared = MediaDirectoryIo::native(parent.open_dir_nofollow("prepared")?)?;
        let identity = prepared.identity().to_owned();
        drop(prepared);
        let directory = PublicationDirectory {
            directory: Some(MediaDirectoryIo::native(
                parent.open_dir_nofollow("other")?,
            )?),
            parent: MediaDirectoryIo::native(parent)?,
            identity,
            prepared_name: "prepared".into(),
            final_name: "final".into(),
            retained_name: None,
            visible: false,
        };
        assert!(matches!(
            directory.verify(&[], &|| Ok(())),
            Err(Error::Conflict(_))
        ));
        Ok(())
    }
}

#[cfg(all(test, windows))]
fn rename_no_replace(directory: &cap_std::fs::Dir, from: &str, to: &str) -> Result<()> {
    super::filesystem::rename_no_replace(directory, from, to)
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
