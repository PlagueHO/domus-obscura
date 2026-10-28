//! Bounded outgoing Soulseek sessions. One I/O owner routes every framed
//! connection; raw file sockets pass exclusively to the application receiver.
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
const QUEUE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const QUEUE_POLL: Duration = Duration::from_secs(60);

mod folder;
mod messages;
mod protocol;
pub mod search;
pub(crate) mod sharing;
mod transport;
use messages::{Event, PeerEvent, ServerEvent};
#[cfg(test)]
use protocol::MAX_FRAME;
use protocol::{Fields, protocol, read_frame, send, send_init, string};
use transport::Channel;

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
            return Err(Error::SoulseekTransport(
                std::io::ErrorKind::UnexpectedEof.into(),
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
    stream: Option<Channel>,
    username: String,
}

impl SoulseekSession {
    pub(crate) fn fork_for_peer(&self, peer: &str) -> Result<Self> {
        self.fork(peer, false)
    }

    fn fork_for_sharing(&self, peer: &str) -> Result<Self> {
        self.fork(peer, true)
    }

    fn fork(&self, peer: &str, sharing: bool) -> Result<Self> {
        username(peer)?;
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| Error::Conflict("Soulseek session is closed".into()))?;
        Ok(Self {
            stream: Some(if sharing {
                stream.metadata_subscription(peer)?
            } else {
                stream.subscribe(peer)?
            }),
            username: self.username.clone(),
        })
    }
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
        sent.map_err(network_error)?;
        let (code, body) =
            frame(&mut stream, Instant::now() + OPERATION_TIMEOUT).map_err(network_error)?;
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
        send(&mut stream, 28, &2u32.to_le_bytes()).map_err(network_error)?;
        send(&mut stream, 35, &[0; 8]).map_err(network_error)?;
        send(&mut stream, 100, &[0]).map_err(network_error)?;
        Ok(Self {
            stream: Some(Channel::server(stream)?),
            username: user.into(),
        })
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// Only one control connection can borrow this session at a time.
    /// Connections are outgoing only. A matching server-requested reverse
    /// control connection can follow a failed direct connection.
    pub fn connect_peer(&mut self, peer: &str) -> Result<PeerControl<'_>> {
        self.stream
            .as_ref()
            .ok_or_else(|| Error::Conflict("Soulseek server session is closed".into()))?
            .expect_control(true)?;
        let address = self.peer_address(peer)?;
        if let Some(stream) = self
            .stream
            .as_ref()
            .ok_or_else(|| Error::Conflict("Soulseek server session is closed".into()))?
            .reuse_peer(peer, address.address)?
        {
            return self.control_from_channel(address, stream, None);
        }
        let socket = match TcpStream::connect_timeout(&address.address.into(), IO_TIMEOUT) {
            Ok(socket) => (socket, None),
            Err(error) => {
                let server = self
                    .stream
                    .as_mut()
                    .ok_or_else(|| Error::Conflict("Soulseek server session is closed".into()))?;
                let event = server.receive(
                    Instant::now() + OPERATION_TIMEOUT,
                    &mut || Ok(()),
                    |event| matches!(event, Event::Server(ServerEvent::Connect { .. })),
                );
                let event = match event {
                    Err(Error::Remote("Soulseek operation exceeded its time limit")) => {
                        return Err(Error::SoulseekTransport(error));
                    }
                    other => other?,
                };
                match event {
                    Event::Server(ServerEvent::Connect {
                        username,
                        kind,
                        address: reverse,
                        token,
                    }) if username == address.username
                        && kind == "P"
                        && reverse == address.address =>
                    {
                        server.check()?;
                        (
                            TcpStream::connect_timeout(&reverse.into(), IO_TIMEOUT)
                                .map_err(Error::SoulseekTransport)?,
                            Some(token),
                        )
                    }
                    _ => {
                        return Err(Error::Remote(
                            "Soulseek reverse control request differs from the selected peer",
                        ));
                    }
                }
            }
        };
        self.control_from_socket(address, socket.0, socket.1)
    }

    #[cfg(test)]
    fn connect_address(&mut self, peer: PeerAddress) -> Result<PeerControl<'_>> {
        let socket = TcpStream::connect_timeout(&peer.address.into(), IO_TIMEOUT)
            .map_err(Error::SoulseekTransport)?;
        self.control_from_socket(peer, socket, None)
    }

    fn control_from_socket(
        &mut self,
        peer: PeerAddress,
        socket: TcpStream,
        reverse_token: Option<u32>,
    ) -> Result<PeerControl<'_>> {
        let stream = match self.stream.as_ref() {
            Some(server) => server.peer_for(socket, &peer.username, peer.address)?,
            #[cfg(test)]
            None => Channel::isolated_peer(socket)?,
            #[cfg(not(test))]
            None => return Err(Error::Conflict("Soulseek server session is closed".into())),
        };
        self.control_from_channel(peer, stream, reverse_token)
    }

    fn control_from_channel(
        &mut self,
        peer: PeerAddress,
        stream: Channel,
        reverse_token: Option<u32>,
    ) -> Result<PeerControl<'_>> {
        let reused = stream.shared_control();
        if !reused {
            stream.share_control(&peer.username, peer.address)?;
            let mut body = Vec::new();
            string(&mut body, &self.username);
            string(&mut body, "P");
            body.extend_from_slice(&0u32.to_le_bytes());
            // Init codes occupy one byte, not the normal four-byte message code.
            if let Some(token) = reverse_token {
                stream.init(0, &token.to_le_bytes())?;
            } else {
                stream.init(1, &body)?;
            }
        }
        if let Some(server) = self.stream.as_ref() {
            server.expect_control(false)?;
        }
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
    stream: Option<Channel>,
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
        self.wait_upload(path, expected_bytes, true, |_| check())
    }

    /// Observe an existing queue without re-submission. Queue observations are
    /// not absence proof. The callback fences ownership and persists evidence.
    pub(crate) fn wait_upload(
        &mut self,
        path: &str,
        expected_bytes: u64,
        submit: bool,
        observe: impl FnMut(Option<QueueObservation>) -> Result<()>,
    ) -> Result<u32> {
        self.wait_upload_until(
            path,
            expected_bytes,
            submit,
            observe,
            Instant::now() + QUEUE_TIMEOUT,
            QUEUE_POLL,
        )
    }

    fn wait_upload_until(
        &mut self,
        path: &str,
        expected_bytes: u64,
        submit: bool,
        mut observe: impl FnMut(Option<QueueObservation>) -> Result<()>,
        deadline: Instant,
        query_interval: Duration,
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
        observe(None)?;
        let mut body = Vec::new();
        string(&mut body, path);
        stream.send(if submit { 43 } else { 51 }, &body)?;
        let mut queried = Instant::now();
        loop {
            let until = deadline.min(queried + query_interval);
            let event = stream.receive(until, &mut || observe(None), |_| true);
            let event = match event {
                Err(Error::Remote("Soulseek operation exceeded its time limit"))
                    if Instant::now() < deadline =>
                {
                    observe(None)?;
                    stream.send(51, &body)?;
                    queried = Instant::now();
                    continue;
                }
                other => other?,
            };
            match event {
                Event::Peer(PeerEvent::Offer {
                    token,
                    path: offered,
                    bytes,
                }) => {
                    if offered != path || bytes != expected_bytes {
                        return Err(Error::Remote(
                            "Soulseek upload offer differs from the selected file or size",
                        ));
                    }
                    observe(Some(QueueObservation::Offer(token)))?;
                    self.pending = Some(token);
                    self.stream = Some(stream);
                    return Ok(token);
                }
                Event::Peer(
                    PeerEvent::Denied { path: denied } | PeerEvent::Failed { path: denied },
                ) => {
                    if denied != path {
                        return Err(protocol());
                    }
                    observe(Some(QueueObservation::Denied))?;
                    return Err(Error::Remote(
                        "Soulseek peer rejected or failed the selected upload",
                    ));
                }
                Event::Peer(PeerEvent::Position {
                    path: queued,
                    place,
                }) => {
                    if queued != path {
                        return Err(protocol());
                    }
                    observe(Some(QueueObservation::Position(place)))?;
                }
                _ => return Err(protocol()),
            }
        }
    }

    /// Call only after durable acceptance. On a failed write, keep that record
    /// for reconciliation; never assume the peer did not receive acceptance.
    pub fn accept_upload(&mut self, token: u32) -> Result<()> {
        if self.pending != Some(token) {
            return Err(Error::Conflict(
                "Soulseek upload token has no pending offer".into(),
            ));
        }
        let stream = self
            .stream
            .take()
            .ok_or_else(|| Error::Conflict("Soulseek peer connection is closed".into()))?;
        let mut body = token.to_le_bytes().to_vec();
        body.push(1);
        stream.send(41, &body)?;
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
        server: &mut Channel,
        token: u32,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<PeerFile> {
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        let event = server.receive(deadline, check, |event| {
            matches!(
                event,
                Event::Server(ServerEvent::Connect { .. } | ServerEvent::CannotConnect(_))
            )
        })?;
        match event {
            Event::Server(ServerEvent::Connect {
                username,
                kind,
                address,
                token: indirect_token,
            }) => {
                if username != self.peer.username || kind != "F" {
                    return Err(Error::Remote(
                        "Soulseek file connection request differs from the selected peer or type",
                    ));
                }
                if address != self.peer.address {
                    return Err(Error::Remote(
                        "Soulseek file connection address changed after peer selection",
                    ));
                }
                check()?;
                server.check()?;
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or(Error::Remote("Soulseek operation exceeded its time limit"))?;
                let mut stream = TcpStream::connect_timeout(
                    &self.peer.address.into(),
                    remaining.min(IO_TIMEOUT),
                )
                .map_err(Error::SoulseekTransport)?;
                check()?;
                stream.set_write_timeout(Some(IO_TIMEOUT))?;
                send_init(&mut stream, 0, &indirect_token.to_le_bytes()).map_err(network_error)?;
                check()?;
                Ok(PeerFile {
                    peer: self.peer.username.clone(),
                    token,
                    stream,
                })
            }
            Event::Server(ServerEvent::CannotConnect(connection_token)) => {
                let _ = connection_token;
                Err(Error::Remote(
                    "Soulseek sent an unsolicited connection failure token",
                ))
            }
            _ => Err(protocol()),
        }
    }
}

