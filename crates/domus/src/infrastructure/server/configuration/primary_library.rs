use super::publication::PreparedConfigurationActivation;
use super::{ConfigurationService, MAXIMUM, digest};
use crate::domain::LibraryPath;
use crate::domain::library::PrimaryLibraryChange;
use crate::infrastructure::filesystem::{
    DirectoryExpectation, FilesystemClient, LibraryDirectory, ProviderMapping, RootRevision,
    StorageConfiguration, StorageRole, StorageRoot, TopologyAssurance,
};
use crate::infrastructure::store::{Store, StoreOpenMode};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LibraryChangeSelection {
    change_id: i64,
    root_id: String,
}

#[derive(Serialize, ts_rs::TS)]
pub struct PrimaryLibraryControl {
    pub available: bool,
    pub reason: Option<String>,
    pub pending_change_id: Option<i64>,
    pub pending_backup: bool,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct InspectPrimaryLibrary {
    pub path: String,
    #[ts(optional)]
    pub backup: Option<bool>,
}

#[derive(Serialize, ts_rs::TS)]
pub struct PrimaryLibraryStorageItem {
    pub role: String,
    pub path: String,
}

#[derive(Serialize, ts_rs::TS)]
pub struct PrimaryLibraryPreview {
    pub path: String,
    pub expected_revision: String,
    pub previous_location_id: Option<i64>,
    pub review_token: String,
    pub storage_layout: Vec<PrimaryLibraryStorageItem>,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct SelectPrimaryLibrary {
    pub path: String,
    pub expected_revision: String,
    pub previous_location_id: Option<i64>,
    pub review_token: String,
    pub confirm_separation: bool,
    #[ts(optional)]
    pub backup: Option<bool>,
}

/// Only storage selection is journalled. Secrets and the complete host document
/// remain in their operator-owned files.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrimaryLibraryDraft {
    path: String,
    root_id: String,
    storage: StorageConfiguration,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    backup: bool,
}

impl ConfigurationService {
    pub(crate) fn primary_library_configuration_path(&self) -> &Path {
        &self.path
    }

    /// Retain the checked native state directory, but not a SQLite selection
    /// context: a supervisor must release the old fence during approved handover.
    pub(crate) fn supervisor_lock(&self) -> Result<crate::infrastructure::local_file::ProcessLock> {
        let parent = self
            .active
            .database
            .parent()
            .ok_or(Error::Internal("database parent is missing"))?;
        let directory = LibraryDirectory::open(parent)?;
        if directory.native_root() != self.state_storage.directory().native_root() {
            return Err(Error::Conflict(
                "The selected state directory changed.".into(),
            ));
        }
        crate::infrastructure::local_file::ProcessLock::acquire_in(
            &directory,
            "developer-supervisor",
        )
    }

    pub(crate) fn inspect_primary_library(
        &self,
        path: &str,
        backup: bool,
    ) -> Result<PrimaryLibraryPreview> {
        self.primary_library_candidate(path, backup)
            .map(|(preview, _)| preview)
    }

    pub(crate) fn approve_primary_library(
        &self,
        request: SelectPrimaryLibrary,
    ) -> Result<PrimaryLibraryChange> {
        if !request.confirm_separation {
            return Err(Error::InvalidInput(
                "Confirm the folder separation before changing library folders.".into(),
            ));
        }
        let (preview, draft) =
            self.primary_library_candidate(&request.path, request.backup.unwrap_or(false))?;
        if request.expected_revision != preview.expected_revision
            || request.previous_location_id != preview.previous_location_id
            || request.review_token != preview.review_token
        {
            return Err(Error::Conflict(
                "The folder or configuration changed. Check the location and review it again."
                    .into(),
            ));
        }
        self.require_editable()?;
        self.require_revision(
            &request.expected_revision,
            &self.read_file(&self.directory()?)?.1,
        )?;
        self.open_store(StoreOpenMode::Existing)?
            .approve_primary_library_change(
                &serde_json::to_string(&draft)?,
                &request.expected_revision,
                preview.previous_location_id,
            )
    }

