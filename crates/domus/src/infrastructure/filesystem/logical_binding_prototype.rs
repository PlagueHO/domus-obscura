//! Test-only configuration and admission model. This does not open native roots.
//! Identity codecs, live handle leases and production bootstrap remain separate.

use super::FilesystemClass;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(windows)]
#[path = "logical_binding_native_windows.rs"]
mod native_windows;

const MAX_BYTES: usize = 65_536;
const MAX_ROOTS: usize = 16;
const MAX_COMPONENTS: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Role {
    State,
    Cache,
    Staging,
    Media,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DirectoryExpectation {
    profile: String,
    components: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    root_id: String,
    binding_revision: u64,
    role: Role,
    provider_scope: String,
    locator: String,
    expected: DirectoryExpectation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RootRevision {
    root_id: String,
    binding_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TopologyAssurance {
    left: RootRevision,
    right: RootRevision,
    selection_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format_version: u32,
    selection_revision: u64,
    roots: Vec<Binding>,
    topology_assurances: Vec<TopologyAssurance>,
}

struct FrozenSelection {
    configuration: Configuration,
    selection_digest: String,
    source_digest: String,
}

fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn selection_digest(configuration: &Configuration) -> Result<String> {
    let mut roots = configuration.roots.clone();
    roots.sort_by(|left, right| left.root_id.cmp(&right.root_id));
    let mut hash = Sha256::new();
    hash.update(b"domus-storage-selection-v1\0");
    hash.update(serde_json::to_vec(&(
        configuration.format_version,
        configuration.selection_revision,
        roots,
    ))?);
    Ok(format!("{:x}", hash.finalize()))
}

impl FrozenSelection {
    fn parse(bytes: &[u8], supported_profiles: &[&str]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::InvalidInput(
                "prototype configuration is too large".into(),
            ));
        }
        let configuration: Configuration = serde_json::from_slice(bytes)?;
        if configuration.format_version != 1
            || configuration.selection_revision == 0
            || configuration.roots.is_empty()
            || configuration.roots.len() > MAX_ROOTS
            || configuration.topology_assurances.len() > MAX_ROOTS * (MAX_ROOTS - 1) / 2
        {
            return Err(Error::InvalidInput(
                "unsupported prototype configuration version, revision or inventory size".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut state_roots = 0;
        for root in &configuration.roots {
            if !bounded_text(&root.root_id, 128)
                || !ids.insert(&root.root_id)
                || root.binding_revision == 0
                || !bounded_text(&root.provider_scope, 128)
                || !bounded_text(&root.locator, 4096)
                || !bounded_text(&root.expected.profile, 128)
                || !supported_profiles.contains(&root.expected.profile.as_str())
                || root.expected.components.is_empty()
                || root.expected.components.len() > MAX_COMPONENTS
                || root
                    .expected
                    .components
                    .iter()
                    .any(|id| !bounded_text(id, 512))
            {
                return Err(Error::InvalidInput(
                    "invalid or unsupported prototype root binding".into(),
                ));
            }
            state_roots += usize::from(root.role == Role::State);
        }
        if state_roots != 1 {
            return Err(Error::InvalidInput(
                "prototype configuration requires exactly one state root".into(),
            ));
        }
        let selection_digest = selection_digest(&configuration)?;
        let mut pairs = BTreeSet::new();
        for assurance in &configuration.topology_assurances {
            let current = |reference: &RootRevision| {
                configuration.roots.iter().any(|root| {
                    root.root_id == reference.root_id
                        && root.binding_revision == reference.binding_revision
                })
            };
            if assurance.left.root_id >= assurance.right.root_id
                || !pairs.insert((&assurance.left.root_id, &assurance.right.root_id))
                || !current(&assurance.left)
                || !current(&assurance.right)
                || assurance.selection_digest != selection_digest
            {
                return Err(Error::Conflict(
                    "prototype topology assurance is malformed, duplicated or stale".into(),
                ));
            }
        }
        Ok(Self {
            configuration,
            selection_digest,
            source_digest: format!("{:x}", Sha256::digest(bytes)),
        })
    }

    fn admit(
        &self,
        active_source_digest: &str,
        batch: &ObservationBatch,
    ) -> Result<AdmissionReview> {
        if active_source_digest != self.source_digest
            || batch.selection_digest != self.selection_digest
        {
            return Err(Error::Conflict(
                "prototype activation or observation belongs to another configuration".into(),
            ));
        }
        if batch.roots.len() != self.configuration.roots.len() {
            return Err(Error::Conflict(
                "prototype root observation is incomplete".into(),
            ));
        }
        for root in &self.configuration.roots {
            let live = batch
                .roots
                .get(&root.root_id)
                .ok_or_else(|| Error::Conflict("prototype root observation is missing".into()))?;
            if live.binding_revision != root.binding_revision
                || live.provider_scope != root.provider_scope
                || live.directory != root.expected
                || !live.retained_no_follow
            {
                return Err(Error::Conflict(
                    "prototype root binding changed or traversal is unsafe".into(),
                ));
            }
            if root.role == Role::State && live.filesystem != FilesystemClass::Local {
                return Err(Error::InvalidInput(
                    "prototype SQLite state requires verified local filesystem evidence".into(),
                ));
            }
        }
        let mut pair_bases = BTreeMap::new();
        for (index, left) in self.configuration.roots.iter().enumerate() {
            for right in &self.configuration.roots[index + 1..] {
                let pair = if left.root_id < right.root_id {
                    (left.root_id.clone(), right.root_id.clone())
                } else {
                    (right.root_id.clone(), left.root_id.clone())
                };
                let evidence = batch.relationships.get(&pair).ok_or_else(|| {
                    Error::Conflict("prototype relationship evidence is missing".into())
                })?;
                let basis = match evidence {
                    Relationship::Separated => RelationshipBasis::Observed,
                    Relationship::OpaqueProviderMapping => {
                        if !self
                            .configuration
                            .topology_assurances
                            .iter()
                            .any(|assurance| {
                                assurance.left.root_id == pair.0
                                    && assurance.right.root_id == pair.1
                            })
                        {
                            return Err(Error::Conflict(
                                "opaque prototype mapping needs an exact operator assurance".into(),
                            ));
                        }
                        RelationshipBasis::OperatorAssured
                    }
                    Relationship::Overlapping | Relationship::UnsupportedTopology => {
                        return Err(Error::Conflict(
                            "prototype roots overlap or topology is unsupported".into(),
                        ));
                    }
                };
                pair_bases.insert(pair, basis);
            }
        }
        Ok(AdmissionReview { pair_bases })
    }
}

#[derive(Clone)]
struct RootObservation {
    binding_revision: u64,
    provider_scope: String,
    directory: DirectoryExpectation,
    retained_no_follow: bool,
    filesystem: FilesystemClass,
}

#[derive(Clone, Copy)]
enum Relationship {
    Separated,
    OpaqueProviderMapping,
    Overlapping,
    UnsupportedTopology,
}

#[derive(Clone)]
struct ObservationBatch {
    selection_digest: String,
    roots: BTreeMap<String, RootObservation>,
    relationships: BTreeMap<(String, String), Relationship>,
}

#[derive(Debug, Eq, PartialEq)]
enum RelationshipBasis {
    Observed,
    OperatorAssured,
}

// A policy review result, deliberately not a serializable filesystem capability.
struct AdmissionReview {
    pair_bases: BTreeMap<(String, String), RelationshipBasis>,
}

fn configuration() -> Configuration {
    Configuration {
        format_version: 1,
        selection_revision: 1,
        roots: [
            ("media", Role::Media, "remote-scope"),
            ("state", Role::State, "local-scope"),
        ]
        .into_iter()
        .map(|(id, role, provider)| Binding {
            root_id: id.into(),
            binding_revision: 1,
            role,
            provider_scope: provider.into(),
            locator: format!("generated/{id}"),
            expected: DirectoryExpectation {
                profile: "fixture-handles-v1".into(),
                components: vec![format!("{id}-boundary"), format!("{id}-root")],
            },
        })
        .collect(),
        topology_assurances: Vec::new(),
    }
}

fn assure(configuration: &mut Configuration) -> Result<()> {
    configuration.topology_assurances = vec![TopologyAssurance {
        left: RootRevision {
            root_id: "media".into(),
            binding_revision: 1,
        },
        right: RootRevision {
            root_id: "state".into(),
            binding_revision: 1,
        },
        selection_digest: selection_digest(configuration)?,
    }];
    Ok(())
}

fn freeze(configuration: &Configuration) -> Result<FrozenSelection> {
    FrozenSelection::parse(&serde_json::to_vec(configuration)?, &["fixture-handles-v1"])
}

fn observations(selection: &FrozenSelection, relationship: Relationship) -> ObservationBatch {
    ObservationBatch {
        selection_digest: selection.selection_digest.clone(),
        roots: selection
            .configuration
            .roots
            .iter()
            .map(|root| {
                (
                    root.root_id.clone(),
                    RootObservation {
                        binding_revision: root.binding_revision,
                        provider_scope: root.provider_scope.clone(),
                        directory: root.expected.clone(),
                        retained_no_follow: true,
                        filesystem: if root.role == Role::State {
                            FilesystemClass::Local
                        } else {
                            FilesystemClass::Remote
                        },
                    },
                )
            })
            .collect(),
        relationships: [(("media".into(), "state".into()), relationship)].into(),
    }
}

fn state_observation(batch: &mut ObservationBatch) -> Result<&mut RootObservation> {
    batch
        .roots
        .get_mut("state")
        .ok_or_else(|| Error::InvalidInput("generated state observation is missing".into()))
}

#[test]
fn logical_binding_prototype_distinguishes_assurance_from_measured_separation() -> Result<()> {
    let mut config = configuration();
    let selection = freeze(&config)?;
    let opaque = observations(&selection, Relationship::OpaqueProviderMapping);
    assert!(matches!(
        selection.admit(&selection.source_digest, &opaque),
        Err(Error::Conflict(_))
    ));
    assure(&mut config)?;
    let selection = freeze(&config)?;
    for (relationship, expected) in [
        (
            Relationship::OpaqueProviderMapping,
            RelationshipBasis::OperatorAssured,
        ),
        (Relationship::Separated, RelationshipBasis::Observed),
    ] {
        let batch = observations(&selection, relationship);
        let admitted = selection.admit(&selection.source_digest, &batch)?;
        assert_eq!(
            admitted.pair_bases.values().collect::<Vec<_>>(),
            vec![&expected]
        );
    }
    Ok(())
}

#[test]
fn logical_binding_prototype_rejects_stale_assurances_without_revision_bumps() -> Result<()> {
    let mut original = configuration();
    assure(&mut original)?;
    let mut changes = Vec::new();
    let mut changed = original.clone();
    changed.roots[0].locator.push_str("-new");
    changes.push(changed);
    let mut changed = original.clone();
    changed.roots[0].provider_scope.push_str("-new");
    changes.push(changed);
    let mut changed = original.clone();
    changed.roots[0].role = Role::Cache;
    changes.push(changed);
    let mut changed = original.clone();
    changed.roots[0].expected.components[0].push_str("-new");
    changes.push(changed);
    let mut changed = original.clone();
    changed.selection_revision += 1;
    changes.push(changed);
    for changed in changes {
        assert!(matches!(freeze(&changed), Err(Error::Conflict(_))));
    }
    Ok(())
}

#[test]
fn logical_binding_prototype_refuses_ambiguous_or_unsupported_documents() -> Result<()> {
    let original = configuration();
    let bytes = serde_json::to_vec(&original)?;
    assert!(FrozenSelection::parse(&bytes, &[]).is_err());
    assert!(FrozenSelection::parse(&vec![b' '; MAX_BYTES + 1], &["fixture-handles-v1"]).is_err());
    let mut document = serde_json::to_value(&original)?;
    document["trust_all_storage"] = true.into();
    assert!(
        FrozenSelection::parse(&serde_json::to_vec(&document)?, &["fixture-handles-v1"]).is_err()
    );
    for (field, value) in [
        ("format_version", serde_json::json!(2)),
        ("selection_revision", serde_json::json!(0)),
        ("roots", serde_json::json!([])),
    ] {
        let mut document = serde_json::to_value(&original)?;
        document[field] = value;
        assert!(
            FrozenSelection::parse(&serde_json::to_vec(&document)?, &["fixture-handles-v1"])
                .is_err()
        );
    }
    for (field, value) in [
        ("binding_revision", serde_json::json!(0)),
        ("role", serde_json::json!("media")),
        (
            "expected",
            serde_json::json!({"profile": "fixture-handles-v1", "components": []}),
        ),
    ] {
        let mut document = serde_json::to_value(&original)?;
        document["roots"][1][field] = value;
        assert!(
            FrozenSelection::parse(&serde_json::to_vec(&document)?, &["fixture-handles-v1"])
                .is_err()
        );
    }
    let mut duplicate = original.clone();
    duplicate.roots.push(duplicate.roots[0].clone());
    assert!(freeze(&duplicate).is_err());
    let mut assured = original.clone();
    assure(&mut assured)?;
    let mut duplicate = assured.clone();
    duplicate
        .topology_assurances
        .push(duplicate.topology_assurances[0].clone());
    assert!(freeze(&duplicate).is_err());
    for (left, right, revision) in [
        ("state", "media", 1),
        ("media", "media", 1),
        ("media", "missing", 1),
        ("media", "state", 2),
    ] {
        let mut changed = assured.clone();
        changed.topology_assurances[0].left.root_id = left.into();
        changed.topology_assurances[0].right.root_id = right.into();
        changed.topology_assurances[0].left.binding_revision = revision;
        assert!(freeze(&changed).is_err());
    }
    Ok(())
}

#[test]
fn logical_binding_prototype_assurance_never_overrides_known_unsafe_evidence() -> Result<()> {
    let mut config = configuration();
    assure(&mut config)?;
    let selection = freeze(&config)?;
    for relationship in [Relationship::Overlapping, Relationship::UnsupportedTopology] {
        let batch = observations(&selection, relationship);
        assert!(matches!(
            selection.admit(&selection.source_digest, &batch),
            Err(Error::Conflict(_))
        ));
    }
    let original = observations(&selection, Relationship::OpaqueProviderMapping);
    let mut failures = Vec::new();
    let mut changed = original.clone();
    state_observation(&mut changed)?.directory.components[1].push_str("-replacement");
    failures.push(changed);
    let mut changed = original.clone();
    state_observation(&mut changed)?
        .provider_scope
        .push_str("-changed");
    failures.push(changed);
    let mut changed = original.clone();
    state_observation(&mut changed)?.binding_revision += 1;
    failures.push(changed);
    let mut changed = original.clone();
    state_observation(&mut changed)?.retained_no_follow = false;
    failures.push(changed);
    let mut changed = original.clone();
    changed.roots.remove("state");
    failures.push(changed);
    let mut changed = original.clone();
    changed.relationships.clear();
    failures.push(changed);
    for batch in failures {
        assert!(matches!(
            selection.admit(&selection.source_digest, &batch),
            Err(Error::Conflict(_))
        ));
    }
    for filesystem in [FilesystemClass::Remote, FilesystemClass::Unknown] {
        let mut batch = original.clone();
        state_observation(&mut batch)?.filesystem = filesystem;
        assert!(matches!(
            selection.admit(&selection.source_digest, &batch),
            Err(Error::InvalidInput(_))
        ));
    }
    Ok(())
}

#[test]
fn logical_binding_prototype_separates_activation_bytes_from_selection_digest() -> Result<()> {
    let mut config = configuration();
    assure(&mut config)?;
    let compact = freeze(&config)?;
    let pretty = FrozenSelection::parse(
        &serde_json::to_vec_pretty(&config)?,
        &["fixture-handles-v1"],
    )?;
    assert_eq!(compact.selection_digest, pretty.selection_digest);
    assert_ne!(compact.source_digest, pretty.source_digest);
    let mut batch = observations(&compact, Relationship::OpaqueProviderMapping);
    assert!(matches!(
        pretty.admit(&compact.source_digest, &batch),
        Err(Error::Conflict(_))
    ));
    batch.selection_digest.push_str("-stale");
    assert!(matches!(
        compact.admit(&compact.source_digest, &batch),
        Err(Error::Conflict(_))
    ));
    config.roots.reverse();
    let reordered = freeze(&config)?;
    assert_eq!(compact.selection_digest, reordered.selection_digest);
    Ok(())
}

#[test]
fn logical_binding_prototype_refuses_before_generated_sqlite_open() -> Result<()> {
    let mut config = configuration();
    assure(&mut config)?;
    let selection = freeze(&config)?;
    let valid = observations(&selection, Relationship::OpaqueProviderMapping);
    let mut replaced = valid.clone();
    state_observation(&mut replaced)?.directory.components[1].push_str("-new");
    let overlap = observations(&selection, Relationship::Overlapping);
    let mut remote = valid.clone();
    state_observation(&mut remote)?.filesystem = FilesystemClass::Remote;
    for (batch, expected_success) in [
        (replaced, false),
        (overlap, false),
        (remote, false),
        (valid, true),
    ] {
        let fixture = tempfile::tempdir()?;
        let database = fixture.path().join("generated.db");
        let admission = selection.admit(&selection.source_digest, &batch);
        if expected_success {
            let review = admission?;
            assert_eq!(review.pair_bases.len(), 1);
            let connection = rusqlite::Connection::open(&database)?;
            connection.execute_batch(
                "CREATE TABLE receipt(value INTEGER); INSERT INTO receipt VALUES(41);",
            )?;
            assert_eq!(
                connection
                    .query_row("SELECT value FROM receipt", [], |row| row.get::<_, i64>(0))?,
                41
            );
            drop(connection);
            assert!(database.is_file());
        } else {
            assert!(admission.is_err());
            assert_eq!(std::fs::read_dir(fixture.path())?.count(), 0);
        }
    }
    Ok(())
}
