use super::{
    Client, Kind, ListedEntry, MAX_DEPTH, MAX_LIVE, MAX_PAGE, MAX_READ, MediaSelection,
    NativeAdapter, Observation, Reference, Request, Response, Selection, Session,
};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

mod mutation;
pub use mutation::MediaOutput;
mod moves;
pub use moves::MediaMove;

/// One bounded media session, either native or explicitly paired. Neither
/// variant exposes a native path or private-storage lease. New output requires
/// an explicit selected-root write grant on both peers.
/// Close leases explicitly; dropping the client retires the complete session.
pub struct MediaClient {
    selection: MediaSelection,
    digest: String,
    owner: String,
    transport: Transport,
    live: BTreeMap<u64, Live>,
    available: bool,
}

enum Transport {
    Local {
        native: Arc<NativeAdapter>,
        session: Arc<Mutex<Session>>,
        slot: Arc<Semaphore>,
    },
    Remote(Client),
}

struct Live {
    writable: bool,
    reference: Reference,
    parent: Option<u64>,
    depth: usize,
    observed: Observation,
    complete: bool,
}

/// Session-owned authority, not a serializable binding or reusable token.
pub struct MediaDirectory {
    owner: String,
    reference: Reference,
    observed: Observation,
}

/// Reads use this exact opened file and its original observation.
pub struct MediaFile {
    owner: String,
    reference: Reference,
    observed: Observation,
}

pub enum MediaNode {
    Directory(MediaDirectory),
    File(MediaFile),
}

pub struct MediaPage {
    pub entries: Vec<ListedEntry>,
    pub complete: bool,
}

impl MediaDirectory {
    pub fn observation(&self) -> &Observation {
        &self.observed
    }
}

impl MediaFile {
    pub fn observation(&self) -> &Observation {
        &self.observed
    }
}

impl MediaClient {
    /// Validate and pin the same native selection as the foreground adapter.
    /// This performs blocking native admission and must run off the executor.
    pub fn local(selection: Selection, protected_inputs: Vec<File>) -> Result<Self> {
        let logical = selection.logical();
        let native = NativeAdapter::new(selection, protected_inputs)?;
        Self::new(
            logical,
            Transport::Local {
                native: Arc::new(native),
                session: Arc::new(Mutex::new(Session::new()?)),
                slot: Arc::new(Semaphore::new(1)),
            },
        )
    }

    /// A foreign identity codec is meaningful only to the explicitly paired
    /// peer. This never admits it to FilesystemClient or opens a local path.
    pub fn remote(selection: MediaSelection, client: Client) -> Result<Self> {
        Self::new(selection, Transport::Remote(client))
    }

    fn new(selection: MediaSelection, transport: Transport) -> Result<Self> {
        selection.validate()?;
        Ok(Self {
            digest: selection.digest()?,
            owner: Session::new()?.connection,
            selection,
            transport,
            live: BTreeMap::new(),
            available: true,
        })
    }

    pub async fn open_root(&mut self, root_id: &str) -> Result<MediaDirectory> {
        self.capacity()?;
        let writable = self.selection.permits_writes(root_id)?;
        let root = self
            .selection
            .media
            .iter()
            .find(|root| root.root_id == root_id)
            .cloned()
            .ok_or_else(|| Error::InvalidInput("media root is not selected".into()))?;
        let response = self
            .execute(Request::OpenRoot {
                selection: self.digest.clone(),
                root,
            })
            .await?;
        match self.insert(response, None)? {
            MediaNode::Directory(directory) => {
                self.live
                    .get_mut(&directory.reference.id)
                    .ok_or(Error::Internal("media root lease disappeared"))?
                    .writable = writable;
                Ok(directory)
            }
            MediaNode::File(_) => self.protocol_error(),
        }
    }

    pub async fn page(&mut self, directory: &MediaDirectory, limit: usize) -> Result<MediaPage> {
        let live = self.require(&directory.owner, &directory.reference, Kind::Directory)?;
        if !(1..=MAX_PAGE).contains(&limit) || live.complete {
            return Err(Error::InvalidInput(
                "media page limit is invalid or enumeration is already complete".into(),
            ));
        }
        let response = self
            .execute(Request::Page {
                directory: directory.reference.clone(),
                limit,
            })
            .await?;
        let Response::Page { entries, complete } = response else {
            return self.protocol_error();
        };
        let mut names = BTreeSet::new();
        for entry in &entries {
            if !names.insert(&entry.name) || self.check_observation(&entry.observed).is_err() {
                return self.protocol_error();
            }
        }
        self.live
            .get_mut(&directory.reference.id)
            .ok_or(Error::Internal("media directory lease disappeared"))?
            .complete = complete;
        Ok(MediaPage { entries, complete })
    }

