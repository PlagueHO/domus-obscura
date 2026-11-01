//! Blocking owner for bounded async protocol adapters. No listener is needed.
use super::{
    ManagedTlsConfiguration,
    acme::{self, Acme},
    azure::AzureDns,
    dns::DnsPropagation,
    http::AcmeHttp,
    state::StateStore,
};
use crate::application::certificates::{CertificateProtocol, OrderState, Step};
use crate::domain::certificates::{CertificateNames, TxtRecord};
use crate::{Error, Result};
use serde_json::Value;
use std::sync::{Arc, Condvar, Mutex};
use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Protocol {
    runtime: tokio::runtime::Runtime,
    configuration: ManagedTlsConfiguration,
    names: CertificateNames,
    http: AcmeHttp,
    azure: AzureDns,
    dns: DnsPropagation,
    account: Option<Acme>,
    order: Option<instant_acme::Order>,
    live_tls: Option<axum_server::tls_rustls::RustlsConfig>,
    live_selection: Option<Arc<Mutex<Option<super::state::CertificateSelection>>>>,
}

impl Protocol {
    fn new(configuration: ManagedTlsConfiguration, hostname: &str) -> Result<Self> {
        let names = configuration.validate(hostname)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let dns = {
            let _entered = runtime.enter();
            DnsPropagation::new()?
        };
        let http = AcmeHttp::new(configuration.environment)?;
        let mut azure = AzureDns::new(configuration.dns.clone(), hostname)?;
        azure.pin_credential()?;
        Ok(Self {
            runtime,
            configuration,
            names,
            http,
            azure,
            dns,
            account: None,
            order: None,
            live_tls: None,
            live_selection: None,
        })
    }
}

