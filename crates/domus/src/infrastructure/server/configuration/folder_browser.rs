use super::{ConfigurationService, digest};
use crate::domain::LibraryPath;
use crate::infrastructure::filesystem::{LibraryDirectory, validate_native_reference};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PAGE_SIZE: usize = 128;
const MAX_SESSIONS: usize = 16;
const SESSION_LIFETIME: Duration = Duration::from_secs(180);

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct BrowseHostFolder {
    pub path: String,
    pub expected_identity: Option<String>,
    pub cursor: Option<String>,
}

#[derive(Serialize, ts_rs::TS)]
pub struct HostFolderEntry {
    pub name: String,
    pub path: String,
    pub identity: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Serialize, ts_rs::TS)]
pub struct HostFolderPage {
    pub path: String,
    pub identity: String,
    pub parent_path: Option<String>,
    pub entries: Vec<HostFolderEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Default)]
pub(super) struct FolderBrowser {
    sessions: BTreeMap<String, FolderSession>,
}

struct FolderSession {
    path: PathBuf,
    directory: LibraryDirectory,
    iterator: cap_std::fs::ReadDir,
    identity: String,
    version: String,
    expires: Instant,
}

fn version(directory: &LibraryDirectory) -> Result<String> {
    let metadata = directory.directory_file()?.metadata()?;
    Ok(digest(
        format!(
            "{}:{:?}",
            directory.native_root().identity,
            metadata.modified()?
        )
        .as_bytes(),
    ))
}

pub(super) fn validate_folder_path(path: &Path) -> Result<()> {
    let path_text = path.as_os_str().as_encoded_bytes();
    if path_text.len() > 4096
        || !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(Error::InvalidInput(
            "Enter an absolute folder path on the application host without parent traversal."
                .into(),
        ));
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let allowed = match path.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(_) | Prefix::VerbatimDisk(_) => true,
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    let valid_part = |part: &std::ffi::OsStr| {
                        part.to_str().is_some_and(|part| {
                            !matches!(part, "" | "." | "..")
                                && !part.contains([':', '/', '\\'])
                                && !part.chars().any(char::is_control)
                        })
                    };
                    valid_part(server)
                        && valid_part(share)
                        && !["IPC$", "PIPE", "MAILSLOT"]
                            .iter()
                            .any(|name| share.eq_ignore_ascii_case(name))
                }
                _ => false,
            },
            _ => false,
        };
        if !allowed {
            return Err(Error::InvalidInput(
                "Enter a Windows drive path or an SMB file share path such as \\\\server\\share."
                    .into(),
            ));
        }
    }
    validate_native_reference(path)
}

impl ConfigurationService {
    pub(crate) fn host_folder_roots(&self) -> Result<Vec<String>> {
        self.require_editable()?;
        self.require_revision(
            self.active_revision(),
            &self.read_file(&self.directory()?)?.1,
        )?;
        #[cfg(windows)]
        {
            let drives = winsafe::GetLogicalDriveStrings()
                .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;
            Ok(drives
                .into_iter()
                .filter(|drive| {
                    let kind = winsafe::GetDriveType(Some(drive));
                    kind == winsafe::co::DRIVE::FIXED
                        || kind == winsafe::co::DRIVE::REMOVABLE
                        || kind == winsafe::co::DRIVE::RAMDISK
                })
                .collect())
        }
        #[cfg(unix)]
        {
            Ok(vec!["/".into()])
        }
    }

    pub(crate) fn close_host_folder(&self, cursor: &str) -> Result<()> {
        self.folder_browser
            .lock()
            .map_err(|_| Error::Internal("folder browser lock failed"))?
            .sessions
            .remove(cursor);
        Ok(())
    }

