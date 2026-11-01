//! ACME protocol operations for one exact DNS identifier. The application owns
//! persistence and retry decisions; this adapter never creates replacement work.
use super::http::{AcmeHttp, endpoint};
use crate::domain::certificates::{AcmeEnvironment, CertificateNames};
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeStatus, ChallengeType, Identifier,
    Key, NewOrder, Order, OrderStatus,
};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::Value;
use std::time::Duration;

pub struct Acme {
    account: Account,
    environment: AcmeEnvironment,
    hostname: String,
}

pub enum AccountOperation {
    Create { terms_of_service_agreed: bool },
    LookupExisting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenewalInformation {
    Unsupported,
    Suggested {
        start: u64,
        end: u64,
        refresh_after_seconds: u32,
    },
}

/// Generate the account key before account submission. The application must
/// save this exact private value before invoking `register`.
pub fn account_key() -> Result<String> {
    let (_, key) = Key::generate_pkcs8().map_err(protocol_error)?;
    Ok(STANDARD.encode(key.secret_pkcs8_der()))
}

/// Generate one key and its exact CSR before finalization. Neither value is a
/// browser contract, and both must survive an uncertain finalization response.
pub fn certificate_request(names: &CertificateNames) -> Result<(String, Vec<u8>)> {
    let key = rcgen::KeyPair::generate()
        .map_err(|_| Error::Internal("certificate key generation failed"))?;
    let mut parameters = rcgen::CertificateParams::new(vec![names.hostname.clone()])
        .map_err(|_| Error::InvalidInput("certificate request hostname is invalid".into()))?;
    parameters.distinguished_name = rcgen::DistinguishedName::new();
    let request = parameters
        .serialize_request(&key)
        .map_err(|_| Error::Internal("certificate request signing failed"))?;
    Ok((key.serialize_pem(), request.der().to_vec()))
}

impl Acme {
    /// LookupExisting reconciles an uncertain submission with the saved key.
    /// A failed lookup must never become a new account creation automatically.
    pub async fn register(
        names: &CertificateNames,
        environment: AcmeEnvironment,
        http: AcmeHttp,
        saved_key: &str,
        operation: AccountOperation,
    ) -> Result<(Self, Value)> {
        let existing_only = match operation {
            AccountOperation::LookupExisting => true,
            AccountOperation::Create {
                terms_of_service_agreed: true,
            } => false,
            AccountOperation::Create {
                terms_of_service_agreed: false,
            } => {
                return Err(Error::InvalidInput(
                    "ACME account creation requires explicit terms acceptance".into(),
                ));
            }
        };
        let der = STANDARD
            .decode(saved_key)
            .map_err(|_| Error::InvalidInput("saved ACME account key is invalid".into()))?;
        if der.len() > 4096 {
            return Err(Error::InvalidInput(
                "saved ACME account key is oversized".into(),
            ));
        }
        let der = PrivatePkcs8KeyDer::from(der);
        let key = Key::from_pkcs8_der(der.clone_key()).map_err(protocol_error)?;
        let builder = Account::builder_with_http(Box::new(http));
        let result = if existing_only {
            builder
                .from_key(
                    (key, PrivateKeyDer::Pkcs8(der)),
                    environment.directory().into(),
                )
                .await
        } else {
            builder
                .create_from_key(
                    (key, PrivateKeyDer::Pkcs8(der)),
                    environment.directory().into(),
                )
                .await
        };
        let (account, credentials) = result.map_err(protocol_error)?;
        Ok((
            Self {
                account,
                environment,
                hostname: names.hostname.clone(),
            },
            serde_json::to_value(credentials)?,
        ))
    }

    pub async fn restore(
        names: &CertificateNames,
        environment: AcmeEnvironment,
        http: AcmeHttp,
        credentials: Value,
    ) -> Result<Self> {
        let credentials: AccountCredentials =
            serde_json::from_value(credentials).map_err(|_| {
                Error::InvalidInput("saved ACME account credentials are invalid".into())
            })?;
        let account = Account::builder_with_http(Box::new(http))
            .from_credentials(credentials)
            .await
            .map_err(protocol_error)?;
        Ok(Self {
            account,
            environment,
            hostname: names.hostname.clone(),
        })
    }

