use super::http::EntraHttpClient;
use super::identity::{EntraTokenVerifier, LoginStart, PendingLogin, VerifiedIdentity};
use crate::domain::identifiers::is_canonical_uuid;
use crate::{Error, Result};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthType, AuthorizationCode, ClientId, ClientSecret, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, RedirectUrl, TokenResponse, TokenUrl,
};
use std::time::{Duration, Instant};

type DiscoveredClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
    EndpointMaybeSet,
>;

#[derive(Clone)]
struct Configuration {
    tenant: String,
    object: String,
    client: String,
    secret: ClientSecret,
    redirect: RedirectUrl,
}

/// The fixed Entra authority is not supplied by a request or a discovery result.
/// This adapter uses the SDK for discovery, PKCE and token exchange, then the
/// application's signed-claim policy for the single permitted identity.
pub struct EntraBroker {
    configuration: Configuration,
    http: EntraHttpClient,
    client: DiscoveredClient,
    verifier: EntraTokenVerifier,
    loaded_at: Instant,
}

impl EntraBroker {
    pub async fn discover(
        tenant: &str,
        object: &str,
        client: &str,
        secret: ClientSecret,
        redirect: RedirectUrl,
        http: EntraHttpClient,
    ) -> Result<Self> {
        if ![tenant, object, client].into_iter().all(is_canonical_uuid) {
            return Err(Error::InvalidInput(
                "Entra IDs must be canonical lowercase UUIDs".into(),
            ));
        }
        if secret.secret().trim().is_empty()
            || secret.secret().len() > 4096
            || secret.secret().chars().any(char::is_control)
        {
            return Err(Error::InvalidInput(
                "Entra client secret must contain 1 through 4096 bytes".into(),
            ));
        }
        let callback = redirect.url();
        if callback.scheme() != "https"
            || callback.host_str().is_none()
            || !callback.username().is_empty()
            || callback.password().is_some()
            || callback.path() != "/auth/callback"
            || callback.query().is_some()
            || callback.fragment().is_some()
        {
            return Err(Error::InvalidInput(
                "Entra callback must be an HTTPS /auth/callback URL".into(),
            ));
        }
        Self::load(
            Configuration {
                tenant: tenant.into(),
                object: object.into(),
                client: client.into(),
                secret,
                redirect,
            },
            http,
        )
        .await
    }

    async fn load(configuration: Configuration, http: EntraHttpClient) -> Result<Self> {
        let authority = format!("https://login.microsoftonline.com/{}", configuration.tenant);
        let issuer = IssuerUrl::new(format!("{authority}/v2.0"))
            .map_err(|_| Error::InvalidInput("Entra issuer is invalid".into()))?;
        let metadata =
            CoreProviderMetadata::discover_async(issuer, &|request| http.clone().execute(request))
                .await
                .map_err(|_| Error::Remote("trusted Entra discovery failed"))?;
        let authorize = format!("{authority}/oauth2/v2.0/authorize");
        let token = format!("{authority}/oauth2/v2.0/token");
        if metadata.authorization_endpoint().as_str() != authorize
            || metadata.token_endpoint().map(|url| url.as_str()) != Some(token.as_str())
        {
            return Err(Error::Remote(
                "Entra discovery returned unexpected authorization endpoints",
            ));
        }
        let verifier = EntraTokenVerifier::new(
            &configuration.tenant,
            &configuration.object,
            &configuration.client,
            metadata.jwks().clone(),
        )?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(configuration.client.clone()),
            Some(configuration.secret.clone()),
        )
        .set_token_uri(
            TokenUrl::new(token).map_err(|_| Error::Remote("Entra token endpoint is invalid"))?,
        )
        .set_redirect_uri(configuration.redirect.clone())
        .set_auth_type(AuthType::RequestBody);
        Ok(Self {
            configuration,
            http,
            client,
            verifier,
            loaded_at: Instant::now(),
        })
    }

    pub async fn refresh(&self) -> Result<Self> {
        Self::load(self.configuration.clone(), self.http.clone()).await
    }

    pub fn verifier(&self) -> &EntraTokenVerifier {
        &self.verifier
    }

    pub fn callback(&self) -> &RedirectUrl {
        &self.configuration.redirect
    }

    pub fn authorization_url(&self, login: &LoginStart) -> Result<reqwest::Url> {
        check_metadata_time(self.loaded_at, Instant::now())?;
        let state = login.state.clone();
        let nonce = login.nonce.clone();
        Ok(self
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                move || state,
                move || nonce,
            )
            .set_pkce_challenge(login.pkce_challenge.clone())
            .add_extra_param("response_mode", "query")
            .url()
            .0)
    }

    pub async fn exchange(&self, code: String, pending: PendingLogin) -> Result<VerifiedIdentity> {
        if code.is_empty() || code.len() > 4096 {
            return Err(Error::Authentication);
        }
        check_metadata_time(self.loaded_at, Instant::now())?;
        let (nonce, verifier) = pending.into_parts();
        let response = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(verifier)
            .request_async(&|request| self.http.clone().execute(request))
            .await
            .map_err(|error| match error {
                openidconnect::RequestTokenError::Request(error) => error,
                openidconnect::RequestTokenError::ServerResponse(error)
                    if matches!(
                        error.error().to_string().as_str(),
                        "invalid_client"
                            | "unauthorized_client"
                            | "invalid_scope"
                            | "unsupported_grant_type"
                    ) =>
                {
                    Error::Remote("Entra rejected the token exchange configuration")
                }
                openidconnect::RequestTokenError::ServerResponse(_) => Error::Authentication,
                openidconnect::RequestTokenError::Parse(_, _)
                | openidconnect::RequestTokenError::Other(_) => {
                    Error::Remote("Entra token response is invalid")
                }
            })?;
        let token = response.id_token().ok_or(Error::Authentication)?;
        check_metadata_time(self.loaded_at, Instant::now())?;
        self.verifier.verify(&token.to_string(), &nonce)
    }
}

fn check_metadata_time(loaded: Instant, now: Instant) -> Result<()> {
    if now.saturating_duration_since(loaded) >= Duration::from_secs(3600) {
        return Err(Error::Remote(
            "Entra signing metadata expired; sign-in is unavailable until refresh succeeds",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_metadata_expires_at_one_hour() {
        let loaded = Instant::now();
        assert!(check_metadata_time(loaded, loaded + Duration::from_secs(3599)).is_ok());
        assert!(check_metadata_time(loaded, loaded + Duration::from_secs(3600)).is_err());
    }
}
