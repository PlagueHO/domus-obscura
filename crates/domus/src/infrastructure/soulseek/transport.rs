//! A session-owned I/O loop. Sockets never have competing readers or writers.
//! Raw file sockets are not registered here; ownership passes to the receiver.
use super::messages::{Event, Kind, decode};
use super::protocol::{MAX_FRAME, protocol, send, send_init};
use super::{IO_TIMEOUT, OPERATION_TIMEOUT};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_CONNECTIONS: usize = 34;
const MAX_COMMANDS: usize = 64;
const MAX_EVENTS: usize = 128;
const MAX_BUFFERED_EVENTS: usize = 8 * MAX_FRAME;
const POLL: Duration = Duration::from_millis(10);
const KEEPALIVE: Duration = Duration::from_secs(60);
static GENERATION: AtomicU64 = AtomicU64::new(1);

struct Envelope {
    event: Option<Event>,
    bytes: usize,
    budget: Arc<AtomicUsize>,
}

impl Drop for Envelope {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

struct State {
    error: Mutex<Option<Error>>,
    borrowers: AtomicUsize,
}

struct ReusedPeer {
    id: u64,
    route: u64,
    state: Arc<State>,
    writer: Arc<Mutex<()>>,
}

enum Command {
    ReusePeer {
        peer: String,
        address: std::net::SocketAddrV4,
        events: mpsc::SyncSender<Envelope>,
        reply: mpsc::SyncSender<Result<Option<ReusedPeer>>>,
    },
    EnableSharing(u64),
    ExpectControl {
        id: u64,
        route: u64,
        enabled: bool,
    },
    Subscribe {
        id: u64,
        peer: String,
        sharing: bool,
        protected: bool,
        events: mpsc::SyncSender<Envelope>,
        reply: mpsc::SyncSender<Result<u64>>,
    },
    Unsubscribe {
        id: u64,
        route: u64,
    },
    Add {
        stream: TcpStream,
        kind: Kind,
        protected: bool,
        peer: Option<String>,
        writer: Arc<Mutex<()>>,
        state: Arc<State>,
        events: mpsc::SyncSender<Envelope>,
        reply: mpsc::SyncSender<Result<u64>>,
    },
    Send {
        id: u64,
        bytes: Vec<u8>,
        timeout: Duration,
        reply: mpsc::SyncSender<Result<()>>,
    },
    Remove(u64),
}

struct Runtime {
    commands: mpsc::SyncSender<Command>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    generation: u64,
    failure: Arc<Mutex<Option<SessionFailure>>>,
    sharing: Mutex<Option<mpsc::SyncSender<super::sharing::SharedPeer>>>,
}

#[derive(Clone, Copy)]
enum SessionFailure {
    Transport(std::io::ErrorKind),
    Protocol(&'static str),
}

impl SessionFailure {
    fn from_error(error: &Error) -> Self {
        match error {
            Error::SoulseekTransport(error) => Self::Transport(error.kind()),
            Error::Remote(message) => Self::Protocol(message),
            _ => Self::Protocol("Soulseek connection failed"),
        }
    }

    fn error(self) -> Error {
        match self {
            Self::Transport(kind) => Error::SoulseekTransport(kind.into()),
            Self::Protocol(message) => Error::Remote(message),
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            // The loop performs nonblocking I/O only and observes stop on every
            // pass. Joining cannot wait for an untrusted peer to send bytes.
            if thread.join().is_err() {
                eprintln!("Soulseek I/O owner panicked during shutdown");
            }
        }
    }
}

pub(super) struct Channel {
    runtime: Arc<Runtime>,
    id: u64,
    events: mpsc::Receiver<Envelope>,
    pending: VecDeque<Envelope>,
    state: Arc<State>,
    route: Option<u64>,
    writer: Arc<Mutex<()>>,
}

impl Drop for Channel {
    fn drop(&mut self) {
        let command = match self.route {
            Some(route) => Command::Unsubscribe { id: self.id, route },
            None => Command::Remove(self.id),
        };
        // A full command queue must not retain an orphan socket.
        if self.runtime.commands.try_send(command).is_err() {
            self.runtime.stop.store(true, Ordering::Release);
        }
    }
}

impl Channel {
    pub(super) fn server(stream: TcpStream) -> Result<Self> {
        Self::new_runtime(KEEPALIVE)?.add(stream, Kind::Server, true)
    }