    /// The application must commit OrderPending before calling this method.
    /// A lost URL is uncertain ownership, not permission to call it again.
    pub async fn new_order(&self) -> Result<Order> {
        let identifiers = [Identifier::Dns(self.hostname.clone())];
        let mut order = self
            .account
            .new_order(&NewOrder::new(&identifiers))
            .await
            .map_err(protocol_error)?;
        self.check_order(&mut order)?;
        Ok(order)
    }

    pub async fn restore_order(&self, url: &str) -> Result<Order> {
        endpoint(url, self.environment.authority())?;
        let mut order = self
            .account
            .order(url.to_owned())
            .await
            .map_err(protocol_error)?;
        self.check_order(&mut order)?;
        Ok(order)
    }

    fn check_order(&self, order: &mut Order) -> Result<()> {
        endpoint(order.url(), self.environment.authority())?;
        let state = order.state();
        if state.authorizations.len() != 1 {
            return Err(Error::Remote(
                "ACME order must have exactly one authorization",
            ));
        }
        endpoint(&state.finalize, self.environment.authority())?;
        if let Some(certificate) = &state.certificate {
            endpoint(certificate, self.environment.authority())?;
        }
        Ok(())
    }

    pub async fn challenge_value(&self, order: &mut Order) -> Result<Option<String>> {
        self.check_order(order)?;
        let mut stream = order.authorizations();
        let mut authorization = stream
            .next()
            .await
            .ok_or(Error::Remote("ACME order authorization is missing"))?
            .map_err(protocol_error)?;
        let identifier = authorization.identifier();
        if identifier.wildcard
            || !matches!(identifier.identifier, Identifier::Dns(name) if name == &self.hostname)
        {
            return Err(Error::Remote(
                "ACME authorization does not match the exact hostname",
            ));
        }
        match authorization.status {
            AuthorizationStatus::Valid => Ok(None),
            AuthorizationStatus::Pending => {
                let challenge = authorization
                    .challenge(ChallengeType::Dns01)
                    .ok_or(Error::Remote("ACME authorization has no DNS-01 challenge"))?;
                Ok(Some(challenge.key_authorization().dns_value()))
            }
            _ => Err(Error::Certificate {
                reason: "ACME authorization is no longer usable; local recovery is required",
                retry_after_seconds: None,
            }),
        }
    }

    /// Call only after authoritative propagation and a durable validation intent.
    pub async fn notify_ready(&self, order: &mut Order, value: &str) -> Result<()> {
        self.check_order(order)?;
        let mut stream = order.authorizations();
        let mut authorization = stream
            .next()
            .await
            .ok_or(Error::Remote("ACME order authorization is missing"))?
            .map_err(protocol_error)?;
        let identifier = authorization.identifier();
        if identifier.wildcard
            || !matches!(identifier.identifier, Identifier::Dns(name) if name == &self.hostname)
        {
            return Err(Error::Remote(
                "ACME authorization does not match the exact hostname",
            ));
        }
        if authorization.status == AuthorizationStatus::Valid {
            return Ok(());
        }
        if authorization.status != AuthorizationStatus::Pending {
            return Err(Error::Remote("ACME authorization cannot be submitted"));
        }
        let mut challenge = authorization
            .challenge(ChallengeType::Dns01)
            .ok_or(Error::Remote("ACME authorization has no DNS-01 challenge"))?;
        if challenge.key_authorization().dns_value() != value {
            return Err(Error::Conflict("saved ACME challenge changed".into()));
        }
        match challenge.status {
            ChallengeStatus::Pending => challenge.set_ready().await.map_err(protocol_error),
            ChallengeStatus::Processing | ChallengeStatus::Valid => Ok(()),
            ChallengeStatus::Invalid => Err(Error::Remote("ACME DNS-01 challenge was rejected")),
        }
    }