    pub async fn open_child(
        &mut self,
        parent: &MediaDirectory,
        entry: &ListedEntry,
    ) -> Result<MediaNode> {
        self.capacity()?;
        let live = self.require(&parent.owner, &parent.reference, Kind::Directory)?;
        if live.depth >= MAX_DEPTH {
            return Err(Error::InvalidInput(
                "media descent depth exceeds its limit".into(),
            ));
        }
        let name = crate::domain::LibraryPath::parse(&entry.name)?;
        if name.as_str().is_empty() || name.as_str().contains('/') || entry.name.len() > 1024 {
            return Err(Error::InvalidInput(
                "media child requires one component".into(),
            ));
        }
        self.check_observation(&entry.observed)?;
        let response = self
            .execute(Request::OpenChild {
                parent: parent.reference.clone(),
                name: entry.name.clone(),
                expected: entry.observed.clone(),
            })
            .await?;
        self.insert(response, Some(parent.reference.id))
    }

    /// Return one exact bounded range. EOF is checked against the opened
    /// observation; a short or contradictory success disables this session.
    pub async fn read(&mut self, file: &MediaFile, offset: u64, count: usize) -> Result<Vec<u8>> {
        let live = self.require(&file.owner, &file.reference, Kind::File)?;
        if !(1..=MAX_READ).contains(&count) || offset > live.observed.length {
            return Err(Error::InvalidInput("invalid bounded media read".into()));
        }
        let length = (live.observed.length - offset).min(count as u64) as usize;
        let eof = offset + length as u64 == live.observed.length;
        let response = self
            .execute(Request::Read {
                file: file.reference.clone(),
                offset,
                count,
            })
            .await?;
        match response {
            Response::Read { bytes, complete } if bytes.len() == length && complete == eof => {
                Ok(bytes)
            }
            _ => self.protocol_error(),
        }
    }

    pub async fn close_directory(&mut self, directory: MediaDirectory) -> Result<()> {
        self.require(&directory.owner, &directory.reference, Kind::Directory)?;
        self.close(directory.reference).await
    }

    pub async fn close_file(&mut self, file: MediaFile) -> Result<()> {
        self.require(&file.owner, &file.reference, Kind::File)?;
        self.close(file.reference).await
    }

    async fn close(&mut self, reference: Reference) -> Result<()> {
        let id = reference.id;
        if !matches!(
            self.execute(Request::Close { reference }).await?,
            Response::Closed
        ) {
            return self.protocol_error();
        }
        let mut removed = BTreeSet::from([id]);
        loop {
            let before = removed.len();
            for (id, live) in &self.live {
                if live.parent.is_some_and(|parent| removed.contains(&parent)) {
                    removed.insert(*id);
                }
            }
            if removed.len() == before {
                break;
            }
        }
        self.live.retain(|id, _| !removed.contains(id));
        Ok(())
    }

    fn require(&self, owner: &str, reference: &Reference, kind: Kind) -> Result<&Live> {
        if owner != self.owner {
            return Err(Error::Conflict(
                "media lease belongs to another client".into(),
            ));
        }
        self.live
            .get(&reference.id)
            .filter(|live| live.reference == *reference && live.observed.kind == kind)
            .ok_or_else(|| Error::Conflict("media lease is closed or stale".into()))
    }

    fn capacity(&self) -> Result<()> {
        if self.live.len() >= MAX_LIVE {
            return Err(Error::Busy("native media capability limit reached"));
        }
        Ok(())
    }

    fn check_observation(&self, observed: &Observation) -> Result<()> {
        super::super::DirectoryExpectation {
            profile: self.selection.private.expected.profile,
            components: vec![observed.identity.clone()],
        }
        .validate_record(&crate::domain::LibraryPath::parse("")?)?;
        if observed.modified_nanoseconds >= 1_000_000_000
            || (observed.kind == Kind::File && observed.links == 0)
        {
            return Err(Error::InvalidInput(
                "invalid media modification time".into(),
            ));
        }
        Ok(())
    }

    fn insert(&mut self, response: Response, parent: Option<u64>) -> Result<MediaNode> {
        let Response::Opened {
            reference,
            observed,
        } = response
        else {
            return self.protocol_error();
        };
        if self.check_observation(&observed).is_err()
            || self.live.contains_key(&reference.id)
            || self
                .live
                .values()
                .any(|live| live.reference.connection != reference.connection)
        {
            return self.protocol_error();
        }
        let depth = match parent {
            Some(id) => {
                self.live
                    .get(&id)
                    .ok_or(Error::Internal("media parent lease disappeared"))?
                    .depth
                    + 1
            }
            None => 0,
        };
        self.live.insert(
            reference.id,
            Live {
                writable: parent
                    .and_then(|id| self.live.get(&id))
                    .is_some_and(|parent| parent.writable),
                reference: reference.clone(),
                parent,
                depth,
                observed: observed.clone(),
                complete: false,
            },
        );
        Ok(match observed.kind {
            Kind::Directory => MediaNode::Directory(MediaDirectory {
                owner: self.owner.clone(),
                reference,
                observed,
            }),
            Kind::File => MediaNode::File(MediaFile {
                owner: self.owner.clone(),
                reference,
                observed,
            }),
        })
    }