    fn primary_library_candidate(
        &self,
        path: &str,
        backup: bool,
    ) -> Result<(PrimaryLibraryPreview, PrimaryLibraryDraft)> {
        self.require_editable()?;
        if self.active.paired_media.is_some() {
            return Err(Error::Conflict(
                "This host uses paired media. Change its selected roots through the native adapter configuration procedure.".into(),
            ));
        }
        super::folder_browser::validate_folder_path(Path::new(path))?;
        let (_, bytes) = self.read_file(&self.directory()?)?;
        self.require_revision(self.active_revision(), &bytes)?;
        let store = self.open_store(StoreOpenMode::ReadCurrent)?;
        store.require_primary_library_idle()?;
        let previous = store.primary_library_selection()?;
        if backup && previous.is_none() {
            return Err(Error::Conflict(
                "Set a primary library before adding a backup.".into(),
            ));
        }
        let mut storage = self
            .active
            .storage
            .clone()
            .ok_or(Error::Internal("selected storage is missing"))?;
        let (expected, opened) = DirectoryExpectation::inspect(
            Path::new(path),
            &LibraryPath::parse("")?,
        )
        .map_err(|error| match error {
            Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => Error::InvalidInput(
                "This folder does not exist. Create it first, or pick an existing folder.".into(),
            ),
            Error::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied => {
                Error::InvalidInput(
                    "Domus cannot open this folder. Check that its account can read it.".into(),
                )
            }
            other => other,
        })?;
        use crate::infrastructure::filesystem::{FilesystemClass, filesystem_class};
        let scan_only = match filesystem_class(&opened.directory().directory_file()?)? {
            FilesystemClass::Local => false,
            FilesystemClass::Remote => true,
            FilesystemClass::Unknown => {
                return Err(Error::Conflict(
                    "The host does not recognize this filesystem for library use.".into(),
                ));
            }
        };
        let identity = expected
            .components
            .last()
            .ok_or(Error::Internal("candidate identity missing"))?;
        let selected = storage.roots.iter().find(|root| {
            root.role == StorageRole::Media && root.expected.components.last() == Some(identity)
        });
        let root_id = if let Some(selected) = selected {
            if selected.scan_only != scan_only {
                return Err(Error::Conflict(
                    "The folder's existing binding does not match its scan and write permissions."
                        .into(),
                ));
            }
            selected.root_id.clone()
        } else {
            let id = format!("library-{}", &digest(identity.as_bytes())[..16]);
            if storage.roots.iter().any(|root| root.root_id == id)
                || storage
                    .providers
                    .iter()
                    .any(|provider| provider.scope == id)
            {
                return Err(Error::Conflict(
                    "This folder conflicts with a retained storage identity.".into(),
                ));
            }
            let root = StorageRoot {
                root_id: id.clone(),
                binding_revision: 1,
                role: StorageRole::Media,
                scan_only,
                provider_scope: id.clone(),
                relative: String::new(),
                expected,
            };
            storage.providers.push(ProviderMapping {
                scope: id.clone(),
                boundary: path.into(),
            });
            storage.roots.push(root);
            id
        };
        let key = format!("storage-root:{root_id}");
        let existing = store.primary_library_target(&key)?;
        if backup && existing.is_some() {
            return Err(Error::Conflict(
                "This folder already has a current or previous library location.".into(),
            ));
        }
        if previous.is_some() && existing == previous {
            return Err(Error::Conflict(
                "This folder is already the primary library.".into(),
            ));
        }
        storage.selection_revision =
            storage.selection_revision.checked_add(1).ok_or_else(|| {
                Error::Conflict("The storage selection revision is exhausted.".into())
            })?;
        let selection_digest = storage.selection_digest()?;
        for assurance in &mut storage.topology_assurances {
            assurance.selection_digest = selection_digest.clone();
        }
        let selected = storage
            .roots
            .iter()
            .find(|root| root.root_id == root_id)
            .ok_or(Error::Internal("candidate root is missing"))?;
        for other in storage.roots.iter().filter(|root| root.root_id != root_id) {
            let (left, right) = if selected.root_id < other.root_id {
                (selected, other)
            } else {
                (other, selected)
            };
            if !storage.topology_assurances.iter().any(|assurance| {
                assurance.left.root_id == left.root_id && assurance.right.root_id == right.root_id
            }) {
                storage.topology_assurances.push(TopologyAssurance {
                    left: RootRevision {
                        root_id: left.root_id.clone(),
                        binding_revision: left.binding_revision,
                    },
                    right: RootRevision {
                        root_id: right.root_id.clone(),
                        binding_revision: right.binding_revision,
                    },
                    selection_digest: selection_digest.clone(),
                });
            }
        }
        let client = FilesystemClient::new(storage.clone(), self.active_revision().into())?;
        // These are inspection leases, not persisted selection or media approval.
        let _candidate = client.open(&root_id, StorageRole::Media)?;
        let _application = client.open_application(
            self.path
                .parent()
                .ok_or(Error::Internal("configuration parent is missing"))?,
        )?;
        let _state = client.open_state(&self.active.database)?;
        let draft = PrimaryLibraryDraft {
            path: path.into(),
            root_id,
            storage,
            backup,
        };
        let body = serde_json::to_vec(&draft)?;
        if body.len() as u64 > MAXIMUM {
            return Err(Error::InvalidInput(
                "The storage selection exceeds 16 KiB.".into(),
            ));
        }
        let review_token = digest(&serde_json::to_vec(&(
            self.active_revision(),
            previous,
            &draft,
        ))?);
        let mut storage_layout = Vec::new();
        for root in &draft.storage.roots {
            storage_layout.push(PrimaryLibraryStorageItem {
                role: if root.root_id == draft.root_id {
                    let role = if backup {
                        "New backup library"
                    } else {
                        "New primary library"
                    };
                    if root.scan_only {
                        format!("{role} (scan only)")
                    } else {
                        role.into()
                    }
                } else {
                    match root.role {
                        StorageRole::Media => "Other selected media folder",
                        StorageRole::Application => "Application storage",
                        StorageRole::State => "Database storage",
                        StorageRole::Cache => "Cache storage",
                        StorageRole::Staging => "Download staging",
                        StorageRole::Certificate => "Certificate storage",
                    }
                    .into()
                },
                path: client
                    .root_path(&root.root_id, root.role)?
                    .to_string_lossy()
                    .into(),
            });
        }
        self.require_revision(
            self.active_revision(),
            &self.read_file(&self.directory()?)?.1,
        )?;
        Ok((
            PrimaryLibraryPreview {
                path: draft.path.clone(),
                expected_revision: self.active_revision().into(),
                previous_location_id: previous,
                review_token,
                storage_layout,
            },
            draft,
        ))
    }

