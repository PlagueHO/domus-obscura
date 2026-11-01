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
    checked_configuration(host, certificate, private_key, false)
}

/// Managed production candidates use independent public trust anchors, not a
/// root supplied by the ACME response. Failed candidates never replace TLS.
pub fn managed_configuration(
    host: &str,
    certificate: &[u8],
    private_key: &[u8],
) -> Result<RustlsConfig> {
    checked_configuration(host, certificate, private_key, true)
}

fn checked_configuration(
    host: &str,
    certificate: &[u8],
    private_key: &[u8],
    public_trust: bool,
) -> Result<RustlsConfig> {
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
    if public_trust {
        anchors.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    } else {
        let anchor = certificates
            .last()
            .ok_or_else(|| Error::InvalidInput("TLS chain is empty".into()))?;
        anchors
            .add(anchor.clone())
            .map_err(|_| Error::InvalidInput("TLS chain anchor is invalid".into()))?;
    }
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

/// Read exact leaf validity after bounded PEM parsing. This does not by itself
/// prove trust; callers must also validate the pair through a TLS verifier.
pub fn certificate_validity(host: &str, certificate: &[u8]) -> Result<(u64, u64)> {
    if certificate.len() > 262144 {
        return Err(Error::InvalidInput(
            "TLS certificate exceeds its size limit".into(),
        ));
    }
    let leaf = rustls_pemfile::certs(&mut &certificate[..])
        .next()
        .ok_or_else(|| Error::InvalidInput("TLS leaf certificate is missing".into()))?
        .map_err(|_| Error::InvalidInput("TLS leaf PEM is invalid".into()))?;
    let (_, parsed) = x509_parser::parse_x509_certificate(&leaf)
        .map_err(|_| Error::InvalidInput("TLS leaf certificate is invalid".into()))?;
    let names = parsed
        .subject_alternative_name()
        .map_err(|_| Error::InvalidInput("TLS subject alternative names are invalid".into()))?
        .ok_or_else(|| Error::InvalidInput("TLS subject alternative name is missing".into()))?;
    if names.value.general_names.len() != 1
        || !matches!(&names.value.general_names[0],
            x509_parser::extensions::GeneralName::DNSName(name) if *name == host
        )
    {
        return Err(Error::InvalidInput(
            "managed TLS must cover only the exact configured hostname".into(),
        ));
    }
    let start = u64::try_from(parsed.validity().not_before.timestamp())
        .map_err(|_| Error::InvalidInput("TLS validity start is unsupported".into()))?;
    let end = u64::try_from(parsed.validity().not_after.timestamp())
        .map_err(|_| Error::InvalidInput("TLS validity end is unsupported".into()))?;
    if start >= end {
        return Err(Error::InvalidInput(
            "TLS validity interval is invalid".into(),
        ));
    }
    Ok((start, end))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificates_managed_trust_is_independent_from_the_supplied_anchor()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let generated = rcgen::generate_simple_self_signed(vec!["api.home.example".into()])?;
        let certificate = generated.cert.pem();
        let key = generated.signing_key.serialize_pem();
        assert!(configuration("api.home.example", certificate.as_bytes(), key.as_bytes()).is_ok());
        assert!(
            managed_configuration("api.home.example", certificate.as_bytes(), key.as_bytes())
                .is_err()
        );
        assert!(certificate_validity("other.home.example", certificate.as_bytes()).is_err());
        let (start, end) = certificate_validity("api.home.example", certificate.as_bytes())?;
        assert!(start < end);
        Ok(())
    }
}