    #[cfg(test)]
    fn server_with_keepalive(stream: TcpStream, interval: Duration) -> Result<Self> {
        Self::new_runtime(interval)?.add(stream, Kind::Server, true)
    }

    fn new_runtime(keepalive: Duration) -> Result<Arc<Runtime>> {
        let (commands, receiver) = mpsc::sync_channel(MAX_COMMANDS);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let failure = Arc::new(Mutex::new(None));
        let failed = Arc::clone(&failure);
        let thread = thread::Builder::new()
            .name("domus-soulseek-io".into())
            .spawn(move || drive(receiver, stopping, failed, keepalive))?;
        Ok(Arc::new(Runtime {
            commands,
            stop,
            thread: Some(thread),
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            failure,
            sharing: Mutex::new(None),
        }))
    }

    pub(super) fn peer(&self, stream: TcpStream) -> Result<Self> {
        let sharing = self
            .runtime
            .sharing
            .lock()
            .map_err(|_| Error::Internal("sharing route poisoned"))?
            .is_some();
        self.runtime.add(
            stream,
            if sharing {
                Kind::SharingPeer
            } else {
                Kind::Peer
            },
            true,
        )
    }

    pub(super) fn sharing_peer_for(&self, stream: TcpStream, peer: &str) -> Result<Self> {
        self.runtime
            .add_named(stream, Kind::SharingPeer, false, Some(peer.into()))
    }

    pub(super) fn peer_for(
        &self,
        stream: TcpStream,
        peer: &str,
        address: std::net::SocketAddrV4,
    ) -> Result<Self> {
        let sharing = self
            .runtime
            .sharing
            .lock()
            .map_err(|_| Error::Internal("sharing route poisoned"))?
            .is_some();
        match self.runtime.add_named(
            stream,
            if sharing {
                Kind::SharingPeer
            } else {
                Kind::Peer
            },
            true,
            Some(peer.into()),
        ) {
            Err(Error::Busy("Soulseek peer control is already connected")) => self
                .reuse_peer(peer, address)?
                .ok_or(Error::Busy("Soulseek peer control changed during handover")),
            result => result,
        }
    }

    pub(super) fn reuse_peer(
        &self,
        peer: &str,
        address: std::net::SocketAddrV4,
    ) -> Result<Option<Self>> {
        let (events, receiver) = mpsc::sync_channel(MAX_EVENTS);
        let (reply, result) = mpsc::sync_channel(1);
        self.runtime.command(Command::ReusePeer {
            peer: peer.into(),
            address,
            events,
            reply,
        })?;
        Ok(result
            .recv_timeout(IO_TIMEOUT)
            .map_err(|_| Error::Remote("Soulseek peer reuse timed out"))??
            .map(|peer| Self {
                runtime: self.runtime.clone(),
                id: peer.id,
                route: Some(peer.route),
                state: peer.state,
                writer: peer.writer,
                events: receiver,
                pending: VecDeque::new(),
            }))
    }

    pub(super) fn acquisition_in_use(&self) -> bool {
        self.state.borrowers.load(Ordering::Acquire) > 0
    }

    pub(super) fn enable_sharing(&self) -> Result<()> {
        self.runtime.command(Command::EnableSharing(self.id))
    }

    pub(super) fn share_connections(
        &self,
        sender: mpsc::SyncSender<super::sharing::SharedPeer>,
    ) -> Result<()> {
        *self
            .runtime
            .sharing
            .lock()
            .map_err(|_| Error::Internal("sharing route poisoned"))? = Some(sender);
        Ok(())
    }

