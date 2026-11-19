use super::LibraryDirectory;
use crate::domain::LibraryPath;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Versioned identity codecs, not filesystem locality or relationship evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum DirectoryIdentityProfile {
    #[serde(rename = "windows-file-id128-chain-v1")]
    WindowsFileId128,
    #[serde(rename = "linux-exportfs-chain-v1")]
    LinuxExportfs,
}

/// Expected identities from the selected provider boundary through every child.
///
/// The caller must obtain this expectation from trusted configuration or an
/// explicitly admitted saved binding. Inspection does not authorize a root.
/// Provider scope, role, topology and approval checks remain separate.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryExpectation {
    pub profile: DirectoryIdentityProfile,
    pub components: Vec<String>,
}

/// A verified directory and its retained ancestor handles.
///
/// Native identity does not admit a storage role or prove topology. Keep this
/// guard alive while using its metadata; it never exposes a bare directory.
pub struct RetainedDirectory {
    pub(super) directory: LibraryDirectory,
}

impl RetainedDirectory {
    pub(crate) fn directory(&self) -> &LibraryDirectory {
        &self.directory
    }

    /// Read the opened object's metadata without resolving its former path.
    pub fn metadata(&self) -> Result<std::fs::Metadata> {
        Ok(self.directory.directory_file()?.metadata()?)
    }
}

impl DirectoryExpectation {
    /// Inspect without saving or approving a binding. Return the retained leaf
    /// so the caller can keep authority through its subsequent admission work.
    pub fn inspect(boundary: &Path, relative: &LibraryPath) -> Result<(Self, RetainedDirectory)> {
        Self::inspect_in(&LibraryDirectory::open(boundary)?, relative)
    }

    pub(crate) fn inspect_in(
        boundary: &LibraryDirectory,
        relative: &LibraryPath,
    ) -> Result<(Self, RetainedDirectory)> {
        let profile = current_profile()?;
        let mut directory = boundary.try_clone()?;
        let mut components = vec![persistent_identity(&directory)?];
        for component in relative.as_str().split('/').filter(|name| !name.is_empty()) {
            directory = directory.open_directory(&LibraryPath::parse(component)?)?;
            components.push(persistent_identity(&directory)?);
        }
        Ok((
            Self {
                profile,
                components,
            },
            RetainedDirectory { directory },
        ))
    }

    /// Verify each component before opening its next child without following
    /// links. Ancestors above the selected boundary stay pinned, not persisted.
    pub fn open(&self, boundary: &Path, relative: &LibraryPath) -> Result<RetainedDirectory> {
        self.validate(relative)?;
        let components: Vec<_> = relative
            .as_str()
            .split('/')
            .filter(|name| !name.is_empty())
            .collect();
        let mut directory = LibraryDirectory::open(boundary)?;
        verify(&directory, &self.components[0])?;
        for (component, expected) in components.iter().zip(self.components.iter().skip(1)) {
            directory = directory.open_directory(&LibraryPath::parse(component)?)?;
            verify(&directory, expected)?;
        }
        Ok(RetainedDirectory { directory })
    }

