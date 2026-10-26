use domus::Error;
use domus::infrastructure::identity::EntraTokenVerifier;
use openidconnect::core::{
    CoreGenderClaim, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
    CoreRsaPrivateSigningKey,
};
use openidconnect::{
    AdditionalClaims, Audience, ClientId, IdToken, IdTokenClaims, IssuerUrl, JsonWebKeyId, Nonce,
    PrivateSigningKey, StandardClaims, SubjectIdentifier,
};
use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};

const TENANT: &str = "12345678-1234-4321-9876-123456789abc";
const OBJECT: &str = "87654321-4321-4321-9876-123456789abc";
const CLIENT: &str = "abcdef12-1234-4321-9876-123456789abc";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Claims {
    #[serde(skip_serializing_if = "Option::is_none")]
    tid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    oid: Option<String>,
    nbf: Option<i64>,
    ver: Option<String>,
}
impl AdditionalClaims for Claims {}

type Token =
    IdToken<Claims, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm>;
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn signing_key() -> Result<CoreRsaPrivateSigningKey, Box<dyn std::error::Error>> {
    let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048)?;
    let pem = key.to_pkcs1_pem(LineEnding::LF)?;
    Ok(CoreRsaPrivateSigningKey::from_pem(
        &pem,
        Some(JsonWebKeyId::new("generated-test-key".into())),
    )?)
}