impl CertificateProtocol for Protocol {
    fn account_key(&mut self) -> Result<String> {
        acme::account_key()
    }
    fn request(&mut self, names: &CertificateNames) -> Result<(String, Vec<u8>)> {
        acme::certificate_request(names)
    }
    fn account(&mut self, key: &str, existing_only: bool) -> Result<Value> {
        let operation = if existing_only {
            acme::AccountOperation::LookupExisting
        } else {
            acme::AccountOperation::Create {
                terms_of_service_agreed: self.configuration.terms_of_service_agreed,
            }
        };
        let (account, credentials) = self.runtime.block_on(Acme::register(
            &self.names,
            self.configuration.environment,
            self.http.clone(),
            key,
            operation,
        ))?;
        self.account = Some(account);
        Ok(credentials)
    }
    fn restore(&mut self, credentials: &Value, order: Option<&str>) -> Result<()> {
        let account = self.runtime.block_on(Acme::restore(
            &self.names,
            self.configuration.environment,
            self.http.clone(),
            credentials.clone(),
        ))?;
        self.order = order
            .map(|url| self.runtime.block_on(account.restore_order(url)))
            .transpose()?;
        self.account = Some(account);
        Ok(())
    }
    fn new_order(&mut self) -> Result<String> {
        let account = self
            .account
            .as_ref()
            .ok_or(Error::Internal("ACME adapter account is missing"))?;
        let order = self.runtime.block_on(account.new_order())?;
        let url = order.url().to_owned();
        self.order = Some(order);
        Ok(url)
    }
    fn challenge(&mut self) -> Result<Option<String>> {
        self.runtime.block_on(
            self.account
                .as_ref()
                .ok_or(Error::Internal("ACME account is missing"))?
                .challenge_value(
                    self.order
                        .as_mut()
                        .ok_or(Error::Internal("ACME order is missing"))?,
                ),
        )
    }
    fn read_dns(&mut self) -> Result<Option<TxtRecord>> {
        self.runtime.block_on(self.azure.read())
    }
    fn write_dns(&mut self, etag: Option<&str>, body: &Value) -> Result<TxtRecord> {
        self.runtime.block_on(self.azure.write_owned(etag, body))
    }
    fn propagated(&mut self, value: &str) -> Result<bool> {
        self.runtime
            .block_on(self.dns.propagated(&self.names, value))
    }
    fn notify(&mut self, value: &str) -> Result<()> {
        self.runtime.block_on(
            self.account
                .as_ref()
                .ok_or(Error::Internal("ACME account is missing"))?
                .notify_ready(
                    self.order
                        .as_mut()
                        .ok_or(Error::Internal("ACME order is missing"))?,
                    value,
                ),
        )
    }
    fn order_status(&mut self) -> Result<OrderState> {
        Ok(
            match self.runtime.block_on(
                self.account
                    .as_ref()
                    .ok_or(Error::Internal("ACME account is missing"))?
                    .refresh(
                        self.order
                            .as_mut()
                            .ok_or(Error::Internal("ACME order is missing"))?,
                    ),
            )? {
                instant_acme::OrderStatus::Pending => OrderState::Pending,
                instant_acme::OrderStatus::Ready => OrderState::Ready,
                instant_acme::OrderStatus::Processing => OrderState::Processing,
                instant_acme::OrderStatus::Valid => OrderState::Valid,
                instant_acme::OrderStatus::Invalid => OrderState::Invalid,
            },
        )
    }
    fn finalize(&mut self, csr: &[u8]) -> Result<()> {
        self.runtime.block_on(
            self.account
                .as_ref()
                .ok_or(Error::Internal("ACME account is missing"))?
                .finalize(
                    self.order
                        .as_mut()
                        .ok_or(Error::Internal("ACME order is missing"))?,
                    csr,
                ),
        )
    }
    fn certificate(&mut self) -> Result<Option<String>> {
        self.runtime.block_on(
            self.account
                .as_ref()
                .ok_or(Error::Internal("ACME account is missing"))?
                .certificate(
                    self.order
                        .as_mut()
                        .ok_or(Error::Internal("ACME order is missing"))?,
                ),
        )
    }
    fn renewal_window(&mut self, pem: &str) -> Result<crate::domain::certificates::RenewalAdvice> {
        Ok(
            match self.runtime.block_on(
                self.account
                    .as_ref()
                    .ok_or(Error::Internal("ACME account is missing"))?
                    .renewal_information(pem.as_bytes()),
            )? {
                acme::RenewalInformation::Unsupported => {
                    self.http.retry_delay()?;
                    crate::domain::certificates::RenewalAdvice::Unsupported
                }
                acme::RenewalInformation::Suggested {
                    start,
                    end,
                    refresh_after_seconds,
                } => {
                    let refresh_after_seconds =
                        refresh_after_seconds.max(self.http.retry_delay()?.unwrap_or(0));
                    crate::domain::certificates::RenewalAdvice::Suggested {
                        start,
                        end,
                        refresh_after_seconds,
                    }
                }
            },
        )
    }
    fn random(&mut self) -> Result<u64> {
        let mut bytes = [0; 8];
        rustls::crypto::ring::default_provider()
            .secure_random
            .fill(&mut bytes)
            .map_err(|_| Error::Internal("certificate renewal randomness failed"))?;
        Ok(u64::from_ne_bytes(bytes))
    }
    fn retry_delay(&mut self) -> Result<Option<u32>> {
        Ok(
            match (self.http.retry_delay()?, self.azure.retry_delay()?) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (left, right) => left.or(right),
            },
        )
    }
    fn activate(
        &mut self,
        selected: super::state::CertificateSelection,
        configuration: axum_server::tls_rustls::RustlsConfig,
    ) -> Result<()> {
        if let Some(live) = &self.live_tls {
            if let Some(selection) = &self.live_selection {
                let mut slot = selection
                    .lock()
                    .map_err(|_| Error::Internal("live certificate selection lock failed"))?;
                live.reload_from_config(configuration.get_inner());
                *slot = Some(selected);
            } else {
                return Err(Error::Internal("live certificate selection is missing"));
            }
        }
        Ok(())
    }
}

type Stop = Arc<(Mutex<bool>, Condvar)>;

/// A host gate stays owned while renewal is stopped for reload preparation.
/// Local commands also take this gate, so they cannot enter that handover gap.
pub(crate) struct HostGuard {
    root: crate::infrastructure::filesystem::NativeRoot,
    lock: crate::infrastructure::local_file::ProcessLock,
}
impl HostGuard {
    pub(crate) fn acquire(path: &Path, previous: Option<Arc<Self>>) -> Result<Arc<Self>> {
        let root = crate::infrastructure::filesystem::LibraryDirectory::open(path)?
            .native_root()
            .clone();
        if let Some(previous) = previous
            && previous.root == root
        {
            previous
                .lock
                .verify(&path.join("lifecycle.json"), "certificate-host")?;
            return Ok(previous);
        }
        let lock = crate::infrastructure::local_file::ProcessLock::acquire(
            &path.join("lifecycle.json"),
            "certificate-host",
        )?;
        if crate::infrastructure::filesystem::LibraryDirectory::open(path)?.native_root() != &root {
            return Err(Error::Conflict(
                "certificate host root changed during gate acquisition".into(),
            ));
        }
        Ok(Arc::new(Self { root, lock }))
    }
}

