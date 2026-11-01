//! Fixed-authority, bounded HTTPS for ACME and the dedicated Azure DNS identity.
use crate::domain::certificates::AcmeEnvironment;
use crate::infrastructure::http::catalog_client;
use crate::{Error, Result};
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Request, Response};
use http_body_util::BodyExt;
use instant_acme::{BodyWrapper, BytesResponse, HttpClient};
use std::future::Future;
use std::pin::Pin;

const MAX_BODY: usize = 262144;

#[derive(Clone, Default)]
pub(super) struct RetryDelay(std::sync::Arc<std::sync::Mutex<Option<u32>>>);

impl RetryDelay {
    pub(super) fn observe(&self, headers: &HeaderMap) -> Result<()> {
        if let Some(seconds) = retry_after(headers, std::time::SystemTime::now())? {
            let mut delay = self
                .0
                .lock()
                .map_err(|_| Error::Internal("certificate retry lock failed"))?;
            *delay = Some(delay.unwrap_or(0).max(seconds));
        }
        Ok(())
    }

    pub(super) fn take(&self) -> Result<Option<u32>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| Error::Internal("certificate retry lock failed"))?
            .take())
    }
}

#[derive(Clone)]
pub struct AcmeHttp {
    client: reqwest::Client,
    environment: AcmeEnvironment,
    retry_delay: RetryDelay,
}

impl AcmeHttp {
    pub fn new(environment: AcmeEnvironment) -> Result<Self> {
        Self::from_builder(environment, reqwest::Client::builder())
    }

    /// Trusted private-CA fixtures may resolve the fixed authority locally.
    /// Security policy is applied last; no runtime endpoint override exists.
    pub fn from_builder(
        environment: AcmeEnvironment,
        builder: reqwest::ClientBuilder,
    ) -> Result<Self> {
        Ok(Self {
            client: catalog_client(builder)?,
            environment,
            retry_delay: RetryDelay::default(),
        })
    }

    pub(super) fn retry_delay(&self) -> Result<Option<u32>> {
        self.retry_delay.take()
    }

    async fn execute(&self, request: Request<BodyWrapper<Bytes>>) -> Result<BytesResponse> {
        let (parts, body) = request.into_parts();
        let url = endpoint(&parts.uri.to_string(), self.environment.authority())?;
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::POST)
            || parts.headers.len() > 64
        {
            return Err(Error::Remote(
                "ACME request is outside the supported protocol",
            ));
        }
        let body = body
            .collect()
            .await
            .map_err(|_| Error::Remote("ACME request body could not be read"))?
            .to_bytes();
        if body.len() > 32768 {
            return Err(Error::Remote("ACME request exceeds its size limit"));
        }
        let response = self
            .client
            .request(parts.method, url)
            .headers(parts.headers)
            .body(body)
            .send()
            .await
            .map_err(|_| Error::Certificate {
                reason: "ACME HTTPS response is uncertain; reconcile the saved operation",
                retry_after_seconds: Some(60),
            })?;
        let status = response.status();
        let headers = response.headers().clone();
        let delay = retry_after(&headers, std::time::SystemTime::now())?;
        self.retry_delay.observe(&headers)?;
        let bytes = bounded_body(response).await?;
        if matches!(status.as_u16(), 429 | 503) {
            return Err(Error::Certificate {
                reason: "ACME requires a saved bounded retry",
                retry_after_seconds: Some(delay.unwrap_or(60)),
            });
        }
        let mut result = Response::builder().status(status);
        for (name, value) in headers.iter() {
            result = result.header(name, value);
        }
        Ok(BytesResponse::from(
            result
                .body(http_body_util::Full::new(Bytes::from(bytes)))
                .map_err(|_| Error::Remote("ACME response headers are invalid"))?,
        ))
    }
}

impl HttpClient for AcmeHttp {
    fn request(
        &self,
        request: Request<BodyWrapper<Bytes>>,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<BytesResponse, instant_acme::Error>> + Send>>
    {
        let client = self.clone();
        Box::pin(async move {
            client
                .execute(request)
                .await
                .map_err(|error| instant_acme::Error::Other(Box::new(error)))
        })
    }
}

pub(super) fn endpoint(value: &str, authority: &str) -> Result<reqwest::Url> {
    let url =
        reqwest::Url::parse(value).map_err(|_| Error::Remote("certificate endpoint is invalid"))?;
    if value.len() > 4096
        || url.scheme() != "https"
        || url.host_str() != Some(authority)
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Remote(
            "certificate endpoint is outside its fixed authority",
        ));
    }
    Ok(url)
}

