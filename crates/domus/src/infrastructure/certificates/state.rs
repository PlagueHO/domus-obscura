//! Private native-root-bound journal. The lock covers all external effects.
//! An interrupted pending file is retained, not overwritten on the next attempt.
use crate::domain::certificates::{AcmeEnvironment, CertificatePhase, TxtReceipt};
use crate::infrastructure::filesystem::{
    LibraryDirectory, NativeRoot, rename_no_replace, sync_directory,
};
use crate::infrastructure::local_file::ProcessLock;
use crate::infrastructure::permissions::FilePermissions;
use crate::{Error, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAXIMUM: u64 = 524288;

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct CertificateStatus {
    pub phase: CertificatePhase,
    pub active_generation: Option<u64>,
    pub expires_at: Option<u64>,
    pub renew_at: Option<u64>,
    pub next_attempt: Option<u64>,
    pub attempts: u8,
    pub last_success: Option<u64>,
    pub recovery_required: bool,
    pub cleanup_pending: bool,
}

impl StateStore {
    /// Adopt only the exact saved-key certificate returned by the saved order.
    /// Unknown slots, partial files and different pairs remain unchanged.
    pub(crate) fn recover_generation(&mut self, certificate: &str, hostname: &str) -> Result<()> {
        self.check_current()?;
        if self.journal.phase != CertificatePhase::Paused
            || self.journal.resume_phase != Some(CertificatePhase::FinalizationPending)
            || self.journal.generations.len() >= 64
        {
            return Err(Error::Conflict(
                "generation recovery requires paused finalization and free history capacity".into(),
            ));
        }
        let number = self.journal.next_generation;
        let next = number
            .checked_add(1)
            .ok_or(Error::Internal("generation counter is exhausted"))?;
        let pending = format!(".generation-{number}.pending");
        let name = format!("generation-{number}.json");
        let exists = |name: &str| -> Result<bool> {
            match self.directory.symlink_metadata(name) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error.into()),
            }
        };
        let source = match (exists(&pending)?, exists(&name)?) {
            (true, false) => pending.as_str(),
            (false, true) => name.as_str(),
            _ => {
                return Err(Error::Conflict(
                    "generation recovery requires exactly one retained candidate file".into(),
                ));
            }
        };
        let bytes = read(&self.directory, source)?;
        let generation: Generation = serde_json::from_slice(&bytes).map_err(|_| {
            Error::InvalidInput(
                "retained generation is incomplete; preserve it for manual recovery".into(),
            )
        })?;
        if generation.hostname != hostname
            || generation.certificate != certificate
            || self.journal.certificate_key.as_ref() != Some(&generation.private_key)
        {
            return Err(Error::Conflict(
                "retained generation differs from the saved order and key".into(),
            ));
        }
        self.check_generation(number, &generation)?;
        self.preserve_recovery_evidence()?;
        if source == pending {
            rename_no_replace(&self.directory, &pending, &name)?;
            sync_directory(&self.directory)?;
        }
        self.journal.generations.insert(number, hash(&bytes));
        self.journal.next_generation = next;
        if let Some(schedule) = generation.initial_renewal {
            self.journal.renewal = Some(schedule);
        }
        self.journal.candidate_generation = Some(number);
        self.journal.phase = CertificatePhase::CleanupPending;
        self.journal.resume_phase = None;
        self.journal.pause_reason = None;
        self.save()
    }
}

/// Never export this type as a browser contract: it includes private account
/// credentials and key material. Only an explicit local owner can change it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    version: u8,
    root: NativeRoot,
    pub binding: String,
    pub environment: AcmeEnvironment,
    pub phase: CertificatePhase,
    pub account: Option<serde_json::Value>,
    pub account_key: Option<String>,
    pub order_url: Option<String>,
    pub certificate_key: Option<String>,
    pub csr: Option<Vec<u8>>,
    pub challenge: Option<String>,
    pub dns_intent: Option<serde_json::Value>,
    pub dns_receipt: Option<TxtReceipt>,
    active_generation: Option<u64>,
    pub generations: std::collections::BTreeMap<u64, String>,
    pub next_generation: u64,
    pub next_attempt: Option<u64>,
    pub attempts: u8,
    pub pause_reason: Option<String>,
    #[serde(default)]
    pub last_success: Option<u64>,
    #[serde(default)]
    pub candidate_generation: Option<u64>,
    #[serde(default)]
    pub retry_interval: Option<u32>,
    #[serde(default)]
    pub resume_phase: Option<CertificatePhase>,
    #[serde(default)]
    pub archive_head: Option<String>,
    #[serde(default)]
    pub policy_binding: Option<String>,
    #[serde(default)]
    pub renewal: Option<crate::domain::certificates::RenewalSchedule>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Archive {
    root: NativeRoot,
    binding: String,
    environment: AcmeEnvironment,
    previous: Option<String>,
    generations: std::collections::BTreeMap<u64, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    pub hostname: String,
    pub environment: AcmeEnvironment,
    pub certificate: String,
    pub private_key: String,
    pub not_before: u64,
    pub not_after: u64,
    pub renew_at: u64,
    /// Initial advice survives interrupted publication. Refreshes stay in the journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_renewal: Option<crate::domain::certificates::RenewalSchedule>,
}

/// Live evidence binds a generation number to its native root and exact bytes.
/// It is never a browser or persisted credential contract.
#[derive(Clone, PartialEq, Eq)]
pub struct CertificateSelection {
    pub(crate) number: u64,
    pub(crate) root: NativeRoot,
    digest: String,
}

pub struct StateStore {
    path: PathBuf,
    root: NativeRoot,
    directory: Dir,
    _lock: ProcessLock,
    revision: String,
    pub journal: Journal,
}

impl StateStore {
    /// Passive status never acquires a lifecycle lock or creates state. Private
    /// account, CSR, key, challenge and provider error text are not returned.
    pub fn status(
        path: &Path,
        binding: &str,
        environment: AcmeEnvironment,
    ) -> Result<CertificateStatus> {
        Self::status_bound(path, binding, environment, None)
    }