    async fn execute(&mut self, request: Request) -> Result<Response> {
        if !self.available {
            return Err(Error::Remote(
                "media client is unavailable; reopen explicitly",
            ));
        }
        // Cancellation can lose a newly created capability or consume a page.
        // Do not continue that session after an unfinished operation.
        self.available = false;
        let result = match &mut self.transport {
            Transport::Remote(client) => client.execute(&request).await,
            Transport::Local {
                native,
                session,
                slot,
            } => {
                let permit = slot
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| Error::Busy("native media client is still draining"))?;
                let native = native.clone();
                let session = session.clone();
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    session
                        .lock()
                        .map_err(|_| Error::Internal("native media session lock failed"))?
                        .dispatch(&native, request)
                })
                .await
                .map_err(|_| Error::Internal("native media operation task failed"))?
            }
        };
        self.available = result.is_ok()
            || matches!(
                result,
                Err(Error::InvalidInput(_) | Error::Conflict(_) | Error::Busy(_))
            );
        result
    }

    fn protocol_error<T>(&mut self) -> Result<T> {
        self.available = false;
        Err(Error::Remote(
            "media peer returned inconsistent lease evidence",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Fixture;
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn media_adapter_cancelled_native_client_cannot_replay_a_lost_operation()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new()?;
        let mut client = MediaClient::local(fixture.selection, Vec::new())?;
        let Transport::Local { session, slot, .. } = &client.transport else {
            return Err("expected native transport".into());
        };
        let session = session.clone();
        let slot = slot.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let owner = std::thread::spawn(move || -> std::result::Result<(), &'static str> {
            let _locked = session.lock().map_err(|_| "fixture session lock failed")?;
            ready
                .send(())
                .map_err(|_| "fixture ready receiver disappeared")?;
            let _ = released.recv();
            Ok(())
        });
        waiting.recv_timeout(Duration::from_secs(3))?;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), client.open_root("library"))
                .await
                .is_err()
        );
        release.send(())?;
        owner.join().map_err(|_| "fixture session owner failed")??;
        assert!(matches!(
            client.open_root("library").await,
            Err(Error::Remote(_))
        ));
        let permit = tokio::time::timeout(Duration::from_secs(3), slot.acquire()).await??;
        drop(permit);
        Ok(())
    }

    #[tokio::test]
    async fn media_adapter_move_cancelled_reply_never_replays_an_uncertain_effect()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut fixture = Fixture::new()?;
        fixture.selection.writable_roots.push("library".into());
        let media = fixture
            .selection
            .provider
            .boundary
            .join("parent")
            .join("media");
        let mut client = MediaClient::local(fixture.selection, Vec::new())?;
        let mut root = client.open_root("library").await?;
        let page = client.page(&root, MAX_PAGE).await?;
        let listed = page.entries.first().ok_or("generated source missing")?;
        let source = client.prepare_move(&root, listed).await?;
        let Transport::Local { session, slot, .. } = &client.transport else {
            return Err("expected native transport".into());
        };
        let session = session.clone();
        let slot = slot.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let owner = std::thread::spawn(move || -> std::result::Result<(), &'static str> {
            let _locked = session.lock().map_err(|_| "fixture session lock failed")?;
            ready
                .send(())
                .map_err(|_| "fixture ready receiver disappeared")?;
            released
                .recv()
                .map_err(|_| "fixture release sender disappeared")?;
            Ok(())
        });
        waiting.recv_timeout(Duration::from_secs(3))?;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                client.rename_directory(&mut root, source, "final")
            )
            .await
            .is_err()
        );
        release.send(())?;
        owner.join().map_err(|_| "fixture session owner failed")??;
        let permit = tokio::time::timeout(Duration::from_secs(3), slot.acquire()).await??;
        drop(permit);
        assert!(!media.join("album").exists());
        assert!(media.join("final").join("track.flac").is_file());
        assert!(matches!(
            client.open_root("library").await,
            Err(Error::Remote(_))
        ));
        Ok(())
    }

    #[test]
    fn media_adapter_foreign_records_never_admit_local_filesystem_access() -> Result<()> {
        use crate::infrastructure::filesystem::{DirectoryExpectation, DirectoryIdentityProfile};
        let fixture = Fixture::new()?;
        #[cfg(windows)]
        let expected = DirectoryExpectation {
            profile: DirectoryIdentityProfile::LinuxExportfs,
            components: vec!["linuxfh:0000000000000001:00000001:01".into()],
        };
        #[cfg(not(windows))]
        let expected = DirectoryExpectation {
            profile: DirectoryIdentityProfile::WindowsFileId128,
            components: vec!["win128:0000000000000001:00000000000000000000000000000001".into()],
        };
        let root = crate::domain::LibraryPath::parse("")?;
        expected.validate_record(&root)?;
        assert!(
            expected
                .open(&fixture.selection.provider.boundary, &root)
                .is_err()
        );
        assert!(expected.validate(&root).is_err());
        Ok(())
    }
}
