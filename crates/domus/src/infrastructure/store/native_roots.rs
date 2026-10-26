use super::Store;
use crate::infrastructure::filesystem::{LibraryDirectory, NativeRoot};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

fn conflict() -> Error {
    Error::Conflict("native root identity changed or needs explicit binding".into())
}

pub(super) fn require_bound_roots(connection: &Connection) -> Result<()> {
    let missing: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM
           (SELECT root FROM locations UNION ALL SELECT root FROM private_roots) r
           LEFT JOIN filesystem_roots n ON n.root = r.root WHERE n.root IS NULL)",
        [],
        |row| row.get(0),
    )?;
    if missing {
        return Err(conflict());
    }
    Ok(())
}

pub(super) fn register(
    connection: &Connection,
    root: &str,
    kind: &str,
    native: &NativeRoot,
) -> Result<()> {
    let existing: Option<(String, String)> = connection
        .query_row(
            "SELECT kind, body FROM filesystem_roots WHERE root = ?1",
            [root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((saved_kind, saved)) = existing {
        if saved_kind != kind || serde_json::from_str::<NativeRoot>(&saved)? != *native {
            return Err(conflict());
        }
        return Ok(());
    }
    let mut statement = connection.prepare("SELECT identity, body FROM filesystem_roots")?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (identity, body) = row?;
        let other: NativeRoot = serde_json::from_str(&body)?;
        if identity != other.identity || other.ancestors.last() != Some(&other.identity) {
            return Err(conflict());
        }
        if native.overlaps(&other) {
            return Err(Error::InvalidInput(
                "media and private roots overlap through native filesystem identity".into(),
            ));
        }
    }
    drop(statement);
    let body = serde_json::to_string(native)?;
    if body.len() > 1024 * 1024 {
        return Err(Error::InvalidInput("root ancestry exceeds 1 MiB".into()));
    }
    connection.execute(
        "INSERT INTO filesystem_roots(root,kind,identity,body) VALUES (?1,?2,?3,?4)",
        params![root, kind, native.identity, body],
    )?;
    Ok(())
}

impl Store {
    pub(crate) fn check_native_root(
        &self,
        root: &str,
        kind: &str,
        native: &NativeRoot,
    ) -> Result<()> {
        require_bound_roots(&self.connection)?;
        let saved: Option<String> = self
            .connection
            .query_row(
                "SELECT body FROM filesystem_roots WHERE root = ?1 AND kind = ?2",
                params![root, kind],
                |row| row.get(0),
            )
            .optional()?;
        if serde_json::from_str::<NativeRoot>(&saved.ok_or_else(conflict)?)? != *native {
            return Err(conflict());
        }
        Ok(())
    }

    /// Explicitly bind a legacy location only after the user has compared the
    /// identity of its actual no-follow handle with the displayed value.
    pub fn bind_location(&self, location: i64, expected: &str) -> Result<()> {
        let location = self.location(location)?;
        let (_, opened) = LibraryDirectory::canonical(Path::new(&location.root))?;
        if opened.native_root().identity != expected {
            return Err(conflict());
        }
        self.check_state_overlap(opened.native_root())?;
        let transaction = self.connection.unchecked_transaction()?;
        register(&transaction, &location.root, "media", opened.native_root())?;
        transaction.commit()?;
        Ok(())
    }

    pub fn bind_staging_root(&self, root: &Path, expected: &str) -> Result<()> {
        crate::infrastructure::filesystem::require_local_disk(root)?;
        let (path, opened) = LibraryDirectory::canonical(root)?;
        let root = path
            .to_str()
            .ok_or_else(|| Error::InvalidInput("staging root is not valid Unicode".into()))?;
        if opened.native_root().identity != expected {
            return Err(conflict());
        }
        self.check_state_overlap(opened.native_root())?;
        let native = opened.native_root().clone();
        let directory = opened.into_directory();
        crate::infrastructure::permissions::FilePermissions::current()?
            .check(&directory.try_clone()?.into_std_file(), true)?;
        let transaction = self.connection.unchecked_transaction()?;
        let registered: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM private_roots WHERE root = ?1 AND kind = 'staging')",
            [root],
            |row| row.get(0),
        )?;
        if !registered {
            return Err(conflict());
        }
        register(&transaction, root, "staging", &native)?;
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn check_state_overlap(&self, native: &NativeRoot) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Internal("database has no parent"))?;
        let (_, directory) = LibraryDirectory::canonical(parent)?;
        if directory.native_root().ancestors.contains(&native.identity) {
            return Err(Error::InvalidInput(
                "media or private root contains the application database through an alias".into(),
            ));
        }
        Ok(())
    }
}
