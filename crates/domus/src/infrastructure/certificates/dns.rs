//! Propagation means an exact TXT answer from every discovered authority.
//! A recursive cache or a successful Azure management write is not this proof.
use crate::domain::certificates::CertificateNames;
use crate::{Error, Result};
use futures_util::StreamExt;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{NameServerConfig, ResolverOpts};
use hickory_resolver::name_server::{NameServer, TokioConnectionProvider};
use hickory_resolver::proto::op::{Message, Query, ResponseCode};
use hickory_resolver::proto::rr::{Name, RData, RecordType};
use hickory_resolver::proto::xfer::{DnsHandle, Protocol};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

pub struct DnsPropagation {
    resolver: TokioResolver,
}

impl DnsPropagation {
    /// Read system resolver configuration only during explicit construction,
    /// outside an async executor thread. No query is sent by this constructor.
    pub fn new() -> Result<Self> {
        let mut builder = TokioResolver::builder_tokio().map_err(|_| {
            Error::InvalidInput("system DNS resolver configuration is unavailable".into())
        })?;
        let options = builder.options_mut();
        options.timeout = Duration::from_secs(3);
        options.attempts = 1;
        options.num_concurrent_reqs = 1;
        options.cache_size = 0;
        Ok(Self {
            resolver: builder.build(),
        })
    }

