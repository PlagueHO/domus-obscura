use super::publication::PreparedConfigurationActivation;
use super::{ConfigurationService, MAXIMUM, digest};
use crate::domain::sharing::ShareAccess;
use crate::infrastructure::filesystem::{native_file_identity, sync_directory};
use crate::infrastructure::permissions::FilePermissions;
use crate::infrastructure::store::{SoulseekAccountChange, Store, StoreOpenMode};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SoulseekAccountSelection {
    pub(super) change_id: i64,
    pub(super) credentials_file: PathBuf,
}

/// Write-only input. It cannot be formatted or serialized into a response.
#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct SaveSoulseekAccount {
    pub expected_revision: String,
    pub username: String,
    pub password: String,
}

impl Drop for SaveSoulseekAccount {
    fn drop(&mut self) {
        std::mem::take(&mut self.username).into_bytes().fill(0);
        std::mem::take(&mut self.password).into_bytes().fill(0);
    }
}

#[derive(Serialize, ts_rs::TS)]
pub struct SoulseekAccountStatus {
    pub available: bool,
    pub reason: Option<String>,
    pub configured: bool,
    pub active_revision: String,
    pub pending_change_id: Option<i64>,
}

fn file_name(id: i64) -> Result<String> {
    if id <= 0 {
        return Err(Error::InvalidInput(
            "Account change ID must be positive.".into(),
        ));
    }
    Ok(format!(".domus-soulseek-{id}.credentials.json"))
}

impl ConfigurationService {
    fn require_offline_soulseek_account(&self) -> Result<()> {
        let worker = self.active.production_worker.as_ref();
        if self.active.local_acquisition.is_some()
            || worker.is_some_and(|worker| worker.acquisition.is_some())
            || worker
                .and_then(|worker| worker.sharing.as_ref())
                .is_some_and(|sharing| {
                    sharing.account.is_some()
                        || sharing.listen.is_some()
                        || sharing.policy.access != ShareAccess::None
                })
        {
            return Err(Error::Conflict(
                "Soulseek acquisition or sharing is configured. Stop and disable those services through the operator procedure before changing this account. Saving an account must not cause a login.".into(),
            ));
        }
        Ok(())
    }

    /// Passive status does not read credentials or make a network request.
    pub(crate) fn soulseek_account_status(&self) -> Result<SoulseekAccountStatus> {
        let (_, bytes) = self.read_file(&self.directory()?)?;
        let reason = self
            .require_editable()
            .and_then(|()| self.require_revision(self.active_revision(), &bytes))
            .and_then(|()| self.require_offline_soulseek_account())
            .err()
            .map(|error| error.to_string());
        Ok(SoulseekAccountStatus {
            available: reason.is_none(),
            reason,
            configured: self.active.soulseek_account.is_some()
                || self.active.local_acquisition.is_some()
                || self
                    .active
                    .production_worker
                    .as_ref()
                    .is_some_and(|worker| {
                        worker.acquisition.is_some()
                            || worker
                                .sharing
                                .as_ref()
                                .is_some_and(|sharing| sharing.account.is_some())
                    }),
            active_revision: self.active_revision().into(),
            pending_change_id: self
                .open_store(StoreOpenMode::ReadCurrent)?
                .pending_soulseek_account_change()?,
        })
    }

