use super::Store;
use crate::infrastructure::filesystem::NativeRoot;
use crate::{Error, Result};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::path::Path;

impl Store {
    pub(crate) fn application_identity(&self) -> Result<String> {
        Ok(self.connection.query_row(
            "SELECT lower(hex(identity)) FROM application_identity WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?)
    }

    /// Reserve the path before any staging write. A failed filesystem setup
    /// leaves the reservation in place, so browsing cannot expose private data.
    pub(crate) fn reserve_staging_root(
        &mut self,
        root: &Path,
        native: &NativeRoot,
    ) -> Result<bool> {
        self.reserve_private_root(root, native, "staging")
    }

    pub(crate) fn reserve_cache_root(&mut self, root: &Path, native: &NativeRoot) -> Result<bool> {
        self.reserve_private_root(root, native, "cache")
    }

    fn reserve_private_root(
        &mut self,
        root: &Path,
        native: &NativeRoot,
        kind: &str,
    ) -> Result<bool> {
        let root_text = root
            .to_str()
            .ok_or_else(|| Error::InvalidInput("private root must be valid Unicode".into()))?;
        if self.path.starts_with(root) {
            return Err(Error::InvalidInput(
                "private roots must not contain the application database".into(),
            ));
        }
        self.check_state_overlap(native)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::native_roots::require_bound_roots(&transaction)?;
        let exists = transaction
            .query_row(
                "SELECT id FROM private_roots WHERE root = ?1 AND kind = ?2",
                params![root_text, kind],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        let mut statement = transaction
            .prepare("SELECT root, 0 FROM locations UNION ALL SELECT root, 1 FROM private_roots")?;
        for other in statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })? {
            let (other, reserved) = other?;
            if exists && reserved && other == root_text {
                continue;
            }
            if root.starts_with(&other) || Path::new(&other).starts_with(root) {
                return Err(Error::InvalidInput(
                    "media and private roots must not overlap".into(),
                ));
            }
        }
        drop(statement);
        super::native_roots::register(&transaction, root_text, kind, native)?;
        if !exists {
            transaction.execute(
                "INSERT INTO private_roots(root, kind) VALUES (?1, ?2)",
                params![root_text, kind],
            )?;
        }
        transaction.commit()?;
        Ok(!exists)
    }
}