/// Keep the native lifecycle lock until the owner has saved and drained.
/// Call `drain` before host replacement and on every host exit path.
pub(crate) struct RenewalOwner {
    stop: Stop,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
}

impl RenewalOwner {
    pub(crate) fn start(
        configuration: ManagedTlsConfiguration,
        hostname: String,
        database: std::path::PathBuf,
        live_tls: axum_server::tls_rustls::RustlsConfig,
        live_selection: Arc<Mutex<Option<super::state::CertificateSelection>>>,
        recovery_required: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let owned_stop = stop.clone();
        let (sender, ready) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new().name("certificate-renewal".into()).spawn(move || {
            let initialized = (|| {
                let binding = configuration.binding(&hostname)?;
                let mut state = StateStore::open(&configuration.state_directory, &binding, configuration.environment)?;
                let database = crate::infrastructure::store::Store::open_read_current(&database)?;
                state.check_database_root(&database)?;
                let selected = live_selection.lock().map_err(|_| Error::Internal("live certificate selection lock failed"))?.clone();
                if let Some(selected) = selected
                    && (state.active_generation_number() != Some(selected.number) || state.selection(selected.number)? != selected) {
                    return Err(Error::Conflict("certificate selection changed after listener preparation; local recovery is required".into()));
                }
                state.bind_policy(&binding, &configuration.resource_binding(&hostname)?)?;
                let mut protocol = Protocol::new(configuration.clone(), &hostname)?;
                protocol.live_tls = Some(live_tls);
                protocol.live_selection = Some(live_selection);
                Ok::<_, Error>((state, protocol, binding))
            })();
            let (mut state, mut protocol, binding) = match initialized {
                Ok(value) => { sender.send(Ok(())).map_err(|_| Error::Internal("certificate owner acknowledgement failed"))?; value }
                Err(error) => { sender.send(Err(error)).map_err(|_| Error::Internal("certificate owner error acknowledgement failed"))?; return Ok(()); }
            };
            let outcome = (|| {
            let names = configuration.validate(&hostname)?;
            if let Some(seconds) = state.journal.retry_interval
                && stopped_after(&owned_stop, Duration::from_secs(u64::from(seconds)))? { return Ok(()); }
            loop {
                if stopped_after(&owned_stop, Duration::ZERO)? { return Ok(()); }
                if !matches!(configuration.binding(&hostname), Ok(current) if current == binding) {
                    crate::application::certificates::pause(&mut state, "certificate credential binding changed; stop the API before local recovery")?;
                }
                let timestamp = match now() {
                    Ok(timestamp) => timestamp,
                    Err(_) => {
                        crate::application::certificates::pause(&mut state, "system time is invalid; local recovery is required")?;
                        eprintln!("Certificate renewal paused because system time is invalid.");
                        while !stopped_after(&owned_stop, Duration::from_secs(86400))? {}
                        return Ok(());
                    }
                };
                let step = crate::application::certificates::advance(&mut state, &mut protocol, &names, timestamp);
                let delay = match step {
                    Ok(Step::Continue) => continue,
                    Ok(Step::Wait(seconds)) => Duration::from_secs(u64::from(seconds)),
                    Ok(Step::Ready) => Duration::from_secs(u64::from(state.journal.retry_interval.unwrap_or(60))),
                    Ok(Step::Paused) | Err(_) => {
                        recovery_required.store(true, std::sync::atomic::Ordering::Release);
                        eprintln!("Certificate renewal is paused; the current TLS configuration is retained. Stop the API before local recovery.");
                        while !stopped_after(&owned_stop, Duration::from_secs(86400))? {}
                        return Ok(());
                    }
                };
                if stopped_after(&owned_stop, delay)? { return Ok(()); }
            }
            })();
            if outcome.is_err() {
                recovery_required.store(true, std::sync::atomic::Ordering::Release);
                eprintln!("Certificate renewal stopped after a local failure; the current TLS configuration is retained.");
            }
            outcome
        })?;
        match ready.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                thread
                    .join()
                    .map_err(|_| Error::Internal("certificate owner initialization panicked"))??;
                Err(error)
            }
            Err(_) => {
                thread
                    .join()
                    .map_err(|_| Error::Internal("certificate owner initialization panicked"))??;
                Err(Error::Internal("certificate owner initialization stopped"))
            }
        }
    }

    pub(crate) async fn drain(&mut self) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        *self
            .stop
            .0
            .lock()
            .map_err(|_| Error::Internal("certificate stop guard was poisoned"))? = true;
        self.stop.1.notify_all();
        tokio::task::spawn_blocking(move || {
            thread
                .join()
                .map_err(|_| Error::Internal("certificate renewal owner panicked"))?
        })
        .await
        .map_err(|_| Error::Internal("certificate renewal drain task failed"))?
    }
}

