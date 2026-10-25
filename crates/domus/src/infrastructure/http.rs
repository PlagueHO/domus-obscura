use super::tls::initialize_crypto;
use crate::{Error, Result};
use openidconnect::{HttpRequest, HttpResponse};
use std::time::Duration;

pub(crate) fn catalog_client(builder: reqwest::ClientBuilder) -> Result<reqwest::Client> {
    initialize_crypto()?;
    builder
        .tls_backend_rustls()
        .tls_danger_accept_invalid_certs(false)
        .tls_danger_accept_invalid_hostnames(false)
        .tls_version_min(reqwest::tls::Version::TLS_1_2)
        .https_only(true)
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(1)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .user_agent(concat!(
            "DomusObscura/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/PlagueHO)"
        ))
        .build()
        .map_err(|_| Error::Remote("catalog HTTP client initialization failed"))
}

#[derive(Clone)]
pub struct EntraHttpClient {
    client: reqwest::Client,
}

impl EntraHttpClient {
    pub fn new() -> Result<Self> {
        Self::from_builder(reqwest::Client::builder())
    }

    /// Permit trusted private-CA fixtures, but apply transport policy last.
    /// There is no runtime option for a different issuer or disabled TLS checks.
    pub fn from_builder(builder: reqwest::ClientBuilder) -> Result<Self> {
        initialize_crypto()?;
        let client = builder
            .tls_backend_rustls()
            .tls_danger_accept_invalid_certs(false)
            .tls_danger_accept_invalid_hostnames(false)
            .tls_version_min(reqwest::tls::Version::TLS_1_2)
            .https_only(true)
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .pool_max_idle_per_host(2)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .map_err(|_| Error::Remote("identity HTTP client initialization failed"))?;
        Ok(Self { client })
    }

    /// Restrict every discovery/JWKS/token request, not only the initial URL.
    /// Never follow a provider redirect or retain an unbounded response body.
    pub async fn execute(self, request: HttpRequest) -> Result<HttpResponse> {
        let (parts, body) = request.into_parts();
        if body.len() > 32768 || parts.headers.len() > 64 {
            return Err(Error::Remote("identity request exceeds its size limit"));
        }
        let url = reqwest::Url::parse(&parts.uri.to_string())
            .map_err(|_| Error::Remote("identity endpoint URL is invalid"))?;
        if url.scheme() != "https"
            || url.host_str() != Some("login.microsoftonline.com")
            || url.port_or_known_default() != Some(443)
            || parts.uri.to_string().len() > 4096
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Remote(
                "identity endpoint is outside the configured authority",
            ));
        }
        let mut response = self
            .client
            .request(parts.method, url)
            .headers(parts.headers)
            .body(body)
            .send()
            .await
            .map_err(|_| Error::Remote("identity HTTPS request failed"))?;
        if response.headers().len() > 64
            || response
                .headers()
                .iter()
                .map(|(name, value)| name.as_str().len() + value.len())
                .sum::<usize>()
                > 16384
        {
            return Err(Error::Remote(
                "identity response headers exceed their size limit",
            ));
        }
        let mut result = openidconnect::http::Response::builder().status(response.status());
        for (name, value) in response.headers() {
            result = result.header(name, value);
        }
        if response
            .content_length()
            .is_some_and(|length| length > 524288)
        {
            return Err(Error::Remote("identity response exceeds 512 KiB"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Remote("identity response could not be read"))?
        {
            if chunk.len() > 524288 - bytes.len() {
                return Err(Error::Remote("identity response exceeds 512 KiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        result
            .body(bytes)
            .map_err(|_| Error::Remote("identity HTTP response is invalid"))
    }
}