    pub(crate) fn status_bound(
        path: &Path,
        binding: &str,
        environment: AcmeEnvironment,
        expected: Option<&CertificateSelection>,
    ) -> Result<CertificateStatus> {
        validate_binding(binding)?;
        let candidate = super::StateDirectory::inspect(path)?;
        if !candidate.exists {
            if expected.is_some() {
                return Err(Error::Conflict(
                    "live certificate state is missing; local recovery is required".into(),
                ));
            }
            return Ok(CertificateStatus {
                phase: CertificatePhase::Uninitialized,
                active_generation: None,
                expires_at: None,
                renew_at: None,
                next_attempt: None,
                attempts: 0,
                recovery_required: false,
                cleanup_pending: false,
                last_success: None,
            });
        }
        let opened = LibraryDirectory::open(path)?;
        let root = opened.native_root().clone();
        let directory = opened.into_directory();
        let bytes = read(&directory, "lifecycle.json")?;
        let journal: Journal = serde_json::from_slice(&bytes)
            .map_err(|_| Error::InvalidInput("certificate journal is invalid".into()))?;
        validate_journal(&journal)?;
        if journal.root != root || journal.binding != binding || journal.environment != environment
        {
            return Err(Error::Conflict(
                "certificate status binding changed; local recovery is required".into(),
            ));
        }
        if expected.is_some_and(|expected| {
            expected.root != root
                || journal.active_generation != Some(expected.number)
                || journal.generations.get(&expected.number) != Some(&expected.digest)
        }) {
            return Err(Error::Conflict("certificate selection is not confirmed on this listener; local recovery is required".into()));
        }
        let (expires_at, renew_at) = if let Some(number) = journal.active_generation {
            let generation_bytes = read(&directory, &format!("generation-{number}.json"))?;
            if journal.generations.get(&number) != Some(&hash(&generation_bytes)) {
                return Err(Error::Conflict(
                    "certificate status generation changed".into(),
                ));
            }
            let generation: Generation = serde_json::from_slice(&generation_bytes)
                .map_err(|_| Error::InvalidInput("certificate generation is invalid".into()))?;
            let renew_at = journal
                .renewal
                .filter(|schedule| schedule.generation == number)
                .map_or(generation.renew_at, |schedule| schedule.renew_at);
            if renew_at > generation.not_after {
                return Err(Error::InvalidInput(
                    "certificate renewal schedule exceeds validity".into(),
                ));
            }
            (Some(generation.not_after), Some(renew_at))
        } else {
            (None, None)
        };
        if read(&directory, "lifecycle.json")? != bytes
            || LibraryDirectory::open(path)?.native_root() != &root
        {
            return Err(Error::Conflict(
                "certificate status changed during inspection".into(),
            ));
        }
        let interrupted = match directory.symlink_metadata(".lifecycle.pending") {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        Ok(CertificateStatus {
            phase: journal.phase,
            active_generation: journal.active_generation,
            expires_at,
            renew_at,
            next_attempt: journal.next_attempt,
            attempts: journal.attempts,
            recovery_required: interrupted
                || journal.pause_reason.is_some()
                || journal.phase == CertificatePhase::Paused,
            cleanup_pending: journal.dns_receipt.is_some() || journal.dns_intent.is_some(),
            last_success: journal.last_success,
        })
    }

    pub(crate) fn bind_database_root(
        &self,
        store: &crate::infrastructure::store::Store,
    ) -> Result<()> {
        self.check_current()?;
        let path = crate::infrastructure::filesystem::LibraryDirectory::canonical(&self.path)?.0;
        store.bind_certificate_root(&path, &self.root)?;
        self.check_current()
    }

    pub(crate) fn check_database_root(
        &self,
        store: &crate::infrastructure::store::Store,
    ) -> Result<()> {
        self.check_current()?;
        let path = crate::infrastructure::filesystem::LibraryDirectory::canonical(&self.path)?.0;
        let path = path
            .to_str()
            .ok_or_else(|| Error::InvalidInput("certificate root path is not UTF-8".into()))?;
        store.check_native_root(path, "certificate", &self.root)?;
        self.check_current()
    }

    /// Initialization is an explicit operation. Existing journals, pending files
    /// and copied ownership records are never accepted as fresh state.
    pub fn initialize(path: &Path, binding: &str, environment: AcmeEnvironment) -> Result<Self> {
        validate_binding(binding)?;
        super::validate_state_directory(path)?;
        let parent =
            LibraryDirectory::open(path.parent().ok_or_else(|| {
                Error::InvalidInput("certificate state parent is missing".into())
            })?)?
            .into_directory();
        FilePermissions::current()?.check(&parent.try_clone()?.into_std_file(), false)?;
        let name = path
            .file_name()
            .ok_or_else(|| Error::InvalidInput("certificate state name is missing".into()))?;
        let mut builder = cap_std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        #[cfg(not(unix))]
        let _ = &mut builder;
        parent.create_dir_with(name, &builder)?;
        sync_directory(&parent)?;
        let opened = LibraryDirectory::open(path)?;
        let root = opened.native_root().clone();
        let directory = opened.into_directory();
        FilePermissions::current()?.check(&directory.try_clone()?.into_std_file(), true)?;
        let lock = ProcessLock::acquire(&path.join("lifecycle.json"), "certificate-lifecycle")?;
        let journal = Journal {
            version: 1,
            root: root.clone(),
            binding: binding.into(),
            environment,
            phase: CertificatePhase::Uninitialized,
            account: None,
            account_key: None,
            order_url: None,
            certificate_key: None,
            csr: None,
            challenge: None,
            dns_intent: None,
            dns_receipt: None,
            active_generation: None,
            generations: Default::default(),
            next_generation: 1,
            next_attempt: None,
            attempts: 0,
            pause_reason: None,
            last_success: None,
            candidate_generation: None,
            retry_interval: None,
            resume_phase: None,
            archive_head: None,
            policy_binding: None,
            renewal: None,
        };
        let bytes = encode(&journal)?;
        write_new(&directory, "lifecycle.json", &bytes)?;
        sync_directory(&directory)?;
        Ok(Self {
            path: path.into(),
            root,
            directory,
            _lock: lock,
            revision: hash(&bytes),
            journal,
        })
    }

    /// Opening an existing journal never silently creates or rebinds state.
    pub fn open(path: &Path, binding: &str, environment: AcmeEnvironment) -> Result<Self> {
        Self::open_checked(path, Some(binding), environment, false)
    }

    pub(crate) fn open_recovery(
        path: &Path,
        environment: AcmeEnvironment,
        pending: bool,
    ) -> Result<Self> {
        Self::open_checked(path, None, environment, pending)
    }

    fn open_checked(
        path: &Path,
        binding: Option<&str>,
        environment: AcmeEnvironment,
        pending: bool,
    ) -> Result<Self> {
        if let Some(binding) = binding {
            validate_binding(binding)?;
        }
        super::validate_state_directory(path)?;
        let opened = LibraryDirectory::open(path)?;
        let root = opened.native_root().clone();
        let directory = opened.into_directory();
        FilePermissions::current()?.check(&directory.try_clone()?.into_std_file(), true)?;
        let lock = ProcessLock::acquire(&path.join("lifecycle.json"), "certificate-lifecycle")?;
        if LibraryDirectory::open(path)?.native_root() != &root {
            return Err(Error::Conflict(
                "certificate state root changed during ownership acquisition".into(),
            ));
        }
        let bytes = read(&directory, "lifecycle.json")?;
        let journal: Journal = serde_json::from_slice(&bytes)
            .map_err(|_| Error::InvalidInput("certificate journal is invalid".into()))?;
        validate_journal(&journal)?;
        if journal.version != 1
            || journal.root != root
            || binding.is_some_and(|binding| journal.binding != binding)
            || journal.environment != environment
        {
            return Err(Error::Conflict(
                "certificate state binding changed; explicit recovery is required".into(),
            ));
        }
        match directory.symlink_metadata(".lifecycle.pending") {
            Ok(_) if !pending => {
                return Err(Error::Conflict(
                    "interrupted certificate state save needs explicit recovery".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(Self {
            path: path.into(),
            root,
            directory,
            _lock: lock,
            revision: hash(&bytes),
            journal,
        })
    }

    /// Preserve exact private recovery evidence before an explicit state edit.
    /// Existing evidence is accepted only when its complete bytes still match.
    pub(crate) fn preserve_recovery_evidence(&self) -> Result<()> {
        self.check_current()?;
        let bytes = read(&self.directory, "lifecycle.json")?;
        let name = format!(".recovery-{}.json", hash(&bytes));
        match self.directory.symlink_metadata(&name) {
            Ok(_) if read(&self.directory, &name)? == bytes => {}
            Ok(_) => return Err(Error::Conflict("private recovery evidence changed".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_new(&self.directory, &name, &bytes)?;
                sync_directory(&self.directory)?;
            }
            Err(error) => return Err(error.into()),
        }
        self.check_current()
    }

    pub(crate) fn recover_pending(&mut self) -> Result<()> {
        self.check_current()?;
        let bytes = read(&self.directory, ".lifecycle.pending")?;
        let proposed: Journal = serde_json::from_slice(&bytes).map_err(|_| {
            Error::InvalidInput(
                "pending certificate journal is incomplete; retain it for manual recovery".into(),
            )
        })?;
        validate_journal(&proposed)?;
        if proposed.root != self.root
            || proposed.binding != self.journal.binding
            || proposed.environment != self.journal.environment
        {
            return Err(Error::Conflict(
                "pending certificate journal bindings changed".into(),
            ));
        }
        self.check_archive_transition(&self.journal, &proposed)?;
        for (number, digest) in &proposed.generations {
            if hash(&read(
                &self.directory,
                &format!("generation-{number}.json"),
            )?) != *digest
            {
                return Err(Error::Conflict(
                    "pending certificate generation is incomplete or changed".into(),
                ));
            }
        }
        self.preserve_recovery_evidence()?;
        let evidence = format!(".recovery-{}.json", hash(&bytes));
        match self.directory.symlink_metadata(&evidence) {
            Ok(_) if read(&self.directory, &evidence)? == bytes => {}
            Ok(_) => return Err(Error::Conflict("pending recovery evidence changed".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_new(&self.directory, &evidence, &bytes)?
            }
            Err(error) => return Err(error.into()),
        }
        sync_directory(&self.directory)?;
        self.check_current()?;
        self.directory
            .rename(".lifecycle.pending", &self.directory, "lifecycle.json")?;
        sync_directory(&self.directory)?;
        if read(&self.directory, "lifecycle.json")? != bytes {
            return Err(Error::Conflict(
                "certificate pending recovery is uncertain".into(),
            ));
        }
        self.revision = hash(&bytes);
        self.journal = proposed;
        Ok(())
    }

    pub(crate) fn bind_policy(&mut self, binding: &str, policy: &str) -> Result<()> {
        validate_binding(policy)?;
        if self.journal.binding != binding {
            return Err(Error::Conflict(
                "cannot infer the original certificate resource policy after binding changes"
                    .into(),
            ));
        }
        match &self.journal.policy_binding {
            Some(saved) if saved == policy => Ok(()),
            Some(_) => Err(Error::Conflict(
                "frozen certificate resource policy changed".into(),
            )),
            None => {
                self.journal.policy_binding = Some(policy.into());
                self.save()
            }
        }
    }

    pub(crate) fn recover_binding(
        &mut self,
        binding: &str,
        hostname: &str,
        policy: &str,
    ) -> Result<()> {
        validate_binding(binding)?;
        validate_binding(policy)?;
        let ready = self.journal.phase == CertificatePhase::Ready
            || (self.journal.phase == CertificatePhase::Paused
                && self.journal.resume_phase == Some(CertificatePhase::Ready));
        let pristine = (self.journal.phase == CertificatePhase::Uninitialized
            || self.journal.resume_phase == Some(CertificatePhase::Uninitialized))
            && self.journal.account.is_none()
            && self.journal.account_key.is_none()
            && self.journal.csr.is_none()
            && self.journal.certificate_key.is_none()
            && self.journal.challenge.is_none()
            && self.journal.generations.is_empty();
        let same_policy = self.journal.policy_binding.as_deref() == Some(policy);
        if !same_policy
            && ((!ready && !pristine)
                || self.journal.dns_intent.is_some()
                || self.journal.dns_receipt.is_some()
                || self.journal.order_url.is_some()
                || self.journal.candidate_generation.is_some())
        {
            return Err(Error::Conflict("complete the saved certificate work with its original configuration before binding rotation".into()));
        }
        self.preserve_recovery_evidence()?;
        let number = self.journal.active_generation.or_else(|| {
            self.journal
                .generations
                .last_key_value()
                .map(|(number, _)| *number)
        });
        let same_hostname = number
            .map(|number| {
                self.generation(number)
                    .map(|generation| generation.hostname == hostname)
            })
            .transpose()?
            .unwrap_or(pristine);
        self.journal.binding = binding.into();
        self.journal.policy_binding = Some(policy.into());
        if !same_policy {
            self.journal.phase = if same_hostname && !pristine {
                CertificatePhase::Ready
            } else {
                CertificatePhase::Uninitialized
            };
            self.journal.resume_phase = None;
            self.journal.pause_reason = None;
            self.journal.retry_interval = None;
            self.journal.next_attempt = None;
            self.journal.attempts = 0;
        }
        let bytes = encode(&self.journal)?;
        write_new(&self.directory, ".lifecycle.pending", &bytes)?;
        self.check_current()?;
        self.directory
            .rename(".lifecycle.pending", &self.directory, "lifecycle.json")?;
        sync_directory(&self.directory)?;
        if read(&self.directory, "lifecycle.json")? != bytes {
            return Err(Error::Conflict(
                "certificate binding recovery is uncertain".into(),
            ));
        }
        self.revision = hash(&bytes);
        Ok(())
    }

    /// Save intent before submission and completion before the next effect.
    /// A failed post-replacement sync/verification is uncertain, never rollback.
    pub fn save(&mut self) -> Result<()> {
        self.check_current()?;
        let previous: Journal =
            serde_json::from_slice(&read(&self.directory, "lifecycle.json")?)
                .map_err(|_| Error::InvalidInput("certificate journal is invalid".into()))?;
        if self.journal.root != self.root
            || self.journal.version != 1
            || self.journal.binding != previous.binding
            || self.journal.environment != previous.environment
            || previous
                .policy_binding
                .as_ref()
                .is_some_and(|policy| self.journal.policy_binding.as_ref() != Some(policy))
        {
            return Err(Error::Conflict(
                "certificate journal binding cannot be edited".into(),
            ));
        }
        self.check_archive_transition(&previous, &self.journal)?;
        validate_journal(&self.journal)?;
        let bytes = encode(&self.journal)?;
        write_new(&self.directory, ".lifecycle.pending", &bytes)?;
        self.check_current()?;
        self.directory
            .rename(".lifecycle.pending", &self.directory, "lifecycle.json")?;
        sync_directory(&self.directory)?;
        if read(&self.directory, "lifecycle.json")? != bytes {
            return Err(Error::Conflict(
                "certificate state save is uncertain; inspect local state".into(),
            ));
        }
        self.revision = hash(&bytes);
        Ok(())
    }

    fn check_archive_transition(&self, previous: &Journal, proposed: &Journal) -> Result<()> {
        let removed = previous
            .generations
            .iter()
            .filter(|(number, digest)| proposed.generations.get(number) != Some(digest))
            .map(|(number, digest)| (*number, digest.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        if previous.archive_head == proposed.archive_head {
            if removed.is_empty() {
                return Ok(());
            }
            return Err(Error::Conflict(
                "certificate generation history cannot be removed without an archive".into(),
            ));
        }
        let digest = proposed
            .archive_head
            .as_deref()
            .ok_or_else(|| Error::Conflict("certificate archive chain cannot be cleared".into()))?;
        validate_binding(digest)?;
        let bytes = read(&self.directory, &format!(".archive-{digest}.json"))?;
        let archive: Archive = serde_json::from_slice(&bytes)
            .map_err(|_| Error::InvalidInput("certificate archive is invalid".into()))?;
        if hash(&bytes) != digest
            || archive.root != self.root
            || archive.binding != previous.binding
            || archive.environment != previous.environment
            || archive.previous != previous.archive_head
            || archive.generations != removed
            || removed.is_empty()
            || previous
                .active_generation
                .is_some_and(|number| removed.contains_key(&number))
            || previous
                .candidate_generation
                .is_some_and(|number| removed.contains_key(&number))
            || removed
                .keys()
                .any(|number| proposed.generations.contains_key(number))
        {
            return Err(Error::Conflict(
                "certificate archive does not preserve the exact inactive history".into(),
            ));
        }
        Ok(())
    }

    /// Explicit history archival keeps the latest four generations and both
    /// selected generations. It never deletes or rewrites certificate files.
    pub(crate) fn archive_generations(&mut self) -> Result<()> {
        if !matches!(
            self.journal.phase,
            CertificatePhase::Ready | CertificatePhase::Paused
        ) {
            return Err(Error::Conflict(
                "certificate history archival requires completed idle work".into(),
            ));
        }
        self.preserve_recovery_evidence()?;
        let retained = self
            .journal
            .generations
            .keys()
            .rev()
            .take(4)
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let archived = self
            .journal
            .generations
            .iter()
            .filter(|(number, _)| {
                !retained.contains(number)
                    && self.journal.active_generation != Some(**number)
                    && self.journal.candidate_generation != Some(**number)
                    && self
                        .journal
                        .renewal
                        .is_none_or(|schedule| schedule.generation != **number)
            })
            .map(|(number, digest)| (*number, digest.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        if archived.is_empty() {
            return Err(Error::Conflict(
                "certificate history has no inactive generations to archive".into(),
            ));
        }
        for number in archived.keys() {
            self.generation(*number)?;
        }
        let archive = Archive {
            root: self.root.clone(),
            binding: self.journal.binding.clone(),
            environment: self.journal.environment,
            previous: self.journal.archive_head.clone(),
            generations: archived,
        };
        let bytes = encode(&archive)?;
        let digest = hash(&bytes);
        let name = format!(".archive-{digest}.json");
        match self.directory.symlink_metadata(&name) {
            Ok(_) if read(&self.directory, &name)? == bytes => {}
            Ok(_) => {
                return Err(Error::Conflict(
                    "certificate archive evidence changed".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_new(&self.directory, &name, &bytes)?
            }
            Err(error) => return Err(error.into()),
        }
        sync_directory(&self.directory)?;
        self.journal
            .generations
            .retain(|number, _| !archive.generations.contains_key(number));
        self.journal.archive_head = Some(digest);
        self.save()
    }

    fn check_generation(&self, number: u64, generation: &Generation) -> Result<()> {
        self.check_current()?;
        if let Some(schedule) = generation.initial_renewal {
            let minimum = crate::domain::certificates::renewal_time(
                generation.not_before,
                generation.not_after,
                schedule.window,
                0,
            )?;
            let maximum = crate::domain::certificates::renewal_time(
                generation.not_before,
                generation.not_after,
                schedule.window,
                u64::MAX,
            )?;
            if schedule.generation != number
                || schedule.renew_at != generation.renew_at
                || !(minimum..=maximum).contains(&schedule.renew_at)
                || schedule.window.is_some() != schedule.refresh_interval.is_some()
                || schedule.refresh_at.is_some() != schedule.refresh_interval.is_some()
                || schedule
                    .refresh_interval
                    .is_some_and(|seconds| !(60..=86400).contains(&seconds))
            {
                return Err(Error::InvalidInput(
                    "certificate initial renewal advice is invalid".into(),
                ));
            }
        }
        let validity = crate::infrastructure::tls::certificate_validity(
            &generation.hostname,
            generation.certificate.as_bytes(),
        )?;
        if validity != (generation.not_before, generation.not_after) {
            return Err(Error::InvalidInput(
                "certificate generation validity does not match its leaf".into(),
            ));
        }
        match generation.environment {
            AcmeEnvironment::Production => crate::infrastructure::tls::managed_configuration(
                &generation.hostname,
                generation.certificate.as_bytes(),
                generation.private_key.as_bytes(),
            )?,
            AcmeEnvironment::Staging => crate::infrastructure::tls::configuration(
                &generation.hostname,
                generation.certificate.as_bytes(),
                generation.private_key.as_bytes(),
            )?,
        };
        if generation.environment != self.journal.environment
            || generation.not_before >= generation.not_after
            || generation.renew_at >= generation.not_after
        {
            return Err(Error::InvalidInput(
                "certificate generation policy is invalid".into(),
            ));
        }
        Ok(())
    }

    /// Publish one independently validated immutable pair before its manifest.
    /// Never replace one PEM path at a time or overwrite an interrupted slot.
    pub fn publish_generation(&mut self, generation: &Generation) -> Result<u64> {
        let number = self.journal.next_generation;
        self.check_generation(number, generation)?;
        if self.journal.generations.len() >= 64 {
            return Err(Error::InvalidInput(
                "certificate generation history is full; archive it through explicit recovery"
                    .into(),
            ));
        }
        let next = number.checked_add(1).ok_or_else(|| {
            Error::InvalidInput("certificate generation counter is exhausted".into())
        })?;
        let bytes = encode(generation)?;
        let pending = format!(".generation-{number}.pending");
        let name = format!("generation-{number}.json");
        write_new(&self.directory, &pending, &bytes)?;
        rename_no_replace(&self.directory, &pending, &name)?;
        sync_directory(&self.directory)?;
        self.journal.next_generation = next;
        self.journal.generations.insert(number, hash(&bytes));
        if let Some(schedule) = generation.initial_renewal {
            self.journal.renewal = Some(schedule);
        }
        self.save()?;
        Ok(number)
    }

    pub fn generation(&self, number: u64) -> Result<Generation> {
        self.check_current()?;
        let bytes = read(&self.directory, &format!("generation-{number}.json"))?;
        if self.journal.generations.get(&number) != Some(&hash(&bytes)) {
            return Err(Error::Conflict(
                "immutable certificate generation changed".into(),
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::InvalidInput("certificate generation is invalid".into()))
    }

    pub fn active_generation_number(&self) -> Option<u64> {
        self.journal.active_generation
    }

    pub(crate) fn next_generation_number(&self) -> u64 {
        self.journal.next_generation
    }

    pub(crate) fn selection(&self, number: u64) -> Result<CertificateSelection> {
        self.generation(number)?;
        Ok(CertificateSelection {
            number,
            root: self.root.clone(),
            digest: self
                .journal
                .generations
                .get(&number)
                .ok_or(Error::Internal(
                    "certificate selection reference is missing",
                ))?
                .clone(),
        })
    }

    /// Validate first, then atomically select the complete immutable pair.
    /// Only success permits a listener replacement. A failed save can be
    /// uncertain after replacement; the caller must retain its current listener.
    pub fn activate_generation(
        &mut self,
        number: u64,
        hostname: &str,
    ) -> Result<(CertificateSelection, axum_server::tls_rustls::RustlsConfig)> {
        let generation = self.generation(number)?;
        let configuration = validated_active(&generation, hostname)?;
        let selection = self.selection(number)?;
        self.select_generation(number)?;
        Ok((selection, configuration))
    }

    fn select_generation(&mut self, number: u64) -> Result<()> {
        self.generation(number)?;
        let previous = self.journal.active_generation.replace(number);
        if let Err(error) = self.save() {
            self.journal.active_generation = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Read the committed active pair without creating files, taking ownership,
    /// or starting renewal. An interrupted candidate does not hide the previous
    /// committed pair. Recheck the exact journal and native root before return.
    pub fn load_active(
        path: &Path,
        binding: &str,
        hostname: &str,
    ) -> Result<(
        CertificateSelection,
        Generation,
        axum_server::tls_rustls::RustlsConfig,
    )> {
        validate_binding(binding)?;
        super::validate_state_directory(path)?;
        let opened = LibraryDirectory::open(path)?;
        let root = opened.native_root().clone();
        let directory = opened.into_directory();
        FilePermissions::current()?.check(&directory.try_clone()?.into_std_file(), true)?;
        let bytes = read(&directory, "lifecycle.json")?;
        let journal: Journal = serde_json::from_slice(&bytes)
            .map_err(|_| Error::InvalidInput("certificate journal is invalid".into()))?;
        validate_journal(&journal)?;
        if journal.root != root || journal.binding != binding {
            return Err(Error::Conflict(
                "active certificate state binding changed".into(),
            ));
        }
        let number = journal.active_generation.ok_or_else(|| {
            Error::InvalidInput("managed TLS has no active generation; run local bootstrap".into())
        })?;
        let generation_bytes = read(&directory, &format!("generation-{number}.json"))?;
        if journal.generations.get(&number) != Some(&hash(&generation_bytes)) {
            return Err(Error::Conflict(
                "immutable active certificate generation changed".into(),
            ));
        }
        let generation: Generation = serde_json::from_slice(&generation_bytes)
            .map_err(|_| Error::InvalidInput("active certificate generation is invalid".into()))?;
        if generation.environment != journal.environment {
            return Err(Error::Conflict(
                "active certificate environment changed".into(),
            ));
        }
        let configuration = validated_active(&generation, hostname)?;
        if read(&directory, "lifecycle.json")? != bytes
            || LibraryDirectory::open(path)?.native_root() != &root
        {
            return Err(Error::Conflict(
                "active certificate changed during passive preparation".into(),
            ));
        }
        Ok((
            CertificateSelection {
                number,
                root,
                digest: hash(&generation_bytes),
            },
            generation,
            configuration,
        ))
    }

    pub(crate) fn check_current(&self) -> Result<()> {
        self._lock
            .verify(&self.path.join("lifecycle.json"), "certificate-lifecycle")?;
        if LibraryDirectory::open(&self.path)?.native_root() != &self.root
            || hash(&read(&self.directory, "lifecycle.json")?) != self.revision
        {
            return Err(Error::Conflict(
                "certificate state changed outside its owner".into(),
            ));
        }

        Ok(())
    }
}

fn validated_active(
    generation: &Generation,
    hostname: &str,
) -> Result<axum_server::tls_rustls::RustlsConfig> {
    if generation.environment != AcmeEnvironment::Production || generation.hostname != hostname {
        return Err(Error::InvalidInput(
            "only an exact-hostname production generation can activate HTTPS".into(),
        ));
    }
    let validity = crate::infrastructure::tls::certificate_validity(
        hostname,
        generation.certificate.as_bytes(),
    )?;
    if validity != (generation.not_before, generation.not_after)
        || generation.not_before >= generation.not_after
        || generation.renew_at >= generation.not_after
    {
        return Err(Error::InvalidInput(
            "active certificate validity policy changed".into(),
        ));
    }
    crate::infrastructure::tls::managed_configuration(
        hostname,
        generation.certificate.as_bytes(),
        generation.private_key.as_bytes(),
    )
}

fn validate_journal(journal: &Journal) -> Result<()> {
    validate_binding(&journal.binding)?;
    if journal.version != 1
        || journal.generations.len() > 64
        || journal.next_generation == 0
        || journal.generations.iter().any(|(number, digest)| {
            *number == 0 || *number >= journal.next_generation || validate_binding(digest).is_err()
        })
        || journal
            .active_generation
            .is_some_and(|number| !journal.generations.contains_key(&number))
        || journal
            .candidate_generation
            .is_some_and(|number| !journal.generations.contains_key(&number))
        || journal
            .retry_interval
            .is_some_and(|seconds| seconds == 0 || seconds > 86400)
        || journal.renewal.is_some_and(|schedule| {
            !journal.generations.contains_key(&schedule.generation)
                || schedule.window.is_some() != schedule.refresh_interval.is_some()
                || schedule.refresh_at.is_some() != schedule.refresh_interval.is_some()
                || schedule
                    .refresh_interval
                    .is_some_and(|seconds| seconds == 0 || seconds > 86400)
                || schedule.window.is_some_and(|(start, end)| start >= end)
        })
        || journal
            .archive_head
            .as_ref()
            .is_some_and(|digest| validate_binding(digest).is_err())
        || journal
            .policy_binding
            .as_ref()
            .is_some_and(|digest| validate_binding(digest).is_err())
    {
        return Err(Error::InvalidInput(
            "certificate journal generation references are invalid".into(),
        ));
    }
    Ok(())
}

fn validate_binding(binding: &str) -> Result<()> {
    if binding.len() != 64
        || !binding
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::InvalidInput(
            "certificate binding must be a SHA-256 digest".into(),
        ));
    }
    Ok(())
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAXIMUM {
        return Err(Error::InvalidInput(
            "certificate state exceeds 512 KiB".into(),
        ));
    }
    Ok(bytes)
}

fn read(directory: &Dir, name: &str) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = directory.open_with(name, &options)?.into_std();
    if !file.metadata()?.is_file()
        || crate::infrastructure::filesystem::is_unsafe_link(&file.metadata()?)
    {
        return Err(Error::InvalidInput(
            "certificate state must be a regular no-follow file".into(),
        ));
    }
    FilePermissions::current()?.check(&file, true)?;
    let mut bytes = Vec::new();
    file.take(MAXIMUM + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAXIMUM {
        return Err(Error::InvalidInput(
            "certificate state exceeds 512 KiB".into(),
        ));
    }
    Ok(bytes)
}

fn write_new(directory: &Dir, name: &str, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = directory.open_with(name, &options)?.into_std();
    FilePermissions::current()?.check(&file, true)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staging_generation() -> std::result::Result<Generation, Box<dyn std::error::Error>> {
        let generated = rcgen::generate_simple_self_signed(vec!["api.home.example".into()])?;
        let certificate = generated.cert.pem();
        let (not_before, not_after) = crate::infrastructure::tls::certificate_validity(
            "api.home.example",
            certificate.as_bytes(),
        )?;
        Ok(Generation {
            hostname: "api.home.example".into(),
            environment: AcmeEnvironment::Staging,
            certificate,
            private_key: generated.signing_key.serialize_pem(),
            not_before,
            not_after,
            renew_at: crate::domain::certificates::renewal_time(not_before, not_after, None, 0)?,
            initial_renewal: None,
        })
    }

    #[test]
    fn certificates_initial_advice_survives_publication_and_retained_generation_recovery()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        let mut generation = staging_generation()?;
        let schedule = crate::domain::certificates::renewal_schedule(
            1,
            (generation.not_before, generation.not_after),
            crate::domain::certificates::RenewalAdvice::Suggested {
                start: generation.not_before + 1000,
                end: generation.not_before + 2000,
                refresh_after_seconds: 120,
            },
            None,
            u64::MAX / 2,
            generation.not_before,
        )?;
        generation.renew_at = schedule.renew_at;
        generation.initial_renewal = Some(schedule);
        state.journal.phase = CertificatePhase::Paused;
        state.journal.resume_phase = Some(CertificatePhase::FinalizationPending);
        state.journal.certificate_key = Some(generation.private_key.clone());
        state.save()?;
        let previous = std::fs::read(path.join("lifecycle.json"))?;
        state.publish_generation(&generation)?;
        drop(state);
        let state = StateStore::open(&path, &binding, AcmeEnvironment::Staging)?;
        assert_eq!(state.journal.renewal, Some(schedule));
        assert_eq!(state.generation(1)?.initial_renewal, Some(schedule));
        let generation_bytes = std::fs::read(path.join("generation-1.json"))?;
        drop(state);

        std::fs::write(path.join("lifecycle.json"), previous)?;
        let mut state = StateStore::open(&path, &binding, AcmeEnvironment::Staging)?;
        state.recover_generation(&generation.certificate, &generation.hostname)?;
        assert_eq!(state.journal.renewal, Some(schedule));
        assert_eq!(state.journal.candidate_generation, Some(1));
        assert_eq!(
            std::fs::read(path.join("generation-1.json"))?,
            generation_bytes
        );
        Ok(())
    }

    #[test]
    fn certificates_invalid_initial_advice_cannot_publish_a_generation()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        let mut generation = staging_generation()?;
        let schedule = crate::domain::certificates::renewal_schedule(
            1,
            (generation.not_before, generation.not_after),
            crate::domain::certificates::RenewalAdvice::Unsupported,
            None,
            0,
            generation.not_before,
        )?;
        let previous = std::fs::read(path.join("lifecycle.json"))?;
        for invalid in [
            crate::domain::certificates::RenewalSchedule {
                generation: 2,
                ..schedule
            },
            crate::domain::certificates::RenewalSchedule {
                renew_at: schedule.renew_at + 1,
                ..schedule
            },
            crate::domain::certificates::RenewalSchedule {
                refresh_interval: Some(120),
                ..schedule
            },
        ] {
            generation.initial_renewal = Some(invalid);
            assert!(state.publish_generation(&generation).is_err());
            assert_eq!(std::fs::read(path.join("lifecycle.json"))?, previous);
            assert!(!path.join("generation-1.json").exists());
            assert!(!path.join(".generation-1.pending").exists());
        }
        Ok(())
    }

    #[test]
    fn certificates_pending_recovery_preserves_both_versions_and_all_submission_evidence()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        let original = read(&state.directory, "lifecycle.json")?;
        state.journal.phase = CertificatePhase::DnsPending;
        state.journal.challenge = Some("b".repeat(43));
        state.journal.dns_intent = Some(serde_json::json!({"fixture":"saved-before-submission"}));
        let proposed = encode(&state.journal)?;
        write_new(&state.directory, ".lifecycle.pending", &proposed)?;
        assert!(StateStore::status(&path, &binding, AcmeEnvironment::Staging)?.recovery_required);
        drop(state);
        assert!(StateStore::open(&path, &binding, AcmeEnvironment::Staging).is_err());
        let mut state = StateStore::open_recovery(&path, AcmeEnvironment::Staging, true)?;
        state.recover_pending()?;
        assert_eq!(read(&state.directory, "lifecycle.json")?, proposed);
        assert_eq!(
            read(
                &state.directory,
                &format!(".recovery-{}.json", hash(&original))
            )?,
            original
        );
        assert_eq!(
            read(
                &state.directory,
                &format!(".recovery-{}.json", hash(&proposed))
            )?,
            proposed
        );
        assert!(state.journal.dns_intent.is_some());
        assert!(state.journal.dns_receipt.is_none());
        let status = serde_json::to_value(StateStore::status(
            &path,
            &binding,
            AcmeEnvironment::Staging,
        )?)?;
        assert!(status.get("account").is_none());
        assert!(status.get("certificate_key").is_none());
        assert!(status.get("challenge").is_none());
        Ok(())
    }

    #[test]
    fn certificates_incomplete_pending_recovery_never_removes_the_artifact()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let state = StateStore::initialize(&path, &"a".repeat(64), AcmeEnvironment::Staging)?;
        let committed = read(&state.directory, "lifecycle.json")?;
        write_new(
            &state.directory,
            ".lifecycle.pending",
            b"partial-private-evidence",
        )?;
        drop(state);
        let mut state = StateStore::open_recovery(&path, AcmeEnvironment::Staging, true)?;
        assert!(state.recover_pending().is_err());
        assert_eq!(
            read(&state.directory, ".lifecycle.pending")?,
            b"partial-private-evidence"
        );
        assert_eq!(read(&state.directory, "lifecycle.json")?, committed);
        Ok(())
    }

    #[test]
    fn certificates_archive_keeps_selected_pairs_and_never_deletes_generation_files()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        let generation = staging_generation()?;
        for _ in 0..6 {
            state.publish_generation(&generation)?;
        }
        state.select_generation(1)?;
        state.journal.phase = CertificatePhase::Ready;
        state.save()?;
        let original = read(&state.directory, "generation-2.json")?;
        state.archive_generations()?;
        assert_eq!(state.active_generation_number(), Some(1));
        assert_eq!(
            state
                .journal
                .generations
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![1, 3, 4, 5, 6]
        );
        assert_eq!(read(&state.directory, "generation-2.json")?, original);
        let archive = state
            .journal
            .archive_head
            .clone()
            .ok_or("archive is missing")?;
        let bytes = read(&state.directory, &format!(".archive-{archive}.json"))?;
        assert_eq!(hash(&bytes), archive);
        assert_eq!(
            serde_json::from_slice::<Archive>(&bytes)?
                .generations
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![2]
        );
        drop(state);
        let mut state = StateStore::open(&path, &binding, AcmeEnvironment::Staging)?;
        assert_eq!(state.publish_generation(&generation)?, 7);
        Ok(())
    }

    #[test]
    fn certificates_binding_rotation_rejects_pending_work_and_retains_private_evidence()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let original_binding = "a".repeat(64);
        let new_binding = "b".repeat(64);
        let mut state = StateStore::initialize(&path, &original_binding, AcmeEnvironment::Staging)?;
        state.publish_generation(&staging_generation()?)?;
        assert!(
            state
                .recover_binding(&new_binding, "api.home.example", &"c".repeat(64))
                .is_err()
        );
        state.journal.phase = CertificatePhase::Ready;
        state.save()?;
        let previous = read(&state.directory, "lifecycle.json")?;
        state.recover_binding(&new_binding, "api.home.example", &"c".repeat(64))?;
        assert_eq!(state.journal.binding, new_binding);
        assert_eq!(
            read(
                &state.directory,
                &format!(".recovery-{}.json", hash(&previous))
            )?,
            previous
        );
        drop(state);
        assert!(StateStore::open(&path, &original_binding, AcmeEnvironment::Staging).is_err());
        assert!(StateStore::open(&path, &new_binding, AcmeEnvironment::Staging).is_ok());
        Ok(())
    }

    #[test]
    fn certificates_credential_rotation_keeps_uncertain_work_on_its_original_resource()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let policy = "c".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        state.bind_policy(&binding, &policy)?;
        state.journal.phase = CertificatePhase::DnsPending;
        state.journal.order_url = Some("saved-original-order".into());
        let intent = serde_json::json!({"fixture":"uncertain-original-resource-write"});
        state.journal.dns_intent = Some(intent.clone());
        state.save()?;
        crate::application::certificates::pause(&mut state, "credential rotation required")?;
        assert!(
            state
                .recover_binding(&"b".repeat(64), "api.home.example", &"d".repeat(64))
                .is_err()
        );
        state.recover_binding(&"b".repeat(64), "api.home.example", &policy)?;
        assert_eq!(state.journal.phase, CertificatePhase::Paused);
        assert_eq!(
            state.journal.resume_phase,
            Some(CertificatePhase::DnsPending)
        );
        assert_eq!(
            state.journal.order_url.as_deref(),
            Some("saved-original-order")
        );
        assert_eq!(state.journal.dns_intent, Some(intent));
        assert!(state.journal.dns_receipt.is_none());
        Ok(())
    }

    #[test]
    fn certificates_retained_generation_recovery_checks_the_original_complete_pair()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let mut state = StateStore::initialize(&path, &"a".repeat(64), AcmeEnvironment::Staging)?;
        let generation = staging_generation()?;
        state.journal.phase = CertificatePhase::FinalizationPending;
        state.journal.certificate_key = Some(generation.private_key.clone());
        state.journal.csr = Some(vec![1, 2, 3]);
        state.save()?;
        crate::application::certificates::pause(&mut state, "interrupted generation publication")?;
        let bytes = encode(&generation)?;
        write_new(&state.directory, ".generation-1.pending", &bytes)?;
        write_new(
            &state.directory,
            ".generation-999.pending",
            b"unknown-retained-work",
        )?;
        assert!(
            state
                .recover_generation("different-certificate", &generation.hostname)
                .is_err()
        );
        assert_eq!(read(&state.directory, ".generation-1.pending")?, bytes);
        state.recover_generation(&generation.certificate, &generation.hostname)?;
        assert_eq!(state.journal.phase, CertificatePhase::CleanupPending);
        assert_eq!(state.journal.candidate_generation, Some(1));
        assert_eq!(state.journal.next_generation, 2);
        assert_eq!(read(&state.directory, "generation-1.json")?, bytes);
        assert_eq!(
            read(&state.directory, ".generation-999.pending")?,
            b"unknown-retained-work"
        );
        assert!(state.active_generation_number().is_none());
        Ok(())
    }

    #[test]
    fn certificates_archive_recovers_the_exact_sixty_four_generation_capacity_boundary()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let mut state = StateStore::initialize(&path, &"a".repeat(64), AcmeEnvironment::Staging)?;
        let generation = staging_generation()?;
        for _ in 0..64 {
            state.publish_generation(&generation)?;
        }
        state.journal.phase = CertificatePhase::Ready;
        state.save()?;
        assert!(state.publish_generation(&generation).is_err());
        assert_eq!(state.journal.next_generation, 65);
        state.archive_generations()?;
        assert_eq!(state.journal.generations.len(), 4);
        assert_eq!(state.publish_generation(&generation)?, 65);
        assert_eq!(
            read(&state.directory, "generation-1.json")?,
            encode(&generation)?
        );
        Ok(())
    }

    #[test]
    fn certificates_manifest_selection_is_atomic_and_staging_cannot_activate()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        let generation = staging_generation()?;
        let first = state.publish_generation(&generation)?;
        assert_eq!(first, 1);
        assert_eq!(state.active_generation_number(), None);
        assert!(
            state
                .activate_generation(first, &generation.hostname)
                .is_err()
        );
        assert_eq!(state.active_generation_number(), None);
        state.select_generation(first)?;
        drop(state);
        let mut state = StateStore::open(&path, &binding, AcmeEnvironment::Staging)?;
        assert_eq!(state.active_generation_number(), Some(first));
        let second = state.publish_generation(&generation)?;
        let committed = std::fs::read(path.join("lifecycle.json"))?;
        assert!(
            state
                .activate_generation(second, &generation.hostname)
                .is_err()
        );
        assert_eq!(state.active_generation_number(), Some(first));
        assert_eq!(std::fs::read(path.join("lifecycle.json"))?, committed);
        assert!(StateStore::load_active(&path, &binding, &generation.hostname).is_err());
        std::fs::write(
            path.join("generation-1.json"),
            b"changed immutable generation",
        )?;
        assert!(state.generation(first).is_err());
        assert!(
            state
                .activate_generation(first, &generation.hostname)
                .is_err()
        );
        assert_eq!(std::fs::read(path.join("lifecycle.json"))?, committed);
        Ok(())
    }

    #[test]
    fn certificates_invalid_generation_references_never_change_the_journal()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        let committed = std::fs::read(path.join("lifecycle.json"))?;
        state.journal.active_generation = Some(1);
        assert!(state.save().is_err());
        state.journal.active_generation = None;
        state.journal.next_generation = 0;
        assert!(state.save().is_err());
        assert_eq!(std::fs::read(path.join("lifecycle.json"))?, committed);
        assert!(!path.join(".lifecycle.pending").exists());
        drop(state);
        let state = StateStore::open(&path, &binding, AcmeEnvironment::Staging)?;
        assert_eq!(state.journal.next_generation, 1);
        Ok(())
    }

    #[test]
    fn certificates_state_stays_outside_git_before_any_creation()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        std::fs::write(parent.path().join(".git"), b"gitdir: fixture")?;
        let path = parent.path().join("certificate-state");
        assert!(StateStore::initialize(&path, &"a".repeat(64), AcmeEnvironment::Staging).is_err());
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn certificates_generations_reject_untrusted_production_material_without_publication()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Production)?;
        let generated = rcgen::generate_simple_self_signed(vec!["api.home.example".into()])?;
        let certificate = generated.cert.pem();
        let (not_before, not_after) = crate::infrastructure::tls::certificate_validity(
            "api.home.example",
            certificate.as_bytes(),
        )?;
        let generation = Generation {
            hostname: "api.home.example".into(),
            environment: AcmeEnvironment::Production,
            certificate,
            private_key: generated.signing_key.serialize_pem(),
            not_before,
            not_after,
            renew_at: crate::domain::certificates::renewal_time(not_before, not_after, None, 0)?,
            initial_renewal: None,
        };
        assert!(state.publish_generation(&generation).is_err());
        assert!(state.journal.active_generation.is_none());
        assert!(state.journal.generations.is_empty());
        assert_eq!(state.journal.next_generation, 1);
        assert!(!path.join("generation-1.json").exists());
        Ok(())
    }

    #[test]
    fn certificates_state_excludes_competing_owners_and_changed_bindings()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("certificates");
        let binding = "a".repeat(64);
        let mut state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        assert!(StateStore::open(&path, &binding, AcmeEnvironment::Staging).is_err());
        state.journal.phase = CertificatePhase::AccountPending;
        state.save()?;
        drop(state);
        assert!(StateStore::open(&path, &"b".repeat(64), AcmeEnvironment::Staging).is_err());
        assert!(StateStore::open(&path, &binding, AcmeEnvironment::Production).is_err());
        let state = StateStore::open(&path, &binding, AcmeEnvironment::Staging)?;
        assert_eq!(state.journal.phase, CertificatePhase::AccountPending);
        Ok(())
    }

    #[test]
    fn certificates_state_rejects_copied_roots_and_interrupted_writes()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let parent = crate::infrastructure::permissions::private_test_directory()?;
        let path = parent.path().join("original");
        let binding = "a".repeat(64);
        let state = StateStore::initialize(&path, &binding, AcmeEnvironment::Staging)?;
        drop(state);
        let copy = parent.path().join("copy");
        std::fs::create_dir(&copy)?;
        std::fs::copy(path.join("lifecycle.json"), copy.join("lifecycle.json"))?;
        assert!(StateStore::open(&copy, &binding, AcmeEnvironment::Staging).is_err());
        std::fs::write(path.join(".lifecycle.pending"), b"interrupted")?;
        assert!(StateStore::open(&path, &binding, AcmeEnvironment::Staging).is_err());
        assert_eq!(
            std::fs::read(path.join(".lifecycle.pending"))?,
            b"interrupted"
        );
        Ok(())
    }
}
