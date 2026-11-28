//! Private developer supervision uses production hosts and their normal drain.
//! Child control is an inherited stdin pipe, not an HTTP operation or PID file.
use super::configuration::{ConfigurationService, Environment};
use crate::infrastructure::permissions::FilePermissions;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::future::Future;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const READY: &str = "DOMUS_CHILD_READY";
const LOG_LINES: usize = 100;
const LINE_BYTES: u64 = 4096;
const RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

pub fn inspect(path: &Path) -> Result<Value> {
    let source = ConfigurationService::open(path)?;
    let validation = source.validate_saved()?;
    Ok(json!({
        "configuration": source.read()?,
        "validation": validation,
        "startup_verified": false,
        "providers_started": false
    }))
}

/// Create a fresh provider-disabled HTTP setup. Existing targets are refused.
pub fn setup(root: &Path) -> Result<PathBuf> {
    super::bootstrap::initialize_developer_at(root)
}

/// The child can receive control only through a pipe inherited from its owner.
/// Losing that pipe latches stop; it never abandons application drain.
pub async fn child(role: &str, path: &Path, revision: &str) -> Result<()> {
    let source = tokio::task::block_in_place(|| ConfigurationService::open(path))?;
    if source.active_revision() != revision {
        return Err(Error::Conflict(
            "developer child configuration changed".into(),
        ));
    }
    let lifecycle = crate::infrastructure::lifecycle::Lifecycle::controlled(|| {
        println!("{READY}");
        std::io::stdout().flush()?;
        Ok(())
    });
    let control = lifecycle.clone();
    std::thread::spawn(move || {
        let mut input = String::new();
        let result = std::io::stdin().lock().take(8).read_line(&mut input);
        if result.is_err() || input != "stop\n" {
            eprintln!("Developer child control closed or invalid; draining.");
        }
        control.request_stop();
    });
    match role {
        "api" => {
            let (_lock, server) = tokio::task::block_in_place(|| {
                Ok::<_, Error>((
                    source.state_lock("developer-api")?,
                    super::PreparedServer::load(path)?,
                ))
            })?;
            server.run_with_lifecycle(lifecycle).await
        }
        "worker" => {
            tokio::task::block_in_place(|| {
                crate::infrastructure::production_worker::PreparedProductionWorker::load(path)
            })?
            .run_with_lifecycle(lifecycle)
            .await
        }
        _ => Err(Error::InvalidInput("unknown developer child role".into())),
    }
}

struct ManagedChild {
    role: &'static str,
    process: Child,
    messages: Receiver<String>,
}

impl ManagedChild {
    fn start(role: &'static str, path: &Path, revision: &str) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .env("DOMUS_NATIVE_SUPERVISED", "1")
            .arg("developer-child")
            .arg(role)
            .arg(path)
            .arg(revision)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, _) in std::env::vars_os() {
            let key = name.to_string_lossy();
            if key == "GH_TOKEN" || key.starts_with("COPILOT_GH_ACCOUNT_") {
                command.env_remove(name);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0000_0200);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut process = command.spawn()?;
        let (sender, messages) = mpsc::sync_channel(LOG_LINES);
        for stream in [
            Box::new(
                process
                    .stdout
                    .take()
                    .ok_or(Error::Internal("child stdout missing"))?,
            ) as Box<dyn std::io::Read + Send>,
            Box::new(
                process
                    .stderr
                    .take()
                    .ok_or(Error::Internal("child stderr missing"))?,
            ),
        ] {
            let sender = sender.clone();
            std::thread::spawn(move || forward_logs(stream, sender));
        }
        Ok(Self {
            role,
            process,
            messages,
        })
    }

    fn collect(&mut self, logs: &mut VecDeque<String>) -> bool {
        let mut ready = false;
        for message in self.messages.try_iter().take(LOG_LINES) {
            ready |= message == READY;
            if message != READY {
                let line = format!("{}: {message}", self.role);
                eprintln!("{line}");
                if logs.len() == LOG_LINES {
                    logs.pop_front();
                }
                logs.push_back(line);
            }
        }
        ready
    }