#[test]
fn only_signed_current_single_user_tokens_with_the_saved_nonce_are_accepted() -> TestResult {
    let key = signing_key()?;
    let keys = CoreJsonWebKeySet::new(vec![key.as_verification_key()]);
    let verifier = EntraTokenVerifier::new(TENANT, OBJECT, CLIENT, keys.clone())?;
    let now = SystemTime::now();
    let claims = IdTokenClaims::new(
        IssuerUrl::new(format!("https://login.microsoftonline.com/{TENANT}/v2.0"))?,
        vec![Audience::new(CLIENT.into())],
        (now + Duration::from_secs(600)).into(),
        now.into(),
        StandardClaims::new(SubjectIdentifier::new("generated-subject".into())),
        Claims {
            tid: Some(TENANT.into()),
            oid: Some(OBJECT.into()),
            nbf: None,
            ver: Some("2.0".into()),
        },
    )
    .set_nonce(Some(Nonce::new("saved-one-time-nonce".into())));
    let sign = |claims| -> Result<String, Box<dyn std::error::Error>> {
        Ok(Token::new(
            claims,
            &key,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            None,
            None,
        )?
        .to_string())
    };
    let valid = sign(claims.clone())?;
    let expected_nonce = Nonce::new("saved-one-time-nonce".into());
    let accepted = verifier.verify(&valid, &expected_nonce)?;
    assert_eq!(accepted.tenant_id(), TENANT);
    assert_eq!(accepted.object_id(), OBJECT);
    assert!(accepted.expires_at() > 0);
    assert!(
        verifier
            .verify(&valid, &Nonce::new("different-nonce".into()))
            .is_err()
    );
    let missing_nonce = sign(claims.clone().set_nonce(None))?;
    assert!(verifier.verify(&missing_nonce, &expected_nonce).is_err());
    let wrong_issuer = sign(claims.clone().set_issuer(IssuerUrl::new(
        "https://login.microsoftonline.com/other/v2.0".into(),
    )?))?;
    assert!(verifier.verify(&wrong_issuer, &expected_nonce).is_err());
    let wrong_audience = sign(
        claims
            .clone()
            .set_audiences(vec![Audience::new("other-client".into())]),
    )?;
    assert!(verifier.verify(&wrong_audience, &expected_nonce).is_err());
    let multiple_audiences = sign(claims.clone().set_audiences(vec![
        Audience::new(CLIENT.into()),
        Audience::new("untrusted-client".into()),
    ]))?;
    assert!(
        verifier
            .verify(&multiple_audiences, &expected_nonce)
            .is_err()
    );
    let wrong_party = sign(
        claims
            .clone()
            .set_authorized_party(Some(ClientId::new("other".into()))),
    )?;
    assert!(verifier.verify(&wrong_party, &expected_nonce).is_err());
    let expired = sign(
        claims
            .clone()
            .set_expiration((now - Duration::from_secs(10)).into()),
    )?;
    assert!(verifier.verify(&expired, &expected_nonce).is_err());
    let future = sign(
        claims
            .clone()
            .set_issue_time((now + Duration::from_secs(300)).into()),
    )?;
    assert!(verifier.verify(&future, &expected_nonce).is_err());
    let mut wrong_identity = claims.clone();
    wrong_identity.additional_claims_mut().oid = Some(CLIENT.into());
    assert!(
        verifier
            .verify(&sign(wrong_identity)?, &expected_nonce)
            .is_err()
    );
    let mut wrong_tenant = claims.clone();
    wrong_tenant.additional_claims_mut().tid = Some(CLIENT.into());
    assert!(
        verifier
            .verify(&sign(wrong_tenant)?, &expected_nonce)
            .is_err()
    );
    let mut future_activation = claims.clone();
    future_activation.additional_claims_mut().nbf =
        Some(i64::try_from(now.duration_since(std::time::UNIX_EPOCH)?.as_secs())? + 300);
    assert!(
        verifier
            .verify(&sign(future_activation)?, &expected_nonce)
            .is_err()
    );
    let mut wrong_version = claims.clone();
    wrong_version.additional_claims_mut().ver = Some("1.0".into());
    assert!(
        verifier
            .verify(&sign(wrong_version)?, &expected_nonce)
            .is_err()
    );
    let mut missing_object = claims.clone();
    missing_object.additional_claims_mut().oid = None;
    assert!(
        verifier
            .verify(&sign(missing_object)?, &expected_nonce)
            .is_err()
    );
    let mut missing_tenant = claims.clone();
    missing_tenant.additional_claims_mut().tid = None;
    assert!(
        verifier
            .verify(&sign(missing_tenant)?, &expected_nonce)
            .is_err()
    );
    let no_subject = sign(
        claims
            .clone()
            .set_subject(SubjectIdentifier::new(String::new())),
    )?;
    assert!(verifier.verify(&no_subject, &expected_nonce).is_err());
    let other_algorithm = Token::new(
        claims.clone(),
        &key,
        CoreJwsSigningAlgorithm::RsaSsaPssSha256,
        None,
        None,
    )?
    .to_string();
    assert!(verifier.verify(&other_algorithm, &expected_nonce).is_err());
    let foreign = signing_key()?;
    let forged = Token::new(
        claims,
        &foreign,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        None,
        None,
    )?
    .to_string();
    assert!(verifier.verify(&forged, &expected_nonce).is_err());
    for invalid in ["", "secret-fixture-invalid-token", &"x".repeat(65537)] {
        let error = verifier
            .verify(invalid, &expected_nonce)
            .err()
            .ok_or("invalid token was accepted")?;
        assert!(matches!(error, Error::Authentication));
        assert_eq!(error.to_string(), "authentication failed");
    }
    assert!(EntraTokenVerifier::new("common", OBJECT, CLIENT, keys).is_err());
    assert!(
        EntraTokenVerifier::new(TENANT, OBJECT, CLIENT, CoreJsonWebKeySet::new(vec![])).is_err()
    );
    Ok(())
}

