//! Explicit native folder binding. Saving configuration never calls these operations.
use crate::domain::{LibraryPath, Page};
use crate::infrastructure::filesystem::{
    LibraryDirectory, ReadDirectory, ReadSnapshot as DirectorySnapshot,
};
use crate::infrastructure::store::Store;
use crate::{Error, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, ts_rs::TS)]
pub struct ShareFolderInspection {
    pub location_id: i64,
    pub relative_path: String,
    pub native_identity: String,
    pub binding_id: Option<i64>,
    pub bound: bool,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct InspectShareFolder {
    pub location_id: i64,
    #[serde(default)]
    pub relative_path: String,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BindShareFolder {
    pub location_id: i64,
    pub relative_path: String,
    pub expected_identity: String,
}

pub(crate) fn selected_directory(
    store: &Store,
    location_id: i64,
    relative: &LibraryPath,
) -> Result<ReadDirectory> {
    if relative.as_str().split('/').count() > 128 {
        return Err(Error::InvalidInput(
            "share folder exceeds the native traversal depth limit".into(),
        ));
    }
    if relative.as_str().split('/').any(|part| {
        part.to_ascii_lowercase().starts_with(".domus-retained-")
            || part.to_ascii_lowercase().starts_with(".domus-pending-")
            || part.to_ascii_lowercase().starts_with(".domus-restore-")
    }) {
        return Err(Error::InvalidInput(
            "publication recovery folders cannot be shared".into(),
        ));
    }
    let location = store.location(location_id)?;
    if !matches!(location.kind.as_str(), "main" | "backup") {
        return Err(Error::InvalidInput(
            "share folders require a main or backup library".into(),
        ));
    }
    let selected = store.read_registered_directory(&location.root, relative)?;
    if let ReadDirectory::Native(selected) = &selected {
        // The existing root registry rejects native overlap with every private root.
        // Recheck the state directory too, including a moved/aliased database parent.
        let state = LibraryDirectory::open(
            store
                .database_path()
                .parent()
                .ok_or(Error::Internal("database parent is missing"))?,
        )?;
        if selected.native_root().overlaps(state.native_root()) {
            return Err(Error::InvalidInput(
                "share folder overlaps application state".into(),
            ));
        }
        store.check_private_ancestry(selected)?;
    }
    Ok(selected)
}

pub fn inspect_share_folder(
    store: &Store,
    request: &InspectShareFolder,
) -> Result<ShareFolderInspection> {
    let relative = LibraryPath::parse(&request.relative_path)?;
    let opened = selected_directory(store, request.location_id, &relative)?;
    let saved: Option<(i64, String)> = store
        .sharing_connection()
        .query_row(
            "SELECT id,native_body FROM share_bindings WHERE location_id=?1 AND relative_path=?2",
            params![request.location_id, relative.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let bound = match &saved {
        Some((_, body)) => serde_json::from_str::<DirectorySnapshot>(body)? == opened.snapshot(),
        None => false,
    };
    Ok(ShareFolderInspection {
        location_id: request.location_id,
        relative_path: relative.as_str().to_owned(),
        native_identity: opened.snapshot().identity()?,
        binding_id: saved.map(|(id, _)| id),
        bound,
    })
}

pub fn bind_share_folder(
    store: &Store,
    request: &BindShareFolder,
) -> Result<ShareFolderInspection> {
    let _guard = crate::infrastructure::shares::publication_visibility_write(store)?;
    let relative = LibraryPath::parse(&request.relative_path)?;
    let opened = selected_directory(store, request.location_id, &relative)?;
    if opened.snapshot().identity()? != request.expected_identity {
        return Err(Error::Conflict(
            "share folder changed; inspect it again".into(),
        ));
    }
    let saved: Option<String> = store
        .sharing_connection()
        .query_row(
            "SELECT native_body FROM share_bindings WHERE location_id=?1 AND relative_path=?2",
            params![request.location_id, relative.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    let body = serde_json::to_string(&opened.snapshot())?;
    if saved.as_ref().is_some_and(|saved| saved != &body) {
        return Err(Error::Conflict(
            "share folder binding changed; automatic rebinding is forbidden".into(),
        ));
    }
    store.sharing_connection().execute(
        "INSERT INTO share_bindings(location_id,relative_path,native_body) VALUES(?1,?2,?3)
         ON CONFLICT(location_id,relative_path) DO NOTHING",
        params![request.location_id, relative.as_str(), body],
    )?;
    inspect_share_folder(
        store,
        &InspectShareFolder {
            location_id: request.location_id,
            relative_path: request.relative_path.clone(),
        },
    )
}

pub fn share_bindings(store: &Store, page: Page) -> Result<Vec<ShareFolderInspection>> {
    let mut statement = store.sharing_connection().prepare(
        "SELECT id,location_id,relative_path,native_body FROM share_bindings WHERE id>?1 ORDER BY id LIMIT ?2",
    )?;
    let rows = statement
        .query_map(params![page.after(), page.limit()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, location_id, relative_path, body)| {
            let snapshot: DirectorySnapshot = serde_json::from_str(&body)?;
            let bound =
                selected_directory(store, location_id, &LibraryPath::parse(&relative_path)?)
                    .is_ok_and(|opened| opened.snapshot() == snapshot);
            Ok(ShareFolderInspection {
                location_id,
                relative_path,
                native_identity: snapshot.identity()?,
                binding_id: Some(id),
                bound,
            })
        })
        .collect()
}
