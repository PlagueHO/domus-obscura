//! One account owner for read-only shares and the existing acquisition adapter.
//! Only the framed transport reads P/server sockets. F sockets have one owner.
use super::messages::{Event, PeerEvent, ServerEvent};
use super::protocol::{Fields, send_init, string};
use super::transport::Channel;
use super::{IO_TIMEOUT, SoulseekSession, public_peer_ip};
use crate::domain::sharing::{ShareAccess, ShareLimits, SharePolicy};
use crate::infrastructure::local_file::ProcessLock;
use crate::infrastructure::shares::{ShareCatalog, SharedFile};
use crate::infrastructure::sharing::{SharingAccount, SharingConfiguration};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

static TOKEN: AtomicU32 = AtomicU32::new(1);

fn token() -> Result<u32> {
    TOKEN
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| {
            Error::Conflict("Soulseek sharing token space exhausted; restart the worker".into())
        })
}

pub(super) struct SharedPeer {
    pub(super) username: String,
    pub(super) address: SocketAddrV4,
    pub(super) channel: Channel,
}

pub(crate) struct Account {
    session: Mutex<SoulseekSession>,
    configuration: SharingAccount,
    database: PathBuf,
    username: String,
    _lock: ProcessLock,
    reconnect: Mutex<super::ReconnectBudget>,
    advertisement: Mutex<(u32, u32, Option<u16>)>,
}

impl Account {
    fn new(database: &Path, configuration: SharingAccount) -> Result<Arc<Self>> {
        let credentials =
            crate::infrastructure::worker::read_credentials(&configuration.credentials_file)?;
        let lock = crate::infrastructure::worker::account_lock(database, &credentials)?;
        let socket = TcpStream::connect_timeout(&configuration.server.into(), IO_TIMEOUT)
            .map_err(Error::SoulseekTransport)?;
        let session = SoulseekSession::login(socket, &credentials.username, &credentials.password)?;
        Ok(Arc::new(Self {
            session: Mutex::new(session),
            configuration,
            database: database.into(),
            username: credentials.username,
            _lock: lock,
            reconnect: Mutex::new(super::ReconnectBudget::default()),
            advertisement: Mutex::new((0, 0, None)),
        }))
    }

    pub(crate) fn session(
        &self,
        peer: &str,
        server: SocketAddrV4,
        username: &str,
    ) -> Result<SoulseekSession> {
        if server != self.configuration.server || username != self.username {
            return Err(Error::Conflict(
                "acquisition and sharing account settings differ".into(),
            ));
        }
        self.session
            .lock()
            .map_err(|_| Error::Internal("sharing account lock poisoned"))?
            .fork_for_peer(peer)
    }

    fn fork(&self, peer: &str) -> Result<SoulseekSession> {
        self.session
            .lock()
            .map_err(|_| Error::Internal("sharing account lock poisoned"))?
            .fork_for_sharing(peer)
    }

    fn send(&self, code: u32, body: &[u8]) -> Result<()> {
        self.session
            .lock()
            .map_err(|_| Error::Internal("sharing account lock poisoned"))?
            .stream
            .as_ref()
            .ok_or(Error::Remote("sharing account is closed"))?
            .send(code, body)
    }

    fn peer_channel(&self, stream: TcpStream, peer: &str) -> Result<Channel> {
        self.session
            .lock()
            .map_err(|_| Error::Internal("sharing account lock poisoned"))?
            .stream
            .as_ref()
            .ok_or(Error::Remote("sharing account is closed"))?
            .sharing_peer_for(stream, peer)
    }

    fn event(&self) -> Result<Option<Event>> {
        self.session
            .lock()
            .map_err(|_| Error::Internal("sharing account lock poisoned"))?
            .stream
            .as_mut()
            .ok_or(Error::Remote("sharing account is closed"))?
            .try_event()
    }

    fn generation(&self) -> Result<u64> {
        Ok(self
            .session
            .lock()
            .map_err(|_| Error::Internal("sharing account lock poisoned"))?
            .stream
            .as_ref()
            .ok_or(Error::Remote("sharing account is closed"))?
            .generation())
    }

    fn reconnect(&self, error: &Error) -> Result<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| Error::Internal("sharing account lock poisoned"))?;
        if session
            .stream
            .as_ref()
            .is_some_and(|stream| stream.check().is_ok())
        {
            return Ok(());
        }
        let delay = self
            .reconnect
            .lock()
            .map_err(|_| Error::Internal("sharing reconnect budget poisoned"))?
            .next(error)
            .ok_or(Error::Remote(
                "sharing reconnect budget exhausted or failure is not retryable",
            ))?;
        std::thread::sleep(delay);
        let store = crate::infrastructure::store::Store::open_read_current(&self.database)?;
        crate::infrastructure::local_file::protect_files(
            &store,
            &[&self.configuration.credentials_file],
        )?;
        let credentials =
            crate::infrastructure::worker::read_credentials(&self.configuration.credentials_file)?;
        if credentials.username != self.username {
            return Err(Error::Conflict(
                "sharing account changed during reconnect".into(),
            ));
        }
        let socket = TcpStream::connect_timeout(&self.configuration.server.into(), IO_TIMEOUT)
            .map_err(Error::SoulseekTransport)?;
        let replacement =
            SoulseekSession::login(socket, &credentials.username, &credentials.password)?;
        let stream = replacement
            .stream
            .as_ref()
            .ok_or(Error::Remote("replacement sharing session is closed"))?;
        stream.enable_sharing()?;
        if let Some(sender) = session
            .stream
            .as_ref()
            .map(Channel::sharing_sender)
            .transpose()?
            .flatten()
        {
            stream.share_connections(sender)?;
        }
        let (directories, files, port) = *self
            .advertisement
            .lock()
            .map_err(|_| Error::Internal("sharing advertisement poisoned"))?;
        stream.send(
            35,
            &[directories.to_le_bytes(), files.to_le_bytes()].concat(),
        )?;
        if let Some(port) = port {
            stream.send(2, &u32::from(port).to_le_bytes())?;
        }
        *session = replacement;
        Ok(())
    }
}

struct Queue {
    waiting: VecDeque<(String, String)>,
    active: BTreeSet<(String, String)>,
    last_peer: Option<String>,
}

impl Queue {
    fn take(&mut self, peer: &str, slots: usize) -> Option<String> {
        if self.active.len() >= slots {
            return None;
        }
        let position = self
            .waiting
            .iter()
            .position(|(peer, _)| Some(peer) != self.last_peer.as_ref())
            .or_else(|| (!self.waiting.is_empty()).then_some(0))?;
        if self.waiting.get(position)?.0 != peer {
            return None;
        }
        let key = self.waiting.remove(position)?;
        self.last_peer = Some(key.0.clone());
        self.active.insert(key.clone());
        Some(key.1)
    }
}

struct PendingFile {
    peer: String,
    address: SocketAddrV4,
    sent: mpsc::SyncSender<Result<TcpStream>>,
    generation: u64,
}