    pub(super) fn sharing_sender(
        &self,
    ) -> Result<Option<mpsc::SyncSender<super::sharing::SharedPeer>>> {
        Ok(self
            .runtime
            .sharing
            .lock()
            .map_err(|_| Error::Internal("sharing route poisoned"))?
            .clone())
    }

    pub(super) fn shared_control(&self) -> bool {
        self.route.is_some()
    }

    pub(super) fn expect_control(&self, enabled: bool) -> Result<()> {
        if let Some(route) = self.route {
            self.runtime.command(Command::ExpectControl {
                id: self.id,
                route,
                enabled,
            })?;
        }
        Ok(())
    }

    pub(super) fn share_control(&self, peer: &str, address: std::net::SocketAddrV4) -> Result<()> {
        let sender = self
            .runtime
            .sharing
            .lock()
            .map_err(|_| Error::Internal("sharing route poisoned"))?
            .clone();
        if let Some(sender) = sender {
            sender
                .try_send(super::sharing::SharedPeer {
                    username: peer.into(),
                    address,
                    channel: self.subscribe_role(peer, true)?,
                })
                .map_err(|_| Error::Busy("sharing control handoff is full"))?;
        }
        Ok(())
    }

    pub(super) fn subscribe(&self, peer: &str) -> Result<Self> {
        self.subscribe_role(peer, false)
    }

    pub(super) fn metadata_subscription(&self, peer: &str) -> Result<Self> {
        self.subscribe_bounded(peer, false, false)
    }

    fn subscribe_role(&self, peer: &str, sharing: bool) -> Result<Self> {
        self.subscribe_bounded(peer, sharing, true)
    }

    fn subscribe_bounded(&self, peer: &str, sharing: bool, protected: bool) -> Result<Self> {
        self.check()?;
        let (events, receiver) = mpsc::sync_channel(MAX_EVENTS);
        let (reply, result) = mpsc::sync_channel(1);
        self.runtime.command(Command::Subscribe {
            id: self.id,
            peer: peer.into(),
            sharing,
            protected,
            events,
            reply,
        })?;
        let route = result
            .recv_timeout(IO_TIMEOUT)
            .map_err(|_| Error::Remote("Soulseek subscription timed out"))??;
        Ok(Self {
            runtime: self.runtime.clone(),
            id: self.id,
            events: receiver,
            pending: VecDeque::new(),
            state: self.state.clone(),
            route: Some(route),
            writer: self.writer.clone(),
        })
    }

    pub(super) fn try_event(&mut self) -> Result<Option<Event>> {
        self.check()?;
        if let Some(mut envelope) = self.pending.pop_front() {
            return Ok(envelope.event.take());
        }
        match self.events.try_recv() {
            Ok(mut envelope) => Ok(envelope.event.take()),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.check()?;
                Err(Error::Remote("Soulseek event route is closed"))
            }
        }
    }

    pub(super) fn send_stream(
        &self,
        code: u32,
        bytes: u32,
        input: &mut impl Read,
        buffer_bytes: usize,
        timeout: Duration,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        if buffer_bytes == 0 || buffer_bytes > 262_144 {
            return Err(Error::InvalidInput(
                "Soulseek streaming buffer is outside its bounds".into(),
            ));
        }
        let _writer = self
            .writer
            .lock()
            .map_err(|_| Error::Internal("Soulseek writer poisoned"))?;
        let result = (|| {
            check()?;
            let length = bytes.checked_add(4).ok_or(protocol())?;
            self.write_with_timeout([length.to_le_bytes(), code.to_le_bytes()].concat(), timeout)?;
            let mut buffer = vec![0; buffer_bytes];
            let mut left = u64::from(bytes);
            while left > 0 {
                check()?;
                let wanted = left.min(buffer.len() as u64) as usize;
                input.read_exact(&mut buffer[..wanted])?;
                self.write_with_timeout(buffer[..wanted].to_vec(), timeout)?;
                left -= wanted as u64;
            }
            check()?;
            Ok(())
        })();
        if result.is_err() {
            self.runtime.command(Command::Remove(self.id))?;
        }
        result
    }