pub(super) async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>> {
    if response.status().is_redirection() {
        return Err(Error::Remote("certificate endpoints must not redirect"));
    }
    if response.headers().len() > 64
        || response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.len())
            .sum::<usize>()
            > 16384
    {
        return Err(Error::Remote(
            "certificate response headers exceed their size limit",
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BODY as u64)
    {
        return Err(Error::Remote("certificate response exceeds 256 KiB"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::Remote("certificate response could not be read"))?
    {
        if chunk.len() > MAX_BODY - bytes.len() {
            return Err(Error::Remote("certificate response exceeds 256 KiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Unsupported or excessive delays require explicit recovery. Do not silently
/// shorten Retry-After, including an HTTP-date value.
pub fn retry_after(headers: &HeaderMap, now: std::time::SystemTime) -> Result<Option<u32>> {
    let mut values = headers.get_all("retry-after").iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(Error::Remote(
            "certificate Retry-After is ambiguous; recovery is required",
        ));
    }
    let value = value
        .to_str()
        .map_err(|_| Error::Remote("certificate Retry-After is invalid"))?;
    let seconds = match value.parse::<u64>() {
        Ok(seconds) => seconds,
        Err(_) => httpdate::parse_http_date(value)
            .map_err(|_| {
                Error::Remote("certificate Retry-After is unsupported; recovery is required")
            })?
            .duration_since(now)
            .map_or(0, |duration| {
                duration.as_secs() + u64::from(duration.subsec_nanos() != 0)
            }),
    };
    if seconds > 86400 {
        return Err(Error::Remote(
            "certificate Retry-After exceeds one day; recovery is required",
        ));
    }
    Ok(Some(seconds as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn certificates_acme_transport_rejects_redirects_oversized_bodies_and_untrusted_tls()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use axum::{Router, routing::get};
        let environment = AcmeEnvironment::Staging;
        let generated = rcgen::generate_simple_self_signed(vec![environment.authority().into()])?;
        let certificate = generated.cert.pem();
        let key = generated.signing_key.serialize_pem();
        let tls = crate::infrastructure::tls::configuration(
            environment.authority(),
            certificate.as_bytes(),
            key.as_bytes(),
        )?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let router = Router::new()
            .route("/ok", get(|| async { "bounded response" }))
            .route(
                "/redirect",
                get(|| async {
                    (
                        axum::http::StatusCode::TEMPORARY_REDIRECT,
                        [("Location", "https://other.invalid/never-follow")],
                    )
                }),
            )
            .route("/oversized", get(|| async { vec![b'x'; MAX_BODY + 1] }));
        let handle = axum_server::Handle::new();
        let serving = tokio::spawn(crate::infrastructure::server::serve_listener(
            listener,
            tls,
            router,
            handle.clone(),
        ));
        assert!(handle.listening().await.is_some());
        let client = AcmeHttp::from_builder(
            environment,
            reqwest::Client::builder()
                .tls_certs_only([reqwest::Certificate::from_pem(certificate.as_bytes())?])
                .resolve(environment.authority(), address),
        )?;
        let request = |path: &str| {
            Request::builder()
                .uri(format!("https://{}{path}", environment.authority()))
                .body(BodyWrapper::default())
        };
        let mut response = client.request(request("/ok")?).await?;
        assert_eq!(
            response.body.into_bytes().await?,
            Bytes::from_static(b"bounded response")
        );
        assert!(client.request(request("/redirect")?).await.is_err());
        assert!(client.request(request("/oversized")?).await.is_err());
        let untrusted = AcmeHttp::from_builder(
            environment,
            reqwest::Client::builder()
                .tls_danger_accept_invalid_certs(true)
                .tls_danger_accept_invalid_hostnames(true)
                .resolve(environment.authority(), address),
        )?;
        assert!(untrusted.request(request("/ok")?).await.is_err());
        handle.shutdown();
        serving.await??;
        Ok(())
    }

    #[test]
    fn certificates_restrict_every_acme_endpoint() {
        let authority = AcmeEnvironment::Production.authority();
        assert!(
            endpoint(
                "https://acme-v02.api.letsencrypt.org/acme/new-order",
                authority
            )
            .is_ok()
        );
        for url in [
            "http://acme-v02.api.letsencrypt.org/directory",
            "https://other.invalid/directory",
            "https://acme-v02.api.letsencrypt.org:444/directory",
            "https://secret@acme-v02.api.letsencrypt.org/directory",
            "https://acme-v02.api.letsencrypt.org/directory#fragment",
        ] {
            assert!(endpoint(url, authority).is_err());
        }
    }

    #[test]
    fn certificates_do_not_shorten_or_hide_retry_after() -> Result<()> {
        let mut headers = HeaderMap::new();
        assert_eq!(retry_after(&headers, std::time::UNIX_EPOCH)?, None);
        headers.insert(
            "retry-after",
            "120".parse().map_err(|_| Error::Internal("test header"))?,
        );
        assert_eq!(retry_after(&headers, std::time::UNIX_EPOCH)?, Some(120));
        headers.insert(
            "retry-after",
            "86401"
                .parse()
                .map_err(|_| Error::Internal("test header"))?,
        );
        assert!(retry_after(&headers, std::time::UNIX_EPOCH).is_err());
        headers.insert(
            "retry-after",
            "unknown"
                .parse()
                .map_err(|_| Error::Internal("test header"))?,
        );
        assert!(retry_after(&headers, std::time::UNIX_EPOCH).is_err());
        Ok(())
    }

    #[test]
    fn certificates_retry_observations_share_the_longest_delay_and_clear_after_each_step()
    -> Result<()> {
        let delay = RetryDelay::default();
        let clone = delay.clone();
        let mut headers = HeaderMap::new();
        headers.insert(
            "retry-after",
            "120".parse().map_err(|_| Error::Internal("test header"))?,
        );
        delay.observe(&headers)?;
        headers.insert(
            "retry-after",
            "30".parse().map_err(|_| Error::Internal("test header"))?,
        );
        clone.observe(&headers)?;
        assert_eq!(clone.take()?, Some(120));
        assert_eq!(delay.take()?, None);
        Ok(())
    }
}
