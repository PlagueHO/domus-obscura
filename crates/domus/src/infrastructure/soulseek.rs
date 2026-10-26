//! Narrow blocking Soulseek server session. No listeners, shares, automatic
//! retries, generated accounts or filesystem access are enabled here.
//! Wire codecs are separate from connection and operation state. This adapter
//! is a short-lived subset, not a persistent concurrent client/event dispatcher.
use crate::{Error, Result};
use std::io::Read;
#[cfg(test)]
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};
use symphonia::core::checksum::Md5;
use symphonia::core::io::Monitor;

const IO_TIMEOUT: Duration = Duration::from_secs(10);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

mod folder;
mod protocol;
pub mod search;
#[cfg(test)]
use protocol::MAX_FRAME;
use protocol::{Fields, protocol, read_frame, send, send_init, string};

fn md5(parts: &[&[u8]]) -> String {
    let mut hash = Md5::default();
    for part in parts {
        hash.process_buf_bytes(part);
    }
    hash.md5()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn username(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 30
        || value.trim() != value
        || !value.bytes().all(|byte| (32..=126).contains(&byte))
    {
        return Err(Error::InvalidInput("Soulseek username must contain 1 through 30 printable ASCII characters without outer spaces".into()));
    }
    Ok(())
}

pub(crate) fn validate_credentials(user: &str, password: &str) -> Result<()> {
    username(user)?;
    if password.is_empty() || password.len() > 128 || password.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "Soulseek password must contain 1 through 128 bytes without control characters".into(),
        ));
    }
    Ok(())
}

fn read_checked(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut position = 0;
    while position < buffer.len() {
        check()?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(Error::Remote("Soulseek operation exceeded its time limit"))?;
        stream.set_read_timeout(Some(remaining.min(IO_TIMEOUT)))?;
        let read = stream.read(&mut buffer[position..])?;
        if read == 0 {
            return Err(Error::Remote(
                "Soulseek closed the connection before the complete message arrived",
            ));
        }
        position += read;
    }
    if Instant::now() >= deadline {
        return Err(Error::Remote("Soulseek operation exceeded its time limit"));
    }
    check()?;
    Ok(())
}

fn frame(stream: &mut TcpStream, deadline: Instant) -> Result<(u32, Vec<u8>)> {
    checked_frame(stream, deadline, &mut || Ok(()))
}

fn checked_frame(
    stream: &mut TcpStream,
    deadline: Instant,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<(u32, Vec<u8>)> {
    read_frame(|buffer| read_checked(stream, buffer, deadline, check))
}

/// An address is bound to the exact server-query username. Soulseek itself
/// does not authenticate or encrypt peer traffic. Never treat this as a TLS
/// identity or open private/local network destinations from an untrusted peer.
pub struct PeerAddress {
    username: String,
    address: SocketAddrV4,
}

impl PeerAddress {
    pub fn username(&self) -> &str {
        &self.username
    }
    pub fn address(&self) -> SocketAddrV4 {
        self.address
    }
}

/// Credentials are neither retained nor serializable. The caller supplies an
/// already connected server socket after explicit network authorization.
/// Soulseek's legacy login sends the password over plaintext TCP.
pub struct SoulseekSession {
    stream: Option<TcpStream>,
    username: String,
}

impl SoulseekSession {
    pub fn login(mut stream: TcpStream, user: &str, password: &str) -> Result<Self> {
        validate_credentials(user, password)?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let mut body = Vec::with_capacity(user.len() + password.len() + 52);
        string(&mut body, user);
        string(&mut body, password);
        // Experimental client version, not an impersonation of another client.
        body.extend_from_slice(&177u32.to_le_bytes());
        string(&mut body, &md5(&[user.as_bytes(), password.as_bytes()]));
        body.extend_from_slice(&1u32.to_le_bytes());
        let sent = send(&mut stream, 1, &body);
        body.fill(0);
        sent?;
        let (code, body) = frame(&mut stream, Instant::now() + OPERATION_TIMEOUT)?;
        if code != 1 {
            return Err(protocol());
        }
        let mut fields = Fields(&body);
        if !fields.boolean()? {
            // Never echo server text: it can contain credentials or log control
            // sequences. Report the bounded allow-listed reason only.
            let reason = fields.text(128)?;
            let message = match reason {
                "INVALIDPASS" => "Soulseek rejected the account password",
                "INVALIDUSERNAME" => "Soulseek rejected the account username",
                "SVRFULL" => "Soulseek server is full",
                "SVRPRIVATE" => "Soulseek server does not accept registrations",
                "INVALIDVERSION" => "Soulseek rejected the client version",
                _ => "Soulseek rejected login",
            };
            return Err(Error::Remote(message));
        }
        fields.text(65_536)?;
        fields.u32()?;
        if fields.text(32)? != md5(&[password.as_bytes()]) {
            return Err(protocol());
        }
        fields.boolean()?;
        fields.end()?;
        // No share tree, upload slots or distributed child acceptance.
        send(&mut stream, 28, &2u32.to_le_bytes())?;
        send(&mut stream, 35, &[0; 8])?;
        send(&mut stream, 100, &[0])?;
        Ok(Self {
            stream: Some(stream),
            username: user.into(),
        })
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// Only one control connection can borrow this session at a time.
    /// Direct connection only: no listener or indirect fallback is created.
    pub fn connect_peer(&mut self, peer: &str) -> Result<PeerControl<'_>> {
        let address = self.peer_address(peer)?;
        self.connect_address(address)
    }

    fn connect_address(&mut self, peer: PeerAddress) -> Result<PeerControl<'_>> {
        let mut stream = TcpStream::connect_timeout(&peer.address.into(), IO_TIMEOUT)?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let mut body = Vec::new();
        string(&mut body, &self.username);
        string(&mut body, "P");
        body.extend_from_slice(&0u32.to_le_bytes());
        // Init codes occupy one byte, not the normal four-byte message code.
        send_init(&mut stream, 1, &body)?;
        Ok(PeerControl {
            session: self,
            peer,
            stream: Some(stream),
            pending: None,
            accepted: None,
        })
    }

    /// A failed query closes the session. Late replies cannot be mistaken for
    /// a new request, and uncertain writes are never retried automatically.
    pub fn peer_address(&mut self, peer: &str) -> Result<PeerAddress> {
        username(peer)?;
        let mut stream = self.stream.take().ok_or_else(|| {
            Error::Conflict(
                "Soulseek session closed after an earlier protocol or transport failure".into(),
            )
        })?;
        let result = Self::query_address(&mut stream, peer);
        if result.is_ok() {
            self.stream = Some(stream);
        }
        result
    }
}