    /// The caller has drained every managed child. Pre-publication failure can
    /// restart the unchanged host; uncertain publication never rolls back.
    pub(crate) fn activate_stopped_primary(self, id: i64) -> Result<Self> {
        let prepared = match self.prepare_primary_activation(id) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.open_store(StoreOpenMode::Existing)?
                    .fail_primary_library_change(id, &error.to_string())?;
                eprintln!("Library folder change rejected: {error}");
                return Ok(self);
            }
        };
        let path = self.path.clone();
        drop(self);
        let candidate = match prepared.validate().and_then(|candidate| {
            let selected = candidate
                .active
                .library_change
                .as_ref()
                .ok_or(Error::Internal("library change selection missing"))?;
            let _root = candidate
                .state_storage
                .client()
                .open(&selected.root_id, StorageRole::Media)?;
            let store = candidate.open_store(StoreOpenMode::ReadCurrent)?;
            let (body, _, _) = store.primary_library_change_intent(id)?;
            let draft: PrimaryLibraryDraft = serde_json::from_str(&body)?;
            let existing =
                store.primary_library_target(&format!("storage-root:{}", selected.root_id))?;
            if draft.backup && existing.is_some() {
                return Err(Error::Conflict(
                    "This folder already has a library location.".into(),
                ));
            }
            drop(store);
            Ok(candidate)
        }) {
            Ok(candidate) => candidate,
            Err(error) => {
                let unchanged = Self::open(&path)?;
                unchanged
                    .open_store(StoreOpenMode::Existing)?
                    .fail_primary_library_change(id, &error.to_string())?;
                eprintln!(
                    "Library folder change rejected before configuration publication: {error}"
                );
                return Ok(unchanged);
            }
        };
        prepared.publish(&candidate)?;
        let mut store = candidate.open_store(StoreOpenMode::Existing)?;
        candidate.activate_configured_primary(&mut store)?;
        Ok(candidate)
    }

    pub(super) fn prepare_primary_activation(
        &self,
        id: i64,
    ) -> Result<PreparedConfigurationActivation> {
        self.require_editable()?;
        let (_, bytes) = self.read_file(&self.directory()?)?;
        let mut store = self.open_store(StoreOpenMode::Existing)?;
        let (body, expected, _) = store.primary_library_change_intent(id)?;
        self.require_revision(&expected, &bytes)?;
        let draft: PrimaryLibraryDraft = serde_json::from_str(&body)?;
        let mut document: serde_json::Value = serde_json::from_slice(&bytes)?;
        document["storage"] = serde_json::to_value(&draft.storage)?;
        document
            .as_object_mut()
            .ok_or(Error::Internal("configuration is not an object"))?
            .remove("primary_library");
        document["library_change"] = serde_json::to_value(LibraryChangeSelection {
            change_id: id,
            root_id: draft.root_id,
        })?;
        let mut candidate = serde_json::to_vec_pretty(&document)?;
        candidate.push(b'\n');
        if candidate.len() as u64 > MAXIMUM {
            return Err(Error::InvalidInput(
                "The host configuration exceeds 16 KiB.".into(),
            ));
        }
        let directory = LibraryDirectory::open(
            self.path
                .parent()
                .ok_or(Error::Internal("configuration parent missing"))?,
        )?;
        if directory.native_root() != &self.parent {
            return Err(Error::Conflict(
                "The configuration directory changed.".into(),
            ));
        }
        store.begin_primary_library_change(id)?;
        Ok(PreparedConfigurationActivation {
            path: self.path.clone(),
            bytes: candidate,
            expected,
            directory,
            pending: format!(".domus-primary-{id}.pending"),
        })
    }

    /// Startup may finish only the exact published, approved configuration.
    /// Ordinary reads and validation never call this operation.
    pub(crate) fn activate_configured_primary(&self, store: &mut Store) -> Result<()> {
        if let Some(selection) = &self.active.library_change {
            let (body, _, _) = store.primary_library_change_intent(selection.change_id)?;
            let draft: PrimaryLibraryDraft = serde_json::from_str(&body)?;
            let change = store.primary_library_change(selection.change_id)?;
            if draft.root_id != selection.root_id
                || (change.state != "active"
                    && self.active.storage.as_ref() != Some(&draft.storage))
            {
                return Err(Error::Conflict(
                    "The library folder configuration differs from its approval.".into(),
                ));
            }
            store.activate_primary_library(
                selection.change_id,
                &selection.root_id,
                self.active_revision(),
                draft.backup,
            )?;
        }
        Ok(())
    }

    pub(crate) fn require_configured_primary(&self, store: &Store) -> Result<()> {
        if let Some(selection) = &self.active.library_change {
            let change = store.primary_library_change(selection.change_id)?;
            let (body, _, previous) = store.primary_library_change_intent(selection.change_id)?;
            let draft: PrimaryLibraryDraft = serde_json::from_str(&body)?;
            if change.state != "active"
                || draft.root_id != selection.root_id
                || !store.library_activation_matches(
                    &selection.root_id,
                    change.location_id,
                    previous,
                    draft.backup,
                )?
            {
                return Err(Error::Conflict(
                    "Start the API and finish library folder activation before starting the worker."
                        .into(),
                ));
            }
        }
        Ok(())
    }
}
