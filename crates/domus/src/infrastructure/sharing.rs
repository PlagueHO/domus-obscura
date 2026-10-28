//! Passive worker sharing settings. Validation never starts a session or binds a root.
use crate::domain::sharing::{ShareAccess, SharePolicy};
use crate::infrastructure::store::Store;
use crate::infrastructure::worker::LocalAcquisition;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddrV4;
use std::path::PathBuf;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SharingConfiguration {
    pub policy: SharePolicy,
    pub account: Option<SharingAccount>,
    pub listen: Option<SocketAddrV4>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharingAccount {
    pub credentials_file: PathBuf,
    pub server: SocketAddrV4,
    pub acknowledge_plaintext_login: bool,
}

impl SharingConfiguration {
    /// Resolve exactly one account. A legacy acquisition account is a fallback,
    /// never a competing login or an implicit override of explicit settings.
    pub fn resolved_account(
        &self,
        acquisition: Option<&LocalAcquisition>,
    ) -> Result<Option<SharingAccount>> {
        let legacy = acquisition.map(|value| SharingAccount {
            credentials_file: value.credentials_file.clone(),
            server: value.server,
            acknowledge_plaintext_login: value.acknowledge_plaintext_login,
        });
        if let (Some(explicit), Some(legacy)) = (&self.account, &legacy)
            && (explicit.credentials_file != legacy.credentials_file
                || explicit.server != legacy.server
                || explicit.acknowledge_plaintext_login != legacy.acknowledge_plaintext_login)
        {
            return Err(Error::InvalidInput(
                "sharing and acquisition account settings conflict".into(),
            ));
        }

        Ok(self.account.clone().or(legacy))
    }

    pub fn validate(&self, store: &Store, acquisition: Option<&LocalAcquisition>) -> Result<()> {
        self.policy.validate()?;
        if self.listen.is_some_and(|listen| {
            listen.port() == 0 || listen.ip().is_multicast() || listen.ip().is_broadcast()
        }) {
            return Err(Error::InvalidInput(
                "sharing listener requires a unicast IPv4 address and nonzero port".into(),
            ));
        }
        let account = self.resolved_account(acquisition)?;
        if self.policy.access != ShareAccess::None && account.is_none() {
            return Err(Error::InvalidInput(
                "sharing requires an account configuration".into(),
            ));
        }
        if let Some(account) = account {
            if !account.credentials_file.is_absolute()
                || !account.acknowledge_plaintext_login
                || account.server.port() == 0
                || !super::soulseek::public_peer_ip(*account.server.ip())
            {
                return Err(Error::InvalidInput("sharing needs an absolute credential-file reference, a public server and explicit plaintext-login acknowledgement".into()));
            }
            super::local_file::protect_files(store, &[&account.credentials_file])?;
            super::worker::read_credentials(&account.credentials_file)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharing_configuration_defaults_are_disabled_and_legacy_accounts_must_match() -> Result<()> {
        let configuration: SharingConfiguration = serde_json::from_str("{}")?;
        assert_eq!(configuration.policy.access, ShareAccess::None);
        assert!(configuration.account.is_none());
        assert!(configuration.listen.is_none());
        let legacy = LocalAcquisition {
            staging_root: PathBuf::from("unused-staging"),
            credentials_file: PathBuf::from("account.json"),
            server: "8.8.8.8:2242"
                .parse()
                .map_err(|_| Error::Internal("fixture server"))?,
            acknowledge_plaintext_login: true,
        };
        let account = configuration
            .resolved_account(Some(&legacy))?
            .ok_or(Error::Internal("missing legacy account"))?;
        assert_eq!(account.credentials_file, legacy.credentials_file);
        let mut explicit = SharingConfiguration {
            account: Some(account),
            ..configuration
        };
        assert!(explicit.resolved_account(Some(&legacy)).is_ok());
        explicit
            .account
            .as_mut()
            .ok_or(Error::Internal("missing explicit account"))?
            .server = "8.8.4.4:2242"
            .parse()
            .map_err(|_| Error::Internal("fixture server"))?;
        assert!(explicit.resolved_account(Some(&legacy)).is_err());
        Ok(())
    }
}