fn stopped_after(stop: &Stop, duration: Duration) -> Result<bool> {
    let start = std::time::Instant::now();
    let mut stopped = stop
        .0
        .lock()
        .map_err(|_| Error::Internal("certificate stop guard was poisoned"))?;
    while !*stopped {
        let Some(remaining) = duration.checked_sub(start.elapsed()) else {
            break;
        };
        if remaining.is_zero() {
            break;
        }
        stopped = stop
            .1
            .wait_timeout(stopped, remaining)
            .map_err(|_| Error::Internal("certificate wait guard was poisoned"))?
            .0;
    }
    Ok(*stopped)
}

fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidInput("system time precedes the Unix epoch".into()))?
        .as_secs())
}

/// Recovery is an explicit local operation under the same native ownership.
/// Receipt adoption requires the operator's independent successful-write ETag.
/// Reading a matching TXT value alone never grants cleanup ownership.
pub fn recover(
    path: &Path,
    action: &str,
    evidence: Option<&str>,
) -> Result<super::state::CertificateStatus> {
    if !matches!(
        (action, evidence),
        (
            "resume"
                | "pending"
                | "bindings"
                | "cleanup"
                | "cleanup-owned"
                | "archive"
                | "generation"
                | "restart",
            None
        ) | ("order" | "write" | "receipt", Some(_))
    ) {
        return Err(Error::InvalidInput(
            "invalid certificate recovery action or evidence".into(),
        ));
    }
    let (configuration, hostname, database) =
        crate::infrastructure::server::configuration::ConfigurationService::certificate_operation_configuration(path)?;
    let binding = configuration.binding(&hostname)?;
    if action == "order" {
        super::http::endpoint(
            evidence.ok_or(Error::Internal("order evidence missing"))?,
            configuration.environment.authority(),
        )?;
    }
    if matches!(action, "write" | "receipt")
        && evidence.is_none_or(|etag| {
            etag.is_empty() || etag.len() > 512 || etag.bytes().any(|byte| !byte.is_ascii_graphic())
        })
    {
        return Err(Error::InvalidInput(
            "certificate recovery ETag is invalid".into(),
        ));
    }
    let mut state = StateStore::open_recovery(
        &configuration.state_directory,
        configuration.environment,
        action == "pending",
    )?;
    let _host_gate = HostGuard::acquire(&configuration.state_directory, None)?;
    state.check_database_root(&database)?;
    if action == "bindings" {
        state.recover_binding(
            &binding,
            &hostname,
            &configuration.resource_binding(&hostname)?,
        )?;
    } else {
        if state.journal.binding != binding {
            return Err(Error::Conflict(
                "recover the original certificate configuration before unfinished work".into(),
            ));
        }
        if action == "archive" {
            state.archive_generations()?;
            return StateStore::status(
                &configuration.state_directory,
                &binding,
                configuration.environment,
            );
        }
        if action == "pending" {
            state.recover_pending()?;
            return StateStore::status(
                &configuration.state_directory,
                &binding,
                configuration.environment,
            );
        }
        if state.journal.phase != crate::domain::certificates::CertificatePhase::Paused {
            return Err(Error::Conflict(
                "certificate recovery requires a paused lifecycle".into(),
            ));
        }
        state.preserve_recovery_evidence()?;
        use crate::domain::certificates::{CertificatePhase as Phase, TxtReceipt};
        match action {
            "generation" | "restart" => {
                if action == "restart"
                    && (state.journal.dns_intent.is_some()
                        || state.journal.dns_receipt.is_some()
                        || state.journal.candidate_generation.is_some())
                {
                    return Err(Error::Conflict(
                        "complete owned DNS cleanup before restarting failed certificate work"
                            .into(),
                    ));
                }
                let mut protocol = Protocol::new(configuration.clone(), &hostname)?;
                protocol.restore(
                    state
                        .journal
                        .account
                        .as_ref()
                        .ok_or(Error::Internal("saved account is missing"))?,
                    Some(state.journal.order_url.as_deref().ok_or_else(|| {
                        Error::Conflict("recover the original order URL first".into())
                    })?),
                )?;
                let observed = protocol.order_status()?;
                if action == "generation" {
                    if observed != OrderState::Valid {
                        return Err(Error::Conflict(
                            "generation recovery requires the original valid order".into(),
                        ));
                    }
                    let certificate = protocol
                        .certificate()?
                        .ok_or(Error::Remote("original order certificate is not available"))?;
                    state.recover_generation(&certificate, &hostname)?;
                    return StateStore::status(
                        &configuration.state_directory,
                        &binding,
                        configuration.environment,
                    );
                }
                if observed != OrderState::Invalid {
                    return Err(Error::Conflict(
                        "only an independently observed invalid order permits explicit restart"
                            .into(),
                    ));
                }
                state.journal.order_url = None;
                state.journal.certificate_key = None;
                state.journal.csr = None;
                state.journal.challenge = None;
                state.journal.phase = Phase::Uninitialized;
            }
            "cleanup-owned" | "receipt" => {
                if state.journal.dns_intent.is_some() {
                    return Err(Error::Conflict(
                        "reconcile the saved DNS intent before owned-value cleanup".into(),
                    ));
                }
                let receipt = state.journal.dns_receipt.clone().ok_or_else(|| {
                    Error::Conflict("successful owned-value receipt is missing".into())
                })?;
                let mut protocol = Protocol::new(configuration.clone(), &hostname)?;
                let record = protocol
                    .read_dns()?
                    .ok_or_else(|| Error::Conflict("owned TXT resource is missing".into()))?;
                if action == "receipt" {
                    if evidence != Some(record.etag.as_str()) {
                        return Err(Error::Conflict(
                            "operator-confirmed current TXT ETag does not match".into(),
                        ));
                    }
                    let refreshed = crate::domain::certificates::TxtReceipt {
                        value: receipt.value,
                        etag: record.etag.clone(),
                    };
                    record.remove(&refreshed)?;
                    state.journal.dns_receipt = Some(refreshed);
                } else {
                    let intent = crate::application::certificates::DnsIntent {
                        remove: true,
                        etag: Some(record.etag.clone()),
                        body: record.remove(&receipt)?,
                    };
                    crate::application::certificates::submit_dns(
                        &mut state,
                        &mut protocol,
                        &intent,
                    )?;
                    state.journal.dns_receipt = None;
                    state.journal.dns_intent = None;
                }
                state.save()?;
                return StateStore::status(
                    &configuration.state_directory,
                    &binding,
                    configuration.environment,
                );
            }
            "resume" => {
                if state.journal.dns_intent.is_some() {
                    return Err(Error::Conflict(
                        "reconcile the saved DNS intent before resuming".into(),
                    ));
                }
                let phase = state
                    .journal
                    .resume_phase
                    .ok_or(Error::Internal("saved recovery phase is missing"))?;
                if phase == Phase::OrderPending {
                    return Err(Error::Conflict(
                        "supply the independently verified saved order URL".into(),
                    ));
                }
                state.journal.phase = phase;
            }
            "order" => {
                if state.journal.resume_phase != Some(Phase::OrderPending)
                    || state.journal.order_url.is_some()
                {
                    return Err(Error::Conflict(
                        "order adoption requires an uncertain new-order URL".into(),
                    ));
                }
                let url = evidence.ok_or(Error::Internal("order recovery evidence is missing"))?;
                let mut protocol = Protocol::new(configuration.clone(), &hostname)?;
                protocol.restore(
                    state
                        .journal
                        .account
                        .as_ref()
                        .ok_or(Error::Internal("saved account is missing"))?,
                    Some(url),
                )?;
                protocol.challenge()?;
                state.journal.order_url = Some(url.into());
                state.journal.phase = Phase::DnsPending;
            }
            "write" | "cleanup" => {
                let intent: crate::application::certificates::DnsIntent = serde_json::from_value(
                    state
                        .journal
                        .dns_intent
                        .clone()
                        .ok_or_else(|| Error::Conflict("saved DNS intent is missing".into()))?,
                )
                .map_err(|_| Error::InvalidInput("saved DNS intent is invalid".into()))?;
                if intent.remove != (action == "cleanup") {
                    return Err(Error::Conflict(
                        "DNS recovery action differs from its saved intent".into(),
                    ));
                }
                let mut protocol = Protocol::new(configuration.clone(), &hostname)?;
                let record = protocol
                    .read_dns()?
                    .ok_or_else(|| Error::Conflict("DNS recovery resource is missing".into()))?;
                let mut properties = record.properties.clone();
                properties
                    .as_object_mut()
                    .ok_or(Error::Remote("DNS recovery properties are invalid"))?
                    .remove("fqdn");
                if properties != intent.body["properties"] {
                    return Err(Error::Conflict(
                        "DNS recovery observation differs from its complete saved intent".into(),
                    ));
                }
                if action == "write" {
                    if evidence != Some(record.etag.as_str()) {
                        return Err(Error::Conflict(
                            "DNS recovery ETag differs from the independent write receipt".into(),
                        ));
                    }
                    state.journal.dns_receipt = Some(TxtReceipt {
                        etag: record.etag,
                        value: state
                            .journal
                            .challenge
                            .clone()
                            .ok_or(Error::Internal("saved challenge is missing"))?,
                    });
                    state.journal.phase = Phase::PropagationPending;
                } else {
                    if state.journal.dns_receipt.is_none() {
                        return Err(Error::Conflict(
                            "DNS cleanup recovery receipts are incomplete".into(),
                        ));
                    }
                    state.journal.dns_receipt = None;
                    if state.journal.candidate_generation.is_none() {
                        state.journal.dns_intent = None;
                        state.save()?;
                        return StateStore::status(
                            &configuration.state_directory,
                            &binding,
                            configuration.environment,
                        );
                    }
                    state.journal.phase = Phase::CleanupPending;
                }
                state.journal.dns_intent = None;
            }
            _ => return Err(Error::Internal("validated recovery action changed")),
        }
        state.journal.pause_reason = None;
        state.journal.resume_phase = None;
        state.journal.attempts = 0;
        state.journal.next_attempt = None;
        state.journal.retry_interval = None;
        state.save()?;
    }
    StateStore::status(
        &configuration.state_directory,
        &binding,
        configuration.environment,
    )
}