struct Context {
    account: Arc<Account>,
    policy: SharePolicy,
    catalog: Mutex<ShareCatalog>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    queue: Mutex<Queue>,
    pending: Mutex<BTreeMap<u32, PendingFile>>,
    peers: Mutex<BTreeMap<String, u32>>,
    handshakes: AtomicUsize,
    browses: AtomicUsize,
    can_receive: bool,
    searches: Mutex<BTreeMap<String, mpsc::SyncSender<(u32, String)>>>,
}

impl Context {
    fn check(&self) -> Result<()> {
        if self.stop.load(Ordering::Acquire) {
            return Err(Error::WorkerStopping);
        }
        self.account
            .session
            .lock()
            .map_err(|_| Error::Internal("sharing account lock poisoned"))?
            .stream
            .as_ref()
            .ok_or(Error::Remote("sharing account is closed"))?
            .check()
    }

    fn catalog(&self) -> Result<std::sync::MutexGuard<'_, ShareCatalog>> {
        let mut catalog = self
            .catalog
            .lock()
            .map_err(|_| Error::Internal("sharing catalog lock poisoned"))?;
        match catalog.response_guard() {
            Ok(_) => {}
            Err(Error::Conflict(_)) => {
                if let Err(error) = catalog.refresh(&self.policy) {
                    self.account.send(35, &[0; 8])?;
                    let mut advertisement = self
                        .account
                        .advertisement
                        .lock()
                        .map_err(|_| Error::Internal("sharing advertisement poisoned"))?;
                    advertisement.0 = 0;
                    advertisement.1 = 0;
                    return Err(error);
                }
                let counts = catalog.counts()?;
                self.account.send(
                    35,
                    &[counts.0.to_le_bytes(), counts.1.to_le_bytes()].concat(),
                )?;
                let mut advertisement = self
                    .account
                    .advertisement
                    .lock()
                    .map_err(|_| Error::Internal("sharing advertisement poisoned"))?;
                advertisement.0 = counts.0;
                advertisement.1 = counts.1;
            }
            Err(error) => return Err(error),
        }
        Ok(catalog)
    }

    fn check_generation(&self, generation: u64) -> Result<()> {
        self.check()?;
        if self.account.generation()? != generation {
            return Err(Error::Remote("sharing session generation changed"));
        }
        Ok(())
    }
}

/// Keep this owner alive until all acquisition and sharing operations have drained.
pub(crate) struct SharingRuntime {
    pub(crate) account: Arc<Account>,
    context: Arc<Context>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl SharingRuntime {
    pub(crate) fn start(
        database: &Path,
        configuration: &SharingConfiguration,
        acquisition: Option<&crate::infrastructure::worker::LocalAcquisition>,
        stop: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Option<Self>> {
        if configuration.policy.access == ShareAccess::None {
            return Ok(None);
        }
        configuration.policy.validate()?;
        let connection = configuration
            .account
            .clone()
            .or_else(|| {
                acquisition.map(|value| SharingAccount {
                    credentials_file: value.credentials_file.clone(),
                    server: value.server,
                    acknowledge_plaintext_login: value.acknowledge_plaintext_login,
                })
            })
            .ok_or_else(|| {
                Error::InvalidInput("sharing requires an account configuration".into())
            })?;
        let catalog = ShareCatalog::build(database, &configuration.policy, stop.clone())?;
        let counts = catalog.counts()?;
        let listener = configuration.listen.map(TcpListener::bind).transpose()?;
        if let Some(listener) = &listener {
            listener.set_nonblocking(true)?;
        }
        let account = Account::new(database, connection)?;
        check_spool_budget(database, &configuration.policy.limits)?;
        let (shared, connections) = mpsc::sync_channel(4);
        {
            let session = account
                .session
                .lock()
                .map_err(|_| Error::Internal("sharing account lock poisoned"))?;
            session
                .stream
                .as_ref()
                .ok_or(Error::Remote("sharing account is closed"))?
                .enable_sharing()?;
            session
                .stream
                .as_ref()
                .ok_or(Error::Remote("sharing account is closed"))?
                .share_connections(shared)?;
        }
        account.send(
            35,
            &[counts.0.to_le_bytes(), counts.1.to_le_bytes()].concat(),
        )?;
        if let Some(listener) = &listener {
            account.send(2, &u32::from(listener.local_addr()?.port()).to_le_bytes())?;
        }
        *account
            .advertisement
            .lock()
            .map_err(|_| Error::Internal("sharing advertisement poisoned"))? = (
            counts.0,
            counts.1,
            listener
                .as_ref()
                .map(|listener| listener.local_addr().map(|address| address.port()))
                .transpose()?,
        );
        let context = Arc::new(Context {
            account: account.clone(),
            policy: configuration.policy.clone(),
            catalog: Mutex::new(catalog),
            stop,
            queue: Mutex::new(Queue {
                waiting: VecDeque::new(),
                active: BTreeSet::new(),
                last_peer: None,
            }),
            pending: Mutex::new(BTreeMap::new()),
            peers: Mutex::new(BTreeMap::new()),
            handshakes: AtomicUsize::new(0),
            browses: AtomicUsize::new(0),
            can_receive: listener.is_some(),
            searches: Mutex::new(BTreeMap::new()),
        });
        let serving = context.clone();
        let thread = std::thread::Builder::new()
            .name("domus-sharing".into())
            .spawn(move || serve(serving, listener, connections))?;
        Ok(Some(Self {
            account,
            context,
            thread: Some(thread),
        }))
    }

    pub(crate) fn check(&self) -> Result<()> {
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
        {
            return Err(Error::Remote(
                "Soulseek sharing stopped; restart the worker after review",
            ));
        }
        Ok(())
    }
}

impl Drop for SharingRuntime {
    fn drop(&mut self) {
        self.context.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Ok(())) => {}
                Ok(Err(Error::WorkerStopping)) => {}
                Ok(Err(error)) => eprintln!("Soulseek sharing stopped: {error}"),
                Err(_) => eprintln!("Soulseek sharing owner panicked"),
            }
        }
    }
}

