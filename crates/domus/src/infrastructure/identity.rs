use crate::domain::identifiers::is_canonical_uuid;
use crate::{Error, Result};
use openidconnect::core::{
    CoreGenderClaim, CoreIdTokenVerifier, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
};
use openidconnect::{
    AdditionalClaims, ClientId, CsrfToken, IdToken, IssuerUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LOGIN_LIFETIME: Duration = Duration::from_secs(300);
const MAX_PENDING_LOGINS: usize = 128;

struct PendingChallenge {
    browser_hash: [u8; 32],
    nonce: Nonce,
    verifier: PkceCodeVerifier,
    expires: Instant,
}

pub struct LoginStart {
    pub state: CsrfToken,
    pub browser_cookie: CsrfToken,
    pub nonce: Nonce,
    pub pkce_challenge: PkceCodeChallenge,
}

pub struct PendingLogin {
    nonce: Nonce,
    verifier: PkceCodeVerifier,
}

impl PendingLogin {
    pub fn into_parts(self) -> (Nonce, PkceCodeVerifier) {
        (self.nonce, self.verifier)
    }
}

/// Pending OAuth secrets stay in bounded process memory, not SQLite. A service
/// restart requires a fresh sign-in. Borrow mutably under the API's short lock;
/// consume the challenge before awaiting any token exchange.
#[derive(Default)]
pub struct LoginChallenges {
    pending: HashMap<[u8; 32], PendingChallenge>,
}

impl LoginChallenges {
    pub fn begin(&mut self) -> Result<LoginStart> {
        self.begin_at(Instant::now())
    }

    fn begin_at(&mut self, now: Instant) -> Result<LoginStart> {
        self.pending.retain(|_, value| value.expires > now);
        if self.pending.len() >= MAX_PENDING_LOGINS {
            return Err(Error::Conflict(
                "too many pending sign-ins; try again later".into(),
            ));
        }
        let state = CsrfToken::new_random();
        let browser_cookie = CsrfToken::new_random();
        let nonce = Nonce::new_random();
        let (pkce_challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        self.pending.insert(
            credential_hash(state.secret()),
            PendingChallenge {
                browser_hash: credential_hash(browser_cookie.secret()),
                nonce: nonce.clone(),
                verifier,
                expires: now + LOGIN_LIFETIME,
            },
        );
        Ok(LoginStart {
            state,
            browser_cookie,
            nonce,
            pkce_challenge,
        })
    }

    pub fn consume(&mut self, state: &str, browser_cookie: &str) -> Result<PendingLogin> {
        self.consume_at(state, browser_cookie, Instant::now())
    }

    fn consume_at(
        &mut self,
        state: &str,
        browser_cookie: &str,
        now: Instant,
    ) -> Result<PendingLogin> {
        if state.is_empty()
            || state.len() > 128
            || browser_cookie.is_empty()
            || browser_cookie.len() > 128
        {
            return Err(Error::Authentication);
        }
        self.pending.retain(|_, value| value.expires > now);
        let hash = credential_hash(state);
        let pending = self.pending.get(&hash).ok_or(Error::Authentication)?;
        if pending.browser_hash != credential_hash(browser_cookie) {
            return Err(Error::Authentication);
        }
        let pending = self.pending.remove(&hash).ok_or(Error::Authentication)?;
        Ok(PendingLogin {
            nonce: pending.nonce,
            verifier: pending.verifier,
        })
    }
}

pub(crate) fn credential_hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EntraClaims {
    tid: String,
    oid: String,
    nbf: Option<i64>,
    ver: Option<String>,
}

impl AdditionalClaims for EntraClaims {}

type EntraIdToken = IdToken<
    EntraClaims,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

/// Only a verified token can create this result. It contains no bearer token.
#[derive(Debug)]
pub struct VerifiedIdentity {
    tenant_id: String,
    object_id: String,
    client_id: String,
    expires_at: i64,
}

impl VerifiedIdentity {
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn object_id(&self) -> &str {
        &self.object_id
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }
}

pub struct EntraTokenVerifier {
    verifier: CoreIdTokenVerifier<'static>,
    tenant_id: String,
    object_id: String,
    client_id: String,
}

impl EntraTokenVerifier {
    pub fn permits_identity(&self, tenant: &str, object: &str, client: &str) -> bool {
        tenant == self.tenant_id && object == self.object_id && client == self.client_id
    }

    /// Keys must come from validated tenant discovery, never a browser request.
    /// Public-key verification does not require the application's client secret.
    pub fn new(
        tenant_id: &str,
        object_id: &str,
        client_id: &str,
        keys: CoreJsonWebKeySet,
    ) -> Result<Self> {
        if ![tenant_id, object_id, client_id]
            .into_iter()
            .all(is_canonical_uuid)
        {
            return Err(Error::InvalidInput(
                "Entra tenant, object, and client IDs must be canonical lowercase UUIDs".into(),
            ));
        }
        if keys.keys().is_empty() || keys.keys().len() > 64 {
            return Err(Error::InvalidInput(
                "provider keys must contain 1 through 64 entries".into(),
            ));
        }
        let issuer = IssuerUrl::new(format!(
            "https://login.microsoftonline.com/{tenant_id}/v2.0"
        ))
        .map_err(|_| Error::InvalidInput("configured Entra issuer is invalid".into()))?;
        let verifier =
            CoreIdTokenVerifier::new_public_client(ClientId::new(client_id.into()), issuer, keys)
                .set_allowed_algs(vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256]);
        Ok(Self {
            verifier,
            tenant_id: tenant_id.into(),
            object_id: object_id.into(),
            client_id: client_id.into(),
        })
    }

    /// The login operation must supply its saved, single-use nonce. There is no
    /// verification-disabled path. All rejected tokens use one redacted error.
    pub fn verify(&self, encoded: &str, expected_nonce: &Nonce) -> Result<VerifiedIdentity> {
        if encoded.is_empty() || encoded.len() > 65536 {
            return Err(Error::Authentication);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::InvalidInput("system clock precedes the Unix epoch".into()))?;
        let now_seconds = i64::try_from(now.as_secs())
            .map_err(|_| Error::InvalidInput("system clock exceeds the supported range".into()))?;
        let latest_issue = now_seconds.checked_add(60).ok_or_else(|| {
            Error::InvalidInput("system clock exceeds the supported range".into())
        })?;
        let verifier = self
            .verifier
            .clone()
            .set_issue_time_verifier_fn(move |issued| {
                if issued.timestamp() < 0 || issued.timestamp() > latest_issue {
                    Err("ID token issue time is invalid".into())
                } else {
                    Ok(())
                }
            });
        let token: EntraIdToken = encoded.parse().map_err(|_| Error::Authentication)?;
        let claims = token
            .claims(&verifier, expected_nonce)
            .map_err(|_| Error::Authentication)?;
        if claims.additional_claims().tid != self.tenant_id
            || claims.additional_claims().oid != self.object_id
            || claims.subject().as_str().is_empty()
            || claims.issue_time() > claims.expiration()
            || claims
                .additional_claims()
                .nbf
                .is_some_and(|not_before| not_before < 0 || not_before > now_seconds)
            || claims
                .additional_claims()
                .ver
                .as_deref()
                .is_some_and(|version| version != "2.0")
            || claims
                .authorized_party()
                .is_some_and(|party| party.as_str() != self.client_id)
        {
            return Err(Error::Authentication);
        }
        Ok(VerifiedIdentity {
            tenant_id: self.tenant_id.clone(),
            object_id: self.object_id.clone(),
            client_id: self.client_id.clone(),
            expires_at: claims.expiration().timestamp(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_challenges_are_bound_to_one_browser_and_consumed_once() -> Result<()> {
        let mut challenges = LoginChallenges::default();
        let login = challenges.begin()?;
        let other = challenges.begin()?;
        assert!(
            challenges
                .consume(login.state.secret(), other.browser_cookie.secret())
                .is_err()
        );
        assert!(
            challenges
                .consume(other.state.secret(), login.browser_cookie.secret())
                .is_err()
        );
        let pending = challenges.consume(login.state.secret(), login.browser_cookie.secret())?;
        let (nonce, verifier) = pending.into_parts();
        assert_eq!(
            credential_hash(nonce.secret()),
            credential_hash(login.nonce.secret())
        );
        let expected = PkceCodeChallenge::from_code_verifier_sha256(&verifier);
        assert_eq!(expected.as_str(), login.pkce_challenge.as_str());
        assert!(
            challenges
                .consume(login.state.secret(), login.browser_cookie.secret())
                .is_err()
        );
        assert!(
            challenges
                .consume(other.state.secret(), other.browser_cookie.secret())
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn expired_challenges_cannot_be_reused() -> Result<()> {
        let mut challenges = LoginChallenges::default();
        let now = Instant::now();
        let login = challenges.begin_at(now)?;
        assert!(
            challenges
                .consume_at(
                    login.state.secret(),
                    login.browser_cookie.secret(),
                    now + LOGIN_LIFETIME,
                )
                .is_err()
        );
        assert!(challenges.pending.is_empty());
        Ok(())
    }

    #[test]
    fn challenge_capacity_is_bounded_and_expired_slots_are_reclaimed() -> Result<()> {
        let mut challenges = LoginChallenges::default();
        let now = Instant::now();
        for _ in 0..MAX_PENDING_LOGINS {
            challenges.begin_at(now)?;
        }
        assert!(challenges.begin_at(now).is_err());
        challenges.begin_at(now + LOGIN_LIFETIME)?;
        assert_eq!(challenges.pending.len(), 1);
        assert!(challenges.consume(&"x".repeat(129), "cookie").is_err());
        Ok(())
    }
}