    /// One attempt has at most four authorities and two addresses per authority.
    /// TCP provides bounded framing and avoids UDP truncation/fallback ambiguity.
    pub async fn propagated(&self, names: &CertificateNames, value: &str) -> Result<bool> {
        let zone = format!("{}.", names.zone);
        let authorities = self
            .resolver
            .ns_lookup(zone)
            .await
            .map_err(|_| retry("authoritative DNS discovery failed"))?;
        let mut authorities = authorities
            .iter()
            .map(|ns| ns.to_utf8().to_ascii_lowercase())
            .collect::<Vec<_>>();
        authorities.sort();
        authorities.dedup();
        if authorities.is_empty() || authorities.len() > 4 {
            return Err(Error::InvalidInput(
                "Azure DNS must have one through four authoritative servers".into(),
            ));
        }
        let name = Name::from_ascii(format!("{}.{}.", names.record, names.zone))
            .map_err(|_| Error::InvalidInput("DNS-01 record name is invalid".into()))?;
        for authority in authorities {
            if ![
                ".azure-dns.com.",
                ".azure-dns.net.",
                ".azure-dns.org.",
                ".azure-dns.info.",
            ]
            .iter()
            .any(|suffix| authority.ends_with(suffix))
            {
                return Err(Error::InvalidInput(
                    "DNS zone is not delegated to Azure DNS authorities".into(),
                ));
            }
            let addresses = self
                .resolver
                .ipv4_lookup(authority)
                .await
                .map_err(|_| retry("authoritative DNS address discovery failed"))?;
            let mut addresses = addresses
                .iter()
                .map(|address| address.0)
                .collect::<Vec<_>>();
            addresses.sort();
            addresses.dedup();
            if addresses.is_empty()
                || addresses.len() > 2
                || addresses.iter().any(|address| !public_address(*address))
            {
                return Err(Error::InvalidInput(
                    "authoritative DNS address is not a bounded public IPv4 endpoint".into(),
                ));
            }
            for address in addresses {
                if !query_authority(SocketAddr::from((address, 53)), &name, value).await? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

async fn query_authority(address: SocketAddr, name: &Name, value: &str) -> Result<bool> {
    let mut options = ResolverOpts::default();
    options.timeout = Duration::from_secs(3);
    options.attempts = 1;
    options.recursion_desired = false;
    let server = NameServer::new(
        NameServerConfig::new(address, Protocol::Tcp),
        options,
        TokioConnectionProvider::default(),
    );
    let mut message = Message::new();
    message
        .set_recursion_desired(false)
        .add_query(Query::query(name.clone(), RecordType::TXT));
    let mut replies = server.send(message);
    let response = tokio::time::timeout(Duration::from_secs(4), replies.next())
        .await
        .map_err(|_| retry("authoritative DNS query timed out"))?
        .ok_or_else(|| retry("authoritative DNS returned no response"))?
        .map_err(|_| retry("authoritative DNS query failed"))?;
    if !response.authoritative()
        || response.truncated()
        || response.answers().len() > 100
        || response.queries().len() != 1
        || response.queries()[0].name() != name
    {
        return Err(Error::Remote(
            "DNS-01 response is not an exact authoritative answer",
        ));
    }
    if response.response_code() == ResponseCode::NXDomain {
        return Ok(false);
    }
    if response.response_code() != ResponseCode::NoError {
        return Err(retry("authoritative DNS returned an error"));
    }
    if response
        .answers()
        .iter()
        .any(|record| record.name() != name || !matches!(record.data(), RData::TXT(_)))
    {
        return Err(Error::Remote(
            "DNS-01 answer contains an alias or unrelated record",
        ));
    }
    Ok(response.answers().iter().any(|record| match record.data() {
        RData::TXT(txt) => txt
            .txt_data()
            .iter()
            .flat_map(|segment| segment.iter().copied())
            .eq(value.bytes()),
        _ => false,
    }))
}

fn public_address(address: Ipv4Addr) -> bool {
    !address.is_private()
        && !address.is_loopback()
        && !address.is_link_local()
        && !address.is_multicast()
        && !address.is_broadcast()
        && !address.is_documentation()
        && address.octets()[0] != 0
        && address.octets()[0] < 240
        && !(address.octets()[0] == 100 && (64..=127).contains(&address.octets()[1]))
        && !(address.octets()[0] == 198 && matches!(address.octets()[1], 18 | 19))
}

fn retry(reason: &'static str) -> Error {
    Error::Certificate {
        reason,
        retry_after_seconds: Some(30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn certificates_dns_requires_exact_authoritative_tcp_answers()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use hickory_resolver::proto::op::MessageType;
        use hickory_resolver::proto::rr::{Record, rdata::TXT};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (authoritative, exact, expected) in [
            (true, true, true),
            (false, true, false),
            (true, false, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let fixture = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await?;
                let length = stream.read_u16().await?;
                if length > 1024 {
                    return Err(std::io::Error::other("fixture request exceeded its bound"));
                }
                let mut bytes = vec![0; usize::from(length)];
                stream.read_exact(&mut bytes).await?;
                let query = Message::from_vec(&bytes).map_err(std::io::Error::other)?;
                assert!(!query.recursion_desired());
                let name = query.queries()[0].name().clone();
                let mut response = Message::new();
                response
                    .set_id(query.id())
                    .set_message_type(MessageType::Response)
                    .set_authoritative(authoritative)
                    .add_query(query.queries()[0].clone());
                response.add_answer(Record::from_rdata(
                    if exact {
                        name
                    } else {
                        Name::from_ascii("other.home.example.").map_err(std::io::Error::other)?
                    },
                    60,
                    RData::TXT(TXT::new(vec!["first".into(), "second".into()])),
                ));
                let bytes = response.to_vec().map_err(std::io::Error::other)?;
                stream
                    .write_u16(u16::try_from(bytes.len()).map_err(std::io::Error::other)?)
                    .await?;
                stream.write_all(&bytes).await?;
                Ok::<_, std::io::Error>(())
            });
            let name = Name::from_ascii("_acme-challenge.api.home.example.")?;
            let result = query_authority(address, &name, "firstsecond").await;
            if expected {
                assert!(result?)
            } else {
                assert!(result.is_err())
            }
            fixture.await??;
        }
        Ok(())
    }

    #[test]
    fn certificates_dns_never_queries_private_authority_addresses() -> Result<()> {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "224.0.0.1",
        ] {
            assert!(!public_address(
                address
                    .parse()
                    .map_err(|_| Error::Internal("fixture IPv4"))?
            ));
        }
        assert!(public_address(Ipv4Addr::new(1, 1, 1, 1)));
        Ok(())
    }
}
