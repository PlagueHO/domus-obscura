//! One bounded lifecycle step under the private journal's native process lock.
//! Intent precedes submission. A lost receipt never becomes a fresh attempt.
use crate::domain::certificates::{
    AcmeEnvironment, CertificateNames, CertificatePhase as Phase, RenewalAdvice, TxtAddition,
    TxtReceipt, TxtRecord, renewal_schedule,
};
use crate::infrastructure::certificates::state::{CertificateSelection, Generation, StateStore};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A synchronous boundary lets journal work remain outside the async executor.
/// Implementations perform one bounded observation or submission, without retries.
pub trait CertificateProtocol {
    fn account_key(&mut self) -> Result<String>;
    fn request(&mut self, names: &CertificateNames) -> Result<(String, Vec<u8>)>;
    fn account(&mut self, key: &str, existing_only: bool) -> Result<Value>;
    fn restore(&mut self, credentials: &Value, order: Option<&str>) -> Result<()>;
    fn new_order(&mut self) -> Result<String>;
    fn challenge(&mut self) -> Result<Option<String>>;
    fn read_dns(&mut self) -> Result<Option<TxtRecord>>;
    fn write_dns(&mut self, etag: Option<&str>, body: &Value) -> Result<TxtRecord>;
    fn propagated(&mut self, value: &str) -> Result<bool>;
    fn notify(&mut self, value: &str) -> Result<()>;
    fn order_status(&mut self) -> Result<OrderState>;
    fn finalize(&mut self, csr: &[u8]) -> Result<()>;
    fn certificate(&mut self) -> Result<Option<String>>;
    fn renewal_window(&mut self, certificate: &str) -> Result<RenewalAdvice>;
    fn random(&mut self) -> Result<u64>;
    /// Returns the longest supported Retry-After observed in this step,
    /// including successful responses. Taking it must clear the observation.
    fn retry_delay(&mut self) -> Result<Option<u32>>;
    fn activate(
        &mut self,
        selection: CertificateSelection,
        configuration: axum_server::tls_rustls::RustlsConfig,
    ) -> Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderState {
    Pending,
    Ready,
    Processing,
    Valid,
    Invalid,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Continue,
    Wait(u32),
    Ready,
    Paused,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DnsIntent {
    pub remove: bool,
    pub etag: Option<String>,
    pub body: Value,
}

/// The caller consumes a saved retry interval with a monotonic wait before
/// advancing. Restarting waits the whole saved interval; wall clocks cannot
/// shorten it. Stop requests are admitted only between complete steps.
pub fn advance(
    state: &mut StateStore,
    protocol: &mut impl CertificateProtocol,
    names: &CertificateNames,
    now: u64,
) -> Result<Step> {
    state.check_current()?;
    if state.journal.phase == Phase::Paused {
        return Ok(Step::Paused);
    }
    if state.journal.next_attempt.is_some() || state.journal.retry_interval.is_some() {
        state.journal.next_attempt = None;
        state.journal.retry_interval = None;
        state.save()?;
    }
    let result = step(state, protocol, names, now);
    let delay = match protocol.retry_delay() {
        Ok(delay) => delay.unwrap_or(0),
        Err(error) => {
            pause(
                state,
                "certificate retry observation failed; local recovery is required",
            )?;
            return Err(error);
        }
    };
    match result {
        Ok(Step::Wait(seconds)) => wait(state, now, seconds.max(delay)),
        Ok(Step::Continue) if delay > 0 => wait(state, now, delay),
        Ok(Step::Ready) if delay > 0 => {
            let seconds = delay.max(state.journal.retry_interval.unwrap_or(0));
            state.journal.retry_interval = Some(seconds);
            state.journal.next_attempt = Some(
                now.checked_add(u64::from(seconds))
                    .ok_or(Error::Internal("certificate retry time overflow"))?,
            );
            state.save()?;
            Ok(Step::Ready)
        }
        Ok(result) => Ok(result),
        Err(Error::Certificate {
            retry_after_seconds: Some(seconds),
            ..
        }) if state.journal.phase != Phase::OrderPending && state.journal.dns_intent.is_none() => {
            wait(state, now, seconds.max(1).max(delay))
        }
        Err(error) => {
            pause(state, "certificate operation needs explicit local recovery")?;
            Err(error)
        }
    }
}

fn wait(state: &mut StateStore, now: u64, seconds: u32) -> Result<Step> {
    if seconds == 0 || seconds > 86400 || state.journal.attempts >= 8 {
        pause(
            state,
            "certificate retry budget or supported delay was exceeded",
        )?;
        return Ok(Step::Paused);
    }
    state.journal.attempts += 1;
    state.journal.retry_interval = Some(seconds);
    state.journal.next_attempt = Some(
        now.checked_add(u64::from(seconds))
            .ok_or(Error::Internal("certificate retry time overflow"))?,
    );
    state.save()?;
    Ok(Step::Wait(seconds))
}

pub(crate) fn pause(state: &mut StateStore, reason: &str) -> Result<()> {
    if state.journal.phase != Phase::Paused {
        state.journal.resume_phase = Some(state.journal.phase);
    }
    state.journal.pause_reason = Some(reason.into());
    state.journal.phase = Phase::Paused;
    state.journal.next_attempt = None;
    state.journal.retry_interval = None;
    state.save()
}

fn restore(state: &StateStore, protocol: &mut impl CertificateProtocol) -> Result<()> {
    protocol.restore(
        state
            .journal
            .account
            .as_ref()
            .ok_or(Error::Internal("saved ACME account is missing"))?,
        state.journal.order_url.as_deref(),
    )
}

pub(crate) fn submit_dns(
    state: &mut StateStore,
    protocol: &mut impl CertificateProtocol,
    intent: &DnsIntent,
) -> Result<TxtRecord> {
    state.journal.dns_intent = Some(serde_json::to_value(intent)?);
    state.save()?;
    match protocol.write_dns(intent.etag.as_deref(), &intent.body) {
        Err(Error::CertificateNoDnsEffect(error)) => {
            state.journal.dns_intent = None;
            state.save()?;
            Err(match *error {
                Error::CertificateDnsConflict => Error::Certificate {
                    reason: "Azure DNS conditional update conflicted; reread under the saved retry budget",
                    retry_after_seconds: Some(1),
                },
                error => error,
            })
        }
        // The Azure adapter preserves timed errors only before PUT submission.
        // Lost PUT responses never carry an automatic retry interval.
        Err(
            error @ Error::Certificate {
                retry_after_seconds: Some(_),
                ..
            },
        ) => {
            state.journal.dns_intent = None;
            state.save()?;
            Err(error)
        }
        result => result,
    }
}

fn step(
    state: &mut StateStore,
    protocol: &mut impl CertificateProtocol,
    names: &CertificateNames,
    now: u64,
) -> Result<Step> {
    match state.journal.phase {
        Phase::Uninitialized if state.journal.account.is_none() => {
            state.journal.account_key = Some(protocol.account_key()?);
            state.journal.phase = Phase::AccountPending;
            state.save()?;
            let credentials = protocol.account(
                state
                    .journal
                    .account_key
                    .as_deref()
                    .ok_or(Error::Internal("ACME account key is missing"))?,
                false,
            )?;
            state.journal.account = Some(credentials);
            state.journal.phase = Phase::Uninitialized;
            state.save()?;
        }
        Phase::AccountPending => {
            state.journal.account = Some(
                protocol.account(
                    state
                        .journal
                        .account_key
                        .as_deref()
                        .ok_or(Error::Internal("saved ACME account key is missing"))?,
                    true,
                )?,
            );
            state.journal.phase = Phase::Uninitialized;
            state.save()?;
        }
        Phase::Uninitialized => {
            restore(state, protocol)?;
            state.journal.phase = Phase::OrderPending;
            state.save()?;
            state.journal.order_url = Some(protocol.new_order()?);
            state.journal.phase = Phase::DnsPending;
            state.save()?;
        }
        Phase::OrderPending => {
            return Err(Error::Certificate {
                reason: "ACME order URL is uncertain; supply its verified URL through local recovery",
                retry_after_seconds: None,
            });
        }
        Phase::DnsPending => {
            if state.journal.dns_intent.is_some() {
                return Err(Error::Certificate {
                    reason: "DNS ownership is uncertain; local recovery is required",
                    retry_after_seconds: None,
                });
            }
            restore(state, protocol)?;
            let Some(value) = protocol.challenge()? else {
                state.journal.phase = Phase::ValidationPending;
                state.save()?;
                return Ok(Step::Continue);
            };
            if state
                .journal
                .challenge
                .as_ref()
                .is_some_and(|saved| saved != &value)
            {
                return Err(Error::Conflict("saved ACME challenge changed".into()));
            }
            state.journal.challenge = Some(value.clone());
            state.save()?;
            let record = protocol.read_dns()?;
            let body = match &record {
                Some(record) => match record.add(&value)? {
                    TxtAddition::Update(body) => body,
                    TxtAddition::AlreadyPresent => {
                        return Err(Error::Conflict(
                            "an existing TXT value is not owned; local recovery is required".into(),
                        ));
                    }
                },
                None => json!({"properties":{"TTL":60,"TXTRecords":[{"value":[value]}]}}),
            };
            let intent = DnsIntent {
                remove: false,
                etag: record.map(|record| record.etag),
                body,
            };
            let written = submit_dns(state, protocol, &intent)?;
            state.journal.dns_receipt = Some(TxtReceipt {
                value,
                etag: written.etag,
            });
            state.journal.dns_intent = None;
            state.journal.phase = Phase::PropagationPending;
            state.save()?;
        }
        Phase::PropagationPending => {
            let value = state
                .journal
                .challenge
                .as_deref()
                .ok_or(Error::Internal("saved DNS challenge is missing"))?;
            if !protocol.propagated(value)? {
                return Ok(Step::Wait(30));
            }
            state.journal.phase = Phase::ValidationPending;
            state.save()?;
        }
        Phase::ValidationPending => {
            restore(state, protocol)?;
            match protocol.order_status()? {
                OrderState::Pending => {
                    protocol.notify(
                        state
                            .journal
                            .challenge
                            .as_deref()
                            .ok_or(Error::Internal("saved challenge is missing"))?,
                    )?;
                    return Ok(Step::Wait(30));
                }
                OrderState::Ready => {
                    let (key, csr) = protocol.request(names)?;
                    state.journal.certificate_key = Some(key);
                    state.journal.csr = Some(csr);
                    state.journal.phase = Phase::FinalizationPending;
                    state.save()?;
                    protocol.finalize(
                        state
                            .journal
                            .csr
                            .as_deref()
                            .ok_or(Error::Internal("saved CSR is missing"))?,
                    )?;
                }
                _ => {
                    return Err(Error::Remote(
                        "ACME validation returned an unexpected order state",
                    ));
                }
            }
        }
        Phase::FinalizationPending => {
            restore(state, protocol)?;
            match protocol.order_status()? {
                OrderState::Ready => {
                    protocol.finalize(
                        state
                            .journal
                            .csr
                            .as_deref()
                            .ok_or(Error::Internal("saved CSR is missing"))?,
                    )?;
                    return Ok(Step::Wait(30));
                }
                OrderState::Processing => return Ok(Step::Wait(30)),
                OrderState::Valid => {}
                _ => return Err(Error::Remote("saved ACME order can no longer be finalized")),
            }
            let Some(certificate) = protocol.certificate()? else {
                return Ok(Step::Wait(30));
            };
            if let Some((&number, _)) = state.journal.generations.last_key_value() {
                let published = state.generation(number)?;
                if published.hostname == names.hostname
                    && published.environment == state.journal.environment
                    && Some(&published.private_key) == state.journal.certificate_key.as_ref()
                    && published.certificate == certificate
                {
                    if let Some(schedule) = published.initial_renewal {
                        state.journal.renewal = Some(schedule);
                    }
                    state.journal.candidate_generation = Some(number);
                    state.journal.phase = Phase::CleanupPending;
                    state.save()?;
                    return Ok(Step::Continue);
                }
            }
            let (not_before, not_after) = crate::infrastructure::tls::certificate_validity(
                &names.hostname,
                certificate.as_bytes(),
            )?;
            let schedule = renewal_schedule(
                state.next_generation_number(),
                (not_before, not_after),
                protocol.renewal_window(&certificate)?,
                None,
                protocol.random()?,
                now,
            )?;
            let generation = Generation {
                hostname: names.hostname.clone(),
                environment: state.journal.environment,
                certificate,
                private_key: state
                    .journal
                    .certificate_key
                    .clone()
                    .ok_or(Error::Internal("saved certificate key is missing"))?,
                not_before,
                not_after,
                renew_at: schedule.renew_at,
                initial_renewal: Some(schedule),
            };
            let number = state.publish_generation(&generation)?;
            if schedule.generation != number {
                return Err(Error::Internal("certificate renewal generation changed"));
            }
            state.journal.candidate_generation = Some(number);
            state.journal.phase = Phase::CleanupPending;
            state.save()?;
        }
        Phase::CleanupPending => {
            let candidate = state.journal.candidate_generation.ok_or(Error::Internal(
                "validated certificate candidate is missing",
            ))?;
            if state.journal.environment == AcmeEnvironment::Production
                && state.active_generation_number() != Some(candidate)
            {
                let (selection, configuration) =
                    state.activate_generation(candidate, &names.hostname)?;
                protocol.activate(selection, configuration)?;
            }
            if state.journal.dns_intent.is_some() {
                return Err(Error::Certificate {
                    reason: "DNS cleanup completion is uncertain; local recovery is required",
                    retry_after_seconds: None,
                });
            }
            if let Some(receipt) = state.journal.dns_receipt.as_ref() {
                let record = protocol.read_dns()?.ok_or_else(|| {
                    Error::Conflict(
                        "owned DNS resource is missing; verify cleanup through local recovery"
                            .into(),
                    )
                })?;
                let intent = DnsIntent {
                    remove: true,
                    etag: Some(record.etag.clone()),
                    body: record.remove(receipt)?,
                };
                submit_dns(state, protocol, &intent)?;
                state.journal.dns_receipt = None;
                state.journal.dns_intent = None;
            }
            state.journal.candidate_generation = None;
            state.journal.order_url = None;
            state.journal.challenge = None;
            state.journal.certificate_key = None;
            state.journal.csr = None;
            state.journal.last_success = Some(now);
            state.journal.attempts = 0;
            state.journal.phase = Phase::Ready;
            schedule_ready(state, now)?;
            state.save()?;
            return Ok(Step::Ready);
        }
        Phase::Ready => {
            let number = state
                .active_generation_number()
                .or_else(|| {
                    state
                        .journal
                        .generations
                        .last_key_value()
                        .map(|(number, _)| *number)
                })
                .ok_or(Error::Internal("ready certificate generation is missing"))?;
            let generation = state.generation(number)?;
            let schedule = state
                .journal
                .renewal
                .filter(|schedule| schedule.generation == number);
            if now >= schedule.map_or(generation.renew_at, |schedule| schedule.renew_at) {
                state.journal.phase = Phase::Uninitialized;
                state.save()?;
            } else if schedule
                .is_none_or(|schedule| schedule.refresh_at.is_some_and(|refresh| now >= refresh))
            {
                restore(state, protocol)?;
                state.journal.renewal = Some(renewal_schedule(
                    number,
                    (generation.not_before, generation.not_after),
                    protocol.renewal_window(&generation.certificate)?,
                    schedule,
                    protocol.random()?,
                    now,
                )?);
                schedule_ready(state, now)?;
                state.save()?;
                return Ok(Step::Ready);
            } else {
                if schedule.is_some_and(|schedule| schedule.refresh_interval.is_some()) {
                    schedule_ready(state, now)?;
                    state.save()?;
                }
                return Ok(Step::Ready);
            }
        }
        Phase::Paused => return Ok(Step::Paused),
    }
    Ok(Step::Continue)
}

fn schedule_ready(state: &mut StateStore, now: u64) -> Result<()> {
    if let Some(schedule) = state.journal.renewal
        && let Some(interval) = schedule.refresh_interval
    {
        let seconds = u64::from(interval).min(schedule.renew_at.saturating_sub(now).max(1));
        state.journal.retry_interval = Some(seconds as u32);
        state.journal.next_attempt = Some(
            now.checked_add(seconds)
                .ok_or(Error::Internal("certificate renewal time overflow"))?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        journal: PathBuf,
        key: String,
        certificate: String,
        record: Option<TxtRecord>,
        order: OrderState,
        creates: usize,
        lookups: usize,
        orders: usize,
        requests: usize,
        writes: usize,
        account_loss: bool,
        order_loss: bool,
        write_loss: bool,
        finalize_loss: bool,
        cleanup_loss: bool,
        propagated: bool,
        advice: RenewalAdvice,
        delay: Option<u32>,
        rejected_write: bool,
        conditional_conflicts: u8,
    }

    impl Fixture {
        fn new(path: PathBuf) -> Result<Self> {
            let key =
                rcgen::KeyPair::generate().map_err(|_| Error::Internal("fixture key failed"))?;
            let parameters = rcgen::CertificateParams::new(vec!["media.example.com".into()])
                .map_err(|_| Error::Internal("fixture parameters failed"))?;
            let certificate = parameters
                .self_signed(&key)
                .map_err(|_| Error::Internal("fixture certificate failed"))?
                .pem();
            Ok(Self {
                journal: path,
                key: key.serialize_pem(),
                certificate,
                record: None,
                order: OrderState::Ready,
                creates: 0,
                lookups: 0,
                orders: 0,
                requests: 0,
                writes: 0,
                account_loss: false,
                order_loss: false,
                write_loss: false,
                finalize_loss: false,
                cleanup_loss: false,
                propagated: true,
                advice: RenewalAdvice::Unsupported,
                delay: None,
                rejected_write: false,
                conditional_conflicts: 0,
            })
        }
        fn saved(&self) -> Result<Value> {
            Ok(serde_json::from_slice(&std::fs::read(&self.journal)?)?)
        }
    }
    fn loss() -> Error {
        Error::Certificate {
            reason: "fixture response lost",
            retry_after_seconds: None,
        }
    }
    impl CertificateProtocol for Fixture {
        fn account_key(&mut self) -> Result<String> {
            Ok("saved-account-key".into())
        }
        fn request(&mut self, _: &CertificateNames) -> Result<(String, Vec<u8>)> {
            self.requests += 1;
            Ok((self.key.clone(), vec![1, 2, 3]))
        }
        fn account(&mut self, key: &str, existing_only: bool) -> Result<Value> {
            let saved = self.saved()?;
            assert_eq!(saved["phase"], "account_pending");
            assert_eq!(saved["account_key"], key);
            if existing_only {
                self.lookups += 1;
            } else {
                self.creates += 1;
            }
            if self.account_loss && !existing_only {
                return Err(loss());
            }
            Ok(json!({"fixture":"private-account"}))
        }
        fn restore(&mut self, _: &Value, _: Option<&str>) -> Result<()> {
            Ok(())
        }
        fn new_order(&mut self) -> Result<String> {
            assert_eq!(self.saved()?["phase"], "order_pending");
            self.orders += 1;
            if self.order_loss {
                return Err(loss());
            }
            Ok("https://acme-staging-v02.api.letsencrypt.org/order/fixture".into())
        }
        fn challenge(&mut self) -> Result<Option<String>> {
            Ok(Some("a".repeat(43)))
        }
        fn read_dns(&mut self) -> Result<Option<TxtRecord>> {
            Ok(self.record.clone())
        }
        fn write_dns(&mut self, _: Option<&str>, body: &Value) -> Result<TxtRecord> {
            let saved = self.saved()?;
            assert_eq!(&saved["dns_intent"]["body"], body);
            if self.conditional_conflicts > 0 {
                self.conditional_conflicts -= 1;
                self.record = Some(TxtRecord {
                    etag: "\"concurrent-owner\"".into(),
                    properties: json!({"TTL":360,"TXTRecords":[{"value":["unrelated","-segmented"]}]}),
                });
                return Err(Error::CertificateNoDnsEffect(Box::new(
                    Error::CertificateDnsConflict,
                )));
            }
            if self.rejected_write {
                return Err(Error::CertificateNoDnsEffect(Box::new(
                    Error::Certificate {
                        reason: "fixture credentials were rejected before DNS submission",
                        retry_after_seconds: None,
                    },
                )));
            }
            self.writes += 1;
            let record = TxtRecord {
                etag: format!("\"{}\"", self.writes),
                properties: body["properties"].clone(),
            };
            self.record = Some(record.clone());
            if (self.write_loss && self.writes == 1) || (self.cleanup_loss && self.writes == 2) {
                return Err(loss());
            }
            Ok(record)
        }
        fn propagated(&mut self, _: &str) -> Result<bool> {
            Ok(self.propagated)
        }
        fn notify(&mut self, _: &str) -> Result<()> {
            assert_eq!(self.saved()?["phase"], "validation_pending");
            self.order = OrderState::Ready;
            Ok(())
        }
        fn order_status(&mut self) -> Result<OrderState> {
            Ok(self.order)
        }
        fn finalize(&mut self, csr: &[u8]) -> Result<()> {
            let saved = self.saved()?;
            assert_eq!(saved["phase"], "finalization_pending");
            assert_eq!(saved["csr"], json!(csr));
            self.order = OrderState::Valid;
            if self.finalize_loss {
                return Err(loss());
            }
            Ok(())
        }
        fn certificate(&mut self) -> Result<Option<String>> {
            Ok(Some(self.certificate.clone()))
        }
        fn renewal_window(&mut self, _: &str) -> Result<RenewalAdvice> {
            Ok(self.advice)
        }
        fn random(&mut self) -> Result<u64> {
            Ok(123)
        }
        fn retry_delay(&mut self) -> Result<Option<u32>> {
            Ok(self.delay.take())
        }
        fn activate(
            &mut self,
            _: CertificateSelection,
            _: axum_server::tls_rustls::RustlsConfig,
        ) -> Result<()> {
            panic!("a staging fixture cannot activate a live listener");
        }
    }

    #[test]
    fn certificates_conditional_conflict_rereads_and_preserves_the_concurrent_record() -> Result<()>
    {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.conditional_conflicts = 1;
        assert_eq!(finish(&mut state, &mut fixture, &names)?, Step::Ready);
        assert_eq!(fixture.orders, 1);
        assert_eq!(
            fixture
                .record
                .ok_or(Error::Internal("fixture record is missing"))?
                .properties,
            json!({"TTL":360,"TXTRecords":[{"value":["unrelated","-segmented"]}]})
        );
        Ok(())
    }

    fn setup() -> Result<(tempfile::TempDir, StateStore, Fixture, CertificateNames)> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("certificates");
        let state = StateStore::initialize(&path, &"a".repeat(64), AcmeEnvironment::Staging)?;
        let fixture = Fixture::new(path.join("lifecycle.json"))?;
        Ok((
            directory,
            state,
            fixture,
            CertificateNames::new("media.example.com", "example.com")?,
        ))
    }
    fn finish(
        state: &mut StateStore,
        fixture: &mut Fixture,
        names: &CertificateNames,
    ) -> Result<Step> {
        for now in 1000..1020 {
            match advance(state, fixture, names, now)? {
                Step::Continue | Step::Wait(_) => {}
                result => return Ok(result),
            }
        }
        Err(Error::Internal("fixture step budget exceeded"))
    }
    fn resume(state: &mut StateStore) -> Result<()> {
        state.journal.phase = state
            .journal
            .resume_phase
            .take()
            .ok_or(Error::Internal("fixture resume phase missing"))?;
        state.journal.pause_reason = None;
        state.save()
    }

    #[test]
    fn certificates_commit_every_intent_and_publish_a_complete_pair() -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        assert_eq!(finish(&mut state, &mut fixture, &names)?, Step::Ready);
        assert_eq!(
            (
                fixture.creates,
                fixture.orders,
                fixture.requests,
                fixture.writes
            ),
            (1, 1, 1, 2)
        );
        assert_eq!(state.generation(1)?.private_key, fixture.key);
        assert!(state.journal.dns_receipt.is_none());
        assert!(state.journal.dns_intent.is_none());
        assert!(state.journal.last_success.is_some());
        assert!(state.active_generation_number().is_none());
        Ok(())
    }

    #[test]
    fn certificates_uncertain_account_recovery_only_looks_up_the_saved_key() -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.account_loss = true;
        assert!(advance(&mut state, &mut fixture, &names, 1000).is_err());
        resume(&mut state)?;
        assert_eq!(finish(&mut state, &mut fixture, &names)?, Step::Ready);
        assert_eq!((fixture.creates, fixture.lookups), (1, 1));
        Ok(())
    }