impl SoulseekSession {
    fn query_address(stream: &mut Channel, peer: &str) -> Result<PeerAddress> {
        let mut body = Vec::new();
        string(&mut body, peer);
        stream.send(3, &body)?;
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        let event = stream.receive(deadline, &mut || Ok(()), |event| {
            matches!(event, Event::Server(ServerEvent::Address { .. }))
        })?;
        if let Event::Server(ServerEvent::Address { username, address }) = event {
            if username != peer {
                return Err(protocol());
            }
            if address.ip().is_unspecified() || address.port() == 0 {
                return Err(Error::Remote(
                    "Soulseek peer is offline or has no reachable port",
                ));
            }
            if !public_peer_ip(*address.ip()) {
                return Err(Error::Remote(
                    "Soulseek peer address is not a public unicast IPv4 address",
                ));
            }
            Ok(PeerAddress {
                username: peer.into(),
                address,
            })
        } else {
            Err(protocol())
        }
    }
}

pub(crate) enum QueueObservation {
    Position(u32),
    Offer(u32),
    Denied,
}

pub(crate) fn network_error(error: Error) -> Error {
    match error {
        Error::Io(error) => Error::SoulseekTransport(error),
        other => other,
    }
}

/// A worker-run budget, not permission to replay any operation. Authentication,
/// protocol, storage and approval failures are never transient network failures.
#[derive(Default)]
pub(crate) struct ReconnectBudget {
    attempts: u8,
}

impl ReconnectBudget {
    pub(crate) fn exhausted(&self) -> bool {
        self.attempts == 3
    }

    pub(crate) fn next(&mut self, error: &Error) -> Option<Duration> {
        let Error::SoulseekTransport(error) = error else {
            return None;
        };
        if self.attempts >= 3
            || !matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::NotConnected
            )
        {
            return None;
        }
        let delay = Duration::from_secs(1 << self.attempts);
        self.attempts += 1;
        Some(delay)
    }
}

impl PeerControl<'_> {
    pub(crate) fn binding(&self) -> Result<(u64, u64)> {
        let channel = self
            .stream
            .as_ref()
            .ok_or_else(|| Error::Conflict("Soulseek peer connection is closed".into()))?;
        channel.check()?;
        Ok((channel.generation(), channel.identity()))
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
