//! Shared storage mechanics, not provider freshness or acquisition authority.
use crate::{Error, Result};
use sha2::{Digest, Sha256};

/// Provider-scoped logical identity. Native paths and URLs are not storage paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    digest: [u8; 32],
}

impl CacheKey {
    pub fn new(provider: &str, namespace: &str, identifier: &str) -> Result<Self> {
        super::provider_name(provider)?;
        super::provider_name(namespace)?;
        if identifier.is_empty()
            || identifier.len() > 4096
            || identifier.chars().any(char::is_control)
        {
            return Err(Error::InvalidInput(
                "cache identifier must contain 1 through 4096 bytes without controls".into(),
            ));
        }
        let mut hash = Sha256::new();
        for part in [provider, namespace, identifier] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
        Ok(Self {
            digest: hash.finalize().into(),
        })
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

#[derive(Debug, Clone)]
pub struct CachedContent {
    pub bytes: Vec<u8>,
    pub sha256: [u8; 32],
    pub stored_at: u64,
    pub expires_at: u64,
}

#[derive(Debug)]
pub enum CacheLookup {
    Miss,
    Expired,
    Corrupt,
    Hit(CachedContent),
}

/// Blocking I/O port. Async hosts must call it outside executor threads.
/// Providers still validate provenance, content and their own freshness rules.
pub trait Cache: Send + Sync {
    fn lookup(&self, key: &CacheKey, now: u64) -> Result<CacheLookup>;
    fn put(&self, key: &CacheKey, bytes: &[u8], now: u64, expires_at: u64) -> Result<[u8; 32]>;
    fn invalidate(&self, key: &CacheKey) -> Result<bool>;
    /// Inspect at most the configured entry bound and remove at most `limit`.
    fn evict_expired(&self, now: u64, limit: u16) -> Result<u16>;
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    #[derive(Default)]
    pub struct MemoryCache {
        pub entries: Mutex<HashMap<[u8; 32], CachedContent>>,
        pub puts: AtomicUsize,
        pub fail_put: AtomicBool,
    }

    impl Cache for MemoryCache {
        fn lookup(&self, key: &CacheKey, now: u64) -> Result<CacheLookup> {
            let entries = self
                .entries
                .lock()
                .map_err(|_| Error::Internal("test cache lock"))?;
            Ok(match entries.get(key.digest()) {
                None => CacheLookup::Miss,
                Some(hit) if now < hit.stored_at || now >= hit.expires_at => CacheLookup::Expired,
                Some(hit) if <[u8; 32]>::from(Sha256::digest(&hit.bytes)) != hit.sha256 => {
                    CacheLookup::Corrupt
                }
                Some(hit) => CacheLookup::Hit(hit.clone()),
            })
        }

        fn put(&self, key: &CacheKey, bytes: &[u8], now: u64, expires_at: u64) -> Result<[u8; 32]> {
            self.puts.fetch_add(1, Ordering::SeqCst);
            if self.fail_put.load(Ordering::SeqCst) {
                return Err(Error::Conflict("injected cache write failure".into()));
            }
            let sha256 = Sha256::digest(bytes).into();
            self.entries
                .lock()
                .map_err(|_| Error::Internal("test cache lock"))?
                .insert(
                    *key.digest(),
                    CachedContent {
                        bytes: bytes.into(),
                        sha256,
                        stored_at: now,
                        expires_at,
                    },
                );
            Ok(sha256)
        }

        fn invalidate(&self, key: &CacheKey) -> Result<bool> {
            Ok(self
                .entries
                .lock()
                .map_err(|_| Error::Internal("test cache lock"))?
                .remove(key.digest())
                .is_some())
        }

        fn evict_expired(&self, now: u64, limit: u16) -> Result<u16> {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| Error::Internal("test cache lock"))?;
            let keys: Vec<_> = entries
                .iter()
                .filter(|(_, entry)| entry.expires_at <= now)
                .take(usize::from(limit))
                .map(|(key, _)| *key)
                .collect();
            for key in &keys {
                entries.remove(key);
            }
            Ok(keys.len() as u16)
        }
    }
}
