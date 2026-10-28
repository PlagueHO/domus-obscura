//! System-wide sharing policy. Native paths and protocol tokens are not authority.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ShareAccess {
    #[default]
    None,
    All,
    Allowlist,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ShareRoot {
    pub binding_id: i64,
    pub alias: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, deny_unknown_fields)]
pub struct ShareLimits {
    pub upload_slots: u32,
    pub queued_files: u32,
    pub queued_per_peer: u32,
    pub buffer_bytes: u32,
    pub roots: u32,
    pub catalog_files: u32,
    /// Estimated encoded catalog-entry bytes, not total SQLite disk usage.
    pub catalog_bytes: u32,
    pub expanded_bytes: u32,
    pub compressed_bytes: u32,
    pub spool_bytes: u32,
    pub peer_connections: u32,
    pub pending_handshakes: u32,
    pub requests_per_minute: u32,
    pub search_results: u32,
    pub concurrent_browses: u32,
    pub idle_seconds: u32,
    pub operation_seconds: u32,
}

impl Default for ShareLimits {
    fn default() -> Self {
        Self {
            upload_slots: 2,
            queued_files: 128,
            queued_per_peer: 16,
            buffer_bytes: 65_536,
            roots: 16,
            catalog_files: 1_000_000,
            catalog_bytes: 256 * 1024 * 1024,
            expanded_bytes: 256 * 1024 * 1024,
            compressed_bytes: 64 * 1024 * 1024,
            spool_bytes: 256 * 1024 * 1024,
            peer_connections: 16,
            pending_handshakes: 4,
            requests_per_minute: 60,
            search_results: 200,
            concurrent_browses: 2,
            idle_seconds: 10,
            operation_seconds: 1800,
        }
    }
}

