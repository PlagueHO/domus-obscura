use super::{DirectoryExpectation, RetainedDirectory};
use crate::domain::LibraryPath;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type StateContext = (
    native_sqlite::Context,
    PathBuf,
    Arc<super::LibraryDirectory>,
);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageRole {
    Application,
    State,
    Cache,
    Staging,
    Certificate,
    Media,
}

/// An operator-selected namespace view, not a physical device declaration.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderMapping {
    pub scope: String,
    pub boundary: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorageRoot {
    pub root_id: String,
    pub binding_revision: u64,
    pub role: StorageRole,
    pub provider_scope: String,
    pub relative: String,
    pub expected: DirectoryExpectation,
}

/// Immutable saved root meaning, without container-local paths or ancestors
/// above the selected boundary. A record is not filesystem authority: reopen
/// it through the current client and retain the returned lease for each use.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalRootBinding {
    format_version: u32,
    root: StorageRoot,
}

impl LogicalRootBinding {
    pub(crate) fn key(&self) -> String {
        format!("storage-root:{}", self.root.root_id)
    }

    /// Stable record identity, not proof of an opened directory or approval.
    pub fn identity(&self) -> Result<String> {
        if self.format_version != 1 {
            return Err(Error::Conflict("unsupported saved root binding".into()));
        }
        let mut hash = Sha256::new();
        hash.update(b"domus-logical-root-v1\0");
        hash.update(serde_json::to_vec(self)?);
        Ok(format!("logical-v1:{:x}", hash.finalize()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogicalDirectorySnapshot {
    format_version: u32,
    root: LogicalRootBinding,
    relative: String,
    expected: DirectoryExpectation,
}

/// Saved evidence has no live handles. Only a new admitted traversal can
/// produce current evidence for comparison with this record.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum DirectorySnapshot {
    Native(super::NativeRoot),
    Logical(LogicalDirectorySnapshot),
}

impl DirectorySnapshot {
    pub(crate) fn identity(&self) -> Result<String> {
        let prefix = match self {
            Self::Native(native) => native.identity.as_str(),
            Self::Logical(snapshot) if snapshot.format_version == 1 => "logical-directory-v1",
            _ => {
                return Err(Error::Conflict(
                    "unsupported saved directory evidence".into(),
                ));
            }
        };
        Ok(format!(
            "{prefix}:{:x}",
            Sha256::digest(serde_json::to_vec(self)?)
        ))
    }
}

#[derive(Clone)]
pub(super) struct DirectorySelection {
    pub(super) client: Arc<FilesystemClient>,
    pub(super) role: StorageRole,
    snapshot: LogicalDirectorySnapshot,
}

impl DirectorySelection {
    pub(super) fn descend(&mut self, component: &str, file: &std::fs::File) -> Result<()> {
        let identity = super::binding::persistent_file_identity(file)?;
        self.client.require_component(self.role, &identity)?;
        let relative = if self.snapshot.relative.is_empty() {
            component.to_owned()
        } else {
            format!("{}/{component}", self.snapshot.relative)
        };
        self.snapshot.relative = LibraryPath::parse(&relative)?.as_str().into();
        self.snapshot.expected.components.push(identity);
        Ok(())
    }

    pub(super) fn snapshot(&self) -> DirectorySnapshot {
        DirectorySnapshot::Logical(self.snapshot.clone())
    }

    pub(super) fn reopen(&self) -> Result<super::LibraryDirectory> {
        let directory = self
            .client
            .open_binding(&self.snapshot.root, self.role)?
            .into_directory()?
            .open_directory(&LibraryPath::parse(&self.snapshot.relative)?)?;
        if directory.snapshot() != self.snapshot() {
            return Err(Error::Conflict(
                "selected directory evidence changed".into(),
            ));
        }
        Ok(directory)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootRevision {
    pub root_id: String,
    pub binding_revision: u64,
}

/// Trusted administrative input for one otherwise opaque relationship.
/// A matching assurance never overrides a known overlap or changed binding.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TopologyAssurance {
    pub left: RootRevision,
    pub right: RootRevision,
    pub selection_digest: String,
}

/// Host configuration owns the complete selection and its native expectations.
/// Database registration and media-write approval remain separate operations.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfiguration {
    pub format_version: u32,
    pub selection_revision: u64,
    pub providers: Vec<ProviderMapping>,
    pub roots: Vec<StorageRoot>,
    #[serde(default)]
    pub expected_selection_file: Option<String>,
    #[serde(default)]
    pub topology_assurances: Vec<TopologyAssurance>,
}

/// Immutable role and relationship policy. This client contains no SQL.
pub struct FilesystemClient {
    selection: StorageConfiguration,
    selection_digest: String,
    fence_digest: String,
    source_digest: String,
    opened_roots: Mutex<BTreeMap<String, (StorageRole, Arc<super::LibraryDirectory>)>>,
    state_context: Mutex<Option<StateContext>>,
}

/// Keep both the frozen selection and its actual no-follow authority alive.
pub struct StorageLease {
    client: Arc<FilesystemClient>,
    root_id: String,
    retained: RetainedDirectory,
    role: StorageRole,
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

impl StorageRoot {
    /// Inspect an existing candidate without saving or approving its selection.
    /// Operators must review the complete layout before changing stopped hosts.
    pub fn inspect(
        root_id: &str,
        binding_revision: u64,
        role: StorageRole,
        provider_scope: &str,
        boundary: &Path,
        relative: &LibraryPath,
    ) -> Result<Self> {
        if !name(root_id) || !name(provider_scope) || binding_revision == 0 {
            return Err(Error::InvalidInput(
                "storage root names and revision are invalid".into(),
            ));
        }
        let (expected, retained) = DirectoryExpectation::inspect(boundary, relative)?;
        if role != StorageRole::Media {
            retained.directory.require_local()?;
            super::super::permissions::FilePermissions::current()?
                .check(&retained.directory.directory_file()?, true)?;
        }
        Ok(Self {
            root_id: root_id.into(),
            binding_revision,
            role,
            provider_scope: provider_scope.into(),
            relative: relative.as_str().into(),
            expected,
        })
    }
}

impl StorageConfiguration {
    /// Inspect candidates for an explicitly selected initial private layout.
    /// This does not save configuration, adopt state or grant media approval.
    pub fn inspect_initial(application: &Path, state: &LibraryPath) -> Result<Self> {
        let directory = super::LibraryDirectory::open(application)?;
        Self::inspect_initial_in(&directory, application, state)
    }

    pub(crate) fn inspect_initial_in(
        directory: &super::LibraryDirectory,
        boundary: &Path,
        state: &LibraryPath,
    ) -> Result<Self> {
        let mut roots = Vec::new();
        let mut expected_selection_file = None;
        for (root_id, role, relative) in [
            (
                "application",
                StorageRole::Application,
                LibraryPath::parse("")?,
            ),
            ("state", StorageRole::State, state.clone()),
        ] {
            let (expected, retained) = DirectoryExpectation::inspect_in(directory, &relative)?;
            if role == StorageRole::State {
                expected_selection_file = super::state::inspect_selection(&retained.directory)?;
            }
            roots.push(StorageRoot {
                root_id: root_id.into(),
                binding_revision: 1,
                role,
                provider_scope: "application".into(),
                relative: relative.as_str().into(),
                expected,
            });
        }
        Ok(Self {
            format_version: 1,
            selection_revision: 1,
            providers: vec![ProviderMapping {
                scope: "application".into(),
                boundary: boundary.into(),
            }],
            roots,
            expected_selection_file,
            topology_assurances: Vec::new(),
        })
    }

    pub(crate) fn resolve_paths(&mut self, parent: &Path) {
        for provider in &mut self.providers {
            if provider.boundary.is_relative()
                && !provider.boundary.has_root()
                && !provider.boundary.as_os_str().is_empty()
            {
                provider.boundary = parent.join(&provider.boundary);
            }
        }
    }

    /// Canonical logical revision, independent of container-local provider
    /// locators. Complete native expectations still bind each selected boundary.
    /// Assurances are excluded to avoid a circular digest.
    pub fn selection_digest(&self) -> Result<String> {
        let mut providers: Vec<_> = self
            .providers
            .iter()
            .map(|provider| &provider.scope)
            .collect();
        providers.sort_unstable();
        let mut roots = self.roots.clone();
        roots.sort_by(|left, right| left.root_id.cmp(&right.root_id));
        let mut hash = Sha256::new();
        hash.update(b"domus-storage-selection-v2\0");
        hash.update(serde_json::to_vec(&(
            self.format_version,
            self.selection_revision,
            providers,
            roots,
            &self.expected_selection_file,
        ))?);
        Ok(format!("{:x}", hash.finalize()))
    }
}

impl FilesystemClient {
    pub fn new(selection: StorageConfiguration, source_digest: String) -> Result<Arc<Self>> {
        if selection.format_version != 1
            || selection.selection_revision == 0
            || selection.providers.is_empty()
            || selection.providers.len() > 4096
            || selection.roots.is_empty()
            || selection.roots.len() > 4096
            || source_digest.len() != 64
            || !source_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::InvalidInput(
                "unsupported storage version, revision or inventory".into(),
            ));
        }
        let mut providers = BTreeMap::new();
        for provider in &selection.providers {
            if !name(&provider.scope) || providers.insert(&provider.scope, provider).is_some() {
                return Err(Error::InvalidInput(
                    "invalid storage provider inventory".into(),
                ));
            }
            super::validate_native_reference(&provider.boundary)?;
        }
        let mut roots = BTreeMap::new();
        let mut boundaries = BTreeMap::new();
        for root in &selection.roots {
            let relative = LibraryPath::parse(&root.relative)?;
            root.expected.validate(&relative)?;
            if !name(&root.root_id)
                || root.binding_revision == 0
                || !providers.contains_key(&root.provider_scope)
                || roots.insert(&root.root_id, root).is_some()
            {
                return Err(Error::InvalidInput("invalid storage root inventory".into()));
            }
            let boundary = (root.expected.profile, &root.expected.components[0]);
            if boundaries
                .insert(&root.provider_scope, boundary)
                .is_some_and(|previous| previous != boundary)
            {
                return Err(Error::Conflict(
                    "one provider scope selects different native boundaries".into(),
                ));
            }
        }
        if selection
            .roots
            .iter()
            .filter(|root| root.role == StorageRole::State)
            .count()
            != 1
        {
            return Err(Error::InvalidInput(
                "storage requires exactly one state root".into(),
            ));
        }
        if let Some(identity) = &selection.expected_selection_file {
            let state = selection
                .roots
                .iter()
                .find(|root| root.role == StorageRole::State)
                .ok_or(Error::Internal("selected state root is missing"))?;
            DirectoryExpectation {
                profile: state.expected.profile,
                components: vec![identity.clone()],
            }
            .validate(&LibraryPath::parse("")?)?;
        }
        let selection_digest = selection.selection_digest()?;
        let mut pairs = BTreeSet::new();
        let maximum_pairs = selection.roots.len() * (selection.roots.len() - 1) / 2;
        if selection.topology_assurances.len() > maximum_pairs {
            return Err(Error::InvalidInput("too many topology assurances".into()));
        }
        for assurance in &selection.topology_assurances {
            let current = |reference: &RootRevision| {
                roots
                    .get(&reference.root_id)
                    .is_some_and(|root| root.binding_revision == reference.binding_revision)
            };
            if assurance.left.root_id >= assurance.right.root_id
                || !pairs.insert((&assurance.left.root_id, &assurance.right.root_id))
                || !current(&assurance.left)
                || !current(&assurance.right)
                || assurance.selection_digest != selection_digest
            {
                return Err(Error::Conflict(
                    "topology assurance is malformed, duplicated or stale".into(),
                ));
            }
        }
        let mut assurances: Vec<_> = selection.topology_assurances.iter().collect();
        assurances.sort_unstable_by(|left, right| {
            (&left.left.root_id, &left.right.root_id)
                .cmp(&(&right.left.root_id, &right.right.root_id))
        });
        let mut hash = Sha256::new();
        hash.update(b"domus-storage-fence-v1\0");
        hash.update(serde_json::to_vec(&(&selection_digest, assurances))?);
        let fence_digest = format!("{:x}", hash.finalize());
        Ok(Arc::new(Self {
            selection,
            selection_digest,
            fence_digest,
            source_digest,
            opened_roots: Mutex::new(BTreeMap::new()),
            state_context: Mutex::new(None),
        }))
    }

    pub fn selection_digest(&self) -> &str {
        &self.selection_digest
    }

    pub fn require_current(&self, source_digest: &str) -> Result<()> {
        if self.source_digest != source_digest {
            return Err(Error::Conflict(
                "storage belongs to another configuration revision".into(),
            ));
        }
        Ok(())
    }

    /// Describe only an already selected root. Saving this value does not
    /// inspect, adopt or approve a filesystem object.
    pub fn binding(&self, root_id: &str, role: StorageRole) -> Result<LogicalRootBinding> {
        let root = self.root(root_id, role)?;
        self.require_relationships(root)?;
        Ok(LogicalRootBinding {
            format_version: 1,
            root: root.clone(),
        })
    }

    /// Match the complete immutable definition before resolving the current
    /// provider mapping. Never reinterpret an old binding under a new revision.
    pub fn open_binding(
        self: &Arc<Self>,
        binding: &LogicalRootBinding,
        role: StorageRole,
    ) -> Result<StorageLease> {
        self.require_binding(binding, role)?;
        self.open(&binding.root.root_id, role)
    }

    pub(crate) fn require_binding(
        &self,
        binding: &LogicalRootBinding,
        role: StorageRole,
    ) -> Result<()> {
        if binding.format_version != 1 || &binding.root != self.root(&binding.root.root_id, role)? {
            return Err(Error::Conflict(
                "saved root differs from the selected logical binding".into(),
            ));
        }
        self.require_relationships(&binding.root)
    }

    /// A leaf identity only finds a candidate. Authority requires reopening its
    /// complete selected chain and matching all current retained ancestors.
    pub(crate) fn binding_for_directory(
        self: &Arc<Self>,
        directory: &super::LibraryDirectory,
        role: StorageRole,
    ) -> Result<LogicalRootBinding> {
        let identity = super::binding::persistent_file_identity(&directory.directory_file()?)?;
        let mut candidates =
            self.selection.roots.iter().filter(|root| {
                root.role == role && root.expected.components.last() == Some(&identity)
            });
        let candidate = candidates
            .next()
            .ok_or_else(|| Error::Conflict("directory has no selected storage role".into()))?;
        if candidates.next().is_some() {
            return Err(Error::Conflict(
                "directory has ambiguous selected roots".into(),
            ));
        }
        let selected = self.open(&candidate.root_id, role)?;
        if selected.directory().native_root() != directory.native_root() {
            return Err(Error::Conflict(
                "directory differs from the complete selected runtime chain".into(),
            ));
        }
        self.binding(&candidate.root_id, role)
    }

    pub(crate) fn require_media_component(&self, file: &std::fs::File) -> Result<()> {
        let identity = super::binding::persistent_file_identity(file)?;
        self.require_component(StorageRole::Media, &identity)
    }

    fn require_component(&self, role: StorageRole, identity: &str) -> Result<()> {
        if self.selection.roots.iter().any(|root| {
            (root.role == StorageRole::Media) != (role == StorageRole::Media)
                && root
                    .expected
                    .components
                    .last()
                    .is_some_and(|last| last == identity)
        }) {
            return Err(Error::Conflict(
                "storage traversal crosses a media/private role boundary".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn require_private_input(
        self: &Arc<Self>,
        directory: &super::LibraryDirectory,
    ) -> Result<()> {
        self.require_service_input(directory, true)
    }

    pub(crate) fn require_non_media_input(
        self: &Arc<Self>,
        directory: &super::LibraryDirectory,
    ) -> Result<()> {
        self.require_service_input(directory, false)
    }

    fn require_service_input(
        self: &Arc<Self>,
        directory: &super::LibraryDirectory,
        private: bool,
    ) -> Result<()> {
        let mut ancestors = directory
            .parents
            .iter()
            .chain(std::iter::once(&directory.directory));
        while let Some(ancestor) = ancestors.next_back() {
            let identity = match super::binding::persistent_file_identity(
                &ancestor.try_clone()?.into_std_file(),
            ) {
                Err(crate::Error::Io(error))
                    if !private && error.kind() == std::io::ErrorKind::Unsupported =>
                {
                    continue;
                }
                result => result?,
            };
            self.require_component(StorageRole::Application, &identity)?;
            if let Some(root) = self.selection.roots.iter().find(|root| {
                root.role != StorageRole::Media
                    && root.expected.components.last() == Some(&identity)
            }) {
                let selected = self.open(&root.root_id, root.role)?;
                if !directory
                    .native_root()
                    .ancestors
                    .starts_with(&selected.directory().native_root().ancestors)
                {
                    return Err(Error::Conflict(
                        "input differs from the selected private runtime chain".into(),
                    ));
                }
                return Ok(());
            }
        }
        if private {
            Err(Error::Conflict(
                "service inputs require a selected private storage boundary".into(),
            ))
        } else {
            Ok(())
        }
    }

    pub fn root_path(&self, root_id: &str, role: StorageRole) -> Result<PathBuf> {
        let root = self.root(root_id, role)?;
        let provider = self
            .selection
            .providers
            .iter()
            .find(|provider| provider.scope == root.provider_scope)
            .ok_or(Error::Internal("selected storage provider is missing"))?;
        let mut path = provider.boundary.clone();
        for component in root
            .relative
            .split('/')
            .filter(|component| !component.is_empty())
        {
            path.push(component);
        }
        Ok(path)
    }

    pub(crate) fn binding_path(
        &self,
        binding: &LogicalRootBinding,
        role: StorageRole,
    ) -> Result<PathBuf> {
        self.require_binding(binding, role)?;
        self.root_path(&binding.root.root_id, role)
    }

    /// Admission checks saved reservations without opening an offline peer root.
    /// Each peer's own native binding is checked when that root is accessed.
    pub fn open(self: &Arc<Self>, root_id: &str, role: StorageRole) -> Result<StorageLease> {
        let root = self.root(root_id, role)?;
        self.require_relationships(root)?;
        let provider = self
            .selection
            .providers
            .iter()
            .find(|provider| provider.scope == root.provider_scope)
            .ok_or(Error::Internal("selected storage provider is missing"))?;
        if role != StorageRole::Media {
            super::require_local_path(&provider.boundary)?;
        }
        let retained = root
            .expected
            .open(&provider.boundary, &LibraryPath::parse(&root.relative)?)?;
        if role != StorageRole::Media {
            retained.directory.require_local()?;
        }
        let boundary = (retained.directory.parents.len() + 1)
            .checked_sub(root.expected.components.len())
            .ok_or(Error::Internal("selected directory chain is incomplete"))?;
        for ancestor in retained.directory.parents.iter().take(boundary) {
            let identity = match super::binding::persistent_file_identity(
                &ancestor.try_clone()?.into_std_file(),
            ) {
                Ok(identity) => identity,
                // An unsupported ancestor above the selected boundary cannot
                // satisfy any supported saved root expectation.
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::Unsupported => {
                    continue;
                }
                Err(error) => return Err(error),
            };
            if self.selection.roots.iter().any(|other| {
                other.root_id != root_id
                    && other.expected.components.last() == Some(&identity)
                    && !(other.role == StorageRole::Application && role != StorageRole::Media)
            }) {
                return Err(Error::Conflict(
                    "selected root lies inside another selected native root".into(),
                ));
            }
        }
        let pins = Arc::new(retained.directory.try_clone()?);
        {
            let mut opened = self
                .opened_roots
                .lock()
                .map_err(|_| Error::Internal("selected root authority lock failed"))?;
            for (id, (other_role, other)) in opened.iter() {
                if id == root_id {
                    if other.native_root() != pins.native_root() {
                        return Err(Error::Conflict(
                            "selected runtime ancestry changed within one client".into(),
                        ));
                    }
                    continue;
                }
                let application_contains_private =
                    |parent_role,
                     parent: &super::LibraryDirectory,
                     child_role,
                     child: &super::LibraryDirectory| {
                        parent_role == StorageRole::Application
                            && child_role != StorageRole::Media
                            && child
                                .native_root()
                                .ancestors
                                .contains(&parent.native_root().identity)
                    };
                if pins.native_root().overlaps(other.native_root())
                    && !application_contains_private(role, &pins, *other_role, other)
                    && !application_contains_private(*other_role, other, role, &pins)
                {
                    return Err(Error::Conflict(
                        "selected roots overlap through retained native ancestry".into(),
                    ));
                }
            }
            if !opened.contains_key(root_id) {
                opened.insert(root_id.into(), (role, pins));
            }
        }
        Ok(StorageLease {
            client: self.clone(),
            root_id: root_id.to_owned(),
            retained,
            role,
        })
    }

    pub fn open_state(self: &Arc<Self>, database: &Path) -> Result<StorageLease> {
        let state = self
            .selection
            .roots
            .iter()
            .find(|root| root.role == StorageRole::State)
            .ok_or(Error::Internal("selected state root is missing"))?;
        let parent = database.parent().ok_or_else(|| {
            Error::InvalidInput("database requires a selected state directory".into())
        })?;
        self.open_at(&state.root_id, StorageRole::State, parent)
    }

    pub(crate) fn sqlite_context(
        self: &Arc<Self>,
        database: &Path,
        create: bool,
    ) -> Result<StateContext> {
        super::state::database_leaf(database)?;
        let state = self.open_state(database)?;
        let mut current = self
            .state_context
            .lock()
            .map_err(|_| Error::Internal("selected state authority lock failed"))?;
        if let Some((context, path, directory)) = current.as_ref() {
            if path.file_name() != database.file_name() {
                return Err(Error::Conflict(
                    "one selected owner cannot change its database component".into(),
                ));
            }
            context.check()?;
            return Ok((context.clone(), path.clone(), directory.clone()));
        }
        let expected = self.selection.expected_selection_file.as_deref().ok_or_else(|| {
            Error::Conflict("state selection has no coordination file identity; explicit fresh setup is required".into())
        })?;
        let fence =
            super::state::selection_fence(&state.retained.directory, expected, &self.fence_digest)?;
        let opened = super::state::sqlite_context_in(
            &state.retained.directory,
            database,
            create,
            Some(fence),
        )?;
        *current = Some(opened.clone());
        Ok(opened)
    }

    pub fn open_application(self: &Arc<Self>, directory: &Path) -> Result<StorageLease> {
        let mut roots = self
            .selection
            .roots
            .iter()
            .filter(|root| root.role == StorageRole::Application);
        let root = roots
            .next()
            .ok_or_else(|| Error::Conflict("application storage root is not selected".into()))?;
        if roots.next().is_some() {
            return Err(Error::Conflict(
                "configuration differs from the selected application root".into(),
            ));
        }
        self.open_at(&root.root_id, StorageRole::Application, directory)
    }

    fn open_at(
        self: &Arc<Self>,
        root_id: &str,
        role: StorageRole,
        path: &Path,
    ) -> Result<StorageLease> {
        if role != StorageRole::Media {
            super::require_local_path(path)?;
        }
        super::validate_native_reference(path)?;
        let selected = self.open(root_id, role)?;
        let candidate = super::LibraryDirectory::open(path)?;
        if candidate.native_root() != selected.directory().native_root() {
            return Err(Error::Conflict(
                "configured path differs from the selected storage root".into(),
            ));
        }
        Ok(selected)
    }

    fn root(&self, root_id: &str, role: StorageRole) -> Result<&StorageRoot> {
        self.selection
            .roots
            .iter()
            .find(|root| root.root_id == root_id && root.role == role)
            .ok_or_else(|| Error::Conflict("root is not selected for this storage role".into()))
    }

    fn require_relationships(&self, selected: &StorageRoot) -> Result<()> {
        let contains_private = |parent: &StorageRoot, child: &StorageRoot| {
            parent.role == StorageRole::Application
                && child.role != StorageRole::Media
                && parent.expected.profile == child.expected.profile
                && parent
                    .expected
                    .components
                    .last()
                    .is_some_and(|identity| child.expected.components.contains(identity))
        };
        for other in self
            .selection
            .roots
            .iter()
            .filter(|root| root.root_id != selected.root_id)
        {
            if contains_private(selected, other) || contains_private(other, selected) {
                continue;
            }
            let selected_path = self.root_path(&selected.root_id, selected.role)?;
            let other_path = self.root_path(&other.root_id, other.role)?;
            if (selected_path.starts_with(&other_path) || other_path.starts_with(&selected_path))
                && !((selected.role == StorageRole::Application
                    && other.role != StorageRole::Media)
                    || (other.role == StorageRole::Application
                        && selected.role != StorageRole::Media))
            {
                return Err(Error::Conflict("selected storage locators overlap".into()));
            }
            if selected.expected.profile == other.expected.profile
                && (selected.expected.components.contains(
                    other
                        .expected
                        .components
                        .last()
                        .ok_or(Error::Internal("root expectation is empty"))?,
                ) || other.expected.components.contains(
                    selected
                        .expected
                        .components
                        .last()
                        .ok_or(Error::Internal("root expectation is empty"))?,
                ))
            {
                return Err(Error::Conflict("selected storage roots overlap".into()));
            }
            let same_filesystem = |root: &StorageRoot| {
                let scope = root.expected.components[0].split(':').nth(1);
                root.expected
                    .components
                    .iter()
                    .all(|identity| identity.split(':').nth(1) == scope)
            };
            if selected.provider_scope == other.provider_scope
                && same_filesystem(selected)
                && same_filesystem(other)
            {
                continue;
            }
            let (left, right) = if selected.root_id < other.root_id {
                (&selected.root_id, &other.root_id)
            } else {
                (&other.root_id, &selected.root_id)
            };
            if !self.selection.topology_assurances.iter().any(|assurance| {
                &assurance.left.root_id == left && &assurance.right.root_id == right
            }) {
                return Err(Error::Conflict(
                    "opaque provider relationship needs an exact operator topology assurance"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

impl StorageLease {
    pub(crate) fn into_directory(self) -> Result<super::LibraryDirectory> {
        let root = self.client.binding(&self.root_id, self.role)?;
        let expected = DirectoryExpectation {
            profile: root.root.expected.profile,
            components: vec![
                root.root
                    .expected
                    .components
                    .last()
                    .cloned()
                    .ok_or(Error::Internal("selected root expectation is empty"))?,
            ],
        };
        let mut directory = self.retained.directory;
        directory.selection = Some(DirectorySelection {
            client: self.client,
            role: self.role,
            snapshot: LogicalDirectorySnapshot {
                format_version: 1,
                root,
                relative: String::new(),
                expected,
            },
        });
        Ok(directory)
    }

    pub(crate) fn directory(&self) -> &super::LibraryDirectory {
        &self.retained.directory
    }

    pub(crate) fn client(&self) -> &Arc<FilesystemClient> {
        &self.client
    }

    pub fn metadata(&self) -> Result<std::fs::Metadata> {
        self.retained.metadata()
    }

    pub fn root_id(&self) -> &str {
        &self.root_id
    }

    pub fn selection_digest(&self) -> &str {
        self.client.selection_digest()
    }
}

#[cfg(test)]
pub(crate) fn fixture_storage(
    application: &Path,
    state: &LibraryPath,
    additional: &[(&str, StorageRole, &Path)],
) -> Result<StorageConfiguration> {
    let mut configuration = StorageConfiguration::inspect_initial(application, state)?;
    let boundary = application
        .ancestors()
        .find(|boundary| {
            additional
                .iter()
                .all(|(_, _, path)| path.starts_with(boundary))
        })
        .ok_or(Error::Internal(
            "fixture roots have no common provider boundary",
        ))?;
    let initial = configuration
        .roots
        .iter()
        .map(|root| {
            Ok((
                root.root_id.clone(),
                root.role,
                application.join(
                    LibraryPath::parse(&root.relative)?
                        .as_str()
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                ),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    configuration.providers[0].boundary = boundary.into();
    configuration.roots.clear();
    for (root_id, role, path) in initial.into_iter().chain(
        additional
            .iter()
            .map(|(id, role, path)| ((*id).into(), *role, path.to_path_buf())),
    ) {
        let relative = path
            .strip_prefix(boundary)
            .map_err(|_| Error::Internal("fixture root escapes its provider"))?
            .iter()
            .map(|part| {
                part.to_str()
                    .ok_or(Error::Internal("fixture path must be Unicode"))
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        let (expected, _) =
            DirectoryExpectation::inspect(boundary, &LibraryPath::parse(&relative)?)?;
        configuration.roots.push(StorageRoot {
            root_id,
            binding_revision: 1,
            role,
            provider_scope: "application".into(),
            relative,
            expected,
        });
    }
    Ok(configuration)
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;

    fn fixture() -> Result<(tempfile::TempDir, StorageConfiguration)> {
        let directory = tempfile::Builder::new()
            .prefix(".storage-selection-")
            .tempdir_in(std::env::current_dir()?)?;
        let boundary = directory.path().join("provider");
        std::fs::create_dir(&boundary)?;
        let mut roots = Vec::new();
        for (root_id, role) in [("state", StorageRole::State), ("media", StorageRole::Media)] {
            std::fs::create_dir(boundary.join(root_id))?;
            let (expected, _) =
                DirectoryExpectation::inspect(&boundary, &LibraryPath::parse(root_id)?)?;
            roots.push(StorageRoot {
                root_id: root_id.into(),
                binding_revision: 1,
                role,
                provider_scope: "local".into(),
                relative: root_id.into(),
                expected,
            });
        }
        Ok((
            directory,
            StorageConfiguration {
                format_version: 1,
                selection_revision: 1,
                providers: vec![ProviderMapping {
                    scope: "local".into(),
                    boundary,
                }],
                roots,
                expected_selection_file: None,
                topology_assurances: Vec::new(),
            },
        ))
    }

    #[test]
    fn logical_binding_reopens_after_runtime_ancestors_change_without_changing_saved_meaning()
    -> Result<()> {
        let (fixture, mut configuration) = fixture()?;
        let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
        let binding = client.binding("media", StorageRole::Media)?;
        let identity = binding.identity()?;
        let bytes = serde_json::to_vec(&binding)?;
        let original = client.open_binding(&binding, StorageRole::Media)?;
        let native = original.directory().native_root().clone();
        drop(original);
        drop(client);
        let parent = fixture.path().join("another-parent");
        std::fs::create_dir(&parent)?;
        let relocated = parent.join("provider");
        std::fs::rename(&configuration.providers[0].boundary, &relocated)?;
        configuration.providers[0].boundary = relocated;
        let replacement = FilesystemClient::new(configuration, "b".repeat(64))?;
        let saved: LogicalRootBinding = serde_json::from_slice(&bytes)?;
        let reopened = replacement.open_binding(&saved, StorageRole::Media)?;
        assert_ne!(
            reopened.directory().native_root().ancestors,
            native.ancestors
        );
        assert_eq!(saved.identity()?, identity);
        assert_eq!(replacement.binding("media", StorageRole::Media)?, saved);
        assert_eq!(serde_json::to_vec(&saved)?, bytes);
        Ok(())
    }

    #[test]
    fn logical_binding_refuses_a_changed_revision_role_or_boundary_even_with_the_same_leaf()
    -> Result<()> {
        let (_fixture, mut configuration) = fixture()?;
        let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
        let binding = client.binding("media", StorageRole::Media)?;
        assert!(client.open_binding(&binding, StorageRole::Cache).is_err());
        let mut changed_boundary = binding.clone();
        changed_boundary.root.expected.components[0] = changed_boundary
            .root
            .expected
            .components
            .last()
            .ok_or(Error::Internal("empty fixture expectation"))?
            .clone();
        assert!(
            client
                .open_binding(&changed_boundary, StorageRole::Media)
                .is_err()
        );
        configuration
            .roots
            .iter_mut()
            .find(|root| root.root_id == "media")
            .ok_or(Error::Internal("missing fixture media"))?
            .binding_revision += 1;
        let changed = FilesystemClient::new(configuration, "b".repeat(64))?;
        assert!(changed.open_binding(&binding, StorageRole::Media).is_err());
        assert_ne!(
            changed.binding("media", StorageRole::Media)?.identity()?,
            binding.identity()?
        );
        Ok(())
    }

    #[test]
    fn storage_selection_never_uses_an_assurance_to_override_measured_ancestor_overlap()
    -> Result<()> {
        let (_fixture, mut configuration) = fixture()?;
        let boundary = configuration.providers[0].boundary.clone();
        std::fs::create_dir(boundary.join("route"))?;
        configuration.providers[0].boundary = boundary.join("route").join("..");
        let nested = boundary.join("state").join("nested-media");
        std::fs::create_dir(&nested)?;
        let (expected, _) = DirectoryExpectation::inspect(&nested, &LibraryPath::parse("")?)?;
        configuration.providers.push(ProviderMapping {
            scope: "nested-provider".into(),
            boundary: nested,
        });
        let media = configuration
            .roots
            .iter_mut()
            .find(|root| root.role == StorageRole::Media)
            .ok_or(Error::Internal("missing fixture media"))?;
        media.provider_scope = "nested-provider".into();
        media.relative.clear();
        media.expected = expected;
        assure(&mut configuration)?;
        let client = FilesystemClient::new(configuration, "a".repeat(64))?;
        assert!(
            !client
                .root_path("media", StorageRole::Media)?
                .starts_with(client.root_path("state", StorageRole::State)?)
        );
        assert!(client.open("media", StorageRole::Media).is_err());
        Ok(())
    }

    #[test]
    fn storage_selection_private_inputs_need_a_complete_selected_private_boundary() -> Result<()> {
        let (_fixture, configuration) = fixture()?;
        let boundary = configuration.providers[0].boundary.clone();
        let client = FilesystemClient::new(configuration, "a".repeat(64))?;
        std::fs::create_dir(boundary.join("state").join("inputs"))?;
        for (path, admitted) in [
            (boundary.join("state").join("inputs"), true),
            (boundary.join("media"), false),
            (boundary, false),
        ] {
            let directory = super::super::LibraryDirectory::open(&path)?;
            assert_eq!(client.require_private_input(&directory).is_ok(), admitted);
        }
        Ok(())
    }

    fn assure(configuration: &mut StorageConfiguration) -> Result<()> {
        configuration.topology_assurances.push(TopologyAssurance {
            left: RootRevision {
                root_id: "media".into(),
                binding_revision: 1,
            },
            right: RootRevision {
                root_id: "state".into(),
                binding_revision: 1,
            },
            selection_digest: configuration.selection_digest()?,
        });
        Ok(())
    }

    #[test]
    fn storage_selection_retains_roles_and_exact_revisions_without_sqlite_effects() -> Result<()> {
        let (_directory, configuration) = fixture()?;
        let reordered = {
            let mut reordered = configuration.clone();
            reordered.roots.reverse();
            reordered
        };
        assert_eq!(
            configuration.selection_digest()?,
            reordered.selection_digest()?
        );
        let state = configuration.providers[0].boundary.join("state");
        let client = FilesystemClient::new(configuration, "a".repeat(64))?;
        let lease = client.open_state(&state.join("state.db"))?;
        assert!(lease.metadata()?.is_dir());
        assert!(!state.join("state.db").exists());
        assert_eq!(lease.root_id(), "state");
        assert_eq!(lease.selection_digest(), client.selection_digest());
        assert!(client.open("media", StorageRole::Staging).is_err());
        assert!(
            client
                .open_state(&state.join("child").join("state.db"))
                .is_err()
        );
        assert!(client.require_current(&"b".repeat(64)).is_err());
        client.require_current(&"a".repeat(64))?;
        Ok(())
    }

    #[test]
    fn storage_selection_preserves_exact_assurances_when_resolving_provider_locators() -> Result<()>
    {
        let (directory, mut configuration) = fixture()?;
        configuration.providers[0].boundary = "provider".into();
        configuration.providers.push(ProviderMapping {
            scope: "media-view".into(),
            boundary: "provider".into(),
        });
        configuration.roots[1].provider_scope = "media-view".into();
        assure(&mut configuration)?;
        let digest = configuration.selection_digest()?;
        configuration.resolve_paths(directory.path());
        configuration.providers.reverse();
        assert_eq!(configuration.selection_digest()?, digest);
        let client = FilesystemClient::new(configuration, "a".repeat(64))?;
        assert!(
            client
                .open("state", StorageRole::State)?
                .metadata()?
                .is_dir()
        );
        Ok(())
    }

    #[test]
    fn storage_selection_locator_independence_never_adopts_a_different_native_boundary()
    -> Result<()> {
        let (directory, mut configuration) = fixture()?;
        let digest = configuration.selection_digest()?;
        let replacement = directory.path().join("another-container-view");
        std::fs::create_dir(&replacement)?;
        std::fs::create_dir(replacement.join("state"))?;
        std::fs::create_dir(replacement.join("media"))?;
        configuration.providers[0].boundary = replacement;
        assert_eq!(configuration.selection_digest()?, digest);
        let client = FilesystemClient::new(configuration.clone(), "b".repeat(64))?;
        assert!(client.open("state", StorageRole::State).is_err());
        configuration.roots[0].binding_revision += 1;
        assert_ne!(configuration.selection_digest()?, digest);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn storage_selection_matches_windows_locators_by_retained_native_ancestry() -> Result<()> {
        let (_directory, configuration) = fixture()?;
        let boundary = &configuration.providers[0].boundary;
        let selection =
            StorageConfiguration::inspect_initial(boundary, &LibraryPath::parse("state")?)?;
        let client = FilesystemClient::new(selection, "a".repeat(64))?;
        for path in [boundary.clone(), std::fs::canonicalize(boundary)?] {
            assert!(client.open_application(&path)?.metadata()?.is_dir());
            assert!(
                client
                    .open_state(&path.join("state").join("state.db"))?
                    .metadata()?
                    .is_dir()
            );
        }
        assert!(
            client
                .open_state(&boundary.join("media").join("state.db"))
                .is_err()
        );
        assert!(client.open_application(&boundary.join("state")).is_err());
        Ok(())
    }

    #[test]
    fn storage_selection_keeps_offline_media_reserved_without_opening_it() -> Result<()> {
        let (directory, configuration) = fixture()?;
        std::fs::rename(
            configuration.providers[0].boundary.join("media"),
            directory.path().join("offline-media"),
        )?;
        let client = FilesystemClient::new(configuration, "a".repeat(64))?;
        assert!(
            client
                .open("state", StorageRole::State)?
                .metadata()?
                .is_dir()
        );
        assert!(client.open("media", StorageRole::Media).is_err());
        Ok(())
    }

    #[test]
    fn storage_selection_requires_exact_assurances_for_opaque_mappings() -> Result<()> {
        for role in [
            StorageRole::Application,
            StorageRole::Cache,
            StorageRole::Staging,
            StorageRole::Certificate,
            StorageRole::Media,
        ] {
            let (_directory, mut configuration) = fixture()?;
            configuration.providers.push(ProviderMapping {
                scope: "another-view".into(),
                boundary: configuration.providers[0].boundary.clone(),
            });
            configuration.roots[1].provider_scope = "another-view".into();
            configuration.roots[1].role = role;
            let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
            assert!(client.open("state", StorageRole::State).is_err());
            assure(&mut configuration)?;
            let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
            assert!(
                client
                    .open("state", StorageRole::State)?
                    .metadata()?
                    .is_dir()
            );
            configuration.roots[1].binding_revision += 1;
            assert!(FilesystemClient::new(configuration, "a".repeat(64)).is_err());
        }
        Ok(())
    }

    #[test]
    fn storage_selection_never_uses_an_assurance_to_override_known_overlap() -> Result<()> {
        let (_directory, mut configuration) = fixture()?;
        let boundary = &configuration.providers[0].boundary;
        std::fs::create_dir(boundary.join("state").join("child"))?;
        configuration.roots[1].relative = "state/child".into();
        configuration.roots[1].expected =
            DirectoryExpectation::inspect(boundary, &LibraryPath::parse("state/child")?)?.0;
        assure(&mut configuration)?;
        let client = FilesystemClient::new(configuration, "a".repeat(64))?;
        assert!(client.open("state", StorageRole::State).is_err());
        Ok(())
    }

    #[test]
    fn storage_selection_protects_the_application_parent_but_allows_private_children() -> Result<()>
    {
        let (_directory, mut configuration) = fixture()?;
        let boundary = configuration.providers[0].boundary.clone();
        let (expected, _) = DirectoryExpectation::inspect(&boundary, &LibraryPath::parse("")?)?;
        configuration.roots.push(StorageRoot {
            root_id: "application".into(),
            binding_revision: 1,
            role: StorageRole::Application,
            provider_scope: "local".into(),
            relative: String::new(),
            expected,
        });
        let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
        assert!(client.open_application(&boundary).is_err());
        configuration
            .roots
            .retain(|root| root.role != StorageRole::Media);
        let client = FilesystemClient::new(configuration.clone(), "a".repeat(64))?;
        assert!(client.open_application(&boundary)?.metadata()?.is_dir());
        assert!(
            client
                .open("state", StorageRole::State)?
                .metadata()?
                .is_dir()
        );
        std::fs::create_dir(boundary.join("state").join("application"))?;
        configuration.roots[1].relative = "state/application".into();
        configuration.roots[1].expected =
            DirectoryExpectation::inspect(&boundary, &LibraryPath::parse("state/application")?)?.0;
        configuration.topology_assurances.push(TopologyAssurance {
            left: RootRevision {
                root_id: "application".into(),
                binding_revision: 1,
            },
            right: RootRevision {
                root_id: "state".into(),
                binding_revision: 1,
            },
            selection_digest: configuration.selection_digest()?,
        });
        let client = FilesystemClient::new(configuration, "a".repeat(64))?;
        assert!(
            client
                .open_application(&boundary.join("state").join("application"))
                .is_err()
        );
        assert!(client.open("state", StorageRole::State).is_err());
        Ok(())
    }

    #[test]
    fn storage_selection_rejects_bad_inventories_before_native_access() -> Result<()> {
        let (_directory, configuration) = fixture()?;
        let mut duplicate = configuration.clone();
        duplicate.roots.push(duplicate.roots[1].clone());
        assert!(FilesystemClient::new(duplicate, "a".repeat(64)).is_err());
        let mut incomplete = configuration.clone();
        incomplete.roots[0].expected.components.remove(0);
        assert!(FilesystemClient::new(incomplete, "a".repeat(64)).is_err());
        let mut unsupported = configuration.clone();
        unsupported.roots[0].expected.components[0] = "copied-owner-marker".into();
        assert!(FilesystemClient::new(unsupported, "a".repeat(64)).is_err());
        let mut escape = configuration.clone();
        escape.roots[1].relative = "../private".into();
        assert!(FilesystemClient::new(escape, "a".repeat(64)).is_err());
        let mut inconsistent = configuration;
        inconsistent.roots[1].expected.components[0] =
            inconsistent.roots[1].expected.components[1].clone();
        assert!(FilesystemClient::new(inconsistent, "a".repeat(64)).is_err());
        Ok(())
    }
}
