use super::Store;
use crate::infrastructure::identity::{EntraTokenVerifier, VerifiedIdentity, credential_hash};
use crate::{Error, Result};
use openidconnect::CsrfToken;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use std::fmt;

/// Return this only through a Secure HttpOnly same-origin session cookie.
/// It deliberately has no serialization or display implementation.
pub struct SessionCredential(CsrfToken);

impl SessionCredential {
    pub fn secret(&self) -> &str {
        self.0.secret()
    }
}

impl fmt::Debug for SessionCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionCredential([redacted])")
    }
}

#[derive(Debug)]
pub struct CreatedSession {
    pub credential: SessionCredential,
    pub expires_at: i64,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct SessionIdentity {
    pub tenant_id: String,
    pub object_id: String,
    pub client_id: String,
    pub expires_at: i64,
}

impl Store {
    /// A verified identity is required; raw JSON claims cannot create sessions.
    /// Only a hash reaches SQLite, and expiry never exceeds the signed token.
    pub fn create_session(
        &mut self,
        identity: &VerifiedIdentity,
        lifetime_seconds: u32,
    ) -> Result<CreatedSession> {
        self.rotate_session(identity, lifetime_seconds, None)
    }

    /// Replace the current browser credential atomically. A failed creation
    /// leaves the previous session intact, including at the capacity boundary.
    pub fn rotate_session(
        &mut self,
        identity: &VerifiedIdentity,
        lifetime_seconds: u32,
        previous: Option<&str>,
    ) -> Result<CreatedSession> {
        if let Some(previous) = previous {
            validate_credential(previous)?;
        }
        if !(1..=28800).contains(&lifetime_seconds) {
            return Err(Error::InvalidInput(
                "session lifetime must be 1 through 28800 seconds".into(),
            ));
        }
        let credential = SessionCredential(CsrfToken::new_random());
        let hash = credential_hash(credential.secret());
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now: i64 = transaction.query_row("SELECT unixepoch()", [], |row| row.get(0))?;
        let expires_at = now
            .checked_add(i64::from(lifetime_seconds))
            .ok_or_else(|| {
                Error::InvalidInput("session expiry exceeds the supported range".into())
            })?
            .min(identity.expires_at());
        if expires_at <= now {
            return Err(Error::Authentication);
        }
        transaction.execute("DELETE FROM sessions WHERE expires_at <= unixepoch()", [])?;
        if let Some(previous) = previous {
            revoke(&transaction, &credential_hash(previous))?;
        }
        let active: i64 =
            transaction.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        if active >= 128 {
            return Err(Error::Conflict(
                "active session limit reached; sign out another session".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO sessions(token_hash, tenant_id, object_id, client_id, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                hash.as_slice(),
                identity.tenant_id(),
                identity.object_id(),
                identity.client_id(),
                expires_at
            ],
        )?;
        transaction.execute(
            "INSERT INTO session_events(event_type, tenant_id, object_id, client_id)
             VALUES ('session_created', ?1, ?2, ?3)",
            params![
                identity.tenant_id(),
                identity.object_id(),
                identity.client_id()
            ],
        )?;
        transaction.commit()?;
        Ok(CreatedSession {
            credential,
            expires_at,
        })
    }

    pub fn session(
        &self,
        credential: &str,
        configured: &EntraTokenVerifier,
    ) -> Result<Option<SessionIdentity>> {
        validate_credential(credential)?;
        let hash = credential_hash(credential);
        let session = self
            .connection
            .query_row(
                "SELECT tenant_id, object_id, client_id, expires_at FROM sessions
             WHERE token_hash = ?1 AND expires_at > unixepoch()",
                [hash.as_slice()],
                |row| {
                    Ok(SessionIdentity {
                        tenant_id: row.get(0)?,
                        object_id: row.get(1)?,
                        client_id: row.get(2)?,
                        expires_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        if let Some(identity) = &session
            && !configured.permits_identity(
                &identity.tenant_id,
                &identity.object_id,
                &identity.client_id,
            )
        {
            return Err(Error::Authentication);
        }
        Ok(session)
    }

    pub fn revoke_session(&mut self, credential: &str) -> Result<bool> {
        validate_credential(credential)?;
        let hash = credential_hash(credential);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revoked = revoke(&transaction, &hash)?;
        transaction.commit()?;
        Ok(revoked)
    }
}

fn revoke(transaction: &Transaction<'_>, hash: &[u8; 32]) -> Result<bool> {
    let identity: Option<(String, String, String)> = transaction
        .query_row(
            "DELETE FROM sessions WHERE token_hash = ?1 RETURNING tenant_id, object_id, client_id",
            [hash.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((tenant, object, client)) = &identity {
        transaction.execute(
            "INSERT INTO session_events(event_type, tenant_id, object_id, client_id)
                 VALUES ('session_revoked', ?1, ?2, ?3)",
            params![tenant, object, client],
        )?;
    }
    Ok(identity.is_some())
}

fn validate_credential(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::Authentication);
    }
    Ok(())
}