fn serve(
    context: Arc<Context>,
    listener: Option<TcpListener>,
    shared: mpsc::Receiver<SharedPeer>,
) -> Result<()> {
    let mut tasks: Vec<JoinHandle<Result<()>>> = Vec::new();
    let mut window = Instant::now();
    let mut requests = 0u32;
    let result = (|| {
        while !context.stop.load(Ordering::Acquire) {
            for index in (0..tasks.len()).rev() {
                if tasks[index].is_finished() {
                    let result = tasks
                        .swap_remove(index)
                        .join()
                        .map_err(|_| Error::Internal("sharing peer task panicked"))?;
                    if let Err(error) = result
                        && !matches!(error, Error::WorkerStopping)
                    {
                        eprintln!("Soulseek share request failed: {error}");
                    }
                }
            }
            if let Err(error) = context.check() {
                if matches!(error, Error::SoulseekTransport(_)) {
                    context.account.reconnect(&error)?;
                    context
                        .pending
                        .lock()
                        .map_err(|_| Error::Internal("sharing file route poisoned"))?
                        .clear();
                    continue;
                }
                return Err(error);
            }
            if window.elapsed() >= Duration::from_secs(60) {
                window = Instant::now();
                requests = 0;
            }
            let capacity = context.policy.limits.peer_connections as usize
                + context.policy.limits.pending_handshakes as usize;
            if tasks.len() < capacity
                && let Ok(peer) = shared.try_recv()
            {
                let context = context.clone();
                tasks.push(std::thread::spawn(move || {
                    serve_peer(context, peer.username, peer.address, peer.channel, None)
                }));
            }
            if let Some(listener) = &listener {
                match listener.accept() {
                    Ok((socket, address)) => {
                        if tasks.len() < capacity
                            && requests < context.policy.limits.requests_per_minute
                            && context.handshakes.load(Ordering::Acquire)
                                < context.policy.limits.pending_handshakes as usize
                            && matches!(address, std::net::SocketAddr::V4(address) if public_peer_ip(*address.ip()))
                        {
                            requests += 1;
                            let context = context.clone();
                            context.handshakes.fetch_add(1, Ordering::AcqRel);
                            tasks.push(std::thread::spawn(move || accept_peer(context, socket)));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if let Some(event) = context.account.event()? {
                match event {
                    Event::Server(ServerEvent::Search {
                        username,
                        token,
                        query,
                    }) if context.policy.allows(&username)
                        && requests < context.policy.limits.requests_per_minute
                        && tasks.len() < capacity =>
                    {
                        requests += 1;
                        if let Some(sender) = context
                            .searches
                            .lock()
                            .map_err(|_| Error::Internal("sharing search route poisoned"))?
                            .get(&username)
                        {
                            if sender.try_send((token, query)).is_err() {
                                eprintln!("Soulseek sharing search route is full or closed");
                            }
                            continue;
                        }
                        let context = context.clone();
                        tasks.push(std::thread::spawn(move || {
                            let mut session = context.account.fork(&username)?;
                            let address = session.peer_address(&username)?.address();
                            let socket = TcpStream::connect_timeout(&address.into(), IO_TIMEOUT)
                                .map_err(Error::SoulseekTransport)?;
                            let channel = context.account.peer_channel(socket, &username)?;
                            channel.init(1, &init_body(&context.account.username, "P"))?;
                            serve_peer(context, username, address, channel, Some((token, query)))
                        }));
                    }
                    Event::Server(ServerEvent::Connect {
                        username,
                        kind,
                        address,
                        token,
                    }) if kind == "P"
                        && context.policy.allows(&username)
                        && public_peer_ip(*address.ip())
                        && address.port() != 0
                        && tasks.len() < capacity
                        && requests < context.policy.limits.requests_per_minute =>
                    {
                        requests += 1;
                        let context = context.clone();
                        tasks.push(std::thread::spawn(move || {
                            let socket = TcpStream::connect_timeout(&address.into(), IO_TIMEOUT)
                                .map_err(Error::SoulseekTransport)?;
                            let channel = context.account.peer_channel(socket, &username)?;
                            channel.init(0, &token.to_le_bytes())?;
                            serve_peer(context, username, address, channel, None)
                        }));
                    }
                    Event::Server(ServerEvent::CannotConnect(token)) => {
                        if let Some(pending) = context
                            .pending
                            .lock()
                            .map_err(|_| Error::Internal("sharing file route poisoned"))?
                            .remove(&token)
                        {
                            let _ = pending.sent.try_send(Err(Error::Remote(
                                "sharing reverse file connection failed",
                            )));
                        }
                    }
                    Event::Server(ServerEvent::Search { .. } | ServerEvent::Connect { .. }) => {}
                    _ => return Err(Error::Remote("unexpected sharing account event")),
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    })();
    context.stop.store(true, Ordering::Release);
    for task in tasks {
        match task.join() {
            Ok(Ok(()) | Err(Error::WorkerStopping)) => {}
            Ok(Err(error)) => eprintln!("Soulseek share drain failed: {error}"),
            Err(_) => eprintln!("Soulseek share peer panicked during drain"),
        }
    }
    result
}

struct Handshake(Arc<Context>);
impl Drop for Handshake {
    fn drop(&mut self) {
        self.0.handshakes.fetch_sub(1, Ordering::AcqRel);
    }
}

fn accept_peer(context: Arc<Context>, mut socket: TcpStream) -> Result<()> {
    let handshake = Handshake(context.clone());
    socket.set_read_timeout(Some(IO_TIMEOUT))?;
    socket.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut size = [0; 4];
    let deadline = Instant::now() + IO_TIMEOUT;
    super::read_checked(&mut socket, &mut size, deadline, &mut || context.check())?;
    let size = u32::from_le_bytes(size) as usize;
    if !(5..=128).contains(&size) {
        return Err(super::protocol());
    }
    let mut bytes = vec![0; size];
    super::read_checked(&mut socket, &mut bytes, deadline, &mut || context.check())?;
    let mut fields = Fields(&bytes[1..]);
    match bytes[0] {
        0 => {
            let token = fields.u32()?;
            fields.end()?;
            let pending = context
                .pending
                .lock()
                .map_err(|_| Error::Internal("sharing file route poisoned"))?
                .remove(&token)
                .ok_or(Error::Remote("unknown sharing file connection token"))?;
            if socket.peer_addr()?.ip() != std::net::IpAddr::V4(*pending.address.ip())
                || !context.policy.allows(&pending.peer)
            {
                return Err(Error::Remote("sharing file connection peer changed"));
            }
            context.check_generation(pending.generation)?;
            pending
                .sent
                .try_send(Ok(socket))
                .map_err(|_| Error::Remote("sharing file request expired"))?;
            Ok(())
        }
        1 => {
            let username = fields.text(30)?.to_owned();
            if fields.text(1)? != "P" || fields.u32()? != 0 || !context.policy.allows(&username) {
                return Err(Error::Remote("sharing peer is not authorized"));
            }
            fields.end()?;
            let mut session = context.account.fork(&username)?;
            let address = session.peer_address(&username)?.address();
            if socket.peer_addr()?.ip() != std::net::IpAddr::V4(*address.ip()) {
                return Err(Error::Remote(
                    "sharing peer differs from the server address",
                ));
            }
            let channel = context.account.peer_channel(socket, &username)?;
            drop(session);
            drop(handshake);
            serve_peer(context, username, address, channel, None)
        }
        _ => Err(super::protocol()),
    }
}

fn init(socket: &mut TcpStream, username: &str, kind: &str) -> Result<()> {
    socket.set_write_timeout(Some(IO_TIMEOUT))?;
    send_init(socket, 1, &init_body(username, kind))
}

fn init_body(username: &str, kind: &str) -> Vec<u8> {
    let mut body = Vec::new();
    string(&mut body, username);
    string(&mut body, kind);
    body.extend_from_slice(&0u32.to_le_bytes());
    body
}

struct PeerGuard {
    context: Arc<Context>,
    username: String,
}
impl Drop for PeerGuard {
    fn drop(&mut self) {
        if let Ok(mut peers) = self.context.peers.lock()
            && let Some(count) = peers.get_mut(&self.username)
        {
            *count -= 1;
            if *count == 0 {
                peers.remove(&self.username);
                if let Ok(mut searches) = self.context.searches.lock() {
                    searches.remove(&self.username);
                }
                if let Ok(mut queue) = self.context.queue.lock() {
                    queue.waiting.retain(|(peer, _)| peer != &self.username);
                }
            }
        }
    }
}

struct UploadSlot {
    context: Arc<Context>,
    peer: String,
    path: String,
}
impl Drop for UploadSlot {
    fn drop(&mut self) {
        if let Ok(mut queue) = self.context.queue.lock() {
            queue.active.remove(&(self.peer.clone(), self.path.clone()));
        }
    }
}

fn deny(channel: &Channel, path: &str, reason: &str) -> Result<()> {
    let mut body = Vec::new();
    string(&mut body, path);
    string(&mut body, reason);
    channel.send(50, &body)
}

fn enqueue(context: &Context, channel: &Channel, peer: &str, path: &str) -> Result<()> {
    if !context.policy.allows(peer) {
        return deny(channel, path, "File not shared.");
    }
    match context.catalog()?.file(path) {
        Ok(_) => {}
        Err(error @ (Error::InvalidInput(_) | Error::Conflict(_))) => {
            eprintln!("Soulseek share request rejected: {error}");
            return deny(channel, path, "File not shared.");
        }
        Err(error) => return Err(error),
    }
    let mut queue = context
        .queue
        .lock()
        .map_err(|_| Error::Internal("sharing queue poisoned"))?;
    let key = (peer.to_owned(), path.to_owned());
    if queue.waiting.contains(&key) || queue.active.contains(&key) {
        return Ok(());
    }
    if queue.waiting.len() >= context.policy.limits.queued_files as usize
        || queue
            .waiting
            .iter()
            .filter(|(user, _)| user == peer)
            .count()
            >= context.policy.limits.queued_per_peer as usize
    {
        return deny(channel, path, "Too many files");
    }
    queue.waiting.push_back(key);
    Ok(())
}

fn serve_peer(
    context: Arc<Context>,
    username: String,
    address: SocketAddrV4,
    mut channel: Channel,
    initial_search: Option<(u32, String)>,
) -> Result<()> {
    {
        let mut peers = context
            .peers
            .lock()
            .map_err(|_| Error::Internal("sharing peer registry poisoned"))?;
        if peers.values().sum::<u32>() >= context.policy.limits.peer_connections {
            return Err(Error::Busy("sharing peer connection limit exceeded"));
        }
        *peers.entry(username.clone()).or_default() += 1;
    }
    let _peer = PeerGuard {
        context: context.clone(),
        username: username.clone(),
    };
    let (search_sender, search_requests) = mpsc::sync_channel(4);
    context
        .searches
        .lock()
        .map_err(|_| Error::Internal("sharing search route poisoned"))?
        .insert(username.clone(), search_sender);
    let mut offered: Option<(u32, SharedFile, UploadSlot, Instant)> = None;
    let mut uploads: Vec<(String, JoinHandle<Result<()>>)> = Vec::new();
    let mut window = Instant::now();
    let mut requests = 0u32;
    let started = Instant::now();
    let mut activity = started;
    let generation = channel.generation();
    let result = (|| {
        if let Some((token, query)) = initial_search {
            search(&context, &channel, token, &query)?;
        }
        while !context.stop.load(Ordering::Acquire) {
            context.check_generation(generation)?;
            if !channel.shared_control()
                && !channel.acquisition_in_use()
                && started.elapsed()
                    >= Duration::from_secs(u64::from(context.policy.limits.operation_seconds))
            {
                return Err(Error::Remote("sharing peer time limit exceeded"));
            }
            if !channel.shared_control()
                && !channel.acquisition_in_use()
                && offered.is_none()
                && uploads.is_empty()
                && activity.elapsed()
                    >= Duration::from_secs(u64::from(context.policy.limits.idle_seconds))
                && !context
                    .queue
                    .lock()
                    .map_err(|_| Error::Internal("sharing queue poisoned"))?
                    .waiting
                    .iter()
                    .any(|(peer, _)| peer == &username)
            {
                return Err(Error::Remote("sharing peer idle time limit exceeded"));
            }
            if window.elapsed() >= Duration::from_secs(60) {
                window = Instant::now();
                requests = 0;
            }
            if let Ok((token, query)) = search_requests.try_recv() {
                activity = Instant::now();
                requests += 1;
                if requests > context.policy.limits.requests_per_minute {
                    return Err(Error::Remote("sharing request rate exceeded"));
                }
                if context.policy.allows(&username) {
                    search(&context, &channel, token, &query)?;
                }
            }
            for index in (0..uploads.len()).rev() {
                if uploads[index].1.is_finished() {
                    let (path, upload) = uploads.swap_remove(index);
                    if let Err(error) = upload
                        .join()
                        .map_err(|_| Error::Internal("sharing upload panicked"))?
                    {
                        eprintln!("Soulseek read-only upload failed: {error}");
                        let mut body = Vec::new();
                        string(&mut body, &path);
                        channel.send(46, &body)?;
                    }
                }
            }
            if offered
                .as_ref()
                .is_some_and(|(_, _, _, at)| at.elapsed() >= IO_TIMEOUT)
            {
                return Err(Error::Remote("sharing upload offer expired"));
            }
            if offered.is_none() {
                let next = {
                    let mut queue = context
                        .queue
                        .lock()
                        .map_err(|_| Error::Internal("sharing queue poisoned"))?;
                    queue.take(&username, context.policy.limits.upload_slots as usize)
                };
                if let Some(path) = next {
                    let slot = UploadSlot {
                        context: context.clone(),
                        peer: username.clone(),
                        path: path.clone(),
                    };
                    let file = context.catalog()?.file(&path)?;
                    if !context.policy.allows(&username) {
                        return Err(Error::Remote("sharing access changed"));
                    }
                    let transfer = token()?;
                    let mut body = 1u32.to_le_bytes().to_vec();
                    body.extend_from_slice(&transfer.to_le_bytes());
                    string(&mut body, &file.wire_path);
                    body.extend_from_slice(&file.bytes.to_le_bytes());
                    channel.send(40, &body)?;
                    offered = Some((transfer, file, slot, Instant::now()));
                }
            }
            if let Some(event) = channel.try_event()? {
                activity = Instant::now();
                requests += 1;
                if requests > context.policy.limits.requests_per_minute {
                    return Err(Error::Remote("sharing request rate exceeded"));
                }
                if !context.policy.allows(&username) {
                    if let Event::Peer(event) = event {
                        refuse(&channel, event)?;
                        continue;
                    }
                    return Err(super::protocol());
                }
                match event {
                    Event::Peer(PeerEvent::ShareList) => browse(&context, &channel, None)?,
                    Event::Peer(PeerEvent::FolderRequest { token, folder }) => {
                        browse(&context, &channel, Some((token, &folder)))?
                    }
                    Event::Peer(PeerEvent::Queue { path }) => {
                        enqueue(&context, &channel, &username, &path)?
                    }
                    Event::Peer(PeerEvent::DownloadRequest { token, path }) => {
                        let mut body = token.to_le_bytes().to_vec();
                        body.push(0);
                        string(&mut body, "Queued");
                        channel.send(41, &body)?;
                        enqueue(&context, &channel, &username, &path)?;
                    }
                    Event::Peer(PeerEvent::QueueRequest { path }) => {
                        let queue = context
                            .queue
                            .lock()
                            .map_err(|_| Error::Internal("sharing queue poisoned"))?;
                        let place = queue
                            .waiting
                            .iter()
                            .position(|key| key == &(username.clone(), path.clone()))
                            .map(|index| (index + 1) as u32)
                            .unwrap_or(0);
                        let mut body = Vec::new();
                        string(&mut body, &path);
                        body.extend_from_slice(&place.to_le_bytes());
                        channel.send(44, &body)?;
                    }
                    Event::Peer(PeerEvent::UploadResponse { token, allowed }) => {
                        let (expected, file, slot, _) = offered
                            .take()
                            .ok_or(Error::Remote("unsolicited sharing upload response"))?;
                        if token != expected {
                            return Err(Error::Remote("sharing upload token differs"));
                        }
                        if allowed {
                            let context = context.clone();
                            let username = username.clone();
                            let path = file.wire_path.clone();
                            uploads.push((
                                path,
                                std::thread::spawn(move || {
                                    let _slot = slot;
                                    upload(&context, &username, address, token, &file, generation)
                                }),
                            ));
                        }
                    }
                    _ => return Err(super::protocol()),
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    })();
    for (_, upload) in uploads {
        match upload.join() {
            Ok(Ok(()) | Err(Error::WorkerStopping)) => {}
            Ok(Err(error)) => eprintln!("Soulseek upload drain failed: {error}"),
            Err(_) => eprintln!("Soulseek upload panicked during drain"),
        }
    }
    result
}

fn upload(
    context: &Context,
    peer: &str,
    address: SocketAddrV4,
    token: u32,
    file: &SharedFile,
    generation: u64,
) -> Result<()> {
    context.check_generation(generation)?;
    if !context.policy.allows(peer) {
        return Err(Error::Remote("sharing peer is not authorized"));
    }
    let input = context.catalog()?.open(file)?;
    let mut socket = match TcpStream::connect_timeout(&address.into(), IO_TIMEOUT) {
        Ok(mut socket) => {
            init(&mut socket, &context.account.username, "F")?;
            socket
        }
        Err(error) => {
            if !context.can_receive {
                return Err(Error::SoulseekTransport(error));
            }
            let connection = self::token()?;
            let (sent, received) = mpsc::sync_channel(1);
            context
                .pending
                .lock()
                .map_err(|_| Error::Internal("sharing file routes poisoned"))?
                .insert(
                    connection,
                    PendingFile {
                        peer: peer.into(),
                        address,
                        sent,
                        generation,
                    },
                );
            let result = (|| {
                let mut body = connection.to_le_bytes().to_vec();
                string(&mut body, peer);
                string(&mut body, "F");
                context.account.send(18, &body)?;
                received
                    .recv_timeout(IO_TIMEOUT)
                    .map_err(|_| Error::Remote("sharing reverse file setup expired"))
            })();
            context
                .pending
                .lock()
                .map_err(|_| Error::Internal("sharing file routes poisoned"))?
                .remove(&connection);
            result??
        }
    };
    socket.set_read_timeout(Some(IO_TIMEOUT))?;
    socket.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut handle = input.file.try_clone()?;
    transmit(
        &mut handle,
        &mut socket,
        token,
        file.bytes,
        &context.policy.limits,
        || {
            context.check_generation(generation)?;
            input.check()
        },
    )
}

fn transmit(
    input: &mut std::fs::File,
    socket: &mut TcpStream,
    token: u32,
    bytes: u64,
    limits: &ShareLimits,
    mut check: impl FnMut() -> Result<()>,
) -> Result<()> {
    check()?;
    let deadline = Instant::now() + Duration::from_secs(u64::from(limits.operation_seconds));
    socket.set_write_timeout(Some(Duration::from_secs(u64::from(limits.idle_seconds))))?;
    socket.write_all(&token.to_le_bytes())?;
    let mut offset = [0; 8];
    super::read_checked(
        socket,
        &mut offset,
        deadline.min(Instant::now() + Duration::from_secs(u64::from(limits.idle_seconds))),
        &mut check,
    )?;
    let offset = u64::from_le_bytes(offset);
    if offset > bytes {
        return Err(Error::Remote("sharing resume offset exceeds the file"));
    }
    input.seek(SeekFrom::Start(offset))?;
    let mut left = bytes - offset;
    let mut buffer = vec![0; limits.buffer_bytes as usize];
    while left > 0 {
        check()?;
        if Instant::now() >= deadline {
            return Err(Error::Remote("sharing upload time limit exceeded"));
        }
        let wanted = left.min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..wanted])?;
        let mut written = 0;
        while written < wanted {
            check()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(Error::Remote("sharing upload time limit exceeded"))?;
            socket.set_write_timeout(Some(
                remaining.min(Duration::from_secs(u64::from(limits.idle_seconds))),
            ))?;
            let count = socket.write(&buffer[written..wanted])?;
            if count == 0 {
                return Err(Error::Remote("sharing receiver closed before completion"));
            }
            written += count;
        }
        left -= wanted as u64;
    }
    check()?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(Error::Remote("sharing upload time limit exceeded"))?;
    socket.set_read_timeout(Some(
        remaining.min(Duration::from_secs(u64::from(limits.idle_seconds))),
    ))?;
    let mut closed = [0; 1];
    if socket.read(&mut closed)? != 0 {
        return Err(Error::Remote("unexpected sharing file data from receiver"));
    }
    Ok(())
}

struct BrowseSlot<'a>(&'a AtomicUsize);
impl Drop for BrowseSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Spool {
    directory: cap_std::fs::Dir,
    name: String,
    file: std::fs::File,
    maximum: u64,
    bytes: u64,
}

fn check_spool_budget(database: &Path, limits: &ShareLimits) -> Result<()> {
    let directory = crate::infrastructure::filesystem::LibraryDirectory::open(
        database
            .parent()
            .ok_or(Error::Internal("sharing database parent missing"))?,
    )?
    .into_directory();
    let mut retained = 0u64;
    let mut count = 0u32;
    for entry in directory.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(parts) = name
            .strip_prefix(".domus-share-")
            .and_then(|name| name.strip_suffix(".pending"))
        else {
            continue;
        };
        let Some((process, token)) = parts.split_once('-') else {
            continue;
        };
        if process.is_empty()
            || token.is_empty()
            || !process
                .bytes()
                .chain(token.bytes())
                .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let metadata = directory.symlink_metadata(name)?;
        if !metadata.is_file() || crate::infrastructure::filesystem::is_unsafe_cap_link(&metadata) {
            return Err(Error::Conflict(
                "sharing spool contains an unsafe retained entry".into(),
            ));
        }
        count += 1;
        retained = retained
            .checked_add(metadata.len())
            .ok_or_else(|| Error::Conflict("sharing retained spool size overflow".into()))?;
        if count >= 128 || retained > u64::from(limits.spool_bytes) {
            return Err(Error::Conflict(
                "sharing retained spools exceed the budget; review and remove them before restart"
                    .into(),
            ));
        }
    }
    let reserved = u64::from(limits.compressed_bytes) * u64::from(limits.concurrent_browses);
    if retained + reserved > u64::from(limits.spool_bytes) {
        return Err(Error::Conflict("sharing retained spools leave insufficient response space; review and remove them before restart".into()));
    }
    Ok(())
}

impl Spool {
    fn new(context: &Context) -> Result<Self> {
        Self::at(&context.account.database, &context.policy.limits)
    }

    fn at(database: &Path, limits: &ShareLimits) -> Result<Self> {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        let directory = crate::infrastructure::filesystem::LibraryDirectory::open(
            database
                .parent()
                .ok_or(Error::Internal("sharing database parent missing"))?,
        )?
        .into_directory();
        let name = format!(".domus-share-{}-{}.pending", std::process::id(), token()?);
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = directory.open_with(&name, &options)?.into_std();
        let spool = Self {
            directory,
            name,
            file,
            maximum: u64::from(limits.compressed_bytes),
            bytes: 0,
        };
        crate::infrastructure::permissions::FilePermissions::current()?.check(&spool.file, true)?;
        Ok(spool)
    }
}

impl Write for Spool {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes + bytes.len() as u64 > self.maximum {
            return Err(std::io::Error::other(
                "sharing compressed response limit exceeded",
            ));
        }
        let written = self.file.write(bytes)?;
        self.bytes += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        if let Err(error) = self.directory.remove_file(&self.name) {
            eprintln!("sharing spool cleanup failed: {error}");
        }
    }
}

fn entry(output: &mut impl Write, file: &SharedFile, full: bool) -> Result<()> {
    let path = if full {
        file.wire_path.as_str()
    } else {
        file.wire_path
            .rsplit('\\')
            .next()
            .ok_or(super::protocol())?
    };
    file_entry(output, path, file.bytes)
}

fn file_entry(output: &mut impl Write, path: &str, bytes: u64) -> Result<()> {
    let mut body = vec![1];
    string(&mut body, path);
    body.extend_from_slice(&bytes.to_le_bytes());
    string(
        &mut body,
        path.rsplit_once('.')
            .map(|(_, extension)| extension)
            .unwrap_or(""),
    );
    body.extend_from_slice(&0u32.to_le_bytes());
    output.write_all(&body)?;
    Ok(())
}

struct Expanded<W> {
    output: W,
    bytes: u64,
    maximum: u64,
}
impl<W: Write> Write for Expanded<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes + bytes.len() as u64 > self.maximum {
            return Err(std::io::Error::other(
                "sharing expanded response limit exceeded",
            ));
        }
        self.output.write_all(bytes)?;
        self.bytes += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

fn browse(context: &Context, channel: &Channel, folder: Option<(u32, &str)>) -> Result<()> {
    let deadline = Instant::now()
        + if channel.shared_control() {
            IO_TIMEOUT
        } else {
            Duration::from_secs(u64::from(context.policy.limits.operation_seconds))
        };
    let check = || {
        if Instant::now() >= deadline {
            return Err(Error::Remote("sharing browse time limit exceeded"));
        }
        context.check()
    };
    if context.browses.fetch_add(1, Ordering::AcqRel)
        >= context.policy.limits.concurrent_browses as usize
    {
        context.browses.fetch_sub(1, Ordering::AcqRel);
        return Err(Error::Busy("sharing browse limit exceeded"));
    }
    let _slot = BrowseSlot(&context.browses);
    let catalog = context.catalog()?;
    let guard = catalog.response_guard()?;
    let spool = Spool::new(context)?;
    let compressed = flate2::write::ZlibEncoder::new(spool, flate2::Compression::fast());
    let mut output = Expanded {
        output: compressed,
        bytes: 0,
        maximum: u64::from(context.policy.limits.expanded_bytes),
    };
    let within = folder.map(|(_, folder)| folder);
    let mut count = 0u32;
    let mut after = String::new();
    loop {
        check()?;
        guard.check()?;
        let directories = catalog.directory_page(&after, 128, within)?;
        if directories.is_empty() {
            break;
        }
        count = count
            .checked_add(directories.len() as u32)
            .ok_or(super::protocol())?;
        after = directories
            .last()
            .ok_or(super::protocol())?
            .wire_path
            .clone();
    }
    if let Some((token, folder)) = folder {
        crate::domain::sources::remote_identifier(folder)?;
        output.write_all(&token.to_le_bytes())?;
        let mut body = Vec::new();
        string(&mut body, folder);
        output.write_all(&body)?;
    }
    output.write_all(&count.to_le_bytes())?;
    after.clear();
    loop {
        check()?;
        guard.check()?;
        let directories = catalog.directory_page(&after, 128, within)?;
        if directories.is_empty() {
            break;
        }
        for directory in &directories {
            let mut body = Vec::new();
            string(&mut body, &directory.wire_path);
            body.extend_from_slice(&directory.files.to_le_bytes());
            output.write_all(&body)?;
            let mut last = 0;
            let mut written = 0u32;
            loop {
                check()?;
                guard.check()?;
                let files = catalog.directory_files(&directory.wire_path, last, 128)?;
                if files.is_empty() {
                    break;
                }
                for file in &files {
                    entry(&mut output, file, false)?;
                }
                written = written
                    .checked_add(files.len() as u32)
                    .ok_or(super::protocol())?;
                last = files.last().ok_or(super::protocol())?.id;
            }
            if written != directory.files {
                return Err(Error::Conflict(
                    "share directory changed during response preparation".into(),
                ));
            }
        }
        after = directories
            .last()
            .ok_or(super::protocol())?
            .wire_path
            .clone();
    }
    if folder.is_none() {
        output.write_all(&[0; 8])?;
    }
    let mut spool = output.output.finish()?;
    spool.file.seek(SeekFrom::Start(0))?;
    let sending = Instant::now();
    channel.send_stream(
        if folder.is_some() { 37 } else { 5 },
        spool.bytes as u32,
        &mut spool.file,
        context.policy.limits.buffer_bytes as usize,
        Duration::from_secs(u64::from(context.policy.limits.idle_seconds)).min(IO_TIMEOUT),
        || {
            check()?;
            if (channel.shared_control() || channel.acquisition_in_use())
                && sending.elapsed() >= IO_TIMEOUT
            {
                return Err(Error::Remote(
                    "sharing browse exceeded the acquisition control time budget",
                ));
            }
            guard.check()
        },
    )
}

fn refuse(channel: &Channel, event: PeerEvent) -> Result<()> {
    let (code, bytes) = match event {
        PeerEvent::ShareList => (5, vec![0; 12]),
        PeerEvent::FolderRequest { token, folder } => {
            let mut bytes = token.to_le_bytes().to_vec();
            string(&mut bytes, &folder);
            bytes.extend_from_slice(&0u32.to_le_bytes());
            (37, bytes)
        }
        PeerEvent::Queue { path } | PeerEvent::QueueRequest { path } => {
            return deny(channel, &path, "File not shared.");
        }
        PeerEvent::DownloadRequest { token, .. } => {
            let mut bytes = token.to_le_bytes().to_vec();
            bytes.push(0);
            string(&mut bytes, "File not shared.");
            return channel.send(41, &bytes);
        }
        _ => return Err(super::protocol()),
    };
    let mut compressed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    compressed.write_all(&bytes)?;
    channel.send(code, &compressed.finish()?)
}

fn search(context: &Context, channel: &Channel, token: u32, query: &str) -> Result<()> {
    if query.trim().len() < 3 || query.len() > 256 || query.chars().any(char::is_control) {
        return Ok(());
    }
    let catalog = context.catalog()?;
    let guard = catalog.response_guard()?;
    let files = catalog.page(
        0,
        context.policy.limits.search_results as u16,
        None,
        Some(query),
    )?;
    if files.is_empty() {
        return Ok(());
    }
    let mut bytes = Vec::new();
    string(&mut bytes, &context.account.username);
    bytes.extend_from_slice(&token.to_le_bytes());
    bytes.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for file in &files {
        entry(&mut bytes, file, true)?;
    }
    let queue = context
        .queue
        .lock()
        .map_err(|_| Error::Internal("sharing queue poisoned"))?;
    bytes.push(u8::from(
        queue.active.len() < context.policy.limits.upload_slots as usize,
    ));
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(queue.waiting.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0; 8]);
    let mut compressed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    compressed.write_all(&bytes)?;
    guard.check()?;
    channel.send(9, &compressed.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn pair() -> std::io::Result<(TcpStream, TcpStream)> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let socket = TcpStream::connect(listener.local_addr()?)?;
        Ok((socket, listener.accept()?.0))
    }

    #[test]
    fn sharing_disabled_starts_no_listener_database_or_account() -> TestResult {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("unused.sqlite");
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let configuration = SharingConfiguration {
            policy: SharePolicy::default(),
            account: None,
            listen: Some(listener.local_addr()?.to_string().parse()?),
        };
        assert!(
            SharingRuntime::start(
                &database,
                &configuration,
                None,
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )?
            .is_none()
        );
        assert!(!database.exists());
        Ok(())
    }

    #[test]
    fn sharing_session_forks_reuse_one_authenticated_account_connection() -> TestResult {
        use super::super::protocol::{read_frame, send};
        let (client, mut server) = pair()?;
        server.set_read_timeout(Some(Duration::from_secs(5)))?;
        let (finish, finished) = mpsc::sync_channel(1);
        let task = std::thread::spawn(move || -> std::result::Result<(), String> {
            let mut run = || -> Result<()> {
                assert_eq!(
                    read_frame(|bytes| {
                        server.read_exact(bytes)?;
                        Ok(())
                    })?
                    .0,
                    1
                );
                let mut reply = vec![1];
                string(&mut reply, "generated fixture");
                reply.extend_from_slice(&0u32.to_le_bytes());
                string(&mut reply, &super::super::md5(&[b"generated-only"]));
                reply.push(0);
                send(&mut server, 1, &reply)?;
                for code in [28, 35, 100] {
                    assert_eq!(
                        read_frame(|bytes| {
                            server.read_exact(bytes)?;
                            Ok(())
                        })?
                        .0,
                        code
                    );
                }
                for _ in 0..2 {
                    let (code, request) = read_frame(|bytes| {
                        server.read_exact(bytes)?;
                        Ok(())
                    })?;
                    assert_eq!(code, 3);
                    let mut fields = Fields(&request);
                    let peer = fields.text(30)?;
                    fields.end()?;
                    let mut body = Vec::new();
                    string(&mut body, peer);
                    body.extend_from_slice(
                        &u32::from(std::net::Ipv4Addr::new(8, 8, 8, 8)).to_le_bytes(),
                    );
                    body.extend_from_slice(&2234u32.to_le_bytes());
                    send(&mut server, 3, &body)?;
                }
                let mut search = Vec::new();
                string(&mut search, "share-peer");
                search.extend_from_slice(&73u32.to_le_bytes());
                string(&mut search, "generated album");
                send(&mut server, 26, &search)?;
                finished
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| Error::Remote("sharing fixture did not finish"))?;
                Ok(())
            };
            run().map_err(|error| error.to_string())
        });
        let mut owner = SoulseekSession::login(client, "generated-user", "generated-only")?;
        owner
            .stream
            .as_ref()
            .ok_or("missing session")?
            .enable_sharing()?;
        let mut acquisition = owner.fork_for_peer("album-peer")?;
        let mut sharing = owner.fork_for_peer("share-peer")?;
        let generation = owner.stream.as_ref().ok_or("missing owner")?.generation();
        assert_eq!(
            acquisition
                .stream
                .as_ref()
                .ok_or("missing acquisition")?
                .generation(),
            generation
        );
        assert_eq!(
            sharing
                .stream
                .as_ref()
                .ok_or("missing sharing")?
                .generation(),
            generation
        );
        let query = std::thread::spawn(move || {
            acquisition
                .peer_address("album-peer")
                .map(|peer| peer.username)
        });
        assert_eq!(sharing.peer_address("share-peer")?.username, "share-peer");
        assert_eq!(
            query.join().map_err(|_| "acquisition query panicked")??,
            "album-peer"
        );
        assert!(matches!(
            owner.stream.as_mut().ok_or("missing owner")?.receive(
                Instant::now() + Duration::from_secs(5),
                &mut || Ok(()),
                |_| true
            )?,
            Event::Server(ServerEvent::Search { token: 73, .. })
        ));
        finish.send(())?;
        task.join().map_err(|_| "account fixture panicked")??;
        Ok(())
    }

    #[test]
    fn sharing_upload_uses_exact_offset_and_keeps_source_unchanged() -> TestResult {
        for offset in [0, 17, 200_000] {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("generated.mp3");
            let original = vec![73; 200_000];
            std::fs::write(&path, &original)?;
            let mut input = std::fs::File::open(&path)?;
            let (mut socket, mut receiver) = pair()?;
            let task = std::thread::spawn(move || -> std::io::Result<()> {
                let mut token = [0; 4];
                receiver.read_exact(&mut token)?;
                assert_eq!(u32::from_le_bytes(token), 71);
                receiver.write_all(&(offset as u64).to_le_bytes())?;
                let mut bytes = vec![0; 200_000 - offset];
                receiver.read_exact(&mut bytes)?;
                assert!(bytes.iter().all(|byte| *byte == 73));
                Ok(())
            });
            transmit(
                &mut input,
                &mut socket,
                71,
                200_000,
                &ShareLimits::default(),
                || Ok(()),
            )?;
            task.join().map_err(|_| "receiver panicked")??;
            assert_eq!(std::fs::read(&path)?, original);
        }
        Ok(())
    }

    #[test]
    fn sharing_queue_respects_slots_and_gives_another_peer_the_next_turn() {
        let mut queue = Queue {
            waiting: [("one", "a"), ("one", "b"), ("two", "c")]
                .into_iter()
                .map(|(peer, path)| (peer.into(), path.into()))
                .collect(),
            active: BTreeSet::new(),
            last_peer: None,
        };
        assert_eq!(queue.take("one", 1), Some("a".into()));
        assert_eq!(queue.take("two", 1), None);
        queue.active.clear();
        assert_eq!(queue.take("one", 1), None);
        assert_eq!(queue.take("two", 1), Some("c".into()));
        queue.active.clear();
        assert_eq!(queue.take("one", 1), Some("b".into()));
        assert!(queue.waiting.is_empty());
    }

    #[test]
    fn sharing_upload_rejects_offset_outside_the_authorized_file() -> TestResult {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("generated.flac");
        std::fs::write(&path, b"unchanged")?;
        let mut input = std::fs::File::open(&path)?;
        let (mut socket, mut receiver) = pair()?;
        let task = std::thread::spawn(move || -> std::io::Result<()> {
            receiver.read_exact(&mut [0; 4])?;
            receiver.write_all(&u64::MAX.to_le_bytes())?;
            Ok(())
        });
        assert!(
            transmit(
                &mut input,
                &mut socket,
                1,
                9,
                &ShareLimits::default(),
                || Ok(())
            )
            .is_err()
        );
        task.join().map_err(|_| "receiver panicked")??;
        assert_eq!(std::fs::read(path)?, b"unchanged");
        Ok(())
    }

    #[test]
    fn sharing_upload_does_not_send_a_token_after_authority_is_lost() -> TestResult {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("generated.jpg");
        std::fs::write(&path, b"unchanged")?;
        let mut input = std::fs::File::open(&path)?;
        let (mut socket, mut receiver) = pair()?;
        assert!(matches!(
            transmit(
                &mut input,
                &mut socket,
                1,
                9,
                &ShareLimits::default(),
                || Err(Error::WorkerStopping)
            ),
            Err(Error::WorkerStopping)
        ));
        drop(socket);
        assert_eq!(receiver.read(&mut [0; 1])?, 0);
        Ok(())
    }

    #[test]
    fn sharing_retained_spools_cannot_escape_the_configured_disk_budget() -> TestResult {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("state.sqlite");
        let limits = ShareLimits {
            compressed_bytes: 1024,
            concurrent_browses: 2,
            spool_bytes: 3072,
            ..ShareLimits::default()
        };
        let retained = directory.path().join(".domus-share-123-456.pending");
        std::fs::write(&retained, vec![0; 1024])?;
        check_spool_budget(&database, &limits)?;
        std::fs::write(&retained, vec![0; 1025])?;
        assert!(check_spool_budget(&database, &limits).is_err());
        assert_eq!(std::fs::metadata(retained)?.len(), 1025);
        Ok(())
    }

    #[test]
    fn sharing_complete_large_listing_is_spooled_and_streamed_without_truncation() -> TestResult {
        use sha2::{Digest, Sha256};
        const FILES: u32 = 60_000;
        let directory = crate::infrastructure::permissions::private_fixture::tempdir()?;
        let database = directory.path().join("state.sqlite");
        let limits = ShareLimits::default();
        let spool = Spool::at(&database, &limits)?;
        let compressed = flate2::write::ZlibEncoder::new(spool, flate2::Compression::fast());
        let mut output = Expanded {
            output: compressed,
            bytes: 0,
            maximum: u64::from(limits.expanded_bytes),
        };
        output.write_all(&1u32.to_le_bytes())?;
        let mut heading = Vec::new();
        string(&mut heading, "Music");
        heading.extend_from_slice(&FILES.to_le_bytes());
        output.write_all(&heading)?;
        for index in 0..FILES {
            let path = format!("{:x}.mp3", Sha256::digest(index.to_le_bytes()));
            file_entry(&mut output, &path, u64::from(index))?;
        }
        output.write_all(&[0; 8])?;
        let mut spool = output.output.finish()?;
        assert!(spool.bytes > super::super::protocol::MAX_FRAME as u64);
        spool.file.seek(SeekFrom::Start(0))?;
        let (socket, mut remote) = pair()?;
        remote.set_read_timeout(Some(Duration::from_secs(30)))?;
        let reader = std::thread::spawn(move || -> std::result::Result<(), String> {
            let run = || -> Result<()> {
                let mut header = [0; 8];
                remote.read_exact(&mut header)?;
                assert_eq!(
                    u32::from_le_bytes(
                        header[4..]
                            .try_into()
                            .map_err(|_| super::super::protocol())?
                    ),
                    5
                );
                let length = u32::from_le_bytes(
                    header[..4]
                        .try_into()
                        .map_err(|_| super::super::protocol())?,
                ) - 4;
                let mut bytes = Vec::new();
                flate2::read::ZlibDecoder::new(remote.take(u64::from(length)))
                    .read_to_end(&mut bytes)?;
                let mut fields = Fields(&bytes);
                assert_eq!(fields.u32()?, 1);
                assert_eq!(fields.text(4096)?, "Music");
                assert_eq!(fields.u32()?, FILES);
                for index in 0..FILES {
                    assert_eq!(fields.take(1)?, &[1]);
                    assert_eq!(
                        fields.text(4096)?,
                        format!("{:x}.mp3", Sha256::digest(index.to_le_bytes()))
                    );
                    assert_eq!(fields.u64()?, u64::from(index));
                    assert_eq!(fields.text(16)?, "mp3");
                    assert_eq!(fields.u32()?, 0);
                }
                assert_eq!(fields.u32()?, 0);
                assert_eq!(fields.u32()?, 0);
                fields.end()
            };
            run().map_err(|error| error.to_string())
        });
        let channel = Channel::isolated_peer(socket)?;
        channel.send_stream(
            5,
            spool.bytes as u32,
            &mut spool.file,
            limits.buffer_bytes as usize,
            IO_TIMEOUT,
            || Ok(()),
        )?;
        reader
            .join()
            .map_err(|_| "large listing receiver panicked")??;
        let name = spool.name.clone();
        drop(spool);
        assert!(!directory.path().join(name).exists());
        Ok(())
    }

    #[test]
    fn sharing_denied_browse_is_a_complete_empty_protocol_response() -> TestResult {
        use super::super::protocol::read_frame;
        for folder in [None, Some((73, "Music\\Album"))] {
            let (socket, mut remote) = pair()?;
            let channel = Channel::isolated_peer(socket)?;
            let request = match folder {
                None => PeerEvent::ShareList,
                Some((token, folder)) => PeerEvent::FolderRequest {
                    token,
                    folder: folder.into(),
                },
            };
            refuse(&channel, request)?;
            let (code, compressed) = read_frame(|bytes| {
                remote.read_exact(bytes)?;
                Ok(())
            })?;
            assert_eq!(code, if folder.is_some() { 37 } else { 5 });
            let mut bytes = Vec::new();
            flate2::read::ZlibDecoder::new(compressed.as_slice()).read_to_end(&mut bytes)?;
            let mut fields = Fields(&bytes);
            if let Some((token, folder)) = folder {
                assert_eq!(fields.u32()?, token);
                assert_eq!(fields.text(4096)?, folder);
            }
            assert_eq!(fields.u32()?, 0);
            if folder.is_none() {
                assert_eq!(fields.u32()?, 0);
                assert_eq!(fields.u32()?, 0);
            }
            fields.end()?;
        }
        Ok(())
    }
}
