use crate::domain::{LibraryPath, Page};
use crate::infrastructure::filesystem::{LibraryDirectory, native_file_identity, sync_directory};
use crate::infrastructure::recovery_files;
use crate::infrastructure::staging::StagingRoot;
use crate::infrastructure::store::{
    RecoveryBinding, RecoveryDecision, RecoveryEntry, RecoveryFile, RecoveryItem,
    RecoveryOperation, RecoveryPreview, ReplacementFile, Store,
};
use crate::{Error, Result};
use cap_fs_ext::DirExt;
use sha2::{Digest, Sha256};
use std::path::Path;

fn conflict() -> Error {
    Error::Conflict("recovery ownership, file evidence or approved state changed".into())
}

fn digest(value: &impl serde::Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

/// Inventory reports recorded logical bytes, not a filesystem allocation scan.
pub fn recovery_inventory(
    store: &Store,
    page: Page,
    job: Option<i64>,
) -> Result<Vec<RecoveryEntry>> {
    store.recovery_inventory(page, job)
}

pub fn preview_recovery(store: &mut Store, id: i64) -> Result<RecoveryPreview> {
    let _visibility = crate::infrastructure::shares::publication_visibility_write(store)?;
    let entry = store.recovery_entry(id)?;
    let mut preview = RecoveryPreview {
        entry,
        files: Vec::new(),
        root_identity: String::new(),
        directory_identity: String::new(),
        current_sha256: String::new(),
        preview_sha256: None,
        verified: false,
        unavailable_reason: None,
    };
    match retained_evidence(store, &preview.entry) {
        Ok((binding, files)) => {
            preview.files = files;
            preview.root_identity = binding.root_identity;
            preview.directory_identity = binding.directory_identity;
            preview.current_sha256 = digest(&(&binding.visible_identity, &binding.visible_files))?;
            preview.verified = true;
            preview.preview_sha256 = Some(digest(&preview)?);
        }
        Err(error) => preview.unavailable_reason = Some(error.to_string()),
    }
    Ok(preview)
}

fn retained_evidence(
    store: &mut Store,
    entry: &RecoveryEntry,
) -> Result<(RecoveryBinding, Vec<RecoveryFile>)> {
    if !entry.eligible || entry.category != "retained" {
        return Err(conflict());
    }
    let restore = store.prepare_album_replacement_restore(
        entry.job_id,
        entry.plan_version,
        entry.location_id.ok_or_else(conflict)?,
        None,
    )?;
    let (identity, expected, visible_identity, visible_files) = match restore.state.as_str() {
        "published" => (
            restore.original_album_identity,
            restore.original_files,
            restore.replacement_album_identity,
            restore.replacement_files,
        ),
        "restored" => (
            restore.replacement_album_identity,
            restore.replacement_files,
            restore.original_album_identity,
            restore.original_files,
        ),
        _ => return Err(conflict()),
    };
    let mut binding = RecoveryBinding {
        root: restore.root,
        root_identity: restore.root_identity,
        root_kind: "media".into(),
        directory: restore.retained_folder,
        directory_identity: identity,
        visible_directory: Some(restore.original_folder),
        visible_identity: Some(visible_identity),
        visible_files,
    };
    let root = open_root(store, &binding)?;
    verify_visible(&root, &binding)?;
    let directory = root
        .open_directory(&LibraryPath::parse(&binding.directory)?)?
        .into_directory();
    let (identity, files) = recovery_files::inspect(&directory, &expected)?;
    if identity != binding.directory_identity {
        return Err(conflict());
    }
    binding.directory_identity = identity;
    Ok((binding, files))
}

pub fn delete_recovery(
    store: &mut Store,
    id: i64,
    decision: RecoveryDecision,
) -> Result<RecoveryOperation> {
    if !decision.acknowledge_irreversible_deletion {
        return Err(Error::InvalidInput(
            "deletion requires explicit irreversible-deletion acknowledgement".into(),
        ));
    }
    let _visibility = crate::infrastructure::shares::publication_visibility_write(store)?;
    let entry = store.recovery_entry(id)?;
    let (binding, files) = retained_evidence(store, &entry)?;
    let mut preview = RecoveryPreview {
        entry: entry.clone(),
        files: files.clone(),
        root_identity: binding.root_identity.clone(),
        directory_identity: binding.directory_identity.clone(),
        current_sha256: digest(&(&binding.visible_identity, &binding.visible_files))?,
        preview_sha256: None,
        verified: true,
        unavailable_reason: None,
    };
    preview.preview_sha256 = Some(digest(&preview)?);
    if preview.preview_sha256.as_deref() != Some(decision.expected_preview_sha256.as_str()) {
        return Err(Error::Conflict(
            "deletion preview changed; inspect this exact version again".into(),
        ));
    }
    let mut items = Vec::new();
    for file in files {
        items.push(item(
            items.len(),
            entry.id,
            format!("{}/{}", binding.directory, file.name),
            file,
            false,
        ));
    }
    items.push(directory_item(
        items.len(),
        entry.id,
        &binding.directory,
        &binding.directory_identity,
    ));
    store.begin_recovery(&entry, &decision.expected_preview_sha256, &binding, &items)?;
    run_operation(store, id, None)
}

/// Continuation never changes the approval or adopts a replacement file.
pub fn continue_recovery(
    store: &mut Store,
    id: i64,
    expected: i64,
    cleanup: bool,
) -> Result<RecoveryOperation> {
    let _visibility = crate::infrastructure::shares::publication_visibility_write(store)?;
    let operation = store.recovery_operation(id)?;
    if cleanup && operation.is_none() && expected == 0 {
        let entry = store.recovery_entry(id)?;
        if entry.category != "scratch" {
            return Err(conflict());
        }
        let (path, _) = store.scratch_admission((id - 1) / 8)?;
        let staging = StagingRoot::open(store, Path::new(&path))?;
        return cleanup_scratch_locked(store, &staging, (id - 1) / 8);
    }
    let operation = operation.ok_or_else(conflict)?;
    if operation.id != expected || (operation.kind == "cleanup") != cleanup {
        return Err(conflict());
    }
    if operation.state == "complete" {
        return Ok(operation);
    }
    if cleanup {
        let binding = store.recovery_binding(operation.id)?;
        let staging = StagingRoot::open(store, Path::new(&binding.root))?;
        run_operation(store, id, Some(&staging))
    } else {
        run_operation(store, id, None)
    }
}

pub(crate) fn cleanup_scratch_locked(
    store: &mut Store,
    staging: &StagingRoot,
    run: i64,
) -> Result<RecoveryOperation> {
    let id = run * 8 + 1;
    if let Some(operation) = store.recovery_operation(id)? {
        if operation.state == "complete" {
            return Ok(operation);
        }
        return run_operation(store, id, Some(staging));
    }
    let result = (|| {
        let entry = store.recovery_entry(id)?;
        require_complete(store, &entry)?;
        verify_published_destinations(store, entry.job_id, entry.plan_version)?;
        let (root_path, root_identity) = store.scratch_admission(run)?;
        if staging.cleanup_binding()? != (root_path.clone(), root_identity.clone()) {
            return Err(conflict());
        }
        let root = staging.cleanup_directory(store)?;
        let (normalized, source) = store.scratch_source(run)?;
        let normalized_dir = root.open_dir_nofollow(&normalized.staging_directory)?;
        let recipe = crate::infrastructure::staging::recipe(&normalized)?;
        let mut expected = vec![
            ReplacementFile {
                name: "recipe.json".into(),
                bytes: recipe.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&recipe)),
            },
            ReplacementFile {
                name: "folder.jpg".into(),
                bytes: normalized.cover_bytes,
                sha256: normalized.cover_sha256.clone(),
            },
        ];
        for file in &source {
            expected.push(ReplacementFile {
                name: file.name.clone(),
                bytes: file.bytes,
                sha256: file.sha256.clone(),
            });
        }
        let (directory_identity, files) = recovery_files::inspect(&normalized_dir, &expected)?;
        drop(normalized_dir);
        let mut items = Vec::new();
        for file in files {
            items.push(item(
                items.len(),
                id,
                format!("{}/{}", normalized.staging_directory, file.name),
                file,
                false,
            ));
        }
        items.push(directory_item(
            items.len(),
            id,
            &normalized.staging_directory,
            &directory_identity,
        ));
        for file in source {
            staging.cleanup_verify_input(file.transfer_id, file.input_bytes, &file.input_sha256)?;
            for suffix in ["input", "receipt"] {
                let name = format!("transfer-{}.{}", file.transfer_id, suffix);
                let observed = recovery_files::file(&root, &name)?;
                items.push(item(
                    items.len(),
                    file.transfer_id * 8,
                    name,
                    observed,
                    false,
                ));
            }
        }
        let binding = RecoveryBinding {
            root: root_path,
            root_identity,
            root_kind: "staging".into(),
            directory: normalized.staging_directory,
            directory_identity,
            visible_directory: None,
            visible_identity: None,
            visible_files: Vec::new(),
        };
        let manifest = digest(&items.iter().map(|i| (&i.path, &i.file)).collect::<Vec<_>>())?;
        store.begin_recovery(&entry, &manifest, &binding, &items)?;
        run_operation(store, id, Some(staging))
    })();
    if let Err(error) = &result {
        store.scratch_error(run, &error.to_string())?;
    }
    result
}

