use super::Store;
use crate::infrastructure::filesystem::media_adapter::{MediaBinding, SelectedMedia};
use crate::infrastructure::filesystem::{
    FilesystemClient, LibraryDirectory, LogicalRootBinding, NativeRoot, StorageRole,
};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

fn conflict() -> Error {
    Error::Conflict("native root identity changed or needs explicit binding".into())
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub(super) enum SavedRoot {
    Native(NativeRoot),
    Logical(LogicalRootBinding),
    Paired(MediaBinding),
}

impl SavedRoot {
    pub(super) fn key(&self, native_path: &str) -> String {
        match self {
            Self::Native(_) => native_path.into(),
            Self::Logical(binding) => binding.key(),
            Self::Paired(binding) => binding.key(),
        }
    }

    fn identity(&self) -> Result<String> {
        match self {
            Self::Native(native) if native.ancestors.last() == Some(&native.identity) => {
                Ok(native.identity.clone())
            }
            Self::Logical(binding) => binding.identity(),
            Self::Paired(binding) => binding.identity(),
            _ => Err(conflict()),
        }
    }

    fn read(identity: &str, body: &str) -> Result<Self> {
        if body.len() > 1024 * 1024 {
            return Err(Error::InvalidInput("root binding exceeds 1 MiB".into()));
        }
        let saved: Self = serde_json::from_str(body)?;
        if saved.identity()? != identity {
            return Err(conflict());
        }
        Ok(saved)
    }
}

fn role(kind: &str) -> Result<StorageRole> {
    match kind {
        "media" => Ok(StorageRole::Media),
        "staging" => Ok(StorageRole::Staging),
        "cache" => Ok(StorageRole::Cache),
        "certificate" => Ok(StorageRole::Certificate),
        _ => Err(Error::InvalidInput(
            "unsupported registered root role".into(),
        )),
    }
}

fn inspect_saved_roots(
    connection: &Connection,
    mut inspect: impl FnMut(&str, &str, &SavedRoot) -> Result<()>,
) -> Result<()> {
    let mut statement =
        connection.prepare("SELECT root, kind, identity, body FROM filesystem_roots")?;
    for (count, row) in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .enumerate()
    {
        if count >= 4096 {
            return Err(Error::InvalidInput(
                "root inventory exceeds 4096 roots".into(),
            ));
        }
        let (path, kind, identity, body) = row?;
        inspect(&path, &kind, &SavedRoot::read(&identity, &body)?)?;
    }
    Ok(())
}

fn inspect_roots(
    connection: &Connection,
    mut inspect: impl FnMut(&str, &str, &NativeRoot) -> Result<()>,
) -> Result<()> {
    inspect_saved_roots(connection, |path, kind, saved| match saved {
        SavedRoot::Native(native) => inspect(path, kind, native),
        SavedRoot::Logical(_) | SavedRoot::Paired(_) => Err(conflict()),
    })
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
    saved: &SavedRoot,
    client: Option<&FilesystemClient>,
    media: Option<&SelectedMedia>,
) -> Result<()> {
    require_saved(client, media, kind, saved)?;
    inspect_saved_roots(connection, |path, other_kind, other| {
        require_saved(client, media, other_kind, other)?;
        if path != root
            && let (SavedRoot::Native(native), SavedRoot::Native(other)) = (saved, other)
            && native.overlaps(other)
        {
            return Err(Error::InvalidInput(
                "media and private roots overlap through native filesystem identity".into(),
            ));
        }
        Ok(())
    })?;
    let existing: Option<(String, String, String)> = connection
        .query_row(
            "SELECT kind, identity, body FROM filesystem_roots WHERE root = ?1",
            [root],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((saved_kind, identity, body)) = existing {
        if saved_kind != kind || SavedRoot::read(&identity, &body)? != *saved {
            return Err(conflict());
        }
        return Ok(());
    }
    let body = serde_json::to_string(saved)?;
    if body.len() > 1024 * 1024 {
        return Err(Error::InvalidInput("root ancestry exceeds 1 MiB".into()));
    }
    connection.execute(
        "INSERT INTO filesystem_roots(root,kind,identity,body) VALUES (?1,?2,?3,?4)",
        params![root, kind, saved.identity()?, body],
    )?;
    Ok(())
}

fn require_saved(
    client: Option<&FilesystemClient>,
    media: Option<&SelectedMedia>,
    kind: &str,
    saved: &SavedRoot,
) -> Result<()> {
    match (client, saved) {
        (Some(client), SavedRoot::Logical(binding)) => client.require_binding(binding, role(kind)?),
        (None, SavedRoot::Native(_)) => Ok(()),
        (_, SavedRoot::Paired(binding)) if kind == "media" => {
            media.ok_or_else(conflict)?.require_binding(binding)
        }
        _ => Err(conflict()),
    }
}

impl Store {
    pub(crate) fn read_registered_directory(
        &self,
        root: &str,
        relative: &crate::domain::LibraryPath,
    ) -> Result<crate::infrastructure::filesystem::ReadDirectory> {
        use crate::infrastructure::filesystem::ReadDirectory;
        match self.saved_root(root, "media")? {
            SavedRoot::Paired(binding) => {
                if !self.connection.is_autocommit() {
                    return Err(Error::Conflict(
                        "media network I/O cannot run inside a database transaction".into(),
                    ));
                }
                ReadDirectory::paired(
                    self.paired_media.clone().ok_or_else(conflict)?,
                    binding,
                    relative,
                )
            }
            _ => Ok(ReadDirectory::Native(Box::new(
                self.open_registered_root(root, "media")?
                    .open_directory(relative)?,
            ))),
        }
    }

    pub(super) fn require_paired_media_bindings(&self) -> Result<()> {
        // Attaching a provider must not turn passive legacy metadata reads into
        // native-root admission. Native effects retain their existing checks.
        inspect_saved_roots(&self.connection, |_, kind, saved| {
            if matches!(saved, SavedRoot::Paired(_)) {
                require_saved(
                    self.filesystem.as_deref(),
                    self.paired_media.as_deref(),
                    kind,
                    saved,
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn require_current_root_bindings(&self) -> Result<()> {
        require_bound_roots(&self.connection)?;
        inspect_saved_roots(&self.connection, |_, kind, saved| {
            require_saved(
                self.filesystem.as_deref(),
                self.paired_media.as_deref(),
                kind,
                saved,
            )
        })
    }

    pub(super) fn prepare_root_binding(
        &self,
        directory: &LibraryDirectory,
        kind: &str,
    ) -> Result<SavedRoot> {
        match &self.filesystem {
            Some(client) => Ok(SavedRoot::Logical(
                client.binding_for_directory(directory, role(kind)?)?,
            )),
            None => Ok(SavedRoot::Native(directory.native_root().clone())),
        }
    }

    fn saved_root(&self, root: &str, kind: &str) -> Result<SavedRoot> {
        require_bound_roots(&self.connection)?;
        let (identity, body): (String, String) = self
            .connection
            .query_row(
                "SELECT identity, body FROM filesystem_roots WHERE root = ?1 AND kind = ?2",
                params![root, kind],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(conflict)?;
        let saved = SavedRoot::read(&identity, &body)?;
        require_saved(
            self.filesystem.as_deref(),
            self.paired_media.as_deref(),
            kind,
            &saved,
        )?;
        Ok(saved)
    }

    /// Resolve an immutable database key through the current selected mapping.
    /// Keep the returned directory alive through all filesystem work.
    pub(crate) fn open_registered_root(&self, root: &str, kind: &str) -> Result<LibraryDirectory> {
        match self.saved_root(root, kind)? {
            SavedRoot::Logical(binding) => Ok(self
                .source()
                .filesystem()?
                .open_binding(&binding, role(kind)?)?
                .into_directory()?),
            SavedRoot::Native(native) => {
                let directory = LibraryDirectory::open(Path::new(root))?;
                if directory.native_root() != &native {
                    return Err(conflict());
                }
                Ok(directory)
            }
            SavedRoot::Paired(_) => Err(Error::Conflict(
                "paired media cannot be opened through a native filesystem path".into(),
            )),
        }
    }

    pub(crate) fn check_registered_media(&self, root: &str) -> Result<()> {
        match self.saved_root(root, "media")? {
            SavedRoot::Paired(binding) => {
                if !self.connection.is_autocommit() {
                    return Err(Error::Conflict(
                        "media network I/O cannot run inside a database transaction".into(),
                    ));
                }
                self.paired_media
                    .as_ref()
                    .ok_or_else(conflict)?
                    .check(&binding)
            }
            _ => self.open_registered_root(root, "media").map(|_| ()),
        }
    }

    pub(crate) fn paired_media_session(
        &self,
        root: &str,
    ) -> Result<Option<crate::infrastructure::filesystem::media_adapter::BlockingMedia>> {
        match self.saved_root(root, "media")? {
            SavedRoot::Paired(binding) => {
                if !self.connection.is_autocommit() {
                    return Err(Error::Conflict(
                        "media network I/O cannot run inside a database transaction".into(),
                    ));
                }
                Ok(Some(
                    self.paired_media
                        .as_ref()
                        .ok_or_else(conflict)?
                        .open(&binding)?,
                ))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn selected_media_binding(&self, root_id: &str) -> Result<SavedRoot> {
        if !self.connection.is_autocommit() {
            return Err(Error::Conflict(
                "media network I/O cannot run inside a database transaction".into(),
            ));
        }
        let media = self.paired_media.as_ref().ok_or_else(conflict)?;
        let binding = media.binding(root_id)?;
        media.check(&binding)?;
        Ok(SavedRoot::Paired(binding))
    }

    pub(crate) fn root_binding_identity(&self, root: &str, kind: &str) -> Result<String> {
        self.saved_root(root, kind)?.identity()
    }

    /// A reported locator is not authority. Consumers must reopen it through
    /// selected admission before using it.
    pub(crate) fn registered_root_path(
        &self,
        root: &str,
        kind: &str,
    ) -> Result<std::path::PathBuf> {
        match self.saved_root(root, kind)? {
            SavedRoot::Native(_) => Ok(root.into()),
            SavedRoot::Logical(binding) => self
                .source()
                .filesystem()?
                .binding_path(&binding, role(kind)?),
            SavedRoot::Paired(_) => Err(Error::Conflict(
                "paired media has no local native path".into(),
            )),
        }
    }

    pub(crate) fn root_key_for_directory(
        &self,
        path: &Path,
        directory: &LibraryDirectory,
        kind: &str,
    ) -> Result<String> {
        let native_path = path
            .to_str()
            .ok_or_else(|| Error::InvalidInput("root reference must be Unicode".into()))?;
        Ok(self.prepare_root_binding(directory, kind)?.key(native_path))
    }

    pub(crate) fn open_candidate_root(
        &self,
        path: &Path,
        kind: &str,
    ) -> Result<(std::path::PathBuf, LibraryDirectory)> {
        let (path, directory) = LibraryDirectory::canonical(path)?;
        match self.prepare_root_binding(&directory, kind)? {
            SavedRoot::Native(_) => Ok((path, directory)),
            SavedRoot::Logical(binding) => Ok((
                path,
                self.source()
                    .filesystem()?
                    .open_binding(&binding, role(kind)?)?
                    .into_directory()?,
            )),
            SavedRoot::Paired(_) => Err(conflict()),
        }
    }

    pub(crate) fn check_private_ancestry(&self, directory: &LibraryDirectory) -> Result<()> {
        if let Some(client) = &self.filesystem {
            client.require_media_component(&directory.directory_file()?)?;
        }
        inspect_saved_roots(&self.connection, |_, kind, saved| {
            require_saved(
                self.filesystem.as_deref(),
                self.paired_media.as_deref(),
                kind,
                saved,
            )?;
            if kind != "media"
                && let SavedRoot::Native(native) = saved
                && directory.native_root().overlaps(native)
            {
                return Err(Error::InvalidInput(
                    "media traversal intersects a private root".into(),
                ));
            }
            Ok(())
        })
    }

    /// Passive separation uses saved ancestry, so an offline library is not
    /// opened or treated as absent. Only the final certificate directory may
    /// be missing; its existing parent remains the no-follow identity anchor.
    pub(crate) fn inspect_certificate_root(&self, path: &Path) -> Result<()> {
        require_bound_roots(&self.connection)?;
        if self.filesystem.is_some() {
            self.open_candidate_root(path, "certificate")?;
            return Ok(());
        }
        let candidate = super::super::certificates::StateDirectory::inspect(path)?;
        inspect_roots(&self.connection, |root, kind, native| {
            if kind == "certificate"
                && candidate.exists
                && candidate.path == Path::new(root)
                && candidate.native == *native
            {
                return Ok(());
            }
            if candidate.overlaps(Path::new(root), native) {
                return Err(Error::InvalidInput(
                    "certificate state overlaps a registered root".into(),
                ));
            }
            Ok(())
        })
    }

    /// Explicit bootstrap records only native root identity, never certificate
    /// or account secrets. Future root registrations reuse the same exclusion.
    pub(crate) fn bind_certificate_root(&self, path: &Path, native: &NativeRoot) -> Result<()> {
        require_bound_roots(&self.connection)?;
        self.check_state_overlap(native)?;
        let root = path
            .to_str()
            .ok_or_else(|| Error::InvalidInput("certificate root must be Unicode".into()))?;
        let (_, directory) = self.open_candidate_root(path, "certificate")?;
        if directory.native_root() != native {
            return Err(conflict());
        }
        let binding = self.prepare_root_binding(&directory, "certificate")?;
        let root = binding.key(root);
        let transaction = self.connection.unchecked_transaction()?;
        register(
            &transaction,
            &root,
            "certificate",
            &binding,
            self.filesystem.as_deref(),
            self.paired_media.as_deref(),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn check_native_root(
        &self,
        root: &str,
        kind: &str,
        native: &NativeRoot,
    ) -> Result<()> {
        let key = if self.filesystem.is_some() && Path::new(root).is_absolute() {
            let directory = LibraryDirectory::open(Path::new(root))?;
            if directory.native_root() != native {
                return Err(conflict());
            }
            self.root_key_for_directory(Path::new(root), &directory, kind)?
        } else {
            root.into()
        };
        if self.open_registered_root(&key, kind)?.native_root() != native {
            return Err(conflict());
        }
        Ok(())
    }

    /// Explicitly bind a legacy location only after the user has compared the
    /// identity of its actual no-follow handle with the displayed value.
    pub fn bind_location(&self, location: i64, expected: &str) -> Result<()> {
        let location = self.location(location)?;
        if self.filesystem.is_some() {
            let directory = self.open_registered_root(&location.root, "media")?;
            return if self.root_binding_identity(&location.root, "media")? == expected
                || directory.native_root().identity == expected
            {
                Ok(())
            } else {
                Err(conflict())
            };
        }
        let (_, opened) = LibraryDirectory::canonical(Path::new(&location.root))?;
        if opened.native_root().identity != expected {
            return Err(conflict());
        }
        self.check_state_overlap(opened.native_root())?;
        let binding = self.prepare_root_binding(&opened, "media")?;
        let transaction = self.connection.unchecked_transaction()?;
        register(
            &transaction,
            &location.root,
            "media",
            &binding,
            self.filesystem.as_deref(),
            self.paired_media.as_deref(),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn bind_staging_root(&self, root: &Path, expected: &str) -> Result<()> {
        crate::infrastructure::filesystem::require_local_disk(root)?;
        let (path, opened) = self.open_candidate_root(root, "staging")?;
        let root = path
            .to_str()
            .ok_or_else(|| Error::InvalidInput("staging root is not valid Unicode".into()))?;
        if opened.native_root().identity != expected {
            return Err(conflict());
        }
        self.check_state_overlap(opened.native_root())?;
        let directory = opened;
        crate::infrastructure::permissions::FilePermissions::current()?
            .check(&directory.directory_file()?, true)?;
        let binding = self.prepare_root_binding(&directory, "staging")?;
        let root = binding.key(root);
        let transaction = self.connection.unchecked_transaction()?;
        let registered: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM private_roots WHERE root = ?1 AND kind = 'staging')",
            [&root],
            |row| row.get(0),
        )?;
        if !registered {
            return Err(conflict());
        }
        register(
            &transaction,
            &root,
            "staging",
            &binding,
            self.filesystem.as_deref(),
            self.paired_media.as_deref(),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn check_state_overlap(&self, native: &NativeRoot) -> Result<()> {
        if self
            .directory
            .native_root()
            .ancestors
            .contains(&native.identity)
        {
            return Err(Error::InvalidInput(
                "media or private root contains the application database through an alias".into(),
            ));
        }
        Ok(())
    }
}