impl ShareLimits {
    pub fn validate(&self) -> Result<()> {
        for (name, value, minimum, maximum) in [
            ("upload_slots", self.upload_slots, 1, 8),
            ("queued_files", self.queued_files, 1, 1024),
            ("queued_per_peer", self.queued_per_peer, 1, 128),
            ("buffer_bytes", self.buffer_bytes, 4096, 262_144),
            ("roots", self.roots, 1, 64),
            ("catalog_files", self.catalog_files, 1, 2_000_000),
            (
                "catalog_bytes",
                self.catalog_bytes,
                1024,
                1024 * 1024 * 1024,
            ),
            (
                "expanded_bytes",
                self.expanded_bytes,
                1024,
                512 * 1024 * 1024,
            ),
            (
                "compressed_bytes",
                self.compressed_bytes,
                1024,
                128 * 1024 * 1024,
            ),
            ("spool_bytes", self.spool_bytes, 1024, 1024 * 1024 * 1024),
            ("peer_connections", self.peer_connections, 1, 24),
            ("pending_handshakes", self.pending_handshakes, 1, 8),
            ("requests_per_minute", self.requests_per_minute, 1, 600),
            ("search_results", self.search_results, 1, 1000),
            ("concurrent_browses", self.concurrent_browses, 1, 4),
            ("idle_seconds", self.idle_seconds, 1, 30),
            ("operation_seconds", self.operation_seconds, 1, 3600),
        ] {
            if !(minimum..=maximum).contains(&value) {
                return Err(Error::InvalidInput(format!(
                    "sharing.{name} must be {minimum} through {maximum}"
                )));
            }
        }
        if self.queued_per_peer > self.queued_files
            || self.idle_seconds > self.operation_seconds
            || self.compressed_bytes > self.spool_bytes / self.concurrent_browses
            || self.upload_slots > self.peer_connections
        {
            return Err(Error::InvalidInput(
                "sharing limits are inconsistent".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, deny_unknown_fields)]
pub struct SharePolicy {
    pub access: ShareAccess,
    pub allowlist: Vec<String>,
    pub roots: Vec<ShareRoot>,
    pub limits: ShareLimits,
}

pub fn valid_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 30
        && value.trim() == value
        && value.bytes().all(|byte| (32..=126).contains(&byte))
}

impl SharePolicy {
    pub fn validate(&self) -> Result<()> {
        self.limits.validate()?;
        if self.allowlist.len() > 256 || self.roots.len() > self.limits.roots as usize {
            return Err(Error::InvalidInput(
                "sharing root or allowlist limit exceeded".into(),
            ));
        }
        let mut users = HashSet::new();
        for user in &self.allowlist {
            if !valid_username(user) || !users.insert(user) {
                return Err(Error::InvalidInput(
                    "invalid or duplicate sharing username".into(),
                ));
            }
        }
        if self.access == ShareAccess::Allowlist && self.allowlist.is_empty() {
            return Err(Error::InvalidInput(
                "sharing allowlist must not be empty".into(),
            ));
        }
        let mut aliases = HashSet::new();
        let mut bindings = HashSet::new();
        for root in &self.roots {
            crate::domain::LibraryPath::parse(&root.alias)?;
            if root.binding_id <= 0
                || root.alias.is_empty()
                || root.alias.len() > 128
                || root.alias.contains(['/', '\\'])
                || !aliases.insert(root.alias.to_lowercase())
                || !bindings.insert(root.binding_id)
            {
                return Err(Error::InvalidInput(
                    "invalid or duplicate sharing alias/binding".into(),
                ));
            }
        }
        if self.access != ShareAccess::None && self.roots.is_empty() {
            return Err(Error::InvalidInput(
                "select at least one bound share folder".into(),
            ));
        }
        Ok(())
    }

    pub fn allows(&self, user: &str) -> bool {
        valid_username(user)
            && match self.access {
                ShareAccess::None => false,
                ShareAccess::All => true,
                ShareAccess::Allowlist => self.allowlist.iter().any(|allowed| allowed == user),
            }
    }
}

/// This list is independent of acquisition formats. Add a future format here
/// only after its sharing behavior and metadata have their own acceptance tests.
pub fn eligible_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        ["mp3", "flac", "jpg", "jpeg"]
            .iter()
            .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharing_catalog_byte_budget_defaults_and_bounds_are_finite() -> Result<()> {
        let mut limits: ShareLimits = serde_json::from_str("{}")?;
        assert_eq!(limits.catalog_bytes, 256 * 1024 * 1024);
        for value in [1024, 1024 * 1024 * 1024] {
            limits.catalog_bytes = value;
            limits.validate()?;
        }
        for value in [0, 1023, 1024 * 1024 * 1024 + 1] {
            limits.catalog_bytes = value;
            assert!(limits.validate().is_err());
        }
        Ok(())
    }

    #[test]
    fn sharing_defaults_deny_access_and_limits_are_finite() {
        let policy = SharePolicy::default();
        assert!(policy.validate().is_ok());
        assert!(!policy.allows("peer"));
        let mut limits = policy.limits;
        limits.upload_slots = 0;
        assert!(limits.validate().is_err());
    }

    #[test]
    fn sharing_access_and_aliases_are_exact() {
        let mut policy = SharePolicy {
            access: ShareAccess::Allowlist,
            allowlist: vec!["peer".into()],
            roots: vec![ShareRoot {
                binding_id: 1,
                alias: "Music".into(),
            }],
            ..SharePolicy::default()
        };
        assert!(policy.validate().is_ok());
        assert!(policy.allows("peer"));
        assert!(!policy.allows("Peer"));
        assert!(!policy.allows("../peer"));
        policy.roots.push(ShareRoot {
            binding_id: 2,
            alias: "music".into(),
        });
        assert!(policy.validate().is_err());
    }

    #[test]
    fn sharing_formats_do_not_expose_state_or_unknown_audio() {
        for name in ["track.MP3", "track.flac", "folder.jpg", "cover.jpeg"] {
            assert!(eligible_extension(name));
        }
        for name in ["secret.json", "state.sqlite", "track.m4a", "log.txt"] {
            assert!(!eligible_extension(name));
        }
    }
}