#[test]
fn sessions_store_hashes_survive_restart_and_require_the_configured_identity() -> TestResult {
    use domus::infrastructure::store::Store;
    let key = signing_key()?;
    let keys = CoreJsonWebKeySet::new(vec![key.as_verification_key()]);
    let verifier = EntraTokenVerifier::new(TENANT, OBJECT, CLIENT, keys.clone())?;
    let now = SystemTime::now();
    let nonce = Nonce::new_random();
    let claims = IdTokenClaims::new(
        IssuerUrl::new(format!("https://login.microsoftonline.com/{TENANT}/v2.0"))?,
        vec![Audience::new(CLIENT.into())],
        (now + Duration::from_secs(600)).into(),
        now.into(),
        StandardClaims::new(SubjectIdentifier::new("session-fixture".into())),
        Claims {
            tid: Some(TENANT.into()),
            oid: Some(OBJECT.into()),
            nbf: None,
            ver: Some("2.0".into()),
        },
    )
    .set_nonce(Some(nonce.clone()));
    let encoded = Token::new(
        claims,
        &key,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        None,
        None,
    )?
    .to_string();
    let identity = verifier.verify(&encoded, &nonce)?;
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    assert!(store.create_session(&identity, 0).is_err());
    assert!(store.create_session(&identity, 28801).is_err());
    let session = store.create_session(&identity, 3600)?;
    assert!(session.expires_at <= identity.expires_at());
    assert!(!format!("{:?}", session).contains(session.credential.secret()));
    assert!(
        store
            .session(session.credential.secret(), &verifier)?
            .is_some()
    );
    drop(store);
    let mut store = Store::open_existing(&database)?;
    let saved = store
        .session(session.credential.secret(), &verifier)?
        .ok_or("missing session")?;
    assert_eq!(saved.tenant_id, TENANT);
    assert_eq!(saved.object_id, OBJECT);
    assert_eq!(saved.client_id, CLIENT);
    let changed_client = EntraTokenVerifier::new(TENANT, OBJECT, OBJECT, keys.clone())?;
    let changed_user = EntraTokenVerifier::new(TENANT, CLIENT, CLIENT, keys)?;
    assert!(
        store
            .session(session.credential.secret(), &changed_client)
            .is_err()
    );
    assert!(
        store
            .session(session.credential.secret(), &changed_user)
            .is_err()
    );
    assert!(store.session("", &verifier).is_err());
    assert!(store.session(&"x".repeat(129), &verifier).is_err());
    assert!(store.session("not-a-real-session", &verifier)?.is_none());
    let raw = rusqlite::Connection::open(&database)?;
    let (length, kind): (i64, String) = raw.query_row(
        "SELECT length(token_hash), typeof(token_hash) FROM sessions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(length, 32);
    assert_eq!(kind, "blob");
    assert!(store.revoke_session(session.credential.secret())?);
    assert!(
        store
            .session(session.credential.secret(), &verifier)?
            .is_none()
    );
    assert!(!store.revoke_session(session.credential.secret())?);
    let expiring = store.create_session(&identity, 600)?;
    raw.execute("UPDATE sessions SET expires_at = 0", [])?;
    assert!(
        store
            .session(expiring.credential.secret(), &verifier)?
            .is_none()
    );
    let mut previous = None;
    for _ in 0..128 {
        previous = Some(store.create_session(&identity, 600)?.credential);
    }
    let previous = previous.ok_or("missing session credential")?;
    assert!(store.create_session(&identity, 600).is_err());
    raw.execute_batch(
        "CREATE TRIGGER fail_test_rotation BEFORE INSERT ON sessions
        BEGIN SELECT RAISE(ABORT, 'isolated rotation failure'); END;",
    )?;
    assert!(
        store
            .rotate_session(&identity, 600, Some(previous.secret()))
            .is_err()
    );
    assert!(store.session(previous.secret(), &verifier)?.is_some());
    raw.execute_batch("DROP TRIGGER fail_test_rotation")?;
    let rotated = store.rotate_session(&identity, 600, Some(previous.secret()))?;
    assert!(store.session(previous.secret(), &verifier)?.is_none());
    assert!(
        store
            .session(rotated.credential.secret(), &verifier)?
            .is_some()
    );
    assert_eq!(
        raw.query_row("SELECT COUNT(*) FROM sessions", [], |row| row
            .get::<_, i64>(0))?,
        128
    );
    assert_eq!(
        raw.query_row(
            "SELECT COUNT(*) FROM session_events WHERE event_type = 'session_revoked'",
            [],
            |row| row.get::<_, i64>(0),
        )?,
        2
    );
    Ok(())
}