    pub(crate) fn validate(&self, relative: &LibraryPath) -> Result<()> {
        if self.profile != current_profile()? {
            return Err(Error::InvalidInput(
                "directory identity profile is unsupported on this host".into(),
            ));
        }
        let count = relative
            .as_str()
            .split('/')
            .filter(|name| !name.is_empty())
            .count()
            + 1;
        let hex = |value: &str, length: usize| {
            value.len() == length
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        let valid = |value: &str| {
            let parts: Vec<_> = value.split(':').collect();
            match self.profile {
                DirectoryIdentityProfile::WindowsFileId128 => {
                    parts.len() == 3
                        && parts[0] == "win128"
                        && hex(parts[1], 16)
                        && hex(parts[2], 32)
                }
                DirectoryIdentityProfile::LinuxExportfs => {
                    parts.len() == 4
                        && parts[0] == "linuxfh"
                        && hex(parts[1], 16)
                        && hex(parts[2], 8)
                        && !parts[3].is_empty()
                        && parts[3].len() <= 256
                        && parts[3].len().is_multiple_of(2)
                        && hex(parts[3], parts[3].len())
                }
            }
        };
        if self.components.len() != count
            || self.components.iter().any(|component| !valid(component))
        {
            return Err(Error::InvalidInput(
                "directory binding must contain a supported complete native chain".into(),
            ));
        }
        Ok(())
    }
}

fn verify(directory: &LibraryDirectory, expected: &str) -> Result<()> {
    if persistent_identity(directory)? != expected {
        return Err(Error::Conflict(
            "directory differs from the approved native binding".into(),
        ));
    }
    Ok(())
}

fn current_profile() -> Result<DirectoryIdentityProfile> {
    #[cfg(windows)]
    {
        Ok(DirectoryIdentityProfile::WindowsFileId128)
    }
    #[cfg(target_os = "linux")]
    {
        Ok(DirectoryIdentityProfile::LinuxExportfs)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err(Error::InvalidInput(
            "this host has no supported persistent directory identity profile".into(),
        ))
    }
}

fn persistent_identity(directory: &LibraryDirectory) -> Result<String> {
    persistent_file_identity(&directory.directory_file()?)
}

pub(super) fn persistent_file_identity(file: &std::fs::File) -> Result<String> {
    #[cfg(windows)]
    {
        super::native_file_identity(file)
    }
    #[cfg(target_os = "linux")]
    {
        let handle = native_file_security::filesystem_handle(file)?;
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        Ok(format!(
            "linuxfh:{}:{:08x}:{}",
            hex(&handle.filesystem_id),
            handle.handle_type,
            hex(&handle.bytes)
        ))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = file;
        Err(Error::InvalidInput(
            "this host has no supported persistent directory identity profile".into(),
        ))
    }
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;

    fn fixture() -> Result<tempfile::TempDir> {
        Ok(tempfile::Builder::new()
            .prefix(".directory-binding-")
            .tempdir_in(std::env::current_dir()?)?)
    }

    #[test]
    fn directory_binding_reopens_the_serialized_complete_chain() -> Result<()> {
        let fixture = fixture()?;
        fs::create_dir_all(fixture.path().join("parent").join("media"))?;
        fs::write(
            fixture.path().join("parent").join("media").join("sentinel"),
            b"retained evidence",
        )?;
        let relative = LibraryPath::parse("parent/media")?;
        let (expected, directory) = DirectoryExpectation::inspect(fixture.path(), &relative)?;
        assert_eq!(expected.components.len(), 3);
        let bytes = serde_json::to_vec(&expected)?;
        drop(directory);
        let saved: DirectoryExpectation = serde_json::from_slice(&bytes)?;
        let reopened = saved.open(fixture.path(), &relative)?;
        let mut actual = Vec::new();
        reopened
            .directory
            .open_file(std::ffi::OsStr::new("sentinel"))?
            .take(128)
            .read_to_end(&mut actual)?;
        assert_eq!(actual, b"retained evidence");
        let mut incomplete = saved;
        incomplete.components.remove(0);
        assert!(incomplete.open(fixture.path(), &relative).is_err());
        Ok(())
    }

    #[test]
    fn directory_binding_rejects_replaced_boundaries_parents_and_leaves() -> Result<()> {
        for component in ["boundary", "parent", "media"] {
            let fixture = fixture()?;
            let boundary = fixture.path().join("boundary");
            let parent = boundary.join("parent");
            let media = parent.join("media");
            fs::create_dir_all(&media)?;
            fs::write(media.join("owner.lock"), b"copied marker")?;
            let relative = LibraryPath::parse("parent/media")?;
            let (expected, directory) = DirectoryExpectation::inspect(&boundary, &relative)?;
            drop(directory);
            let replaced = match component {
                "boundary" => &boundary,
                "parent" => &parent,
                _ => &media,
            };
            let retained = fixture.path().join("retained");
            fs::rename(replaced, &retained)?;
            fs::create_dir_all(&parent)?;
            if component == "media" {
                fs::create_dir(&media)?;
                fs::write(media.join("owner.lock"), b"copied marker")?;
            } else {
                let original_media = if component == "boundary" {
                    retained.join("parent").join("media")
                } else {
                    retained.join("media")
                };
                fs::rename(original_media, &media)?;
                let (actual, _directory) = DirectoryExpectation::inspect(&boundary, &relative)?;
                assert_eq!(actual.components.last(), expected.components.last());
            }
            assert!(matches!(
                expected.open(&boundary, &relative),
                Err(Error::Conflict(_))
            ));
            assert_eq!(fs::read(media.join("owner.lock"))?, b"copied marker");
        }
        Ok(())
    }

    #[test]
    fn directory_binding_ignores_only_ancestors_above_selected_boundary() -> Result<()> {
        let fixture = fixture()?;
        let first = fixture.path().join("first").join("provider");
        let second = fixture.path().join("second").join("provider");
        fs::create_dir_all(first.join("parent").join("media"))?;
        fs::create_dir(fixture.path().join("second"))?;
        let relative = LibraryPath::parse("parent/media")?;
        let (expected, directory) = DirectoryExpectation::inspect(&first, &relative)?;
        let ancestors = directory.directory.native_root().ancestors.clone();
        drop(directory);
        fs::rename(&first, &second)?;
        let reopened = expected.open(&second, &relative)?;
        assert_ne!(ancestors, reopened.directory.native_root().ancestors);
        assert!(reopened.metadata()?.is_dir());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn directory_binding_rejects_alias_links_to_the_original_leaf() -> Result<()> {
        let fixture = fixture()?;
        fs::create_dir_all(fixture.path().join("parent").join("media"))?;
        let relative = LibraryPath::parse("parent/media")?;
        let (expected, directory) = DirectoryExpectation::inspect(fixture.path(), &relative)?;
        drop(directory);
        fs::rename(
            fixture.path().join("parent"),
            fixture.path().join("retained"),
        )?;
        std::os::unix::fs::symlink(
            fixture.path().join("retained"),
            fixture.path().join("parent"),
        )?;
        assert!(expected.open(fixture.path(), &relative).is_err());
        Ok(())
    }
}
