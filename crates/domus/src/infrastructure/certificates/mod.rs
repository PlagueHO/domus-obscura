//! Optional managed TLS. Constructing adapters never performs network I/O.
pub mod acme;
pub mod azure;
pub mod dns;
mod driver;
pub mod http;
pub mod state;
pub(crate) use driver::HostGuard;
pub(crate) use driver::RenewalOwner;
pub use driver::bootstrap;
pub use driver::recover;

use crate::domain::certificates::{AcmeEnvironment, CertificateNames};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub(crate) struct StateDirectory {
    pub path: PathBuf,
    pub native: crate::infrastructure::filesystem::NativeRoot,
    pub exists: bool,
}

impl StateDirectory {
    pub fn inspect(path: &std::path::Path) -> Result<Self> {
        validate_state_directory(path)?;
        let (path, directory, exists) =
            match crate::infrastructure::filesystem::LibraryDirectory::canonical(path) {
                Ok((path, directory)) => (path, directory, true),
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    let parent = path.parent().ok_or_else(|| {
                        Error::InvalidInput("certificate parent is missing".into())
                    })?;
                    let (parent, directory) =
                        crate::infrastructure::filesystem::LibraryDirectory::canonical(parent)?;
                    (
                        parent.join(path.file_name().ok_or_else(|| {
                            Error::InvalidInput("certificate directory name is missing".into())
                        })?),
                        directory,
                        false,
                    )
                }
                Err(error) => return Err(error),
            };
        let native = directory.native_root().clone();
        crate::infrastructure::permissions::FilePermissions::current()?
            .check(&directory.into_directory().into_std_file(), true)?;
        Ok(Self {
            path,
            native,
            exists,
        })
    }

    pub fn overlaps(
        &self,
        path: &std::path::Path,
        native: &crate::infrastructure::filesystem::NativeRoot,
    ) -> bool {
        self.path.starts_with(path)
            || path.starts_with(&self.path)
            || self.native.ancestors.contains(&native.identity)
            || (self.exists && native.ancestors.contains(&self.native.identity))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedTlsConfiguration {
    pub environment: AcmeEnvironment,
    pub state_directory: PathBuf,
    pub dns: azure::AzureDnsConfiguration,
    pub terms_of_service_agreed: bool,
}

pub fn status(configuration: &std::path::Path) -> Result<state::CertificateStatus> {
    let (managed, hostname, _) = crate::infrastructure::server::configuration::ConfigurationService::certificate_configuration(configuration)?;
    state::StateStore::status(
        &managed.state_directory,
        &managed.binding(&hostname)?,
        managed.environment,
    )
}

impl ManagedTlsConfiguration {
    /// Credential rotation can retain pending work only when this frozen
    /// hostname, authority and Azure identity/resource policy stays unchanged.
    pub fn resource_binding(&self, hostname: &str) -> Result<String> {
        self.validate(hostname)?;
        let bytes = serde_json::to_vec(&(
            hostname,
            self.environment,
            &self.dns.tenant_id,
            &self.dns.client_id,
            &self.dns.subscription_id,
            &self.dns.resource_group,
            &self.dns.zone,
        ))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// Validate policy only. Account creation and DNS mutation are never setup
    /// validation effects. Credential contents remain private local files.
    pub fn validate(&self, hostname: &str) -> Result<CertificateNames> {
        if !self.terms_of_service_agreed {
            return Err(Error::InvalidInput(
                "managed TLS requires explicit ACME terms acceptance".into(),
            ));
        }
        validate_state_directory(&self.state_directory)?;
        self.dns.names(hostname)
    }

    /// Bind account/resource/hostname policy and the exact credential contents.
    /// Rotation is an explicit recovery boundary, never an ambient fallback.
    pub fn binding(&self, hostname: &str) -> Result<String> {
        self.validate(hostname)?;
        let bytes = crate::infrastructure::local_file::read_local(
            &self.dns.client_secret_file,
            4096,
            true,
        )?;
        let mut hash = Sha256::new();
        hash.update(serde_json::to_vec(self)?);
        hash.update([0]);
        hash.update(hostname.as_bytes());
        hash.update([0]);
        hash.update(bytes);
        Ok(format!("{:x}", hash.finalize()))
    }
}

fn validate_state_directory(path: &std::path::Path) -> Result<()> {
    crate::infrastructure::local_file::validate_reference(path)?;
    for (depth, ancestor) in path.ancestors().enumerate() {
        if depth > 64 {
            return Err(Error::InvalidInput(
                "certificate state path exceeds its depth limit".into(),
            ));
        }
        let directory = match crate::infrastructure::filesystem::LibraryDirectory::open(ancestor) {
            Ok(directory) => directory.into_directory(),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(error),
        };
        match directory.symlink_metadata(".git") {
            Ok(_) => {
                return Err(Error::InvalidInput(
                    "certificate state must stay outside Git workspaces".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