    /// One observation, without a hidden polling loop or sleep.
    pub async fn refresh(&self, order: &mut Order) -> Result<OrderStatus> {
        let status = order.refresh().await.map_err(protocol_error)?.status;
        self.check_order(order)?;
        Ok(status)
    }

    pub async fn finalize(&self, order: &mut Order, csr: &[u8]) -> Result<()> {
        self.check_order(order)?;
        if self.challenge_value(order).await?.is_some() {
            return Err(Error::Remote(
                "ACME finalization requires the exact valid authorization",
            ));
        }
        if order.state().status != OrderStatus::Ready || csr.is_empty() || csr.len() > 16384 {
            return Err(Error::InvalidInput(
                "ACME finalization requires a ready order and saved CSR".into(),
            ));
        }
        order.finalize_csr(csr).await.map_err(protocol_error)?;
        self.check_order(order)
    }

    pub async fn certificate(&self, order: &mut Order) -> Result<Option<String>> {
        self.check_order(order)?;
        let certificate = order.certificate().await.map_err(protocol_error)?;
        if certificate.as_ref().is_some_and(|pem| pem.len() > 262144) {
            return Err(Error::Remote("ACME certificate exceeds its size limit"));
        }
        Ok(certificate)
    }

    pub async fn renewal_information(&self, pem: &[u8]) -> Result<RenewalInformation> {
        let identifier = certificate_identifier(pem)?;
        let (info, delay) = match self.account.renewal_info(&identifier).await {
            Err(instant_acme::Error::Unsupported("ACME renewal information (ARI)")) => {
                return Ok(RenewalInformation::Unsupported);
            }
            result => result.map_err(protocol_error)?,
        };
        if delay > Duration::from_secs(86400) {
            return Err(Error::Certificate {
                reason: "ACME renewal refresh exceeds one day; local recovery is required",
                retry_after_seconds: None,
            });
        }
        let start = u64::try_from(info.suggested_window.start.unix_timestamp())
            .map_err(|_| Error::Remote("ACME renewal window starts before the Unix epoch"))?;
        let end = u64::try_from(info.suggested_window.end.unix_timestamp())
            .map_err(|_| Error::Remote("ACME renewal window ends before the Unix epoch"))?;
        if start >= end {
            return Err(Error::Remote("ACME renewal window is invalid"));
        }
        Ok(RenewalInformation::Suggested {
            start,
            end,
            refresh_after_seconds: u32::try_from(delay.as_secs())
                .map_err(|_| Error::Remote("ACME renewal refresh is invalid"))?
                + u32::from(delay.subsec_nanos() != 0),
        })
    }
}

fn certificate_identifier(pem: &[u8]) -> Result<instant_acme::CertificateIdentifier<'static>> {
    if pem.len() > 262144 {
        return Err(Error::InvalidInput(
            "ACME renewal certificate is oversized".into(),
        ));
    }
    let leaf = rustls_pemfile::certs(&mut std::io::Cursor::new(pem))
        .next()
        .transpose()?
        .ok_or_else(|| Error::InvalidInput("ACME renewal certificate is missing".into()))?;
    let (remaining, certificate) = x509_parser::parse_x509_certificate(leaf.as_ref())
        .map_err(|_| Error::InvalidInput("ACME renewal certificate is invalid".into()))?;
    if !remaining.is_empty() {
        return Err(Error::InvalidInput(
            "ACME renewal certificate has trailing data".into(),
        ));
    }
    let mut identifiers = certificate.extensions().iter().filter_map(|extension| {
        match extension.parsed_extension() {
            x509_parser::extensions::ParsedExtension::AuthorityKeyIdentifier(aki) => {
                aki.key_identifier.as_ref().map(|identifier| identifier.0)
            }
            _ => None,
        }
    });
    let authority = identifiers.next().ok_or_else(|| {
        Error::InvalidInput("ACME renewal certificate has no authority key identifier".into())
    })?;
    if authority.is_empty() || identifiers.next().is_some() {
        return Err(Error::InvalidInput(
            "ACME renewal authority key identifier is invalid".into(),
        ));
    }
    let serial = certificate.raw_serial();
    let first = serial
        .iter()
        .position(|byte| *byte != 0)
        .ok_or_else(|| Error::InvalidInput("ACME renewal certificate serial is invalid".into()))?;
    Ok(
        instant_acme::CertificateIdentifier::new(authority.into(), (&serial[first..]).into())
            .into_owned(),
    )
}

