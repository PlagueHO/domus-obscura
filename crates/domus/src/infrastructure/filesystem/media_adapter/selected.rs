use super::{Client, MediaClient, MediaSelection};
use crate::infrastructure::filesystem::{
    StorageConfiguration, StorageRole, StorageRoot, TopologyAssurance,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;

/// Frozen private inputs. Each new operation gets its own connection pool and
/// capability table; a reqwest pool must never be shared between media sessions.
pub(crate) struct MediaPairing {
    pub address: SocketAddr,
    pub server_ca: Vec<u8>,
    pub paired_server: Vec<u8>,
    pub certificate: Vec<u8>,
    pub private_key: Vec<u8>,
}

impl MediaPairing {
    pub(crate) fn client(&self, selection: &MediaSelection) -> Result<MediaClient> {
        MediaClient::remote(
            selection.clone(),
            Client::new(
                self.address,
                &self.server_ca,
                &self.paired_server,
                &self.certificate,
                &self.private_key,
            )?,
        )
    }
}

/// Durable meaning only. Reopening requires the current frozen provider and a
/// new authenticated lease. It contains neither a native locator nor a token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MediaBinding {
    format_version: u32,
    media_provider_scope: String,
    pub(crate) root: StorageRoot,
}

impl MediaBinding {
    pub(crate) fn key(&self) -> String {
        format!("paired-media-root:{}", self.root.root_id)
    }

    pub(crate) fn identity(&self) -> Result<String> {
        use sha2::{Digest, Sha256};
        if self.format_version != 1
            || self.root.role != StorageRole::Media
            || self.media_provider_scope != self.root.provider_scope
        {
            return Err(Error::Conflict("unsupported paired media binding".into()));
        }
        Ok(format!(
            "paired-media-v1:{:x}",
            Sha256::digest(serde_json::to_vec(self)?)
        ))
    }
}

/// Media-only remote authority. Private storage keeps its original local
/// FilesystemClient. Opaque cross-provider relations require exact operator
/// assurances; none can override known native overlap.
pub(crate) struct SelectedMedia {
    selection: MediaSelection,
    pairing: MediaPairing,
}

impl SelectedMedia {
    pub(crate) fn new(
        selection: MediaSelection,
        pairing: MediaPairing,
        local: &StorageConfiguration,
        assurances: &[TopologyAssurance],
    ) -> Result<Self> {
        selection.validate()?;
        if local
            .providers
            .iter()
            .any(|provider| provider.scope == selection.provider_scope)
        {
            return Err(Error::Conflict(
                "paired media requires a distinct provider scope".into(),
            ));
        }
        let mut roots = BTreeMap::new();
        for root in local
            .roots
            .iter()
            .chain(selection.media.iter())
            .chain(std::iter::once(&selection.private))
        {
            if roots.insert(root.root_id.as_str(), root).is_some() {
                return Err(Error::Conflict(
                    "paired and local storage root IDs must be distinct".into(),
                ));
            }
        }
        let digest = selection.storage_digest(local)?;
        let mut required = BTreeSet::new();
        for remote in selection
            .media
            .iter()
            .chain(std::iter::once(&selection.private))
        {
            for native in &local.roots {
                if remote.role != StorageRole::Media && native.role != StorageRole::Media {
                    continue;
                }
                if remote.expected.profile == native.expected.profile
                    && (remote
                        .expected
                        .components
                        .iter()
                        .any(|identity| native.expected.components.last() == Some(identity))
                        || native
                            .expected
                            .components
                            .iter()
                            .any(|identity| remote.expected.components.last() == Some(identity)))
                {
                    return Err(Error::Conflict(
                        "paired media overlaps a selected local root".into(),
                    ));
                }
                let pair = if remote.root_id < native.root_id {
                    (remote.root_id.as_str(), native.root_id.as_str())
                } else {
                    (native.root_id.as_str(), remote.root_id.as_str())
                };
                required.insert(pair);
            }
        }
        let mut supplied = BTreeSet::new();
        for assurance in assurances {
            let current = |reference: &crate::infrastructure::filesystem::RootRevision| {
                roots
                    .get(reference.root_id.as_str())
                    .is_some_and(|root| root.binding_revision == reference.binding_revision)
            };
            let pair = (
                assurance.left.root_id.as_str(),
                assurance.right.root_id.as_str(),
            );
            if assurance.selection_digest != digest
                || !current(&assurance.left)
                || !current(&assurance.right)
                || !required.contains(&pair)
                || !supplied.insert(pair)
            {
                return Err(Error::Conflict(
                    "paired media topology assurance is malformed, duplicated or stale".into(),
                ));
            }
        }
        if supplied != required {
            return Err(Error::Conflict("paired media requires an exact operator topology assurance for each opaque local relationship".into()));
        }
        // Validate pairing without contacting its peer.
        pairing.client(&selection)?;
        Ok(Self { selection, pairing })
    }

    pub(crate) fn binding(&self, root_id: &str) -> Result<MediaBinding> {
        let root = self
            .selection
            .media
            .iter()
            .find(|root| root.root_id == root_id)
            .ok_or_else(|| Error::Conflict("paired media root is not selected".into()))?;
        Ok(MediaBinding {
            format_version: 1,
            media_provider_scope: self.selection.provider_scope.clone(),
            root: root.clone(),
        })
    }

    pub(crate) fn require_binding(&self, binding: &MediaBinding) -> Result<()> {
        if self.binding(&binding.root.root_id)? != *binding {
            return Err(Error::Conflict(
                "saved paired media root differs from its selected binding".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn open(&self, binding: &MediaBinding) -> Result<super::BlockingMedia> {
        self.require_binding(binding)?;
        super::BlockingMedia::new(
            self.pairing.client(&self.selection)?,
            binding.root.root_id.clone(),
        )
    }

    /// Synchronous use cases already run on blocking threads. Network I/O is
    /// never performed while a SQL transaction is open.
    pub(crate) fn check(&self, binding: &MediaBinding) -> Result<()> {
        self.require_binding(binding)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut client = self.pairing.client(&self.selection)?;
        runtime.block_on(async {
            let root = client.open_root(&binding.root.root_id).await?;
            client.close_directory(root).await
        })
    }
}