/// A direct control channel to the exact address returned for the selected
/// server username. This binding is not cryptographic peer authentication.
pub struct PeerControl<'a> {
    session: &'a mut SoulseekSession,
    peer: PeerAddress,
    stream: Option<TcpStream>,
    pending: Option<u32>,
    accepted: Option<u32>,
}

/// A file socket created only from the selected server-bound peer and its
/// indirect request. The file-transfer token still needs verification on wire.
pub struct PeerFile {
    peer: String,
    token: u32,
    stream: TcpStream,
}

impl PeerFile {
    pub(crate) fn into_parts(self) -> (String, u32, TcpStream) {
        (self.peer, self.token, self.stream)
    }
}

impl PeerControl<'_> {
    pub fn username(&self) -> &str {
        self.peer.username()
    }

    /// Commit submission intent before calling. A failure drops the socket;
    /// uncertain queues are not automatically retried. No uploads are served.
    /// The callback checks worker ownership while bounded messages arrive.
    pub fn queue_upload(
        &mut self,
        path: &str,
        expected_bytes: u64,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<u32> {
        if path.is_empty()
            || path.len() > 4096
            || path.chars().any(char::is_control)
            || expected_bytes == 0
        {
            return Err(Error::InvalidInput(
                "invalid selected Soulseek file path or size".into(),
            ));
        }
        if self.pending.is_some() || self.accepted.is_some() {
            return Err(Error::Conflict(
                "finish the pending Soulseek file connection before another queue request".into(),
            ));
        }
        let mut stream = self.stream.take().ok_or_else(|| {
            Error::Conflict("Soulseek peer connection closed after an earlier failure".into())
        })?;
        check()?;
        let mut body = Vec::new();
        string(&mut body, path);
        send(&mut stream, 43, &body)?;
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        for _ in 0..128 {
            let (code, body) = checked_frame(&mut stream, deadline, &mut check)?;
            let mut fields = Fields(&body);
            match code {
                40 => {
                    if fields.u32()? != 1 {
                        return Err(protocol());
                    }
                    let token = fields.u32()?;
                    if fields.text(4096)? != path || fields.u64()? != expected_bytes {
                        return Err(Error::Remote(
                            "Soulseek upload offer differs from the selected file or size",
                        ));
                    }
                    fields.end()?;
                    check()?;
                    self.pending = Some(token);
                    self.stream = Some(stream);
                    return Ok(token);
                }
                46 | 50 => {
                    if fields.text(4096)? != path {
                        return Err(protocol());
                    }
                    if code == 50 {
                        fields.text(4096)?;
                    }
                    fields.end()?;
                    return Err(Error::Remote(
                        "Soulseek peer rejected or failed the selected upload",
                    ));
                }
                44 => {
                    if fields.text(4096)? != path {
                        return Err(protocol());
                    }
                    fields.u32()?;
                    fields.end()?;
                }
                _ => {
                    return Err(Error::Remote(
                        "Soulseek peer sent an unsupported message while queuing",
                    ));
                }
            }
        }
        Err(Error::Remote(
            "Soulseek upload offer exceeded its message limit",
        ))
    }

    /// Call only after durable acceptance. On a failed write, keep that record
    /// for reconciliation; never assume the peer did not receive acceptance.
    pub fn accept_upload(&mut self, token: u32) -> Result<()> {
        if self.pending != Some(token) {
            return Err(Error::Conflict(
                "Soulseek upload token has no pending offer".into(),
            ));
        }
        let mut stream = self
            .stream
            .take()
            .ok_or_else(|| Error::Conflict("Soulseek peer connection is closed".into()))?;
        let mut body = token.to_le_bytes().to_vec();
        body.push(1);
        send(&mut stream, 41, &body)?;
        self.pending = None;
        self.accepted = Some(token);
        self.stream = Some(stream);
        Ok(())
    }

    /// The uploader asks the server for a reverse F connection. Dial only the
    /// exact selected address, then send its independent PierceFireWall token.
    /// No listener, port forwarding, resume or connection retry is enabled.
    pub fn file_connection(
        &mut self,
        token: u32,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<PeerFile> {
        if self.accepted != Some(token) || self.stream.is_none() {
            return Err(Error::Conflict(
                "Soulseek file connection has no accepted upload".into(),
            ));
        }
        let mut server = self
            .session
            .stream
            .take()
            .ok_or_else(|| Error::Conflict("Soulseek server session is closed".into()))?;
        let result = self.connect_file(&mut server, token, &mut check);
        match result {
            Ok(file) => {
                self.session.stream = Some(server);
                self.accepted = None;
                Ok(file)
            }
            Err(error) => {
                self.stream = None;
                Err(error)
            }
        }
    }

    fn connect_file(
        &self,
        server: &mut TcpStream,
        token: u32,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<PeerFile> {
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        for _ in 0..128 {
            let (code, body) = checked_frame(server, deadline, check)?;
            if code == 41 {
                return Err(Error::Remote(
                    "Soulseek account signed in elsewhere; session closed",
                ));
            }
            if code != 18 {
                continue;
            }
            let mut fields = Fields(&body);
            if fields.text(30)? != self.peer.username || fields.text(1)? != "F" {
                return Err(Error::Remote(
                    "Soulseek file connection request differs from the selected peer or type",
                ));
            }
            let ip = Ipv4Addr::from(fields.u32()?);
            let port = u16::try_from(fields.u32()?).map_err(|_| protocol())?;
            if SocketAddrV4::new(ip, port) != self.peer.address {
                return Err(Error::Remote(
                    "Soulseek file connection address changed after peer selection",
                ));
            }
            let indirect_token = fields.u32()?;
            fields.boolean()?;
            if !fields.0.is_empty() {
                fields.u32()?;
                fields.u32()?;
            }
            fields.end()?;
            check()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(Error::Remote("Soulseek operation exceeded its time limit"))?;
            let mut stream =
                TcpStream::connect_timeout(&self.peer.address.into(), remaining.min(IO_TIMEOUT))?;
            check()?;
            stream.set_write_timeout(Some(IO_TIMEOUT))?;
            send_init(&mut stream, 0, &indirect_token.to_le_bytes())?;
            check()?;
            return Ok(PeerFile {
                peer: self.peer.username.clone(),
                token,
                stream,
            });
        }
        Err(Error::Remote(
            "Soulseek file connection exceeded its message limit",
        ))
    }
}

impl SoulseekSession {
    fn query_address(stream: &mut TcpStream, peer: &str) -> Result<PeerAddress> {
        let mut body = Vec::new();
        string(&mut body, peer);
        send(stream, 3, &body)?;
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        for _ in 0..128 {
            let (code, body) = frame(stream, deadline)?;
            if code == 41 {
                return Err(Error::Remote(
                    "Soulseek account signed in elsewhere; session closed",
                ));
            }
            if code != 3 {
                continue;
            }
            let mut fields = Fields(&body);
            if fields.text(30)? != peer {
                return Err(protocol());
            }
            let ip = Ipv4Addr::from(fields.u32()?);
            let port = fields.u32()?;
            if !fields.0.is_empty() {
                fields.u32()?;
                fields.take(2)?;
            }
            fields.end()?;
            if ip.is_unspecified() || port == 0 {
                return Err(Error::Remote(
                    "Soulseek peer is offline or has no reachable port",
                ));
            }
            let port = u16::try_from(port).map_err(|_| protocol())?;
            if !public_peer_ip(ip) {
                return Err(Error::Remote(
                    "Soulseek peer address is not a public unicast IPv4 address",
                ));
            }
            return Ok(PeerAddress {
                username: peer.into(),
                address: SocketAddrV4::new(ip, port),
            });
        }
        Err(Error::Remote(
            "Soulseek peer lookup exceeded its message limit",
        ))
    }
}

pub(crate) fn public_peer_ip(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_broadcast()
        && !ip.is_multicast()
        && a != 0
        && a < 224
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 192 && b == 0 && (c == 0 || c == 2))
        && !(a == 192 && b == 88 && c == 99)
        && !(a == 198 && (b == 18 || b == 19 || b == 51 && c == 100))
        && !(a == 203 && b == 0 && c == 113)
}

#[cfg(test)]
mod tests;