    #[cfg(test)]
    pub(super) fn isolated_peer(stream: TcpStream) -> Result<Self> {
        Self::new_runtime(KEEPALIVE)?.add(stream, Kind::Peer, true)
    }

    pub(super) fn generation(&self) -> u64 {
        self.runtime.generation
    }

    pub(super) fn identity(&self) -> u64 {
        self.id
    }

    pub(super) fn check(&self) -> Result<()> {
        let failure = self
            .state
            .error
            .lock()
            .map_err(|_| Error::Internal("Soulseek connection state poisoned"))?
            .as_ref()
            .map(SessionFailure::from_error);
        // A server disconnect must not make a known peer protocol failure retryable.
        if let Some(failure @ SessionFailure::Protocol(_)) = failure {
            return Err(failure.error());
        }
        self.runtime.check()?;
        if let Some(failure) = failure {
            return Err(failure.error());
        }
        Ok(())
    }

    pub(super) fn send(&self, code: u32, body: &[u8]) -> Result<()> {
        let _writer = self
            .writer
            .lock()
            .map_err(|_| Error::Internal("Soulseek writer poisoned"))?;
        let mut bytes = Vec::new();
        send(&mut bytes, code, body)?;
        self.write(bytes)
    }

    pub(super) fn init(&self, code: u8, body: &[u8]) -> Result<()> {
        let _writer = self
            .writer
            .lock()
            .map_err(|_| Error::Internal("Soulseek writer poisoned"))?;
        let mut bytes = Vec::new();
        send_init(&mut bytes, code, body)?;
        self.write(bytes)
    }

    fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.write_with_timeout(bytes, IO_TIMEOUT)
    }

    fn write_with_timeout(&self, bytes: Vec<u8>, timeout: Duration) -> Result<()> {
        self.check()?;
        let (reply, result) = mpsc::sync_channel(1);
        self.runtime.command(Command::Send {
            id: self.id,
            bytes,
            timeout,
            reply,
        })?;
        let outcome = match result.recv_timeout(timeout + Duration::from_secs(1)) {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::SoulseekTransport(
                std::io::ErrorKind::TimedOut.into(),
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Remote(
                "Soulseek I/O owner closed before write completion",
            )),
        };
        if outcome.is_err() {
            self.check()?;
        }
        outcome
    }

    pub(super) fn receive(
        &mut self,
        deadline: Instant,
        check: &mut impl FnMut() -> Result<()>,
        matches: impl Fn(&Event) -> bool,
    ) -> Result<Event> {
        loop {
            check()?;
            if Instant::now() >= deadline {
                return Err(Error::Remote("Soulseek operation exceeded its time limit"));
            }
            if let Some(position) = self
                .pending
                .iter()
                .position(|event| event.event.as_ref().is_some_and(&matches))
            {
                let mut envelope = self.pending.remove(position).ok_or(protocol())?;
                return envelope.event.take().ok_or(protocol());
            }
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100));
            match self.events.recv_timeout(wait) {
                Ok(mut envelope) => {
                    check()?;
                    if Instant::now() < deadline && envelope.event.as_ref().is_some_and(&matches) {
                        return envelope.event.take().ok_or(protocol());
                    }
                    if self.pending.len() >= MAX_EVENTS {
                        return Err(Error::Remote("Soulseek pending routing limit exceeded"));
                    }
                    self.pending.push_back(envelope);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.runtime.check()?;
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self
                        .state
                        .error
                        .lock()
                        .map_err(|_| Error::Internal("Soulseek connection state poisoned"))?
                        .as_ref()
                        .map(|error| SessionFailure::from_error(error).error())
                        .unwrap_or(Error::Remote(
                            "Soulseek I/O owner closed without connection evidence",
                        )));
                }
            }
        }
    }
}

impl Runtime {
    fn check(&self) -> Result<()> {
        match *self
            .failure
            .lock()
            .map_err(|_| Error::Internal("Soulseek session state poisoned"))?
        {
            Some(failure) => Err(failure.error()),
            None => Ok(()),
        }
    }