fn item(
    ordinal: usize,
    artifact_id: i64,
    path: String,
    file: RecoveryFile,
    directory: bool,
) -> RecoveryItem {
    RecoveryItem {
        ordinal: ordinal as i64,
        artifact_id,
        path,
        file,
        directory,
        state: "planned".into(),
    }
}
fn directory_item(ordinal: usize, artifact: i64, path: &str, identity: &str) -> RecoveryItem {
    item(
        ordinal,
        artifact,
        path.into(),
        RecoveryFile {
            name: path.rsplit('/').next().unwrap_or("").into(),
            bytes: 0,
            sha256: String::new(),
            identity: identity.into(),
        },
        true,
    )
}

fn open_root(store: &Store, binding: &RecoveryBinding) -> Result<LibraryDirectory> {
    let root = LibraryDirectory::open(Path::new(&binding.root))?;
    store.check_native_root(&binding.root, &binding.root_kind, root.native_root())?;
    if root.native_root().identity != binding.root_identity {
        return Err(conflict());
    }
    Ok(root)
}

fn verify_visible(root: &LibraryDirectory, binding: &RecoveryBinding) -> Result<()> {
    if let Some(path) = &binding.visible_directory {
        let directory = root
            .open_directory(&LibraryPath::parse(path)?)?
            .into_directory();
        let (identity, _) = recovery_files::inspect(&directory, &binding.visible_files)?;
        if Some(&identity) != binding.visible_identity.as_ref() {
            return Err(conflict());
        }
    }
    Ok(())
}

