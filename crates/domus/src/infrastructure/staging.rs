use super::artwork::CoverJpeg;
use super::filesystem::NativeRoot;
use super::filesystem::{LibraryDirectory, is_unsafe_cap_link, is_unsafe_link, require_local_disk};
use super::media::{NormalizedAudio, normalize_audio};
use super::permissions::FilePermissions;
use super::store::{NormalizationRun, NormalizationWork, Store, TransferFile};
use crate::{Error, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, DirBuilder, OpenOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

mod playback;
mod progress;
mod replay;
use progress::ProgressIo;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputReceipt {
    profile: u8,
    transfer_id: i64,
    bytes: u64,
    sha256: String,
}

/// One worker owns the root's OS lock for its complete lifetime. All names below
/// this handle are application-controlled. No peer path is opened.
pub struct StagingRoot {
    directory: Dir,
    permissions: FilePermissions,
    identity: String,
    path: PathBuf,
    native: NativeRoot,
    _owner: File,
}

pub struct ReceivedInput {
    transfer_id: i64,
    bytes: u64,
    sha256: [u8; 32],
    file: File,
}

impl ReceivedInput {
    pub fn byte_length(&self) -> u64 {
        self.bytes
    }
    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }
}

impl Read for ReceivedInput {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buffer)
    }
}
impl Seek for ReceivedInput {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(position)
    }
}

pub struct NormalizationDirectory<'root> {
    directory: Dir,
    run: i64,
    cover_sha256: String,
    cover_bytes: u64,
    permissions: &'root FilePermissions,
}

fn conflict() -> Error {
    Error::Conflict("staging ownership, file contents or frozen recipe changed".into())
}

fn hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn recipe(run: &NormalizationRun) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "run_id": run.id,
        "job_id": run.job_id,
        "plan_version": run.plan_version,
        "claim_generation": run.claim_generation,
        "profile": run.profile,
        "recipe_sha256": run.recipe_sha256,
        "cover_sha256": run.cover_sha256,
        "cover_bytes": run.cover_bytes,
        "cover_width": run.cover_width,
        "cover_height": run.cover_height,
    }))?)
}

fn options(create: bool, write: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(write)
        .create_new(create)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        // The writable root marker needs concurrent opens for lock contention.
        // Every other writer is exclusive; sealed readers deny write sharing.
        options.share_mode(if write { 0 } else { 1 });
    }
    options
}

fn regular(file: File, permissions: &FilePermissions) -> Result<File> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_unsafe_link(&metadata) {
        return Err(Error::InvalidInput(
            "staging files must be regular and must not be links".into(),
        ));
    }
    permissions.check(&file, true)?;
    Ok(file)
}

fn open_file(directory: &Dir, name: &str, permissions: &FilePermissions) -> Result<File> {
    regular(
        directory
            .open_with(name, &options(false, false))?
            .into_std(),
        permissions,
    )
}

fn create_file(directory: &Dir, name: &str, permissions: &FilePermissions) -> Result<File> {
    regular(
        directory.open_with(name, &options(true, true))?.into_std(),
        permissions,
    )
}

fn existing_writer(directory: &Dir, name: &str, permissions: &FilePermissions) -> Result<File> {
    regular(
        directory.open_with(name, &options(false, true))?.into_std(),
        permissions,
    )
}

fn read_record(directory: &Dir, name: &str, permissions: &FilePermissions) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_file(directory, name, permissions)?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(Error::InvalidInput("staging record exceeds 4 KiB".into()));
    }
    Ok(bytes)
}

fn write_record(
    directory: &Dir,
    name: &str,
    bytes: &[u8],
    permissions: &FilePermissions,
) -> Result<()> {
    if bytes.len() > 4096 {
        return Err(Error::InvalidInput("staging record exceeds 4 KiB".into()));
    }
    let mut file = create_file(directory, name, permissions)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_directory(directory)?;
    let expected: [u8; 32] = Sha256::digest(bytes).into();
    if hash_file(&mut file, bytes.len() as u64)? != expected {
        return Err(conflict());
    }
    Ok(())
}

