use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Media transport admission is separate from application HTTP admission.
/// Private-LAN mode requires one exact peer and the paired TLS server name.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkAccess {
    #[default]
    Loopback,
    PrivateLan {
        server_name: String,
        peer_ip: IpAddr,
    },
}

impl NetworkAccess {
    pub(crate) fn validate(&self) -> Result<()> {
        let Self::PrivateLan {
            server_name,
            peer_ip,
        } = self
        else {
            return Ok(());
        };
        if !private_address(*peer_ip)
            || server_name.is_empty()
            || server_name.len() > 253
            || server_name.parse::<IpAddr>().is_ok()
            || reqwest::Url::parse(&format!("https://{server_name}/"))
                .map_or(true, |url| url.domain() != Some(server_name.as_str()))
            || server_name.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            })
        {
            return Err(Error::InvalidInput(
                "Private media transport requires a fixed private or loopback peer IP and an exact lowercase TLS DNS name.".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn server_name(&self) -> &str {
        match self {
            Self::Loopback => "localhost",
            Self::PrivateLan { server_name, .. } => server_name,
        }
    }

    pub(crate) fn permits_peer(&self, address: IpAddr) -> bool {
        match self {
            Self::Loopback => address.is_loopback(),
            Self::PrivateLan { peer_ip, .. } => address == *peer_ip,
        }
    }

    pub(crate) fn permits_listener(&self, address: IpAddr) -> bool {
        match self {
            Self::Loopback => address.is_loopback(),
            Self::PrivateLan { .. } => private_address(address),
        }
    }
}

fn private_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_loopback() || address.is_private(),
        IpAddr::V6(address) => {
            address.is_loopback()
                || (address.segments()[0] & 0xfe00 == 0xfc00 && !address.is_unicast_link_local())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn media_adapter_network_defaults_to_loopback_and_requires_an_exact_private_peer() -> TestResult
    {
        let default: NetworkAccess = serde_json::from_str(r#"{"mode":"loopback"}"#)?;
        assert!(default.permits_peer("127.0.0.1".parse()?));
        assert!(!default.permits_peer("192.168.1.2".parse()?));
        let access: NetworkAccess = serde_json::from_str(
            r#"{"mode":"private_lan","server_name":"media.internal","peer_ip":"192.168.1.2"}"#,
        )?;
        access.validate()?;
        assert!(access.permits_peer("192.168.1.2".parse()?));
        assert!(!access.permits_peer("192.168.1.3".parse()?));
        assert!(!access.permits_listener("0.0.0.0".parse()?));
        assert!(!access.permits_listener("8.8.8.8".parse()?));
        Ok(())
    }

    #[test]
    fn media_adapter_network_rejects_public_wildcard_and_ambiguous_addresses() -> TestResult {
        for peer in [
            "0.0.0.0",
            "8.8.8.8",
            "::",
            "::ffff:192.168.1.2",
            "fe80::1",
            "ff02::1",
        ] {
            let access = NetworkAccess::PrivateLan {
                server_name: "media.internal".into(),
                peer_ip: peer.parse()?,
            };
            assert!(access.validate().is_err());
        }
        for name in [
            "https://media.internal",
            "media.internal:8443",
            "*.internal",
            "media..internal",
            "127.0.0.1",
            "MEDIA.internal",
            "2130706433",
            "0x7f000001",
            "127.1",
        ] {
            let access = NetworkAccess::PrivateLan {
                server_name: name.into(),
                peer_ip: "192.168.1.2".parse()?,
            };
            assert!(access.validate().is_err());
        }
        Ok(())
    }
}