/// Explicit first issuance creates only the separately configured private root.
/// Existing state is resumed under the same OS lock, never initialized again.
pub fn bootstrap(path: &Path) -> Result<super::state::CertificateStatus> {
    let (configuration, hostname, database) =
        crate::infrastructure::server::configuration::ConfigurationService::certificate_operation_configuration(path)?;
    let binding = configuration.binding(&hostname)?;
    let candidate = super::StateDirectory::inspect(&configuration.state_directory)?;
    let mut state = if candidate.exists {
        StateStore::open(
            &configuration.state_directory,
            &binding,
            configuration.environment,
        )?
    } else {
        StateStore::initialize(
            &configuration.state_directory,
            &binding,
            configuration.environment,
        )?
    };
    let _host_gate = HostGuard::acquire(&configuration.state_directory, None)?;
    state.bind_database_root(&database)?;
    state.bind_policy(&binding, &configuration.resource_binding(&hostname)?)?;
    let names = configuration.validate(&hostname)?;
    let mut protocol = Protocol::new(configuration.clone(), &hostname)?;
    if let Some(seconds) = state.journal.retry_interval {
        std::thread::sleep(Duration::from_secs(u64::from(seconds)));
    }
    for _ in 0..64 {
        if configuration.binding(&hostname)? != binding {
            return Err(Error::Conflict(
                "certificate credential binding changed; use explicit local recovery".into(),
            ));
        }
        match crate::application::certificates::advance(&mut state, &mut protocol, &names, now()?)?
        {
            Step::Continue => {}
            Step::Wait(seconds) => std::thread::sleep(Duration::from_secs(u64::from(seconds))),
            Step::Ready => {
                return StateStore::status(
                    &configuration.state_directory,
                    &binding,
                    configuration.environment,
                );
            }
            Step::Paused => {
                return Err(Error::Certificate {
                    reason: "certificate lifecycle is paused; explicit local recovery is required",
                    retry_after_seconds: None,
                });
            }
        }
    }
    Err(Error::Certificate {
        reason: "certificate lifecycle step budget was exceeded; inspect its saved state",
        retry_after_seconds: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificates_spurious_wakes_do_not_shorten_a_monotonic_wait() -> Result<()> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let wake = stop.clone();
        let thread = std::thread::spawn(move || {
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(5));
                wake.1.notify_all();
            }
        });
        let start = std::time::Instant::now();
        assert!(!stopped_after(&stop, Duration::from_millis(50))?);
        assert!(start.elapsed() >= Duration::from_millis(50));
        thread
            .join()
            .map_err(|_| Error::Internal("fixture wake thread failed"))?;
        *stop
            .0
            .lock()
            .map_err(|_| Error::Internal("fixture stop guard failed"))? = true;
        assert!(stopped_after(&stop, Duration::from_secs(86400))?);
        Ok(())
    }

    #[tokio::test]
    async fn certificates_paused_owner_drains_and_the_host_gate_excludes_handover_commands()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = crate::infrastructure::permissions::private_test_directory()?;
        let secret = directory.path().join("dns-secret.txt");
        std::fs::write(&secret, "generated-fixture-secret")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600))?;
        }
        let configuration = ManagedTlsConfiguration {
            environment: crate::domain::certificates::AcmeEnvironment::Staging,
            state_directory: directory.path().join("certificates"),
            terms_of_service_agreed: true,
            dns: super::super::azure::AzureDnsConfiguration {
                tenant_id: "11111111-1111-1111-1111-111111111111".into(),
                client_id: "22222222-2222-2222-2222-222222222222".into(),
                subscription_id: "33333333-3333-3333-3333-333333333333".into(),
                resource_group: "fixture-dns".into(),
                zone: "example.com".into(),
                client_secret_file: secret,
            },
        };
        let binding = configuration.binding("media.example.com")?;
        let database_path = directory.path().join("state.db");
        let database = crate::infrastructure::store::Store::open(&database_path)?;
        let mut state = StateStore::initialize(
            &configuration.state_directory,
            &binding,
            configuration.environment,
        )?;
        state.bind_database_root(&database)?;
        crate::application::certificates::pause(&mut state, "fixture requires explicit recovery")?;
        drop(state);
        drop(database);
        let gate = HostGuard::acquire(&configuration.state_directory, None)?;
        let reused = HostGuard::acquire(&configuration.state_directory, Some(gate.clone()))?;
        assert!(Arc::ptr_eq(&gate, &reused));
        let generated = rcgen::generate_simple_self_signed(vec!["media.example.com".into()])?;
        let tls = crate::infrastructure::tls::configuration(
            "media.example.com",
            generated.cert.pem().as_bytes(),
            generated.signing_key.serialize_pem().as_bytes(),
        )?;
        let path = configuration.state_directory.clone();
        let environment = configuration.environment;
        let mut owner = tokio::task::spawn_blocking(move || {
            RenewalOwner::start(
                configuration,
                "media.example.com".into(),
                database_path,
                tls,
                Arc::new(Mutex::new(None)),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
        })
        .await??;
        assert!(StateStore::open(&path, &binding, environment).is_err());
        tokio::time::timeout(Duration::from_secs(10), owner.drain()).await??;
        let state = StateStore::open(&path, &binding, environment)?;
        assert!(matches!(
            HostGuard::acquire(&path, None),
            Err(Error::Busy(_))
        ));
        assert_eq!(
            state.journal.phase,
            crate::domain::certificates::CertificatePhase::Paused
        );
        drop(state);
        drop(reused);
        drop(gate);
        assert!(HostGuard::acquire(&path, None).is_ok());
        Ok(())
    }
}