    /// A picker observes folders only. It neither grants a storage role nor
    /// establishes that a network filesystem supports media publication.
    pub(crate) fn browse_host_folder(&self, request: BrowseHostFolder) -> Result<HostFolderPage> {
        self.require_editable()?;
        self.require_revision(
            self.active_revision(),
            &self.read_file(&self.directory()?)?.1,
        )?;
        let path = Path::new(&request.path);
        validate_folder_path(path)?;
        let mut browser = self
            .folder_browser
            .lock()
            .map_err(|_| Error::Internal("folder browser lock failed"))?;
        let now = Instant::now();
        browser.sessions.retain(|_, session| session.expires > now);
        let (id, mut session) = if let Some(cursor) = request.cursor {
            let session = browser.sessions.remove(&cursor).ok_or_else(|| {
                Error::Conflict("Folder listing expired. Reload this folder.".into())
            })?;
            if session.path != path
                || request.expected_identity.as_deref() != Some(&session.identity)
            {
                return Err(Error::Conflict(
                    "The listing belongs to another folder. Reload this folder.".into(),
                ));
            }
            (cursor, session)
        } else {
            if browser.sessions.len() >= MAX_SESSIONS {
                return Err(Error::Busy(
                    "Close or reload a folder browser before opening another",
                ));
            }
            let directory = LibraryDirectory::open(path)?;
            self.state_storage
                .client()
                .require_media_directory(&directory)?;
            let identity = directory.native_root().identity.clone();
            if request
                .expected_identity
                .is_some_and(|expected| expected != identity)
            {
                return Err(Error::Conflict(
                    "This folder changed. Reload it before selecting it.".into(),
                ));
            }
            let current_version = version(&directory)?;
            let iterator = directory.entries()?;
            let mut nonce = [0; 32];
            rustls::crypto::ring::default_provider()
                .secure_random
                .fill(&mut nonce)
                .map_err(|_| Error::Internal("folder listing generation failed"))?;
            let id = digest(&nonce);
            (
                id,
                FolderSession {
                    path: path.to_owned(),
                    directory,
                    iterator,
                    identity,
                    version: current_version,
                    expires: now + SESSION_LIFETIME,
                },
            )
        };
        session.directory.check_named_directory(&|| Ok(()))?;
        if version(&session.directory)? != session.version {
            return Err(Error::Conflict(
                "Folder contents changed. Reload this folder.".into(),
            ));
        }
        let mut entries = Vec::new();
        let mut complete = false;
        for _ in 0..PAGE_SIZE {
            let Some(entry) = session.iterator.next() else {
                complete = true;
                break;
            };
            let entry = entry?;
            let metadata = entry.metadata()?;
            let unsafe_link = crate::infrastructure::filesystem::is_unsafe_cap_link(&metadata);
            if !metadata.is_dir() && !unsafe_link {
                continue;
            }
            let name = entry.file_name().into_string().map_err(|_| {
                Error::InvalidInput("This folder contains a name that is not Unicode.".into())
            })?;
            let child_path = session.path.join(&name);
            let mut item = HostFolderEntry {
                name: name.clone(),
                path: child_path.to_string_lossy().into_owned(),
                identity: None,
                unavailable_reason: None,
            };
            if unsafe_link {
                item.unavailable_reason =
                    Some("Links and reparse points cannot be selected.".into());
            } else {
                let relative = LibraryPath::parse(&name)?;
                let child = match session.directory.open_directory(&relative) {
                    Ok(child) => child,
                    Err(Error::Io(error))
                        if error.kind() == std::io::ErrorKind::PermissionDenied =>
                    {
                        item.unavailable_reason =
                            Some("The application account cannot open this folder.".into());
                        entries.push(item);
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                match self.state_storage.client().require_media_directory(&child) {
                    Ok(()) => item.identity = Some(child.native_root().identity.clone()),
                    Err(Error::Conflict(_)) => {
                        item.unavailable_reason = Some("Private application storage.".into());
                    }
                    Err(error) => return Err(error),
                }
            }
            entries.push(item);
        }
        session.directory.check_named_directory(&|| Ok(()))?;
        if version(&session.directory)? != session.version {
            return Err(Error::Conflict(
                "Folder contents changed. Reload this folder.".into(),
            ));
        }
        self.require_revision(
            self.active_revision(),
            &self.read_file(&self.directory()?)?.1,
        )?;
        let result = HostFolderPage {
            path: session.path.to_string_lossy().into_owned(),
            identity: session.identity.clone(),
            parent_path: session
                .path
                .parent()
                .filter(|parent| parent.is_absolute())
                .map(|parent| parent.to_string_lossy().into_owned()),
            entries,
            next_cursor: (!complete).then_some(id.clone()),
        };
        if !complete {
            browser.sessions.insert(id, session);
        }
        Ok(result)
    }
}

#[cfg(all(test, windows))]
#[test]
fn backend_configuration_folder_browser_accepts_smb_shares_but_not_device_paths() {
    for path in [
        r"D:\Music",
        r"\\generated-server\Music",
        r"\\generated-server\Music\Albums",
        r"\\?\UNC\generated-server\Music\Albums",
    ] {
        assert!(validate_folder_path(Path::new(path)).is_ok(), "{path}");
    }
    for path in [
        r"\\generated-server",
        r"\\generated-server\Music\..\Private",
        r"\\generated-server\Music\Album:stream",
        r"\\generated-server\ipc$",
        r"\\generated-server\pipe",
        r"\\.\pipe\domus",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1",
    ] {
        assert!(validate_folder_path(Path::new(path)).is_err(), "{path}");
    }
}