    pub(crate) fn save_soulseek_account(
        &self,
        request: SaveSoulseekAccount,
    ) -> Result<SoulseekAccountChange> {
        self.require_editable()?;
        self.require_offline_soulseek_account()?;
        crate::infrastructure::soulseek::validate_credentials(
            &request.username,
            &request.password,
        )?;
        let directory = self.directory()?;
        let (_, saved) = self.read_file(&directory)?;
        self.require_revision(self.active_revision(), &saved)?;
        self.require_revision(&request.expected_revision, &saved)?;
        FilePermissions::current()?.check(&directory.directory_file()?, true)?;
        let mut store = self.open_store(StoreOpenMode::Existing)?;
        // Commit non-secret intent before creating an immutable file. Interrupted
        // saves remain visible and can be cancelled without deleting evidence.
        let id = store.reserve_soulseek_account(&request.expected_revision)?;
        let saved = (|| {
            use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
            use std::io::Write;
            #[derive(Serialize)]
            struct CredentialFile<'a> {
                username: &'a str,
                password: &'a str,
            }
            let mut bytes = serde_json::to_vec(&CredentialFile {
                username: &request.username,
                password: &request.password,
            })?;
            let result = (|| {
                let mut options = cap_std::fs::OpenOptions::new();
                options
                    .write(true)
                    .create_new(true)
                    .follow(FollowSymlinks::No);
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = directory.open_with(file_name(id)?, &options)?.into_std();
                FilePermissions::current()?.check(&file, true)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                let identity = native_file_identity(&file)?;
                sync_directory(&directory)?;
                self.require_editable()?;
                self.require_revision(&request.expected_revision, &self.read_file(&directory)?.1)?;
                store.approve_soulseek_account(id, &identity)
            })();
            bytes.fill(0);
            result
        })();
        if saved.is_err() {
            store.fail_soulseek_account(id,
                "Account save failed. The current account is unchanged. Preserve private files and check host storage before trying again.")?;
        }
        saved?;
        store.soulseek_account_change(id)
    }

    fn check_soulseek_account_file(&self, store: &Store, id: i64) -> Result<()> {
        let name = file_name(id)?;
        let file = self.directory()?.open_file(std::ffi::OsStr::new(&name))?;
        FilePermissions::current()?.check(&file, true)?;
        let (_, identity) = store.soulseek_account_intent(id)?;
        if identity.as_deref() != Some(native_file_identity(&file)?.as_str()) {
            return Err(Error::Conflict("The approved account file was replaced. Save a new account instead of changing retained files.".into()));
        }
        Ok(())
    }

    pub(super) fn prepare_soulseek_account(
        &self,
        id: i64,
    ) -> Result<PreparedConfigurationActivation> {
        self.require_editable()?;
        self.require_offline_soulseek_account()?;
        let directory = self.directory()?;
        let (_, bytes) = self.read_file(&directory)?;
        let mut store = self.open_store(StoreOpenMode::Existing)?;
        let (expected, _) = store.soulseek_account_intent(id)?;
        self.require_revision(&expected, &bytes)?;
        self.require_revision(self.active_revision(), &bytes)?;
        self.check_soulseek_account_file(&store, id)?;
        let mut document: serde_json::Value = serde_json::from_slice(&bytes)?;
        document["soulseek_account"] = serde_json::to_value(SoulseekAccountSelection {
            change_id: id,
            credentials_file: file_name(id)?.into(),
        })?;
        let mut candidate = serde_json::to_vec_pretty(&document)?;
        candidate.push(b'\n');
        if candidate.len() as u64 > MAXIMUM {
            return Err(Error::InvalidInput(
                "The host configuration exceeds 16 KiB.".into(),
            ));
        }
        store.begin_soulseek_account(id, &digest(&candidate))?;
        Ok(PreparedConfigurationActivation {
            path: self.path.clone(),
            bytes: candidate,
            expected,
            directory,
            pending: format!(".domus-soulseek-{id}.pending"),
        })
    }

    /// Only the supervisor may call this after draining API and worker. Failed
    /// preparation restarts unchanged settings; uncertain publication stops.
    pub(crate) fn activate_stopped_soulseek_account(self, id: i64) -> Result<Self> {
        let prepared = match self.prepare_soulseek_account(id) {
            Ok(prepared) => prepared,
            Err(_) => {
                self.open_store(StoreOpenMode::Existing)?.fail_soulseek_account(id,
                    "Account activation failed before publication. No account reference was published by this operation.")?;
                eprintln!(
                    "Soulseek account activation rejected before publication. Check the saved result."
                );
                self.require_revision(
                    self.active_revision(),
                    &self.read_file(&self.directory()?)?.1,
                )?;
                return Ok(self);
            }
        };
        let path = self.path.clone();
        drop(self);
        let candidate = match prepared.validate() {
            Ok(candidate) => candidate,
            Err(_) => {
                let unchanged = Self::open(&path)?;
                if unchanged.active_revision() != prepared.expected {
                    return Err(Error::Conflict(
                        "Host configuration changed during account validation. Keep all components stopped and preserve activation evidence for operator recovery.".into(),
                    ));
                }
                unchanged.open_store(StoreOpenMode::Existing)?.fail_soulseek_account(id,
                    "Account validation failed before publication. The saved host configuration is unchanged.")?;
                eprintln!(
                    "Soulseek account validation rejected before publication. Check the saved result."
                );
                return Ok(unchanged);
            }
        };
        prepared.publish(&candidate)?;
        candidate.activate_configured_soulseek_account(
            &candidate.open_store(StoreOpenMode::Existing)?,
        )?;
        Ok(candidate)
    }

    /// Recovery completes only the published candidate revision and exact
    /// retained file. Merely validating or reading host settings cannot activate it.
    pub(crate) fn activate_configured_soulseek_account(&self, store: &Store) -> Result<()> {
        if let Some(selection) = &self.active.soulseek_account {
            self.require_soulseek_account_reference(selection)?;
            self.check_soulseek_account_file(store, selection.change_id)?;
            store.complete_soulseek_account(selection.change_id, self.active_revision())?;
        }
        Ok(())
    }

    pub(crate) fn require_configured_soulseek_account(&self, store: &Store) -> Result<()> {
        if let Some(selection) = &self.active.soulseek_account {
            self.require_soulseek_account_reference(selection)?;
            self.check_soulseek_account_file(store, selection.change_id)?;
            if store.soulseek_account_change(selection.change_id)?.state != "active" {
                return Err(Error::Conflict(
                    "Finish account activation through the API before starting the worker.".into(),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn require_soulseek_account_reference(
        &self,
        selection: &SoulseekAccountSelection,
    ) -> Result<()> {
        let expected = self
            .path
            .parent()
            .ok_or(Error::Internal("configuration parent missing"))?
            .join(file_name(selection.change_id)?);
        if selection.credentials_file != expected {
            return Err(Error::Conflict(
                "The account file reference differs from its approval.".into(),
            ));
        }
        Ok(())
    }
}