fn require_complete(store: &Store, entry: &RecoveryEntry) -> Result<()> {
    store.check_recovery_current(entry)?;
    let destinations = store.publication_destinations(entry.job_id, entry.plan_version)?;
    if destinations.is_empty()
        || destinations
            .iter()
            .any(|d| d.verified_at.is_none() || d.last_error.is_some())
    {
        return Err(conflict());
    }
    Ok(())
}

fn verify_published_destinations(store: &Store, job: i64, version: i64) -> Result<()> {
    let mut destinations = store.publication_destinations(job, version)?;
    destinations.sort_by_key(|d| d.kind == "main");
    for destination in destinations {
        if destination
            .replacement
            .as_ref()
            .is_some_and(|r| r.state != "published")
        {
            return Err(Error::Conflict(
                "scratch cleanup cannot cross a restore transition".into(),
            ));
        }
        let expected = store.recovery_publication_files(destination.id)?;
        let root = LibraryDirectory::open(Path::new(&destination.root))?;
        store.check_native_root(&destination.root, "media", root.native_root())?;
        if root.native_root().identity != destination.root_identity {
            return Err(conflict());
        }
        let directory = root
            .open_directory(&LibraryPath::parse(&destination.relative_folder)?)?
            .into_directory();
        let (identity, _) = recovery_files::inspect(&directory, &expected)?;
        if Some(identity) != destination.album_identity {
            return Err(conflict());
        }
    }
    Ok(())
}

