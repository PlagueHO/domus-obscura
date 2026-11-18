//! Isolated Windows identity profile. This does not classify storage locality.

use super::{
    Binding, DirectoryExpectation, FilesystemClass, FrozenSelection, ObservationBatch,
    Relationship, RootObservation, configuration,
};
use crate::domain::LibraryPath;
use crate::infrastructure::filesystem::LibraryDirectory;
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::path::Path;

const PROFILE: &str = "windows-file-id128-chain-v1";

struct RetainedDirectoryChain {
    directories: Vec<LibraryDirectory>,
    expected: DirectoryExpectation,
}

impl RetainedDirectoryChain {
    fn open(boundary: &Path, relative: &LibraryPath) -> Result<Self> {
        let mut directories = vec![LibraryDirectory::open(boundary)?];
        if !relative.as_str().is_empty() {
            for component in relative.as_str().split('/') {
                let parent = directories
                    .last()
                    .ok_or(Error::Internal("native prototype boundary is missing"))?;
                directories.push(parent.open_directory(&LibraryPath::parse(component)?)?);
            }
        }
        let expected = DirectoryExpectation {
            profile: PROFILE.into(),
            components: directories
                .iter()
                .map(|directory| directory.native_root().identity.clone())
                .collect(),
        };
        Ok(Self {
            directories,
            expected,
        })
    }

    fn reopen(
        expected: &DirectoryExpectation,
        boundary: &Path,
        relative: &LibraryPath,
    ) -> Result<Self> {
        let current = Self::open(boundary, relative)?;
        if current.expected != *expected {
            return Err(Error::Conflict(
                "native prototype directory chain differs from the approved expectation".into(),
            ));
        }
        Ok(current)
    }

    fn leaf(&self) -> Result<&LibraryDirectory> {
        self.directories
            .last()
            .ok_or(Error::Internal("native prototype leaf is missing"))
    }

    fn observation(&self, binding: &Binding) -> RootObservation {
        RootObservation {
            binding_revision: binding.binding_revision,
            provider_scope: binding.provider_scope.clone(),
            directory: self.expected.clone(),
            retained_no_follow: true,
            filesystem: FilesystemClass::Unknown,
        }
    }

    fn relationship(&self, other: &Self) -> Result<Relationship> {
        Ok(
            if self
                .leaf()?
                .native_root()
                .overlaps(other.leaf()?.native_root())
            {
                Relationship::Overlapping
            } else {
                Relationship::OpaqueProviderMapping
            },
        )
    }
}

#[test]
fn logical_windows_profile_detects_known_overlap_without_inventing_separation() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    std::fs::create_dir(fixture.path().join("state"))?;
    std::fs::create_dir_all(fixture.path().join("media").join("child"))?;
    let state = RetainedDirectoryChain::open(fixture.path(), &LibraryPath::parse("state")?)?;
    let media = RetainedDirectoryChain::open(fixture.path(), &LibraryPath::parse("media")?)?;
    let child =
        RetainedDirectoryChain::open(&fixture.path().join("media"), &LibraryPath::parse("child")?)?;
    let same = RetainedDirectoryChain::reopen(
        &media.expected,
        fixture.path(),
        &LibraryPath::parse("media")?,
    )?;
    for (left, right) in [(&media, &same), (&media, &child), (&child, &media)] {
        assert!(matches!(
            left.relationship(right)?,
            Relationship::Overlapping
        ));
    }
    assert!(matches!(
        state.relationship(&media)?,
        Relationship::OpaqueProviderMapping
    ));
    Ok(())
}

#[test]
fn logical_windows_profile_reopens_serialized_chain_and_reads_from_retained_handle() -> Result<()> {
    use std::io::Read;

    let fixture = tempfile::tempdir()?;
    let media = fixture.path().join("parent").join("media");
    std::fs::create_dir_all(&media)?;
    std::fs::write(media.join("sentinel"), b"generated native profile")?;
    let relative = LibraryPath::parse("parent/media")?;
    let first = RetainedDirectoryChain::open(fixture.path(), &relative)?;
    assert_eq!(first.directories.len(), 3);
    for (directory, id) in first.directories.iter().zip(&first.expected.components) {
        let file = directory.directory.try_clone()?.into_std_file();
        let file_id::FileId::HighRes {
            volume_serial_number,
            file_id,
        } = file_id::get_high_res_file_id_from_file(&file)?
        else {
            return Err(Error::InvalidInput(
                "native profile fixture did not supply a full Windows file identity".into(),
            ));
        };
        assert_eq!(
            id,
            &format!("win128:{volume_serial_number:016x}:{file_id:032x}")
        );
    }
    let bytes = serde_json::to_vec(&first.expected)?;
    drop(first);
    let saved: DirectoryExpectation = serde_json::from_slice(&bytes)?;
    let current = RetainedDirectoryChain::reopen(&saved, fixture.path(), &relative)?;
    let mut payload = Vec::new();
    current
        .leaf()?
        .open_file(std::ffi::OsStr::new("sentinel"))?
        .take(128)
        .read_to_end(&mut payload)?;
    assert_eq!(payload, b"generated native profile");
    Ok(())
}

#[test]
fn logical_windows_profile_refuses_replaced_chain_even_with_copied_marker() -> Result<()> {
    for target in ["boundary", "parent", "media"] {
        let fixture = tempfile::tempdir()?;
        let boundary = fixture.path().join("boundary");
        let parent = boundary.join("parent");
        let media = parent.join("media");
        std::fs::create_dir_all(&media)?;
        std::fs::write(media.join("owner.lock"), b"not directory authority")?;
        let relative = LibraryPath::parse("parent/media")?;
        let opened = RetainedDirectoryChain::open(&boundary, &relative)?;
        let saved: DirectoryExpectation =
            serde_json::from_slice(&serde_json::to_vec(&opened.expected)?)?;
        drop(opened);
        let replaced = match target {
            "boundary" => &boundary,
            "parent" => &parent,
            _ => &media,
        };
        std::fs::rename(replaced, fixture.path().join("retained"))?;
        std::fs::create_dir_all(&media)?;
        std::fs::write(media.join("owner.lock"), b"not directory authority")?;
        assert!(matches!(
            RetainedDirectoryChain::reopen(&saved, &boundary, &relative),
            Err(Error::Conflict(_))
        ));
        assert_eq!(
            std::fs::read(media.join("owner.lock"))?,
            b"not directory authority"
        );
    }
    Ok(())
}

#[test]
fn logical_windows_profile_does_not_infer_sqlite_locality_from_native_identity() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let opened = RetainedDirectoryChain::open(fixture.path(), &LibraryPath::parse("")?)?;
    let mut config = configuration();
    config.roots.retain(|root| root.root_id == "state");
    let binding = config
        .roots
        .first_mut()
        .ok_or(Error::Internal("generated state binding is missing"))?;
    binding.expected = opened.expected.clone();
    let observation = opened.observation(binding);
    let selection = FrozenSelection::parse(&serde_json::to_vec(&config)?, &[PROFILE])?;
    let batch = ObservationBatch {
        selection_digest: selection.selection_digest.clone(),
        roots: [("state".into(), observation)].into(),
        relationships: BTreeMap::new(),
    };
    assert!(matches!(
        selection.admit(&selection.source_digest, &batch),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(std::fs::read_dir(fixture.path())?.count(), 0);
    Ok(())
}