fn protocol_error(error: instant_acme::Error) -> Error {
    match error {
        instant_acme::Error::Other(error) => match error.downcast::<Error>() {
            Ok(error) => *error,
            Err(_) => Error::Certificate {
                reason: "ACME transport failed; reconcile submitted work before recovery",
                retry_after_seconds: None,
            },
        },
        _ => Error::Certificate {
            reason: "ACME rejected the operation; reconcile submitted work before recovery",
            retry_after_seconds: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x509_parser::prelude::FromDer;

    #[tokio::test]
    async fn certificates_acme_creation_requires_terms_before_key_parsing_or_network() -> Result<()>
    {
        let names = CertificateNames::new("api.home.example", "home.example")?;
        let environment = AcmeEnvironment::Staging;
        let result = Acme::register(
            &names,
            environment,
            AcmeHttp::new(environment)?,
            "not-a-key",
            AccountOperation::Create {
                terms_of_service_agreed: false,
            },
        )
        .await;
        assert!(matches!(result, Err(Error::InvalidInput(message))
            if message == "ACME account creation requires explicit terms acceptance"));
        Ok(())
    }

    #[tokio::test]
    async fn certificates_acme_fixture_restores_account_order_and_finalizes_saved_csr()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::infrastructure::api::ApiError;
        use axum::http::StatusCode;
        use axum::{
            Json, Router,
            body::Bytes,
            routing::{get, head, post},
        };
        use serde_json::json;
        use std::sync::{
            Arc,
            atomic::{AtomicU8, Ordering},
        };
        let environment = AcmeEnvironment::Staging;
        let authority = environment.authority();
        let base = format!("https://{authority}");
        let nonce = [("Replay-Nonce", "Zml4dHVyZS1ub25jZQ")];
        let names = CertificateNames::new("api.home.example", "home.example")?;
        let (private_key, csr) = certificate_request(&names)?;
        let certificate_key = rcgen::KeyPair::from_pem(&private_key)?;
        let mut parameters = rcgen::CertificateParams::new(vec![names.hostname.clone()])?;
        parameters.use_authority_key_identifier_extension = true;
        let certificate = parameters.self_signed(&certificate_key)?.pem();
        let generated = rcgen::generate_simple_self_signed(vec![authority.into()])?;
        let anchor = generated.cert.pem();
        let tls = crate::infrastructure::tls::configuration(
            authority,
            anchor.as_bytes(),
            generated.signing_key.serialize_pem().as_bytes(),
        )?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let phase = Arc::new(AtomicU8::new(0));
        let order_url = format!("{base}/orders/1");
        let authorization_url = format!("{base}/authorization/1");
        let finalize_url = format!("{base}/finalize/1");
        let certificate_url = format!("{base}/certificate/1");
        let challenge_url = format!("{base}/challenge/1");
        let order_state = {
            let phase = phase.clone();
            let authorization_url = authorization_url.clone();
            let finalize_url = finalize_url.clone();
            let certificate_url = certificate_url.clone();
            move || {
                let phase = phase.load(Ordering::SeqCst);
                json!({
                    "status": match phase { 0 => "pending", 1 => "ready", _ => "valid" },
                    "authorizations": [authorization_url],
                    "finalize": finalize_url,
                    "certificate": if phase >= 2 { Some(&certificate_url) } else { None }
                })
            }
        };
        let decode_payload = |body: &Bytes| -> std::result::Result<Value, ApiError> {
            use base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let envelope: Value = serde_json::from_slice(body).map_err(Error::from)?;
            let payload = envelope["payload"]
                .as_str()
                .ok_or(Error::Internal("fixture JWS payload"))?;
            let bytes = URL_SAFE_NO_PAD
                .decode(payload)
                .map_err(|_| Error::Internal("fixture JWS encoding"))?;
            Ok(serde_json::from_slice(&bytes).map_err(Error::from)?)
        };
        let router = Router::new()
            .route(
                "/directory",
                get({
                    let base = base.clone();
                    move || async move {
                        Json(json!({
                            "newNonce": format!("{base}/nonce"),
                            "newAccount": format!("{base}/new-account"),
                            "newOrder": format!("{base}/new-order"),
                            "renewalInfo": format!("{base}/renewal-info"),
                        }))
                    }
                }),
            )
            .route("/nonce", head(move || async move { nonce }))
            .route(
                "/new-account",
                post({
                    let base = base.clone();
                    move |body: Bytes| async move {
                        let payload = decode_payload(&body)?;
                        assert_eq!(payload["termsOfServiceAgreed"], true);
                        Ok::<_, ApiError>((
                            StatusCode::CREATED,
                            nonce,
                            [("Location", format!("{base}/accounts/1"))],
                            Json(json!({"status": "valid"})),
                        ))
                    }
                }),
            )
            .route(
                "/new-order",
                post({
                    let order_state = order_state.clone();
                    let order_url = order_url.clone();
                    move |body: Bytes| async move {
                        let payload = decode_payload(&body)?;
                        assert_eq!(
                            payload["identifiers"],
                            json!([{"type": "dns", "value": "api.home.example"}])
                        );
                        Ok::<_, ApiError>((
                            StatusCode::CREATED,
                            nonce,
                            [("Location", order_url)],
                            Json(order_state()),
                        ))
                    }
                }),
            )
            .route(
                "/orders/1",
                post({
                    let order_state = order_state.clone();
                    move || async move { (nonce, Json(order_state())) }
                }),
            )
            .route(
                "/authorization/1",
                post({
                    let phase = phase.clone();
                    let challenge_url = challenge_url.clone();
                    move || async move {
                        let status = if phase.load(Ordering::SeqCst) == 0 {
                            "pending"
                        } else {
                            "valid"
                        };
                        (
                            nonce,
                            Json(json!({
                                "status": status,
                                "identifier": {"type": "dns", "value": "api.home.example"},
                                "challenges": [{
                                    "type": "dns-01", "url": challenge_url, "status": status,
                                    "token": "Zml4dHVyZS10b2tlbg"
                                }]
                            })),
                        )
                    }
                }),
            )
            .route(
                "/challenge/1",
                post({
                    let phase = phase.clone();
                    move |body: Bytes| async move {
                        assert_eq!(decode_payload(&body)?, json!({}));
                        phase.store(1, Ordering::SeqCst);
                        Ok::<_, ApiError>((
                            nonce,
                            Json(json!({
                                "type": "dns-01", "url": challenge_url, "status": "processing",
                                "token": "Zml4dHVyZS10b2tlbg"
                            })),
                        ))
                    }
                }),
            )
            .route(
                "/finalize/1",
                post({
                    let phase = phase.clone();
                    let csr = csr.clone();
                    move |body: Bytes| async move {
                        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
                        let payload = decode_payload(&body)?;
                        assert_eq!(payload["csr"], URL_SAFE_NO_PAD.encode(csr));
                        phase.store(2, Ordering::SeqCst);
                        Ok::<_, ApiError>((nonce, Json(order_state())))
                    }
                }),
            )
            .route(
                "/certificate/1",
                post({
                    let certificate = certificate.clone();
                    move || async move { (nonce, certificate) }
                }),
            )
            .route(
                "/renewal-info/{identifier}",
                get(|| async {
                    (
                        [("Retry-After", "60")],
                        Json(json!({
                            "suggestedWindow": {
                                "start": "1970-01-01T00:16:40Z", "end": "1970-01-01T00:33:20Z"
                            }
                        })),
                    )
                }),
            )
            .layer(axum::extract::DefaultBodyLimit::max(32768));
        let handle = axum_server::Handle::new();
        let serving = tokio::spawn(crate::infrastructure::server::serve_listener(
            listener,
            tls,
            router,
            handle.clone(),
        ));
        assert!(handle.listening().await.is_some());
        let trusted_certificate = reqwest::Certificate::from_pem(anchor.as_bytes())?;
        let http = || {
            AcmeHttp::from_builder(
                environment,
                reqwest::Client::builder()
                    .tls_certs_only([trusted_certificate.clone()])
                    .resolve(authority, address),
            )
        };
        let saved_key = account_key()?;
        let (account, credentials) = Acme::register(
            &names,
            environment,
            http()?,
            &saved_key,
            AccountOperation::Create {
                terms_of_service_agreed: true,
            },
        )
        .await?;
        let order_url = account.new_order().await?.url().to_owned();
        drop(account);
        let account = Acme::restore(&names, environment, http()?, credentials).await?;
        let mut order = account.restore_order(&order_url).await?;
        let value = account
            .challenge_value(&mut order)
            .await?
            .ok_or(Error::Internal("fixture DNS-01 value"))?;
        let (recovered, _) = Acme::register(
            &names,
            environment,
            http()?,
            &saved_key,
            AccountOperation::LookupExisting,
        )
        .await?;
        let mut recovered_order = recovered.restore_order(&order_url).await?;
        assert_eq!(
            recovered.challenge_value(&mut recovered_order).await?,
            Some(value.clone())
        );
        account.notify_ready(&mut order, &value).await?;
        assert_eq!(account.refresh(&mut order).await?, OrderStatus::Ready);
        account.finalize(&mut order, &csr).await?;
        assert_eq!(
            account.certificate(&mut order).await?,
            Some(certificate.clone())
        );
        crate::infrastructure::tls::configuration(
            &names.hostname,
            certificate.as_bytes(),
            private_key.as_bytes(),
        )?;
        assert_eq!(
            account.renewal_information(certificate.as_bytes()).await?,
            RenewalInformation::Suggested {
                start: 1000,
                end: 2000,
                refresh_after_seconds: 60
            }
        );
        handle.shutdown();
        serving.await??;
        Ok(())
    }

    #[test]
    fn certificates_acme_keys_and_csr_survive_serialization() -> Result<()> {
        let saved_key = account_key()?;
        let der = STANDARD
            .decode(&saved_key)
            .map_err(|_| Error::Internal("test account key encoding"))?;
        Key::from_pkcs8_der(PrivatePkcs8KeyDer::from(der)).map_err(protocol_error)?;
        let names = CertificateNames::new("api.home.example", "home.example")?;
        let (key, csr) = certificate_request(&names)?;
        assert!(rustls_pemfile::private_key(&mut std::io::Cursor::new(key))?.is_some());
        let (_, parsed) =
            x509_parser::certification_request::X509CertificationRequest::from_der(&csr)
                .map_err(|_| Error::Internal("test CSR parsing"))?;
        parsed
            .verify_signature()
            .map_err(|_| Error::Internal("test CSR signature"))?;
        let names = parsed
            .requested_extensions()
            .ok_or(Error::Internal("test CSR extensions"))?
            .filter_map(|extension| match extension {
                x509_parser::extensions::ParsedExtension::SubjectAlternativeName(names) => {
                    Some(names)
                }
                _ => None,
            })
            .flat_map(|names| names.general_names.iter())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![&x509_parser::extensions::GeneralName::DNSName(
                "api.home.example"
            )]
        );
        Ok(())
    }
}