fn run_operation(
    store: &mut Store,
    id: i64,
    staging: Option<&StagingRoot>,
) -> Result<RecoveryOperation> {
    let operation = store.recovery_operation(id)?.ok_or_else(conflict)?;
    let result = (|| {
        let entry = store.recovery_entry(id)?;
        require_complete(store, &entry)?;
        let binding = store.recovery_binding(operation.id)?;
        let root = open_root(store, &binding)?;
        if operation.kind == "cleanup" {
            staging.ok_or_else(conflict)?.cleanup_directory(store)?;
            verify_published_destinations(store, entry.job_id, entry.plan_version)?;
        } else {
            verify_visible(&root, &binding)?;
        }
        let items = store.recovery_items(operation.id)?;
        check_layout(&root, &binding, &items)?;
        for item in &items {
            store.check_worker_stop()?;
            store.check_recovery_current(&entry)?;
            let checked_root = open_root(store, &binding)?;
            if let Some(staging) = staging {
                staging.cleanup_directory(store)?;
            }
            if item.state == "removed" {
                continue;
            }
            let path = LibraryPath::parse(&item.path)?;
            let parent = checked_root
                .open_directory(&LibraryPath::parse(path.parent())?)?
                .into_directory();
            if path.parent() == binding.directory
                && native_file_identity(&parent.try_clone()?.into_std_file())?
                    != binding.directory_identity
            {
                return Err(conflict());
            }
            let existing = parent.symlink_metadata(&item.file.name);
            match existing {
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && item.state == "removing" => {
                }
                Err(error) => return Err(error.into()),
                Ok(_) => {
                    if item.state == "planned" {
                        // Verify before intent, then the native adapter verifies again before unlink.
                        if item.directory {
                            let dir = parent.open_dir_nofollow(&item.file.name)?;
                            if native_file_identity(&dir.into_std_file())? != item.file.identity {
                                return Err(conflict());
                            }
                        } else if recovery_files::file(&parent, &item.file.name)? != item.file {
                            return Err(conflict());
                        }
                        store.recovery_item_state(
                            operation.id,
                            item.ordinal,
                            "planned",
                            "removing",
                        )?;
                    }
                    if item.directory {
                        recovery_files::remove_directory(
                            &parent,
                            &item.file.name,
                            &item.file.identity,
                        )?;
                    } else {
                        recovery_files::delete_file(&parent, &item.file)?;
                    }
                    sync_directory(&parent)?;
                }
            }
            store.recovery_item_state(operation.id, item.ordinal, "removing", "removed")?;
        }
        store.recovery_result(operation.id, None)?;
        Ok(())
    })();
    if let Err(error) = result {
        store
            .recovery_result(operation.id, Some(&error.to_string()))
            .map_err(|record| {
                Error::Conflict(format!(
                    "{error}; recovery error could not be saved: {record}; inspect the journal"
                ))
            })?;
        return Err(error);
    }
    store.recovery_operation(id)?.ok_or_else(conflict)
}

fn check_layout(
    root: &LibraryDirectory,
    binding: &RecoveryBinding,
    items: &[RecoveryItem],
) -> Result<()> {
    let directory = match root.open_directory(&LibraryPath::parse(&binding.directory)?) {
        Ok(directory) => Some(directory.into_directory()),
        Err(Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound
                && items.iter().any(|i| {
                    i.directory && i.path == binding.directory && i.state != "planned"
                })
                && items
                    .iter()
                    .filter(|i| i.path.starts_with(&format!("{}/", binding.directory)))
                    .all(|i| i.state == "removed") =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(directory) = directory {
        if native_file_identity(&directory.try_clone()?.into_std_file())?
            != binding.directory_identity
        {
            return Err(conflict());
        }
        for (index, child) in directory.entries()?.enumerate() {
            if index >= 1002 {
                return Err(conflict());
            }
            let name = child?.file_name().into_string().map_err(|_| conflict())?;
            let path = format!("{}/{}", binding.directory, name);
            if !items.iter().any(|i| i.path == path && i.state != "removed") {
                return Err(conflict());
            }
        }
    }
    for item in items.iter().filter(|i| !i.directory) {
        let path = LibraryPath::parse(&item.path)?;
        let parent = match root.open_directory(&LibraryPath::parse(path.parent())?) {
            Ok(parent) => parent.into_directory(),
            Err(Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound && item.state == "removed" =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        match parent.symlink_metadata(&item.file.name) {
            Ok(_) if item.state == "removed" => return Err(conflict()),
            Ok(_) => {
                if recovery_files::file(&parent, &item.file.name)? != item.file {
                    return Err(conflict());
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && item.state != "planned" => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