    async fn wait_ready(&mut self, logs: &mut VecDeque<String>) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if self.collect(logs) {
                return Ok(());
            }
            if let Some(status) = self.process.try_wait()? {
                return Err(Error::Conflict(format!(
                    "{} exited before readiness: {status}",
                    self.role
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Conflict(format!(
                    "{} readiness timed out",
                    self.role
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn drain(&mut self, logs: &mut VecDeque<String>) -> Result<()> {
        if let Some(mut input) = self.process.stdin.take()
            && let Err(error) = input.write_all(b"stop\n")
        {
            eprintln!(
                "Developer {} stop delivery failed: {error}; waiting for exit.",
                self.role
            );
        }
        loop {
            self.collect(logs);
            if let Some(status) = self.process.try_wait()? {
                self.collect(logs);
                return if status.success() {
                    Ok(())
                } else {
                    Err(Error::Conflict(format!(
                        "{} exited with {status}; inspect receipts before recovery",
                        self.role
                    )))
                };
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn forward_logs(stream: impl Read, sender: SyncSender<String>) {
    let mut reader = std::io::BufReader::new(stream);
    loop {
        let mut bytes = Vec::new();
        let message = match reader
            .by_ref()
            .take(LINE_BYTES + 1)
            .read_until(b'\n', &mut bytes)
        {
            Ok(0) => break,
            Ok(_) if bytes.len() as u64 <= LINE_BYTES => {
                String::from_utf8_lossy(&bytes).trim_end().into()
            }
            Ok(_) => {
                if !bytes.ends_with(b"\n")
                    && let Err(error) = reader.skip_until(b'\n')
                {
                    let _ = sender.send(format!("ERROR: developer child log read failed: {error}"));
                    break;
                }
                "ERROR: developer child log exceeded line limit".into()
            }
            Err(error) => {
                let _ = sender.send(format!("ERROR: developer child log read failed: {error}"));
                break;
            }
        };
        if sender.send(message).is_err() {
            break;
        }
    }
}

async fn drain(children: &mut [ManagedChild], logs: &mut VecDeque<String>) -> Result<()> {
    let mut failure = None;
    for child in children.iter_mut().rev() {
        if let Err(error) = child.drain(logs).await {
            eprintln!("Developer shutdown: {error}");
            failure = Some(error);
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn verify_browser(source: &ConfigurationService) -> Result<()> {
    verify_probe(tokio::task::block_in_place(|| probe_info(source))?).await
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    origin: String,
    revision: String,
    worker: bool,
    certificate: Option<String>,
    browser_sha256: String,
}

fn probe_info(source: &ConfigurationService) -> Result<Probe> {
    use sha2::{Digest, Sha256};
    let configuration = &source.active;
    if matches!(configuration.environment, Environment::Production) {
        return Err(Error::InvalidInput(
            "developer probes require explicit local authentication".into(),
        ));
    }
    let certificate = if matches!(configuration.environment, Environment::Development) {
        let path = configuration
            .certificate_file
            .as_ref()
            .ok_or(Error::InvalidInput(
                "developer HTTPS certificate is missing".into(),
            ))?;
        Some(
            String::from_utf8(source.read_input(path, 1024 * 1024, false)?)
                .map_err(|_| Error::InvalidInput("public certificate is not PEM text".into()))?,
        )
    } else {
        None
    };
    let assets = configuration
        .browser_assets
        .as_ref()
        .ok_or(Error::InvalidInput(
            "built browser assets are missing".into(),
        ))?;
    let file = crate::infrastructure::filesystem::LibraryDirectory::open(assets)?
        .open_file(std::ffi::OsStr::new("index.html"))?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::InvalidInput(
            "browser index exceeds probe limit".into(),
        ));
    }
    Ok(Probe {
        origin: configuration.origin.clone(),
        revision: source.active_revision().into(),
        worker: configuration.production_worker.is_some(),
        certificate,
        browser_sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

pub fn remote_probe_info(path: &Path) -> Result<Probe> {
    probe_info(&ConfigurationService::open(path)?)
}

pub async fn verify_probe(probe: Probe) -> Result<()> {
    use sha2::{Digest, Sha256};
    crate::infrastructure::tls::initialize_crypto()?;
    let url = reqwest::Url::parse(&probe.origin)
        .map_err(|_| Error::InvalidInput("invalid developer origin".into()))?;
    let local = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if !local
        || !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::InvalidInput(
            "developer probes require an exact loopback origin".into(),
        ));
    }
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5));
    if let Some(certificate) = &probe.certificate {
        builder = builder.add_root_certificate(
            reqwest::Certificate::from_pem(certificate.as_bytes()).map_err(probe_error)?,
        );
    }
    let client = builder.build().map_err(probe_error)?;
    let html = request_bytes(&client, &probe.origin, "/").await?;
    if format!("{:x}", Sha256::digest(html)) != probe.browser_sha256 {
        return Err(Error::Conflict(
            "served browser differs from selected built assets".into(),
        ));
    }
    let session: Value =
        serde_json::from_slice(&request_bytes(&client, &probe.origin, "/api/session").await?)?;
    if session["object_id"] != "_anonymous" {
        return Err(Error::Conflict(
            "local session did not confirm the anonymous identity".into(),
        ));
    }
    let configuration: Value = serde_json::from_slice(
        &request_bytes(&client, &probe.origin, "/api/system/configuration").await?,
    )?;
    if configuration["active_revision"] != probe.revision {
        return Err(Error::Conflict(
            "API revision differs from selected configuration".into(),
        ));
    }
    if probe.worker {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let status: Value = serde_json::from_slice(
                &request_bytes(&client, &probe.origin, "/api/local/status").await?,
            )?;
            if status["worker"]["online"] == true
                && status["worker"]["active_revision"] == probe.revision
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Conflict(
                    "worker heartbeat does not confirm the selected revision".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Ok(())
}

async fn request_bytes(client: &reqwest::Client, origin: &str, resource: &str) -> Result<Vec<u8>> {
    let mut response = client
        .get(format!("{origin}{resource}"))
        .send()
        .await
        .map_err(probe_error)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(Error::Conflict(format!(
            "developer readiness {resource}: {}",
            response.status()
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(probe_error)? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(Error::InvalidInput(
                "developer readiness response exceeded its limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn probe_error(error: reqwest::Error) -> Error {
    Error::Conflict(format!(
        "developer readiness request failed: {}",
        error.without_url()
    ))
}

/// Run in the foreground. Stop/restart commands never terminate saved PIDs.
pub async fn run(path: &Path) -> Result<()> {
    let (source, validation) = tokio::task::block_in_place(|| {
        let source = ConfigurationService::open(path)?;
        let validation = source.validate_saved()?;
        Ok::<_, Error>((source, validation))
    })?;
    if !validation.errors.is_empty() {
        return Err(Error::InvalidInput(serde_json::to_string(
            &validation.errors,
        )?));
    }
    if matches!(source.active.environment, Environment::Production) {
        return Err(Error::InvalidInput(
            "developer supervision requires an explicit local authentication mode".into(),
        ));
    }
    let (_lock, endpoint) = tokio::task::block_in_place(|| {
        Ok::<_, Error>((source.supervisor_lock()?, endpoint(path)?))
    })?;
    let mut listener = tokio::task::block_in_place(|| ControlListener::bind(&endpoint))?;
    let mut children = Vec::new();
    let mut logs = VecDeque::new();
    let stop = crate::infrastructure::lifecycle::Lifecycle::foreground();
    let mut stop_connection = None;
    let mut origin = source.active.origin.clone();
    let mut source = source;
    loop {
        let mut primary_change = None;
        let mut account_change = None;
        let startup = async {
            children.push(ManagedChild::start("api", path, source.active_revision())?);
            children
                .last_mut()
                .ok_or(Error::Internal("API child missing"))?
                .wait_ready(&mut logs)
                .await?;
            if source.active.production_worker.is_some() {
                children.push(ManagedChild::start(
                    "worker",
                    path,
                    source.active_revision(),
                )?);
                children
                    .last_mut()
                    .ok_or(Error::Internal("worker child missing"))?
                    .wait_ready(&mut logs)
                    .await?;
            }
            verify_browser(&source).await
        };
        let startup = await_startup(startup, &mut listener, &stop, &mut stop_connection).await;
        if let Err(error) = startup {
            if stop_connection.is_none() {
                eprintln!("Developer startup failed: {error}");
            }
            let result = drain(&mut children, &mut logs).await;
            tokio::task::block_in_place(|| drop(listener));
            drop(_lock);
            if let Some(mut connection) = stop_connection {
                let response = match &result {
                    Ok(()) => json!({"state": "stopped"}),
                    Err(error) => json!({"state": "failed", "error": error.to_string()}),
                };
                write_response(&mut connection, response).await?;
                return result;
            }
            result?;
            return Err(error);
        }
        println!("Domus ready: {origin}");
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let outcome = async { loop {
        tokio::select! {
            stopped = stop.stopped() => break stopped.map_err(Error::from),
            connection = listener.accept() => {
                let mut connection = match connection { Ok(value) => value, Err(error) => break Err(error) };
                let request = tokio::time::timeout(Duration::from_secs(5), read_command(&mut connection)).await;
                match request {
                    Ok(Ok(command)) if command == "stop" => {
                        stop_connection = Some(connection);
                        break Ok(());
                    }
                    Ok(Ok(command)) if command == "status" => {
                        let (matches, inspection_error) = match tokio::task::block_in_place(|| source.read()) {
                            Ok(state) => (state.saved_revision == source.active_revision(), None),
                            Err(error) => (false, Some(error.to_string())),
                        };
                        reply_to_control(&mut connection, json!({
                            "state": if matches { "ready" } else { "configuration_changed" },
                            "inspection_error": inspection_error,
                            "saved_configuration_matches": matches, "origin": source.active.origin,
                            "revision": source.active_revision(),
                            "processes": children.iter().map(|child| json!({
                                "role": child.role, "pid": child.process.id()
                            })).collect::<Vec<_>>()
                        })).await;
                    }
                    Ok(Ok(command)) if command == "logs" => {
                        reply_to_control(&mut connection, json!({"lines": logs})).await;
                    }
                    Ok(Ok(command)) if primary_change_id(&command).is_some() => {
                        let pending = tokio::task::block_in_place(|| {
                            source.require_editable()?;
                            source.open_store(crate::infrastructure::store::StoreOpenMode::ReadCurrent)?
                                .pending_primary_library_change()
                        });
                        match pending {
                            Ok(Some(id)) if Some(id) == primary_change_id(&command) => {
                                write_response(&mut connection, json!({"state":"restarting","change_id":id})).await?;
                                primary_change = Some(id);
                                break Ok(());
                            }
                            Ok(_) => reply_to_control(&mut connection, json!({"error":"The requested library folder change is no longer pending. Check its saved result."})).await,
                            Err(error) => reply_to_control(&mut connection, json!({"error":error.to_string()})).await,
                        }
                    }
                    Ok(Ok(command)) if account_change_id(&command).is_some() => {
                        let requested = account_change_id(&command).ok_or(Error::Internal("account command ID missing"))?;
                        let approved = tokio::task::block_in_place(|| {
                            source.require_editable()?;
                            if source.read()?.saved_revision != source.active_revision() {
                                return Err(Error::Conflict("Host configuration changed. Check it before applying an account.".into()));
                            }
                            let store = source.open_store(crate::infrastructure::store::StoreOpenMode::ReadCurrent)?;
                            Ok::<_, Error>(store.pending_soulseek_account_change()? == Some(requested)
                                && store.soulseek_account_change(requested)?.state == "approved")
                        });
                        match approved {
                            Ok(true) => {
                                write_response(&mut connection, json!({"state":"restarting","change_id":requested})).await?;
                                account_change = Some(requested);
                                break Ok(());
                            }
                            Ok(false) => reply_to_control(&mut connection, json!({"error":"The requested account change is no longer approved. Check its saved result."})).await,
                            Err(error) => reply_to_control(&mut connection, json!({"error":error.to_string()})).await,
                        }
                    }
                    _ => {
                        reply_to_control(&mut connection, json!({"error": "invalid or timed-out local command"})).await;
                    }
                }
            }
            _ = tick.tick() => {
                let mut failure = None;
                for child in &mut children {
                    child.collect(&mut logs);
                    if let Some(status) = child.process.try_wait()? {
                        failure = Some(Error::Conflict(format!("{} unexpectedly exited: {status}", child.role)));
                        break;
                    }
                }
                if let Some(error) = failure { break Err(error); }
            }
        }
    }}.await;
        let result = drain(&mut children, &mut logs).await.and(outcome);
        if let Some(id) = primary_change {
            result?;
            children.clear();
            source = tokio::task::block_in_place(|| source.activate_stopped_primary(id))?;
            origin.clone_from(&source.active.origin);
            continue;
        }
        if let Some(id) = account_change {
            result?;
            children.clear();
            // Account activation can change only the account reference, not the browser origin.
            source = tokio::task::block_in_place(|| source.activate_stopped_soulseek_account(id))
                .map_err(|_| Error::Conflict(
                    "Soulseek account activation could not finish safely. Keep all components stopped and check the saved activation state.".into(),
                ))?;
            continue;
        }
        tokio::task::block_in_place(|| drop(listener));
        drop(_lock);
        if let Some(mut connection) = stop_connection {
            let response = match &result {
                Ok(()) => json!({"state": "stopped"}),
                Err(error) => json!({"state": "failed", "error": error.to_string()}),
            };
            write_response(&mut connection, response).await?;
        }
        return result;
    }
}

async fn await_startup(
    startup: impl Future<Output = Result<()>>,
    listener: &mut ControlListener,
    stop: &crate::infrastructure::lifecycle::Lifecycle,
    stop_connection: &mut Option<ControlConnection>,
) -> Result<()> {
    tokio::pin!(startup);
    loop {
        tokio::select! {
            result = &mut startup => return result,
            result = stop.stopped() => {
                result?;
                return Err(Error::Conflict("developer startup stopped before readiness".into()));
            }
            accepted = listener.accept() => match accepted {
                Ok(mut connection) => {
                    let request = tokio::time::timeout(
                        Duration::from_secs(5),
                        read_command(&mut connection),
                    ).await;
                    if matches!(request, Ok(Ok(command)) if command == "stop") {
                        *stop_connection = Some(connection);
                        return Err(Error::Conflict("developer startup stopped before readiness".into()));
                    }
                    reply_to_control(&mut connection, json!({"state": "starting"})).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn endpoint(path: &Path) -> Result<PathBuf> {
    crate::infrastructure::local_file::validate_reference(path)?;
    let parent = path
        .parent()
        .ok_or(Error::InvalidInput("configuration parent missing".into()))?;
    let directory = crate::infrastructure::filesystem::LibraryDirectory::open(parent)?;
    FilePermissions::current()?.check(&directory.directory_file()?, true)?;
    directory.require_local()?;
    #[cfg(windows)]
    {
        use sha2::{Digest, Sha256};
        let binding = serde_json::to_vec(directory.native_root())?;
        Ok(PathBuf::from(format!(
            r"\\.\pipe\domus-developer-{:x}",
            Sha256::digest(binding)
        )))
    }
    #[cfg(unix)]
    {
        Ok(parent.join(".domus-developer.sock"))
    }
}

async fn read_command(stream: &mut (impl AsyncRead + Unpin)) -> Result<String> {
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take(32)
        .read_until(b'\n', &mut bytes)
        .await?;
    if !bytes.ends_with(b"\n") || bytes.len() > 32 {
        return Err(Error::InvalidInput("invalid local control request".into()));
    }
    String::from_utf8(bytes[..bytes.len() - 1].to_vec())
        .map_err(|_| Error::InvalidInput("local control request is not UTF-8".into()))
}

async fn write_response(stream: &mut (impl AsyncWrite + Unpin), value: Value) -> Result<()> {
    let bytes = serde_json::to_vec(&value)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        stream.write_all(&bytes).await?;
        stream.shutdown().await
    })
    .await
    .map_err(|_| Error::Conflict("developer control response timed out".into()))??;
    Ok(())
}

async fn reply_to_control(stream: &mut (impl AsyncWrite + Unpin), value: Value) {
    if let Err(error) = write_response(stream, value).await {
        eprintln!("Developer control response failed: {error}");
    }
}

fn primary_change_id(command: &str) -> Option<i64> {
    command
        .strip_prefix("primary:")?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}

fn account_change_id(command: &str) -> Option<i64> {
    command
        .strip_prefix("account:")?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}

pub async fn control(path: &Path, command: &str) -> Result<Value> {
    if !["stop", "status", "logs"].contains(&command)
        && primary_change_id(command).is_none()
        && account_change_id(command).is_none()
    {
        return Err(Error::InvalidInput(
            "unknown local control operation".into(),
        ));
    }
    let endpoint = tokio::task::block_in_place(|| endpoint(path))?;
    #[cfg(windows)]
    let mut stream = {
        let stream = tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint)?;
        FilePermissions::current()?.check_pipe(&stream)?;
        stream
    };
    #[cfg(unix)]
    let mut stream = {
        let stream = tokio::net::UnixStream::connect(endpoint).await?;
        if stream.peer_cred()?.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::InvalidInput(
                "local control owner is not the current user".into(),
            ));
        }
        stream
    };
    stream.write_all(format!("{command}\n").as_bytes()).await?;
    read_response(stream).await
}

async fn read_response(stream: impl AsyncRead + Unpin) -> Result<Value> {
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(120),
        stream.take(RESPONSE_BYTES + 1).read_to_end(&mut bytes),
    )
    .await
    .map_err(|_| {
        Error::Conflict(
            "developer control timed out; shutdown is unconfirmed; do not restart".into(),
        )
    })??;
    if bytes.len() as u64 > RESPONSE_BYTES {
        return Err(Error::InvalidInput(
            "developer control response exceeded its limit".into(),
        ));
    }
    let response: Value = serde_json::from_slice(&bytes)?;
    if let Some(error) = response.get("error") {
        return Err(Error::Conflict(error.to_string()));
    }
    Ok(response)
}

#[cfg(windows)]
type ControlConnection = tokio::net::windows::named_pipe::NamedPipeServer;

#[cfg(windows)]
struct ControlListener {
    server: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    path: PathBuf,
}

#[cfg(windows)]
impl ControlListener {
    fn create(
        path: &Path,
        first: bool,
    ) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        let permissions = FilePermissions::current()?;
        let server =
            native_file_security::create_named_pipe(path, &permissions.pipe_security(), first)?;
        permissions.check_pipe(&server)?;
        Ok(server)
    }

    fn bind(path: &Path) -> Result<Self> {
        Ok(Self {
            server: Some(Self::create(path, true)?),
            path: path.to_owned(),
        })
    }

    async fn accept(&mut self) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        let server = self
            .server
            .as_mut()
            .ok_or(Error::Internal("local pipe listener missing"))?;
        server.connect().await?;
        let next = Self::create(&self.path, false)?;
        self.server
            .replace(next)
            .ok_or(Error::Internal("local pipe listener missing"))
    }
}

#[cfg(unix)]
type ControlConnection = tokio::net::UnixStream;

#[cfg(unix)]
struct ControlListener {
    listener: tokio::net::UnixListener,
    parent: crate::infrastructure::filesystem::LibraryDirectory,
    name: std::ffi::OsString,
    identity: rustix::fs::Stat,
}

#[cfg(unix)]
impl ControlListener {
    fn bind(path: &Path) -> Result<Self> {
        let parent = crate::infrastructure::filesystem::LibraryDirectory::open(
            path.parent()
                .ok_or(Error::Internal("socket parent missing"))?,
        )?;
        parent.require_local()?;
        FilePermissions::current()?.check(&parent.directory_file()?, true)?;
        let name = path
            .file_name()
            .ok_or(Error::Internal("socket name missing"))?
            .to_owned();
        match parent.symlink_metadata(&name) {
            Ok(_) => return Err(Error::Conflict(
                "local control path is occupied; inspect stopped ownership before explicit cleanup"
                    .into(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        parent.check_named_directory(&|| Ok(()))?;
        let identity = rustix::fs::statat(
            parent.directory_file()?,
            &name,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(std::io::Error::from)?;
        if rustix::fs::FileType::from_raw_mode(identity.st_mode) != rustix::fs::FileType::Socket {
            return Err(Error::Conflict("local control socket was replaced".into()));
        }
        Ok(Self {
            listener,
            parent,
            name,
            identity,
        })
    }

    async fn accept(&mut self) -> Result<tokio::net::UnixStream> {
        let (stream, _) = self.listener.accept().await?;
        if stream.peer_cred()?.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::InvalidInput(
                "local control peer is not the current user".into(),
            ));
        }
        Ok(stream)
    }
}

#[cfg(unix)]
impl Drop for ControlListener {
    fn drop(&mut self) {
        let cleanup = || -> Result<()> {
            let current = rustix::fs::statat(
                self.parent.directory_file()?,
                &self.name,
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(std::io::Error::from)?;
            if current.st_dev != self.identity.st_dev
                || current.st_ino != self.identity.st_ino
                || current.st_mode != self.identity.st_mode
            {
                return Err(Error::Conflict(
                    "local control socket changed; replacement was preserved".into(),
                ));
            }
            self.parent.remove_file(&self.name)?;
            Ok(())
        };
        if let Err(error) = cleanup() {
            eprintln!("Developer control cleanup failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_logs_discard_oversized_lines_without_closing_the_stream() {
        for length in [LINE_BYTES, LINE_BYTES + 1, LINE_BYTES * 3] {
            let mut input = vec![b'x'; length as usize];
            input.extend_from_slice(format!("\n{READY}\nlast line\n").as_bytes());
            let (sender, receiver) = mpsc::sync_channel(LOG_LINES);
            forward_logs(input.as_slice(), sender);
            assert_eq!(
                receiver.iter().collect::<Vec<_>>(),
                [
                    "ERROR: developer child log exceeded line limit",
                    READY,
                    "last line"
                ]
            );
        }
    }

    #[tokio::test]
    async fn developer_control_accepts_fully_escaped_logs_and_rejects_oversized_responses()
    -> Result<()> {
        let response = json!({"lines": vec!["\0".repeat(LINE_BYTES as usize - 1); LOG_LINES]});
        assert!(serde_json::to_vec(&response)?.len() > 512 * 1024);
        let (mut server, client) = tokio::io::duplex(16 * 1024);
        let (written, received) = tokio::join!(
            write_response(&mut server, response.clone()),
            read_response(client)
        );
        written?;
        assert_eq!(received?, response);

        let oversized = vec![b' '; RESPONSE_BYTES as usize + 1];
        let Err(error) = read_response(oversized.as_slice()).await else {
            return Err(Error::Internal(
                "oversized responses must report the input limit",
            ));
        };
        assert!(error.to_string().contains("exceeded its limit"));
        Ok(())
    }

    #[tokio::test]
    async fn developer_control_replies_bound_unresponsive_clients_and_survive_disconnects()
    -> Result<()> {
        let (mut stream, client) = tokio::io::duplex(16);
        let response = json!({"lines": ["x".repeat(4096)]});
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            write_response(&mut stream, response),
        )
        .await
        .map_err(|_| Error::Internal("an unread reply must not block supervision"))?;
        let Err(error) = result else {
            return Err(Error::Internal("an unread reply must report timeout"));
        };
        assert!(error.to_string().contains("response timed out"));
        drop(client);
        reply_to_control(&mut stream, json!({"state": "ready"})).await;
        Ok(())
    }

    #[tokio::test]
    async fn developer_control_rejects_incomplete_and_oversized_commands() -> Result<()> {
        for input in [
            b"stop".as_slice(),
            b"0123456789012345678901234567890123\n",
            b"\xff\n",
        ] {
            let mut reader = input;
            assert!(read_command(&mut reader).await.is_err());
        }
        assert_eq!(read_command(&mut b"status\n".as_slice()).await?, "status");
        assert_eq!(
            primary_change_id(
                &read_command(&mut b"primary:9223372036854775807\n".as_slice()).await?
            ),
            Some(i64::MAX)
        );
        for command in [
            "primary",
            "primary:0",
            "primary:-1",
            "primary:9223372036854775808",
        ] {
            assert_eq!(primary_change_id(command), None);
        }
        assert_eq!(
            account_change_id("account:9223372036854775807"),
            Some(i64::MAX)
        );
        for command in [
            "account",
            "account:0",
            "account:-1",
            "account:9223372036854775808",
        ] {
            assert_eq!(account_change_id(command), None);
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn developer_control_stop_interrupts_pending_startup() -> Result<()> {
        let fixture = crate::infrastructure::permissions::private_test_directory()?;
        let configuration = fixture.path().join("config.json");
        let mut listener = ControlListener::bind(&endpoint(&configuration)?)?;
        let stop = crate::infrastructure::lifecycle::Lifecycle::controlled(|| Ok(()));
        let mut stop_connection = None;
        let client_configuration = configuration.clone();
        let client = tokio::spawn(async move { control(&client_configuration, "stop").await });
        let startup = await_startup(
            std::future::pending::<Result<()>>(),
            &mut listener,
            &stop,
            &mut stop_connection,
        )
        .await;
        assert!(startup.is_err());
        let mut connection = stop_connection
            .take()
            .ok_or(Error::Internal("startup stop connection missing"))?;
        write_response(&mut connection, json!({"state": "stopped"})).await?;
        drop(connection);
        let response = client
            .await
            .map_err(|_| Error::Internal("startup stop client failed"))??;
        assert_eq!(response["state"], "stopped");
        Ok(())
    }

    #[tokio::test]
    async fn developer_probe_refuses_non_loopback_and_non_origin_inputs() -> Result<()> {
        for origin in [
            "https://example.com",
            "http://127.0.0.1/path",
            "http://user@localhost",
            "http://localhost?token=forbidden",
            "ftp://localhost",
        ] {
            assert!(
                verify_probe(Probe {
                    origin: origin.into(),
                    revision: String::new(),
                    worker: false,
                    certificate: None,
                    browser_sha256: String::new(),
                })
                .await
                .is_err()
            );
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn developer_private_socket_preserves_occupied_and_replaced_paths() -> Result<()> {
        let fixture = crate::infrastructure::permissions::private_test_directory()?;
        let path = fixture.path().join(".domus-developer.sock");
        let listener = ControlListener::bind(&path)?;
        assert!(ControlListener::bind(&path).is_err());
        std::fs::remove_file(&path)?;
        std::fs::write(&path, b"preserve replacement")?;
        drop(listener);
        assert_eq!(std::fs::read(&path)?, b"preserve replacement");
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn developer_private_pipe_round_trip_and_duplicate_refusal() -> Result<()> {
        let path = PathBuf::from(format!(
            r"\\.\pipe\domus-developer-test-{}",
            std::process::id()
        ));
        let mut listener = ControlListener::bind(&path)?;
        assert!(ControlListener::bind(&path).is_err());
        let client = tokio::spawn(async move {
            let mut client = tokio::net::windows::named_pipe::ClientOptions::new().open(path)?;
            FilePermissions::current()?.check_pipe(&client)?;
            client.write_all(b"status\n").await?;
            Result::Ok(())
        });
        let mut stream = listener.accept().await?;
        assert_eq!(read_command(&mut stream).await?, "status");
        client
            .await
            .map_err(|_| Error::Internal("pipe test task failed"))??;
        Ok(())
    }
}
