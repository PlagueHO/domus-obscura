use crate::{Error, Result};
use axum_server::tls_rustls::RustlsConfig;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{ServerName, UnixTime};
use rustls::{RootCertStore, ServerConfig};
use std::sync::{Arc, OnceLock};

pub fn initialize_crypto() -> Result<()> {
    static INITIALIZED: OnceLock<bool> = OnceLock::new();
    let initialized = INITIALIZED.get_or_init(|| {
        if rustls::crypto::CryptoProvider::get_default().is_some() {
            return true;
        }
        match rustls::crypto::ring::default_provider().install_default() {
            Ok(()) => true,
            Err(_) => rustls::crypto::CryptoProvider::get_default().is_some(),
        }
    });
    if !initialized {
        return Err(Error::Internal("TLS crypto provider initialization failed"));
    }
    Ok(())
}

/// Validate the key pair, chain, hostname and current validity before activation.
/// The supplied chain anchors this local check; HTTPS clients independently
/// decide whether the certificate is trusted by their configured trust store.
pub fn configuration(host: &str, certificate: &[u8], private_key: &[u8]) -> Result<RustlsConfig> {
    if certificate.len() > 262144 || private_key.len() > 16384 {
        return Err(Error::InvalidInput(
            "TLS material exceeds its size limit".into(),
        ));
    }
    let certificates = rustls_pemfile::certs(&mut &certificate[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Error::InvalidInput("TLS certificate PEM is invalid".into()))?;
    if certificates.is_empty() || certificates.len() > 8 {
        return Err(Error::InvalidInput(
            "TLS chain must contain 1 through 8 certificates".into(),
        ));
    }
    let key = rustls_pemfile::private_key(&mut &private_key[..])
        .map_err(|_| Error::InvalidInput("TLS private key PEM is invalid".into()))?
        .ok_or_else(|| Error::InvalidInput("TLS private key is missing".into()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut anchors = RootCertStore::empty();
    let anchor = certificates
        .last()
        .ok_or_else(|| Error::InvalidInput("TLS chain is empty".into()))?;
    anchors
        .add(anchor.clone())
        .map_err(|_| Error::InvalidInput("TLS chain anchor is invalid".into()))?;
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(anchors),
        provider.clone(),
    )
    .build()
    .map_err(|_| Error::InvalidInput("TLS chain verifier could not be created".into()))?;
    let name = ServerName::try_from(host.to_owned())
        .map_err(|_| Error::InvalidInput("TLS hostname is invalid".into()))?;
    verifier
        .verify_server_cert(
            &certificates[0],
            &certificates[1..],
            &name,
            &[],
            UnixTime::now(),
        )
        .map_err(|_| {
            Error::InvalidInput(
                "TLS certificate is invalid, expired, or has the wrong hostname".into(),
            )
        })?;
    let mut server = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::InvalidInput("TLS protocol configuration failed".into()))?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|_| Error::InvalidInput("TLS certificate and private key do not match".into()))?;
    server.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(RustlsConfig::from_config(Arc::new(server)))
}

/// Failed candidates leave the shared active configuration unchanged.
pub fn reload(
    active: &RustlsConfig,
    host: &str,
    certificate: &[u8],
    private_key: &[u8],
) -> Result<()> {
    let checked = configuration(host, certificate, private_key)?;
    active.reload_from_config(checked.get_inner());
    Ok(())
}