fn sync_directory(_directory: &Dir) -> Result<()> {
    #[cfg(unix)]
    _directory.try_clone()?.into_std_file().sync_all()?;
    // Windows sync_all on each writable file uses FlushFileBuffers. A Unix-style
    // directory fsync is not exposed by this adapter. Power-loss acceptance is separate.
    Ok(())
}

fn hash_file(file: &mut File, expected_bytes: u64) -> Result<[u8; 32]> {
    hash_file_with_progress(file, expected_bytes, &|| Ok(()))
}

fn hash_file_with_progress(
    file: &mut File,
    expected_bytes: u64,
    heartbeat: &dyn Fn() -> std::io::Result<()>,
) -> Result<[u8; 32]> {
    heartbeat()?;
    if file.metadata()?.len() != expected_bytes {
        return Err(conflict());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut remaining = expected_bytes;
    let mut buffer = [0u8; 65_536];
    while remaining > 0 {
        heartbeat()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if file.metadata()?.len() != expected_bytes {
        return Err(conflict());
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(hash.finalize().into())
}

fn check_directory(directory: &Dir, permissions: &FilePermissions) -> Result<()> {
    let metadata = directory.dir_metadata()?;
    if !metadata.is_dir() || is_unsafe_cap_link(&metadata) {
        return Err(Error::InvalidInput(
            "staging directories must not be links or reparse points".into(),
        ));
    }
    permissions.check(&directory.try_clone()?.into_std_file(), true)
}

impl StagingRoot {
    /// Open an existing owner-only root, reserve it against media registration,
    /// and bind it to this database's durable identity. Never delete owner.lock.
    pub fn open(store: &mut Store, path: &Path) -> Result<Self> {
        require_local_disk(path)?;
        let (path, opened) = LibraryDirectory::canonical(path)?;
        require_local_disk(&path)?;
        let native = opened.native_root().clone();
        let directory = opened.into_directory();
        let permissions = FilePermissions::current()?;
        check_directory(&directory, &permissions)?;
        let application_identity = store.application_identity()?;
        let identity = format!("domus-staging-v1\n{application_identity}\n");
        let marker_exists = match directory.symlink_metadata("owner.lock") {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if !marker_exists && directory.entries()?.next().transpose()?.is_some() {
            return Err(Error::Conflict(
                "an unowned staging root must be empty".into(),
            ));
        }
        let new_reservation = store.reserve_staging_root(&path, &native)?;
        store.check_native_root(
            path.to_str()
                .ok_or_else(|| Error::InvalidInput("staging root is not valid Unicode".into()))?,
            "staging",
            &native,
        )?;
        if !marker_exists && !new_reservation {
            return Err(Error::Conflict(
                "reserved staging root identity is missing; explicit recovery is required".into(),
            ));
        }
        let marker_options = options(!marker_exists, true);
        #[cfg(windows)]
        let marker_options = {
            use cap_std::fs::OpenOptionsExt;
            let mut marker_options = marker_options;
            marker_options.share_mode(3);
            marker_options
        };
        let mut owner = regular(
            directory
                .open_with("owner.lock", &marker_options)?
                .into_std(),
            &permissions,
        )?;
        match owner.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(Error::Conflict(
                    "staging is owned by another live worker".into(),
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        if !marker_exists {
            owner.write_all(identity.as_bytes())?;
            owner.sync_all()?;
            sync_directory(&directory)?;
        } else {
            let mut saved = Vec::new();
            (&mut owner).take(129).read_to_end(&mut saved)?;
            if saved != identity.as_bytes() {
                return Err(Error::Conflict(
                    "staging belongs to another database or its identity is incomplete".into(),
                ));
            }
        }
        Ok(Self {
            directory,
            permissions,
            identity: application_identity,
            path,
            native,
            _owner: owner,
        })
    }

    pub(crate) fn check_store(&self, store: &Store) -> Result<()> {
        if self.identity != store.application_identity()? {
            return Err(conflict());
        }
        store.check_native_root(
            self.path.to_str().ok_or_else(conflict)?,
            "staging",
            &self.native,
        )?;
        Ok(())
    }

    /// Consume an exact, already-active transfer stream. Failed files remain
    /// private and unreceipted. This does not submit or retry a peer request.
    pub fn receive(
        &self,
        transfer: &TransferFile,
        source: &mut impl Read,
    ) -> Result<ReceivedInput> {
        self.receive_with_progress(transfer, source, &|| Ok(()))
    }

    pub(crate) fn receive_with_progress(
        &self,
        transfer: &TransferFile,
        source: &mut impl Read,
        heartbeat: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<ReceivedInput> {
        if transfer.id <= 0 || transfer.expected_bytes <= 0 || transfer.state != "active" {
            return Err(Error::InvalidInput(
                "staging requires an active transfer with a positive size and ID".into(),
            ));
        }
        let bytes = transfer.expected_bytes as u64;
        let name = format!("transfer-{}.input", transfer.id);
        heartbeat()?;
        let mut output = create_file(&self.directory, &name, &self.permissions)?;
        let mut remaining = bytes;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65_536];
        while remaining > 0 {
            heartbeat()?;
            let count = remaining.min(buffer.len() as u64) as usize;
            source.read_exact(&mut buffer[..count])?;
            heartbeat()?;
            output.write_all(&buffer[..count])?;
            hash.update(&buffer[..count]);
            remaining -= count as u64;
        }
        if source.read(&mut buffer[..1])? != 0 {
            return Err(Error::InvalidInput(
                "transfer stream exceeds its expected size".into(),
            ));
        }
        heartbeat()?;
        output.sync_all()?;
        sync_directory(&self.directory)?;
        let sha256: [u8; 32] = hash.finalize().into();
        if hash_file_with_progress(&mut output, bytes, heartbeat)? != sha256 {
            return Err(conflict());
        }
        drop(output);
        heartbeat()?;
        write_record(
            &self.directory,
            &format!("transfer-{}.receipt", transfer.id),
            &serde_json::to_vec(&InputReceipt {
                profile: 1,
                transfer_id: transfer.id,
                bytes,
                sha256: hex(&sha256),
            })?,
            &self.permissions,
        )?;
        let mut file = open_file(&self.directory, &name, &self.permissions)?;
        if hash_file_with_progress(&mut file, bytes, heartbeat)? != sha256 {
            return Err(conflict());
        }
        heartbeat()?;
        Ok(ReceivedInput {
            transfer_id: transfer.id,
            bytes,
            sha256,
            file,
        })
    }

    pub fn open_input(&self, work: &NormalizationWork) -> Result<ReceivedInput> {
        self.open_input_with_progress(work, &|| Ok(()))
    }

    pub(crate) fn open_input_with_progress(
        &self,
        work: &NormalizationWork,
        heartbeat: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<ReceivedInput> {
        if work.transfer_id <= 0 || work.input_bytes == 0 {
            return Err(conflict());
        }
        let input = self.read_received(work.transfer_id, work.input_bytes, heartbeat)?;
        if hex(input.sha256()) != work.input_sha256 {
            return Err(conflict());
        }
        Ok(input)
    }

    /// Reconcile a fully sealed local transfer without another peer submission.
    /// A missing/partial receipt is not proof that the stream completed.
    pub fn received(&self, transfer: &TransferFile) -> Result<ReceivedInput> {
        if transfer.id <= 0
            || transfer.expected_bytes <= 0
            || !matches!(transfer.state.as_str(), "active" | "received")
        {
            return Err(conflict());
        }
        let input = self.read_received(transfer.id, transfer.expected_bytes as u64, &|| Ok(()))?;
        if transfer
            .payload_sha256
            .as_ref()
            .is_some_and(|hash| hash != input.sha256())
        {
            return Err(conflict());
        }
        Ok(input)
    }

    fn read_received(
        &self,
        transfer_id: i64,
        bytes: u64,
        heartbeat: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<ReceivedInput> {
        let receipt: InputReceipt = serde_json::from_slice(&read_record(
            &self.directory,
            &format!("transfer-{transfer_id}.receipt"),
            &self.permissions,
        )?)?;
        if receipt.profile != 1 || receipt.transfer_id != transfer_id || receipt.bytes != bytes {
            return Err(conflict());
        }
        let mut file = open_file(
            &self.directory,
            &format!("transfer-{transfer_id}.input"),
            &self.permissions,
        )?;
        let sha256 = hash_file_with_progress(&mut file, bytes, heartbeat)?;
        if hex(&sha256) != receipt.sha256 {
            return Err(conflict());
        }
        Ok(ReceivedInput {
            transfer_id,
            bytes,
            sha256,
            file,
        })
    }

    pub fn create_normalization(
        &self,
        run: &NormalizationRun,
    ) -> Result<NormalizationDirectory<'_>> {
        if run.id <= 0 || run.staging_directory != format!("normalization-{}", run.id) {
            return Err(conflict());
        }
        let mut builder = DirBuilder::new();
        builder.recursive(false);
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        self.directory
            .create_dir_with(&run.staging_directory, &builder)?;
        let directory = self.directory.open_dir_nofollow(&run.staging_directory)?;
        check_directory(&directory, &self.permissions)?;
        write_record(&directory, "recipe.json", &recipe(run)?, &self.permissions)?;
        sync_directory(&self.directory)?;
        Ok(NormalizationDirectory {
            directory,
            run: run.id,
            cover_sha256: run.cover_sha256.clone(),
            cover_bytes: run.cover_bytes,
            permissions: &self.permissions,
        })
    }

    pub fn open_normalization(&self, run: &NormalizationRun) -> Result<NormalizationDirectory<'_>> {
        if run.id <= 0 || run.staging_directory != format!("normalization-{}", run.id) {
            return Err(conflict());
        }
        let directory = self.directory.open_dir_nofollow(&run.staging_directory)?;
        check_directory(&directory, &self.permissions)?;
        if read_record(&directory, "recipe.json", &self.permissions)? != recipe(run)? {
            return Err(conflict());
        }
        Ok(NormalizationDirectory {
            directory,
            run: run.id,
            cover_sha256: run.cover_sha256.clone(),
            cover_bytes: run.cover_bytes,
            permissions: &self.permissions,
        })
    }
}

impl NormalizationDirectory<'_> {
    pub(crate) fn publication_input(
        &self,
        work: Option<&NormalizationWork>,
        heartbeat: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<File> {
        let (name, bytes, hash) = if let Some(work) = work {
            let receipt = work.receipt.as_ref().ok_or_else(conflict)?;
            if work.run_id != self.run
                || work.track_number == 0
                || work.track_number > 1000
                || work.staging_name
                    != format!("track-{:04}.{}", work.track_number, work.format.extension())
                || receipt.cover_sha256 != self.cover_sha256
                || receipt.input_sha256 != work.input_sha256
                || receipt.tags_sha256 != work.tags_sha256
            {
                return Err(conflict());
            }
            (
                work.staging_name.as_str(),
                receipt.bytes,
                receipt.sha256.clone(),
            )
        } else {
            ("folder.jpg", self.cover_bytes, self.cover_sha256.clone())
        };
        let mut file = open_file(&self.directory, name, self.permissions)?;
        if hex(&hash_file_with_progress(&mut file, bytes, heartbeat)?) != hash {
            return Err(conflict());
        }
        Ok(file)
    }

    /// Create fresh output or prove an interrupted output byte-for-byte without
    /// rewriting it. Save the synced evidence using the live fenced claim.
    pub fn normalize(
        &self,
        work: &NormalizationWork,
        input: &mut ReceivedInput,
        cover: &CoverJpeg,
    ) -> Result<NormalizedAudio> {
        self.normalize_with_progress(work, input, cover, &|| Ok(()))
    }

    pub(crate) fn normalize_with_progress(
        &self,
        work: &NormalizationWork,
        input: &mut ReceivedInput,
        cover: &CoverJpeg,
        heartbeat: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<NormalizedAudio> {
        heartbeat()?;
        if work.run_id != self.run
            || input.transfer_id != work.transfer_id
            || input.bytes != work.input_bytes
            || hex(input.sha256()) != work.input_sha256
            || work.receipt.is_some()
            || work.track_number == 0
            || work.track_number > 1000
            || work.staging_name
                != format!("track-{:04}.{}", work.track_number, work.format.extension())
            || hex(cover.sha256()) != self.cover_sha256
            || cover.bytes().len() as u64 != self.cover_bytes
        {
            return Err(conflict());
        }
        let (mut output, recovered) =
            match create_file(&self.directory, &work.staging_name, self.permissions) {
                Ok(file) => (file, false),
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => (
                    existing_writer(&self.directory, &work.staging_name, self.permissions)?,
                    true,
                ),
                Err(error) => return Err(error),
            };
        let mut input = ProgressIo::new(input, heartbeat);
        let evidence = if recovered {
            let mut replay = replay::ReplayOutput::new(&mut output)?;
            normalize_audio(
                &mut input,
                &mut ProgressIo::new(&mut replay, heartbeat),
                work.format,
                &work.tags,
                cover,
            )?
        } else {
            normalize_audio(
                &mut input,
                &mut ProgressIo::new(&mut output, heartbeat),
                work.format,
                &work.tags,
                cover,
            )?
        };
        if evidence.input_sha256 != work.input_sha256 || evidence.tags_sha256 != work.tags_sha256 {
            return Err(conflict());
        }
        heartbeat()?;
        output.sync_all()?;
        sync_directory(&self.directory)?;
        if hex(&hash_file_with_progress(
            &mut output,
            evidence.bytes,
            heartbeat,
        )?) != evidence.sha256
        {
            return Err(conflict());
        }
        Ok(evidence)
    }

    pub fn write_cover(&self, cover: &CoverJpeg) -> Result<()> {
        if hex(cover.sha256()) != self.cover_sha256
            || cover.bytes().len() as u64 != self.cover_bytes
        {
            return Err(conflict());
        }
        let mut file = match create_file(&self.directory, "folder.jpg", self.permissions) {
            Ok(mut file) => {
                file.write_all(cover.bytes())?;
                file
            }
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                existing_writer(&self.directory, "folder.jpg", self.permissions)?
            }
            Err(error) => return Err(error),
        };
        file.sync_all()?;
        sync_directory(&self.directory)?;
        let expected: [u8; 32] = Sha256::digest(cover.bytes()).into();
        if hash_file(&mut file, self.cover_bytes)? != expected {
            return Err(conflict());
        }
        Ok(())
    }

    pub fn verify_audio(&self, work: &NormalizationWork) -> Result<()> {
        self.verify_audio_with_progress(work, &|| Ok(()))
    }

    pub(crate) fn verify_audio_with_progress(
        &self,
        work: &NormalizationWork,
        heartbeat: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<()> {
        self.publication_input(Some(work), heartbeat)?;
        Ok(())
    }

    pub fn verify_cover(&self) -> Result<()> {
        let mut file = open_file(&self.directory, "folder.jpg", self.permissions)?;
        if hex(&hash_file(&mut file, self.cover_bytes)?) != self.cover_sha256 {
            return Err(conflict());
        }
        Ok(())
    }
}
