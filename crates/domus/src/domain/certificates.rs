//! Certificate names, durable renewal times and exact TXT-value ownership.
//! Native paths and protocol credentials are deliberately outside this policy.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Select the certificate source only for an enabled HTTPS host. Anonymous
/// loopback HTTP does not activate either source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    #[default]
    Manual,
    AzureDns,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum AcmeEnvironment {
    Staging,
    Production,
}

impl AcmeEnvironment {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Staging => "https://acme-staging-v02.api.letsencrypt.org/directory",
            Self::Production => "https://acme-v02.api.letsencrypt.org/directory",
        }
    }

    pub fn authority(self) -> &'static str {
        match self {
            Self::Staging => "acme-staging-v02.api.letsencrypt.org",
            Self::Production => "acme-v02.api.letsencrypt.org",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateNames {
    pub hostname: String,
    pub zone: String,
    pub record: String,
}

impl CertificateNames {
    /// Exactly one non-wildcard hostname must belong to the configured zone.
    /// The DDNS CNAME target is never a certificate identifier.
    pub fn new(hostname: &str, zone: &str) -> Result<Self> {
        if !dns_name(hostname)
            || !dns_name(zone)
            || hostname.parse::<std::net::IpAddr>().is_ok()
            || zone.parse::<std::net::IpAddr>().is_ok()
            || hostname == "localhost"
        {
            return Err(Error::InvalidInput(
                "managed TLS requires canonical lowercase DNS names".into(),
            ));
        }
        let record = if hostname == zone {
            "_acme-challenge".to_owned()
        } else {
            let prefix = hostname.strip_suffix(&format!(".{zone}")).ok_or_else(|| {
                Error::InvalidInput("certificate hostname is outside the Azure DNS zone".into())
            })?;
            format!("_acme-challenge.{prefix}")
        };
        if record.len() + zone.len() + 1 > 253 {
            return Err(Error::InvalidInput(
                "DNS-01 record name exceeds its size limit".into(),
            ));
        }
        Ok(Self {
            hostname: hostname.into(),
            zone: zone.into(),
            record,
        })
    }
}

fn dns_name(name: &str) -> bool {
    (1..=253).contains(&name.len())
        && name.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// The selected ARI instant is saved once, not randomized on each restart.
/// A lifetime-based deadline remains a backstop even if ARI is later.
pub fn renewal_time(
    not_before: u64,
    not_after: u64,
    ari_window: Option<(u64, u64)>,
    random: u64,
) -> Result<u64> {
    let lifetime = not_after
        .checked_sub(not_before)
        .filter(|span| *span > 0)
        .ok_or_else(|| Error::InvalidInput("certificate validity interval is invalid".into()))?;
    let backstop = not_before
        + if lifetime < 10 * 86400 {
            lifetime / 2
        } else {
            (u128::from(lifetime) * 2 / 3) as u64
        };
    match ari_window {
        None => Ok(backstop),
        Some((start, end)) if start < end && end <= not_after => {
            let width = end - start;
            let offset = ((u128::from(random) * u128::from(width)) >> 64) as u64;
            Ok((start + offset).min(backstop))
        }
        Some(_) => Err(Error::InvalidInput("ACME renewal window is invalid".into())),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum CertificatePhase {
    Uninitialized,
    AccountPending,
    OrderPending,
    DnsPending,
    PropagationPending,
    ValidationPending,
    FinalizationPending,
    CleanupPending,
    Ready,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenewalAdvice {
    Unsupported,
    Suggested {
        start: u64,
        end: u64,
        refresh_after_seconds: u32,
    },
}

/// A refresh changes journal scheduling, never the immutable certificate.
/// No window or refresh interval explicitly records unsupported ARI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenewalSchedule {
    pub generation: u64,
    pub renew_at: u64,
    pub window: Option<(u64, u64)>,
    pub refresh_interval: Option<u32>,
    pub refresh_at: Option<u64>,
}

pub fn renewal_schedule(
    generation: u64,
    validity: (u64, u64),
    advice: RenewalAdvice,
    previous: Option<RenewalSchedule>,
    random: u64,
    now: u64,
) -> Result<RenewalSchedule> {
    let (window, refresh_interval) = match advice {
        RenewalAdvice::Unsupported => (None, None),
        RenewalAdvice::Suggested {
            start,
            end,
            refresh_after_seconds,
        } if refresh_after_seconds <= 86400 => {
            (Some((start, end)), Some(refresh_after_seconds.max(60)))
        }
        RenewalAdvice::Suggested { .. } => {
            return Err(Error::Remote(
                "ACME renewal refresh exceeds one day; local recovery is required",
            ));
        }
    };
    let selected = renewal_time(validity.0, validity.1, window, random)?;
    let renew_at = previous
        .filter(|old| old.generation == generation && old.window == window)
        .map_or(selected, |old| old.renew_at);
    Ok(RenewalSchedule {
        generation,
        renew_at,
        window,
        refresh_interval,
        refresh_at: refresh_interval
            .map(|seconds| {
                now.checked_add(u64::from(seconds))
                    .ok_or(Error::Internal("ACME renewal refresh time overflow"))
            })
            .transpose()?,
    })
}

/// A receipt proves one successful conditional update. A matching pre-existing
/// TXT string is not a receipt and must never become owned cleanup work.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TxtReceipt {
    pub value: String,
    pub etag: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TxtRecord {
    pub etag: String,
    pub properties: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TxtAddition {
    AlreadyPresent,
    Update(Value),
}

impl TxtRecord {
    pub fn validate(&self) -> Result<()> {
        self.values().map(|_| ())
    }

    fn values(&self) -> Result<&Vec<Value>> {
        if self.etag.is_empty()
            || self.etag.len() > 512
            || self.etag.bytes().any(|byte| !byte.is_ascii_graphic())
        {
            return Err(Error::Remote("Azure DNS returned an invalid ETag"));
        }
        self.properties
            .get("TTL")
            .and_then(Value::as_u64)
            .filter(|ttl| *ttl > 0 && *ttl <= i32::MAX as u64)
            .ok_or(Error::Remote("Azure DNS returned an invalid TXT TTL"))?;
        let values = self
            .properties
            .get("TXTRecords")
            .and_then(Value::as_array)
            .ok_or(Error::Remote("Azure DNS returned invalid TXT records"))?;
        if values.len() > 100 {
            return Err(Error::Remote(
                "Azure DNS TXT record exceeds the value limit",
            ));
        }
        for entry in values {
            let strings = entry
                .get("value")
                .and_then(Value::as_array)
                .ok_or(Error::Remote("Azure DNS returned invalid TXT segments"))?;
            if strings.is_empty()
                || strings.len() > 256
                || strings.iter().any(|segment| {
                    segment
                        .as_str()
                        .is_none_or(|text| text.len() > 255 || !text.is_ascii())
                })
            {
                return Err(Error::Remote("Azure DNS returned invalid TXT segments"));
            }
        }
        Ok(values)
    }

    pub fn add(&self, value: &str) -> Result<TxtAddition> {
        challenge_value(value)?;
        let entries = self.values()?;
        if entries.iter().any(|entry| logical_value(entry) == value) {
            return Ok(TxtAddition::AlreadyPresent);
        }
        if entries.len() == 100 {
            return Err(Error::InvalidInput(
                "Azure DNS TXT record has no free value slot".into(),
            ));
        }
        let mut properties = self.properties.clone();
        properties["TXTRecords"]
            .as_array_mut()
            .ok_or(Error::Internal("validated TXT array disappeared"))?
            .push(json!({ "value": [value] }));
        // fqdn is server-derived. TTL, metadata and all unrelated values remain.
        properties
            .as_object_mut()
            .ok_or(Error::Remote("invalid TXT properties"))?
            .remove("fqdn");
        Ok(TxtAddition::Update(json!({ "properties": properties })))
    }

    pub fn remove(&self, receipt: &TxtReceipt) -> Result<Value> {
        challenge_value(&receipt.value)?;
        if self.etag != receipt.etag {
            return Err(Error::Conflict(
                "Azure DNS TXT record changed; cleanup needs explicit recovery".into(),
            ));
        }
        let entries = self.values()?;
        let owned = json!({ "value": [&receipt.value] });
        if entries.iter().filter(|entry| **entry == owned).count() != 1 {
            return Err(Error::Conflict(
                "the exact owned TXT value is no longer unique".into(),
            ));
        }
        let mut properties = self.properties.clone();
        properties["TXTRecords"] = Value::Array(
            entries
                .iter()
                .filter(|entry| **entry != owned)
                .cloned()
                .collect(),
        );
        properties
            .as_object_mut()
            .ok_or(Error::Remote("invalid TXT properties"))?
            .remove("fqdn");
        Ok(json!({ "properties": properties }))
    }
}

fn logical_value(entry: &Value) -> String {
    entry["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn challenge_value(value: &str) -> Result<()> {
    if value.len() != 43
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(Error::InvalidInput(
            "DNS-01 challenge value is invalid".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificates_mode_defaults_to_manual_and_rejects_unknown_sources() -> Result<()> {
        assert_eq!(TlsMode::default(), TlsMode::Manual);
        assert_eq!(serde_json::to_string(&TlsMode::AzureDns)?, "\"azure_dns\"");
        assert!(serde_json::from_str::<TlsMode>("\"unconfigured\"").is_err());
        Ok(())
    }

    #[test]
    fn certificates_bound_the_complete_dns01_record_name() -> Result<()> {
        let prefix = format!("{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63));
        let accepted = format!("{prefix}.{}.home.example", "d".repeat(32));
        assert_eq!(accepted.len(), 237);
        let names = CertificateNames::new(&accepted, "home.example")?;
        assert_eq!(format!("{}.{}", names.record, names.zone).len(), 253);
        let rejected = format!("{prefix}.{}.home.example", "d".repeat(33));
        assert_eq!(rejected.len(), 238);
        assert!(CertificateNames::new(&rejected, "home.example").is_err());
        assert!(CertificateNames::new(&rejected, &rejected).is_err());
        Ok(())
    }

    #[test]
    fn certificates_cover_only_the_exact_zone_hostname() -> Result<()> {
        assert_eq!(
            CertificateNames::new("api.home.example", "home.example")?.record,
            "_acme-challenge.api"
        );
        assert_eq!(
            CertificateNames::new("home.example", "home.example")?.record,
            "_acme-challenge"
        );
        for name in [
            "*.home.example",
            "nothome.example",
            "HOME.example",
            "127.0.0.1",
            "home.example.",
        ] {
            assert!(CertificateNames::new(name, "home.example").is_err());
        }
        Ok(())
    }

    #[test]
    fn certificates_renew_from_actual_lifetime_and_saved_ari_time() -> Result<()> {
        assert_eq!(
            renewal_time(100, 100 + 90 * 86400, None, 0)?,
            100 + 60 * 86400
        );
        assert_eq!(
            renewal_time(100, 100 + 6 * 86400, None, 0)?,
            100 + 3 * 86400
        );
        assert_eq!(
            renewal_time(100, 100 + 90 * 86400, Some((200, 300)), 0)?,
            200
        );
        assert_eq!(
            renewal_time(100, 100 + 90 * 86400, Some((200, 300)), u64::MAX)?,
            299
        );
        assert!(renewal_time(10, 10, None, 0).is_err());
        assert!(renewal_time(10, 100, Some((40, 20)), 0).is_err());
        Ok(())
    }

    #[test]
    fn certificates_preserve_unrelated_txt_and_never_claim_preexisting_values() -> Result<()> {
        let value = "a".repeat(43);
        let properties = json!({
            "TTL": 321,
            "metadata": { "owner": "setup" },
            "TXTRecords": [{ "value": ["unrelated", " segments"] }]
        });
        let record = TxtRecord {
            etag: "\"one\"".into(),
            properties: properties.clone(),
        };
        let TxtAddition::Update(update) = record.add(&value)? else {
            panic!("new value must need an update")
        };
        assert_eq!(update["properties"]["TTL"], 321);
        assert_eq!(update["properties"]["metadata"], properties["metadata"]);
        assert_eq!(
            update["properties"]["TXTRecords"][0],
            properties["TXTRecords"][0]
        );
        let written = TxtRecord {
            etag: "\"two\"".into(),
            properties: update["properties"].clone(),
        };
        assert_eq!(written.add(&value)?, TxtAddition::AlreadyPresent);
        assert_eq!(
            written.remove(&TxtReceipt {
                value: value.clone(),
                etag: "\"two\"".into()
            })?["properties"],
            properties
        );
        assert!(
            written
                .remove(&TxtReceipt {
                    value,
                    etag: "\"one\"".into()
                })
                .is_err()
        );
        Ok(())
    }
}