    fn command(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .map_err(|_| Error::Remote("Soulseek command queue is full or closed"))
    }

    fn add(self: &Arc<Self>, stream: TcpStream, kind: Kind, protected: bool) -> Result<Channel> {
        self.add_named(stream, kind, protected, None)
    }

    fn add_named(
        self: &Arc<Self>,
        stream: TcpStream,
        kind: Kind,
        protected: bool,
        peer: Option<String>,
    ) -> Result<Channel> {
        self.check()?;
        stream.set_nonblocking(true)?;
        let (events, receiver) = mpsc::sync_channel(MAX_EVENTS);
        let state = Arc::new(State {
            error: Mutex::new(None),
            borrowers: AtomicUsize::new(0),
        });
        let writer = Arc::new(Mutex::new(()));
        let (reply, result) = mpsc::sync_channel(1);
        self.command(Command::Add {
            stream,
            kind,
            protected,
            peer,
            writer: writer.clone(),
            state: Arc::clone(&state),
            events,
            reply,
        })?;
        let id = result
            .recv_timeout(IO_TIMEOUT)
            .map_err(|_| Error::Remote("Soulseek connection registration timed out"))??;
        Ok(Channel {
            runtime: Arc::clone(self),
            id,
            events: receiver,
            state,
            pending: VecDeque::new(),
            route: None,
            writer,
        })
    }
}

struct Output {
    bytes: Vec<u8>,
    position: usize,
    started: Instant,
    timeout: Duration,
    reply: Option<mpsc::SyncSender<Result<()>>>,
}

struct Connection {
    stream: TcpStream,
    kind: Kind,
    protected: bool,
    peer: Option<String>,
    writer: Arc<Mutex<()>>,
    state: Arc<State>,
    events: mpsc::SyncSender<Envelope>,
    input: Vec<u8>,
    expected: usize,
    last_byte: Instant,
    started: Instant,
    outputs: VecDeque<Output>,
    ping: Instant,
    notification_window: Instant,
    notifications: usize,
    connection_tokens: BTreeSet<(String, String, u32)>,
    offers: BTreeMap<u32, (String, u64)>,
    routes: BTreeMap<u64, (String, bool, bool, mpsc::SyncSender<Envelope>)>,
    sharing: bool,
    awaiting_sharing: bool,
}

