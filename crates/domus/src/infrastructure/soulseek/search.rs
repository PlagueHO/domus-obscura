//! One bounded search over outgoing server-mediated P connections. No listener,
//! shares, upload acceptance, download, automatic retry or source approval.
#[cfg(test)]
use super::protocol::MAX_EXPANDED;
use super::protocol::{expanded, file};
use super::*;
pub use crate::domain::sources::SourceFileMetadata as SearchFile;
use serde::Serialize;
use std::collections::HashSet;

const MAX_RESULTS: usize = 1000;
const MAX_PEERS: usize = 16;
const MAX_SEARCH_MESSAGES: usize = 256;
const SEARCH_WINDOW: Duration = Duration::from_secs(45);
const PEER_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize)]
pub struct SearchPeer {
    pub peer: String,
    pub slot_free: bool,
    pub speed: u32,
    pub queue_length: u32,
    pub files: Vec<SearchFile>,
}

#[derive(Serialize)]
pub struct SearchIssue {
    pub peer: String,
    pub error: String,
}

#[derive(Serialize)]
pub struct SearchReport {
    pub query: String,
    pub peers: Vec<SearchPeer>,
    pub issues: Vec<SearchIssue>,
    pub limit_reached: bool,
    pub audio_downloaded: bool,
    pub edition_verified: bool,
}

fn parse_response(input: &[u8], peer: &str, token: u32) -> Result<SearchPeer> {
    let output = expanded(input)?;
    let mut fields = Fields(&output);
    if fields.text(30)? != peer || fields.u32()? != token {
        return Err(Error::Remote(
            "Soulseek search response has a different peer or token",
        ));
    }
    let count = fields.u32()? as usize;
    if count > MAX_RESULTS {
        return Err(protocol());
    }
    let mut files = Vec::with_capacity(count);
    let mut paths = HashSet::new();
    for _ in 0..count {
        let result = file(&mut fields)?;
        if !paths.insert(result.remote_path.replace('/', "\\").to_lowercase()) {
            return Err(protocol());
        }
        files.push(result);
    }
    let slot_free = fields.boolean()?;
    let speed = fields.u32()?;
    let queue_length = fields.u32()?;
    // Older clients omit the private results extension. Parse it when present
    // but never suggest files which the peer has not made publicly available.
    if !fields.0.is_empty() {
        fields.u32()?;
        let private_count = fields.u32()? as usize;
        if private_count > MAX_RESULTS - count {
            return Err(protocol());
        }
        for _ in 0..private_count {
            file(&mut fields)?;
        }
    }
    fields.end()?;
    Ok(SearchPeer {
        peer: peer.into(),
        slot_free,
        speed,
        queue_length,
        files,
    })
}

impl SoulseekSession {
    /// Consume the server session so late results cannot enter a later search
    /// or transfer operation. Only public IPv4 reverse P connections are used.
    pub fn search(mut self, query: &str, token: u32) -> Result<SearchReport> {
        if token == 0
            || query.trim().is_empty()
            || query.len() > 256
            || query.chars().any(char::is_control)
        {
            return Err(Error::InvalidInput(
                "search needs a nonzero token and 1 through 256 query bytes without controls"
                    .into(),
            ));
        }
        let mut server = self
            .stream
            .take()
            .ok_or_else(|| Error::Conflict("Soulseek server session is closed".into()))?;
        let mut body = token.to_le_bytes().to_vec();
        string(&mut body, query);
        send(&mut server, 26, &body)?;
        let mut report = SearchReport {
            query: query.into(),
            peers: Vec::new(),
            issues: Vec::new(),
            limit_reached: false,
            audio_downloaded: false,
            edition_verified: false,
        };
        let deadline = Instant::now() + SEARCH_WINDOW;
        let mut seen = HashSet::new();
        for message in 0..MAX_SEARCH_MESSAGES {
            if Instant::now() >= deadline {
                break;
            }
            if message + 1 == MAX_SEARCH_MESSAGES {
                report.limit_reached = true;
            }
            // An idle search is normal. Do not discard a partially read frame:
            // only a timeout before its first byte can end this wait.
            server.set_read_timeout(Some(Duration::from_millis(500)))?;
            match server.peek(&mut [0; 1]) {
                Ok(0) => return Err(Error::Remote("Soulseek server closed during search")),
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            let (code, body) = frame(&mut server, deadline)?;
            if code == 41 {
                return Err(Error::Remote(
                    "Soulseek account signed in elsewhere; session closed",
                ));
            }
            if code != 18 {
                continue;
            }
            let (peer, connection_token) = search_request(&body)?;
            if !seen.insert(peer.username.clone()) {
                continue;
            }
            if seen.len() > MAX_PEERS {
                report.limit_reached = true;
                break;
            }
            let peer_deadline = deadline.min(Instant::now() + PEER_WINDOW);
            match search_peer(&peer, connection_token, token, peer_deadline) {
                Ok(result) => report.peers.push(result),
                Err(error) => report.issues.push(SearchIssue {
                    peer: peer.username,
                    error: error.to_string(),
                }),
            }
        }
        Ok(report)
    }
}

fn search_request(body: &[u8]) -> Result<(PeerAddress, u32)> {
    let mut fields = Fields(body);
    let name = fields.text(30)?;
    username(name).map_err(|_| protocol())?;
    if fields.text(1)? != "P" {
        return Err(protocol());
    }
    let ip = Ipv4Addr::from(fields.u32()?);
    let port = u16::try_from(fields.u32()?).map_err(|_| protocol())?;
    let token = fields.u32()?;
    fields.boolean()?;
    if !fields.0.is_empty() {
        fields.u32()?;
        fields.u32()?;
    }
    fields.end()?;
    if !public_peer_ip(ip) || port == 0 {
        return Err(Error::Remote(
            "Soulseek search peer is not a public unicast IPv4 destination",
        ));
    }
    Ok((
        PeerAddress {
            username: name.into(),
            address: SocketAddrV4::new(ip, port),
        },
        token,
    ))
}

fn search_peer(
    peer: &PeerAddress,
    connection_token: u32,
    token: u32,
    deadline: Instant,
) -> Result<SearchPeer> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(protocol)?;
    let mut stream = TcpStream::connect_timeout(&peer.address.into(), remaining)?;
    stream.set_write_timeout(Some(
        deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(protocol)?,
    ))?;
    send_init(&mut stream, 0, &connection_token.to_le_bytes())?;
    let (code, body) = frame(&mut stream, deadline)?;
    if code != 9 {
        return Err(protocol());
    }
    parse_response(&body, &peer.username, token)
}

#[cfg(test)]
mod tests;
