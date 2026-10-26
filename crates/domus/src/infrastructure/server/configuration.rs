//! Redacted configuration inspection. Reading saved JSON never activates it.
use super::{Configuration, Environment};
use crate::application::services::ServiceConfiguration;
use crate::{Error, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendCacheConfiguration {
    pub root: String,
    pub item_bytes: u32,
    pub total_bytes: u64,
    pub entries: u16,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendLocalAcquisition {
    pub staging_root: String,
    pub credentials_file_configured: bool,
    pub server: String,
    pub acknowledge_plaintext_login: bool,
}

/// Secret values and secret-file paths are never part of this browser contract.
/// Entra identifiers are represented only by their configured state.
#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendConfiguration {
    pub environment: String,
    pub system_settings_read_only: bool,
    pub services: ServiceConfiguration,
    pub cache: Option<BackendCacheConfiguration>,
    pub database: String,
    pub listen: String,
    pub origin: String,
    pub tenant_id_configured: bool,
    pub object_id_configured: bool,
    pub client_id_configured: bool,
    pub client_secret_file_configured: bool,
    pub certificate_file: String,
    pub private_key_file_configured: bool,
    pub browser_assets: Option<String>,
    pub local_acquisition: Option<BackendLocalAcquisition>,
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct BackendConfigurationState {
    pub active: BackendConfiguration,
    pub saved: BackendConfiguration,
    pub active_revision: String,
    pub saved_revision: String,
    pub restart_required: bool,
    /// A changed file still needs complete startup validation before activation.
    pub saved_requires_validation: bool,
}

pub(crate) struct ConfigurationSource {
    path: PathBuf,
    active: BackendConfiguration,
    active_revision: String,
}

impl ConfigurationSource {
    pub(super) fn new(path: &Path, bytes: &[u8], active: &Configuration) -> Self {
        Self {
            path: path.to_owned(),
            active: summary(active),
            active_revision: digest(bytes),
        }
    }

    pub(crate) fn read(&self) -> Result<BackendConfigurationState> {
        let bytes = crate::infrastructure::local_file::read_local(&self.path, 16384, false)?;
        let saved: Configuration = serde_json::from_slice(&bytes).map_err(|_| {
            Error::InvalidInput("saved backend configuration JSON is invalid".into())
        })?;
        let saved_revision = digest(&bytes);
        let restart_required = saved_revision != self.active_revision;
        Ok(BackendConfigurationState {
            active: self.active.clone(),
            saved: summary(&saved),
            active_revision: self.active_revision.clone(),
            saved_revision,
            restart_required,
            saved_requires_validation: restart_required,
        })
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn summary(configuration: &Configuration) -> BackendConfiguration {
    BackendConfiguration {
        environment: match configuration.environment {
            Environment::Production => "production",
            Environment::Development => "development",
        }
        .into(),
        system_settings_read_only: configuration.system_settings_read_only,
        services: configuration.services.clone(),
        cache: configuration
            .cache
            .as_ref()
            .map(|cache| BackendCacheConfiguration {
                root: cache.root.to_string_lossy().into_owned(),
                item_bytes: cache.limits.item_bytes,
                total_bytes: cache.limits.total_bytes,
                entries: cache.limits.entries,
            }),
        database: configuration.database.to_string_lossy().into_owned(),
        listen: configuration.listen.to_string(),
        origin: configuration.origin.clone(),
        tenant_id_configured: configuration.tenant_id.is_some(),
        object_id_configured: configuration.object_id.is_some(),
        client_id_configured: configuration.client_id.is_some(),
        client_secret_file_configured: configuration.client_secret_file.is_some(),
        certificate_file: configuration
            .certificate_file
            .to_string_lossy()
            .into_owned(),
        private_key_file_configured: !configuration.private_key_file.as_os_str().is_empty(),
        browser_assets: configuration
            .browser_assets
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        local_acquisition: configuration.local_acquisition.as_ref().map(|local| {
            BackendLocalAcquisition {
                staging_root: local.staging_root.to_string_lossy().into_owned(),
                credentials_file_configured: !local.credentials_file.as_os_str().is_empty(),
                server: local.server.to_string(),
                acknowledge_plaintext_login: local.acknowledge_plaintext_login,
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn backend_configuration_inspection_redacts_references_and_never_activates_saved_changes()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let state = crate::infrastructure::permissions::private_test_directory()?;
        let path = state.path().join("configuration.json");
        let mut saved = json!({
            "environment":"development",
            "database":state.path().join("generated.db"),
            "listen":"127.0.0.1:8443","origin":"https://localhost:8443",
            "certificate_file":state.path().join("certificate.pem"),
            "private_key_file":state.path().join("private-path-must-stay-hidden.pem"),
            "client_secret_file":state.path().join("secret-path-must-stay-hidden.txt"),
            "tenant_id":"tenant-must-stay-hidden","object_id":"object-must-stay-hidden",
            "client_id":"client-must-stay-hidden",
            "cache":{"root":state.path().join("generated-cache")},
            "local_acquisition":{
                "staging_root":state.path().join("generated-staging"),
                "credentials_file":state.path().join("credential-path-must-stay-hidden.json"),
                "server":"8.8.8.8:2242","acknowledge_plaintext_login":true
            }
        });
        let bytes = serde_json::to_vec(&saved)?;
        std::fs::write(&path, &bytes)?;
        // This inspection fixture is deliberately not a valid startup config.
        let active = serde_json::from_slice(&bytes)?;
        let source = ConfigurationSource::new(&path, &bytes, &active);
        let original = source.read()?;
        assert!(!original.restart_required);
        assert!(!original.saved_requires_validation);
        assert!(original.active.system_settings_read_only);
        assert_eq!(
            original
                .active
                .cache
                .as_ref()
                .ok_or("cache absent")?
                .item_bytes,
            9 * 1024 * 1024
        );
        let rendered = serde_json::to_string(&original)?;
        for hidden in [
            "private-path-must-stay-hidden",
            "secret-path-must-stay-hidden",
            "tenant-must-stay-hidden",
            "object-must-stay-hidden",
            "client-must-stay-hidden",
            "credential-path-must-stay-hidden",
        ] {
            assert!(!rendered.contains(hidden));
        }
        assert!(original.active.private_key_file_configured);
        assert!(
            original
                .active
                .local_acquisition
                .as_ref()
                .ok_or("local absent")?
                .credentials_file_configured
        );
        saved["origin"] = json!("https://localhost:9443");
        saved["system_settings_read_only"] = json!(false);
        let changed = serde_json::to_vec(&saved)?;
        std::fs::write(&path, &changed)?;
        let inspected = source.read()?;
        assert!(inspected.restart_required);
        assert!(inspected.saved_requires_validation);
        assert_eq!(inspected.active.origin, "https://localhost:8443");
        assert_eq!(inspected.saved.origin, "https://localhost:9443");
        assert!(inspected.active.system_settings_read_only);
        assert!(!inspected.saved.system_settings_read_only);
        assert_eq!(inspected.active_revision, original.active_revision);
        assert_ne!(inspected.active_revision, inspected.saved_revision);
        assert_eq!(std::fs::read(&path)?, changed);
        saved["unsupported"] = json!("never-echo-this-value");
        std::fs::write(&path, serde_json::to_vec(&saved)?)?;
        let error = source.read().err().ok_or("invalid JSON was accepted")?;
        assert!(!error.to_string().contains("never-echo-this-value"));
        std::fs::write(&path, b"{broken")?;
        assert!(source.read().is_err());
        std::fs::write(&path, vec![b' '; 16385])?;
        assert!(source.read().is_err());
        assert!(!state.path().join("generated.db").exists());
        assert!(!state.path().join("generated-cache").exists());
        Ok(())
    }
}