impl Connection {
    fn poll(&mut self, budget: &Arc<AtomicUsize>, keepalive: Duration) -> Result<()> {
        let now = Instant::now();
        if matches!(self.kind, Kind::Server) && now.duration_since(self.ping) >= keepalive {
            let mut bytes = Vec::new();
            send(&mut bytes, 32, &[])?;
            self.outputs.push_back(Output {
                bytes,
                position: 0,
                started: now,
                timeout: IO_TIMEOUT,
                reply: None,
            });
            self.ping = now;
        }
        if let Some(output) = self.outputs.front_mut() {
            if now.duration_since(output.started) >= output.timeout {
                return Err(Error::SoulseekTransport(
                    std::io::ErrorKind::TimedOut.into(),
                ));
            }
            match self.stream.write(&output.bytes[output.position..]) {
                Ok(0) => {
                    return Err(Error::SoulseekTransport(
                        std::io::ErrorKind::UnexpectedEof.into(),
                    ));
                }
                Ok(count) => output.position += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(Error::SoulseekTransport(error)),
            }
            if output.position == output.bytes.len()
                && let Some(output) = self.outputs.pop_front()
                && let Some(reply) = output.reply
            {
                let _ = reply.try_send(Ok(()));
            }
        }
        // One frame per connection per pass keeps a noisy peer from starving
        // server maintenance or another connection's deadline.
        if self.awaiting_sharing {
            if now.duration_since(self.started) >= IO_TIMEOUT {
                return Err(Error::Remote("Soulseek sharing handoff timed out"));
            }
            return Ok(());
        }
        let mut buffer = [0; 8192];
        let wanted = (self.expected - self.input.len()).min(buffer.len());
        match self.stream.read(&mut buffer[..wanted]) {
            Ok(0) => {
                return Err(Error::SoulseekTransport(
                    std::io::ErrorKind::UnexpectedEof.into(),
                ));
            }
            Ok(count) => {
                if self.input.is_empty() {
                    self.started = now;
                }
                self.last_byte = now;
                self.input.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(Error::SoulseekTransport(error)),
        }
        if !self.input.is_empty()
            && (now.duration_since(self.last_byte) >= IO_TIMEOUT
                || now.duration_since(self.started) >= OPERATION_TIMEOUT)
        {
            return Err(Error::Remote("Soulseek partial frame timed out"));
        }
        if self.input.len() == 4 && self.expected == 4 {
            let size =
                u32::from_le_bytes(self.input[..4].try_into().map_err(|_| protocol())?) as usize;
            if !(4..=MAX_FRAME).contains(&size) {
                return Err(protocol());
            }
            self.expected = size + 4;
        }
        if self.input.len() == self.expected && self.expected > 4 {
            let bytes = std::mem::take(&mut self.input);
            self.expected = 4;
            let code = u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| protocol())?);
            let mut event = decode(self.kind, code, bytes[8..].to_vec())?;
            if !self.sharing
                && matches!(
                    event,
                    Some(Event::Server(super::messages::ServerEvent::Search { .. }))
                )
            {
                event = None;
            }
            if let Some(Event::Peer(super::messages::PeerEvent::Offer { token, path, bytes })) =
                &event
            {
                if let Some((previous_path, previous_bytes)) = self.offers.get(token) {
                    if previous_path != path || previous_bytes != bytes {
                        return Err(Error::Remote(
                            "Soulseek transfer token was reused for a different offer",
                        ));
                    }
                    // Identical retransmitted offers are consumed once, not
                    // forwarded into the next selected file's negotiation.
                    event = None;
                } else {
                    if self.offers.len() >= 4096 {
                        return Err(Error::Remote("Soulseek transfer token limit exceeded"));
                    }
                    self.offers.insert(*token, (path.clone(), *bytes));
                }
            }
            if let Some(event) = event {
                if let Event::Server(super::messages::ServerEvent::Connect {
                    username,
                    kind,
                    token,
                    ..
                }) = &event
                    && (self.connection_tokens.len() >= 4096
                        || !self
                            .connection_tokens
                            .insert((username.clone(), kind.clone(), *token)))
                {
                    return Err(Error::Remote(
                        "Soulseek duplicate connection token or token limit exceeded",
                    ));
                }
                let routes = self
                    .routes
                    .values()
                    .filter(|(peer, sharing, control, _)| match &event {
                        Event::Server(super::messages::ServerEvent::Address {
                            username, ..
                        }) => !sharing && username == peer,
                        Event::Server(super::messages::ServerEvent::Connect {
                            username,
                            kind,
                            ..
                        }) => {
                            !sharing
                                && username == peer
                                && (kind == "F" || (*control && kind == "P"))
                        }
                        Event::Peer(
                            super::messages::PeerEvent::ShareList
                            | super::messages::PeerEvent::FolderRequest { .. }
                            | super::messages::PeerEvent::Queue { .. }
                            | super::messages::PeerEvent::QueueRequest { .. }
                            | super::messages::PeerEvent::DownloadRequest { .. }
                            | super::messages::PeerEvent::UploadResponse { .. },
                        ) => *sharing,
                        Event::Peer(_) => !sharing,
                        _ => false,
                    })
                    .map(|(_, _, _, sender)| sender)
                    .collect::<Vec<_>>();
                if routes.is_empty()
                    && self.sharing
                    && matches!(
                        event,
                        Event::Peer(
                            super::messages::PeerEvent::ShareList
                                | super::messages::PeerEvent::FolderRequest { .. }
                                | super::messages::PeerEvent::Queue { .. }
                                | super::messages::PeerEvent::QueueRequest { .. }
                                | super::messages::PeerEvent::DownloadRequest { .. }
                                | super::messages::PeerEvent::UploadResponse { .. }
                        )
                    )
                {
                    return Err(Error::Remote(
                        "Soulseek sharing control handler is unavailable",
                    ));
                }
                let destinations = if routes.is_empty() {
                    vec![&self.events]
                } else {
                    routes
                };
                for destination in destinations {
                    let count = bytes.len() + 64;
                    let previous = budget.fetch_add(count, Ordering::Relaxed);
                    let envelope = Envelope {
                        event: Some(event.clone()),
                        bytes: count,
                        budget: budget.clone(),
                    };
                    let maximum =
                        if self.protected || self.state.borrowers.load(Ordering::Acquire) > 0 {
                            MAX_BUFFERED_EVENTS
                        } else {
                            MAX_BUFFERED_EVENTS - 2 * MAX_FRAME
                        };
                    if previous + count > maximum {
                        return Err(Error::Remote("Soulseek event byte limit exceeded"));
                    }
                    destination
                        .try_send(envelope)
                        .map_err(|_| Error::Remote("Soulseek event queue is full or closed"))?;
                }
            } else {
                if now.duration_since(self.notification_window) >= OPERATION_TIMEOUT {
                    self.notifications = 0;
                    self.notification_window = now;
                }
                self.notifications += 1;
                if self.notifications >= MAX_EVENTS {
                    return Err(Error::Remote("Soulseek notification rate limit exceeded"));
                }
            }
        }
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn drive(
    commands: mpsc::Receiver<Command>,
    stop: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<SessionFailure>>>,
    keepalive: Duration,
) {
    let mut connections: BTreeMap<u64, Connection> = BTreeMap::new();
    let mut next = 0u64;
    let budget = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::Acquire) {
        for _ in 0..MAX_COMMANDS {
            let Ok(command) = commands.try_recv() else {
                break;
            };
            match command {
                Command::ReusePeer {
                    peer,
                    address,
                    events,
                    reply,
                } => {
                    let result = if let Some((id, connection)) =
                        connections.iter_mut().find(|(_, connection)| {
                            !connection.protected
                                && connection.peer.as_ref() == Some(&peer)
                                && connection.state.borrowers.load(Ordering::Acquire) == 0
                                && connection.stream.peer_addr().is_ok_and(|remote| {
                                    remote.ip() == std::net::IpAddr::V4(*address.ip())
                                })
                        }) {
                        match connection.writer.try_lock() {
                            Ok(_writer) => {
                                next += 1;
                                connection.routes.insert(next, (peer, false, false, events));
                                connection.state.borrowers.fetch_add(1, Ordering::AcqRel);
                                Ok(Some(ReusedPeer {
                                    id: *id,
                                    route: next,
                                    state: connection.state.clone(),
                                    writer: connection.writer.clone(),
                                }))
                            }
                            Err(std::sync::TryLockError::WouldBlock) => {
                                Err(Error::Busy("Soulseek peer control is writing a frame"))
                            }
                            Err(std::sync::TryLockError::Poisoned(_)) => {
                                Err(Error::Internal("Soulseek writer poisoned"))
                            }
                        }
                    } else {
                        Ok(None)
                    };
                    let id = result
                        .as_ref()
                        .ok()
                        .and_then(|peer| peer.as_ref().map(|peer| peer.id));
                    if reply.try_send(result).is_err()
                        && let Some(id) = id
                    {
                        connections.remove(&id);
                    }
                }
                Command::EnableSharing(id) => {
                    if let Some(connection) = connections.get_mut(&id) {
                        connection.sharing = true;
                    }
                }
                Command::ExpectControl { id, route, enabled } => {
                    if let Some(connection) = connections.get_mut(&id)
                        && let Some((_, _, control, _)) = connection.routes.get_mut(&route)
                    {
                        *control = enabled;
                    }
                }
                Command::Subscribe {
                    id,
                    peer,
                    sharing,
                    protected,
                    events,
                    reply,
                } => {
                    let result = match connections.get_mut(&id) {
                        Some(connection)
                            if connection.routes.len() < if protected { 24 } else { 20 } =>
                        {
                            next += 1;
                            if sharing {
                                connection.sharing = true;
                                connection.awaiting_sharing = false;
                            }
                            connection
                                .routes
                                .insert(next, (peer, sharing, false, events));
                            Ok(next)
                        }
                        _ => Err(Error::Busy("Soulseek server route limit exceeded")),
                    };
                    let _ = reply.try_send(result);
                }
                Command::Unsubscribe { id, route } => {
                    if let Some(connection) = connections.get_mut(&id)
                        && let Some((_, false, _, _)) = connection.routes.remove(&route)
                        && matches!(connection.kind, Kind::Peer | Kind::SharingPeer)
                    {
                        connection.state.borrowers.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                Command::Add {
                    stream,
                    kind,
                    protected,
                    peer,
                    writer,
                    state,
                    events,
                    reply,
                } => {
                    if peer.as_ref().is_some_and(|peer| {
                        connections
                            .values()
                            .any(|connection| connection.peer.as_ref() == Some(peer))
                    }) {
                        let _ = reply.try_send(Err(Error::Busy(
                            "Soulseek peer control is already connected",
                        )));
                        continue;
                    }
                    if connections.len() >= MAX_CONNECTIONS
                        || (!protected
                            && matches!(kind, Kind::SharingPeer)
                            && connections.len() >= MAX_CONNECTIONS - 3)
                    {
                        let _ = reply
                            .try_send(Err(Error::Remote("Soulseek connection limit exceeded")));
                        continue;
                    }

                    next += 1;
                    let now = Instant::now();
                    connections.insert(
                        next,
                        Connection {
                            stream,
                            kind,
                            protected,
                            peer,
                            writer,
                            state,
                            events,
                            input: Vec::new(),
                            expected: 4,
                            last_byte: now,
                            started: now,
                            outputs: VecDeque::new(),
                            ping: now,
                            notification_window: now,
                            notifications: 0,
                            connection_tokens: BTreeSet::new(),
                            offers: BTreeMap::new(),
                            routes: BTreeMap::new(),
                            sharing: false,
                            awaiting_sharing: matches!(kind, Kind::SharingPeer) && protected,
                        },
                    );
                    if reply.try_send(Ok(next)).is_err() {
                        connections.remove(&next);
                    }
                }
                Command::Send {
                    id,
                    bytes,
                    timeout,
                    reply,
                } => {
                    if let Some(connection) = connections.get_mut(&id) {
                        if connection.outputs.len() >= MAX_COMMANDS
                            || connection
                                .outputs
                                .iter()
                                .map(|output| output.bytes.len())
                                .sum::<usize>()
                                + bytes.len()
                                > 2 * MAX_FRAME
                        {
                            let _ = reply.try_send(Err(Error::Remote(
                                "Soulseek write queue limit exceeded",
                            )));
                        } else {
                            connection.outputs.push_back(Output {
                                bytes,
                                position: 0,
                                started: Instant::now(),
                                timeout,
                                reply: Some(reply),
                            });
                        }
                    } else {
                        let _ = reply.try_send(Err(Error::SoulseekTransport(
                            std::io::ErrorKind::NotConnected.into(),
                        )));
                    }
                }
                Command::Remove(id) => {
                    connections.remove(&id);
                }
            }
        }
        connections.retain(|_, connection| {
            if matches!(connection.kind, Kind::Peer | Kind::SharingPeer)
                && let Ok(failed) = failure.lock()
                && let Some(failed) = *failed
            {
                if let Ok(mut saved) = connection.state.error.lock() {
                    *saved = Some(failed.error());
                }
                return false;
            }
            if let Err(error) = connection.poll(&budget, keepalive) {
                if matches!(connection.kind, Kind::Server)
                    && let Ok(mut failure) = failure.lock()
                {
                    *failure = Some(SessionFailure::from_error(&error));
                }
                if let Ok(mut saved) = connection.state.error.lock() {
                    *saved = Some(error);
                }
                false
            } else {
                true
            }
        });
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests;