    #[test]
    fn certificates_uncertain_order_never_creates_replacement_work() -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.order_loss = true;
        assert!(finish(&mut state, &mut fixture, &names).is_err());
        assert_eq!(state.journal.resume_phase, Some(Phase::OrderPending));
        resume(&mut state)?;
        assert!(advance(&mut state, &mut fixture, &names, 1100).is_err());
        assert_eq!(fixture.orders, 1);
        Ok(())
    }

    #[test]
    fn certificates_lost_dns_write_retains_intent_without_inventing_ownership() -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.write_loss = true;
        assert!(finish(&mut state, &mut fixture, &names).is_err());
        assert!(state.journal.dns_intent.is_some());
        assert!(state.journal.dns_receipt.is_none());
        resume(&mut state)?;
        assert!(advance(&mut state, &mut fixture, &names, 1100).is_err());
        assert_eq!(fixture.writes, 1);
        Ok(())
    }

    #[test]
    fn certificates_confirmed_rejection_can_resume_without_discarding_an_uncertain_write()
    -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.rejected_write = true;
        assert!(finish(&mut state, &mut fixture, &names).is_err());
        assert_eq!(state.journal.resume_phase, Some(Phase::DnsPending));
        assert!(state.journal.dns_intent.is_none());
        assert!(state.journal.dns_receipt.is_none());
        assert_eq!(fixture.writes, 0);
        resume(&mut state)?;
        fixture.rejected_write = false;
        assert_eq!(finish(&mut state, &mut fixture, &names)?, Step::Ready);
        assert_eq!(fixture.orders, 1);
        Ok(())
    }

    #[test]
    fn certificates_successful_response_backoff_is_saved_before_the_next_step() -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.delay = Some(120);
        assert_eq!(
            advance(&mut state, &mut fixture, &names, 1000)?,
            Step::Wait(120)
        );
        assert_eq!(state.journal.retry_interval, Some(120));
        assert_eq!(state.journal.next_attempt, Some(1120));
        assert_eq!(fixture.orders, 0);
        Ok(())
    }

    #[test]
    fn certificates_finalization_continuation_restores_the_published_initial_advice() -> Result<()>
    {
        let (_directory, mut state, mut fixture, names) = setup()?;
        let (start, _) = crate::infrastructure::tls::certificate_validity(
            &names.hostname,
            fixture.certificate.as_bytes(),
        )?;
        fixture.advice = RenewalAdvice::Suggested {
            start: start + 1000,
            end: start + 2000,
            refresh_after_seconds: 120,
        };
        assert_eq!(finish(&mut state, &mut fixture, &names)?, Step::Ready);
        let selected = state.journal.renewal;
        state.journal.phase = Phase::FinalizationPending;
        state.journal.certificate_key = Some(fixture.key.clone());
        state.journal.renewal = None;
        state.save()?;
        assert_eq!(
            advance(&mut state, &mut fixture, &names, 1020)?,
            Step::Continue
        );
        assert_eq!(state.journal.renewal, selected);
        assert_eq!(state.journal.candidate_generation, Some(1));
        assert_eq!(state.next_generation_number(), 2);
        Ok(())
    }

    #[test]
    fn certificates_ari_refresh_preserves_an_unchanged_selection_and_immutable_generation()
    -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        let (start, _) = crate::infrastructure::tls::certificate_validity(
            &names.hostname,
            fixture.certificate.as_bytes(),
        )?;
        fixture.advice = RenewalAdvice::Suggested {
            start: start + 1000,
            end: start + 2000,
            refresh_after_seconds: 120,
        };
        assert_eq!(finish(&mut state, &mut fixture, &names)?, Step::Ready);
        let original = state.generation(1)?.renew_at;
        let selected = state
            .journal
            .renewal
            .ok_or(Error::Internal("fixture schedule is missing"))?;
        assert_eq!(state.journal.retry_interval, Some(120));
        let refresh = selected
            .refresh_at
            .ok_or(Error::Internal("fixture refresh is missing"))?;
        assert_eq!(
            advance(&mut state, &mut fixture, &names, refresh)?,
            Step::Ready
        );
        assert_eq!(
            state
                .journal
                .renewal
                .ok_or(Error::Internal("fixture schedule is missing"))?
                .renew_at,
            selected.renew_at
        );
        fixture.advice = RenewalAdvice::Suggested {
            start: start + 500,
            end: start + 700,
            refresh_after_seconds: 180,
        };
        assert_eq!(
            advance(&mut state, &mut fixture, &names, refresh + 120)?,
            Step::Ready
        );
        let changed = state
            .journal
            .renewal
            .ok_or(Error::Internal("fixture schedule is missing"))?;
        assert!(changed.renew_at < selected.renew_at);
        assert_eq!(state.generation(1)?.renew_at, original);
        assert_eq!(state.journal.retry_interval, Some(180));
        assert_eq!(fixture.orders, 1);
        Ok(())
    }

    #[test]
    fn certificates_existing_matching_txt_is_never_owned_or_removed() -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.record = Some(TxtRecord {
            etag: "\"other-owner\"".into(),
            properties: json!({"TTL":300,"TXTRecords":[{"value":["a".repeat(43)]}]}),
        });
        assert!(finish(&mut state, &mut fixture, &names).is_err());
        assert_eq!(fixture.writes, 0);
        assert!(state.journal.dns_receipt.is_none());
        Ok(())
    }

    #[test]
    fn certificates_lost_finalization_recovers_only_the_saved_csr_and_key() -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.finalize_loss = true;
        assert!(finish(&mut state, &mut fixture, &names).is_err());
        assert_eq!(state.journal.resume_phase, Some(Phase::FinalizationPending));
        let csr = state.journal.csr.clone();
        resume(&mut state)?;
        assert_eq!(finish(&mut state, &mut fixture, &names)?, Step::Ready);
        assert_eq!(fixture.requests, 1);
        assert_eq!(csr, Some(vec![1, 2, 3]));
        assert_eq!(state.generation(1)?.private_key, fixture.key);
        Ok(())
    }

    #[test]
    fn certificates_cleanup_failure_retains_the_validated_candidate_and_receipt() -> Result<()> {
        let (_directory, mut state, mut fixture, names) = setup()?;
        fixture.cleanup_loss = true;
        assert!(finish(&mut state, &mut fixture, &names).is_err());
        assert_eq!(state.journal.candidate_generation, Some(1));
        assert!(state.journal.dns_receipt.is_some());
        assert!(state.journal.dns_intent.is_some());
        assert_eq!(state.generation(1)?.certificate, fixture.certificate);
        assert!(state.journal.last_success.is_none());
        Ok(())
    }

    #[test]
    fn certificates_propagation_retry_is_durable_and_finite() -> Result<()> {
        let (directory, mut state, mut fixture, names) = setup()?;
        fixture.propagated = false;
        for now in 1000..1003 {
            assert_eq!(
                advance(&mut state, &mut fixture, &names, now)?,
                Step::Continue
            );
        }
        assert_eq!(
            advance(&mut state, &mut fixture, &names, 1003)?,
            Step::Wait(30)
        );
        drop(state);
        let mut state = StateStore::open(
            &directory.path().join("certificates"),
            &"a".repeat(64),
            AcmeEnvironment::Staging,
        )?;
        assert_eq!(state.journal.retry_interval, Some(30));
        assert_eq!(state.journal.next_attempt, Some(1033));
        for now in 1004..1011 {
            assert_eq!(
                advance(&mut state, &mut fixture, &names, now)?,
                Step::Wait(30)
            );
        }
        assert_eq!(
            advance(&mut state, &mut fixture, &names, 1011)?,
            Step::Paused
        );
        assert_eq!(state.journal.attempts, 8);
        assert!(state.journal.dns_receipt.is_some());
        Ok(())
    }
}
