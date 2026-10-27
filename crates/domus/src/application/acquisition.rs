use crate::domain::JobState;
use crate::infrastructure::soulseek::{PeerControl, QueueObservation};
use crate::infrastructure::staging::{ReceivedInput, StagingRoot};
use crate::infrastructure::store::{JobClaim, Store, TransferObservation};
use crate::{Error, Result};
use std::cell::Cell;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Download the already-selected complete source and normalize with the exact
/// approved artwork. It stops at the durable Publishing handoff, not final
/// destination publication. A restart never requeues an uncertain transfer.
pub fn acquire_album(
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    peer: &mut PeerControl<'_>,
) -> Result<i64> {
    staging.check_store(store)?;
    store.renew_claim(claim, 300)?;
    reconcile_local_inputs(store, claim, staging)?;
    let files = store.reconciliation_files(claim)?;
    let first = files
        .first()
        .ok_or(Error::Internal("selected source is empty"))?;
    if first.peer != peer.username() {
        return Err(Error::Conflict(
            "peer connection differs from the selected album source".into(),
        ));
    }
    let cover = store.approved_artwork(first.job_id, first.plan_version)?;
    // Verify all retained receipts before creating any new remote effects.
    for file in &files {
        if file.state == "received" {
            staging.received(file)?;
        }
    }
    // Reconcile all uncertain work before submitting any remaining planned file.
    for file in &files {
        if matches!(file.state.as_str(), "submitting" | "active") {
            recover_transfer(store, claim, staging, file, peer)?;
        }
    }
    let files = store.acquisition_files(claim)?;
    for file in files {
        store.renew_claim(claim, 300)?;
        store.job_work_boundary(claim)?;
        if file.state == "planned" {
            acquire_transfer(store, claim, staging, file.id, peer)?;
        }
    }
    store.job_work_boundary(claim)?;
    store.advance(claim, JobState::Downloading, JobState::Validating)?;
    super::normalize_album(store, claim, staging, &cover)
}

/// Persist submission before queueing the immutable selected file. Acceptance
/// commits before the upload response. A failure leaves its uncertain journal
/// state intact for reconciliation rather than silently resubmitting it.
pub fn negotiate_transfer(
    store: &mut Store,
    claim: &JobClaim,
    transfer_id: i64,
    peer: &mut PeerControl<'_>,
) -> Result<()> {
    store.job_work_boundary(claim)?;
    let transfer = store.planned_transfer(claim, transfer_id)?;
    if transfer.peer != peer.username() {
        return Err(Error::Conflict(
            "peer connection differs from the selected source".into(),
        ));
    }
    let binding = peer.binding()?;
    store.start_transfer(claim, transfer_id)?;
    store.observe_transfer(claim, transfer_id, binding, TransferObservation::Submitted)?;
    let renewed = Cell::new(Instant::now());
    let offered = peer.wait_upload(
        &transfer.remote_path,
        transfer.expected_bytes as u64,
        true,
        |observation| {
            if renewed.get().elapsed() >= Duration::from_secs(30) {
                store.renew_claim(claim, 300)?;
                renewed.set(Instant::now());
            }
            store.submitting_transfer(claim, transfer_id)?;
            store.require_job_running(claim.job(), claim.plan_version())?;
            record_queue_observation(store, claim, transfer_id, binding, observation)?;
            Ok(())
        },
    );
    let token = match offered {
        Ok(token) => token,
        Err(error) => {
            store.observe_transfer(
                claim,
                transfer_id,
                binding,
                TransferObservation::Interrupted,
            )?;
            return Err(error);
        }
    };
    store.accept_transfer(claim, transfer_id, token)?;
    store.active_transfer(claim, transfer_id)?;
    store.submission_approval(claim)?;
    peer.accept_upload(token)?;
    Ok(())
}

fn record_queue_observation(
    store: &mut Store,
    claim: &JobClaim,
    file: i64,
    binding: (u64, u64),
    observation: Option<QueueObservation>,
) -> Result<()> {
    if let Some(observation) = observation {
        let observation = match observation {
            QueueObservation::Position(place) => TransferObservation::Queued(place),
            QueueObservation::Offer(token) => TransferObservation::Offered(token),
            QueueObservation::Denied => TransferObservation::Denied,
        };
        store.observe_transfer(claim, file, binding, observation)?;
    }
    Ok(())
}

/// Recover a sealed stream whose database commit was interrupted. Verify the
/// whole source before new effects. Missing or partial files never become input.
pub(crate) fn reconcile_local_inputs(
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
) -> Result<()> {
    staging.check_store(store)?;
    let files = store.reconciliation_files(claim)?;
    for file in files {
        store.renew_claim(claim, 300)?;
        let renewed = Cell::new(Instant::now());
        let input = staging.retained_input(&file, &|| {
            store
                .check_transfer_owner(claim)
                .map_err(io::Error::other)?;
            if renewed.get().elapsed() >= Duration::from_secs(30) {
                store.renew_claim(claim, 300).map_err(io::Error::other)?;
                renewed.set(Instant::now());
            }
            Ok(())
        })?;
        match (file.state.as_str(), input) {
            ("active", Some(input)) => {
                store.observe_transfer(claim, file.id, (0, 0), TransferObservation::Sealed)?;
                store.receive_transfer(
                    claim,
                    file.id,
                    input.byte_length() as i64,
                    input.sha256(),
                )?;
            }
            ("active", None) if file.received_bytes > 0 => {
                return Err(Error::Conflict(
                    "interrupted transfer has byte progress but no sealed input; operator review required".into(),
                ));
            }
            ("received", Some(_)) | ("planned" | "submitting" | "active", None) => {}
            _ => {
                return Err(Error::Conflict(
                    "transfer journal and retained input disagree; operator review required".into(),
                ));
            }
        }
    }
    Ok(())
}

fn recover_transfer(
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    transfer: &crate::infrastructure::store::TransferFile,
    peer: &mut PeerControl<'_>,
) -> Result<()> {
    if transfer.peer != peer.username() {
        return Err(Error::Conflict(
            "recovery peer differs from the frozen source".into(),
        ));
    }
    if staging.retained_input(transfer, &|| Ok(()))?.is_some() {
        return Err(Error::Conflict(
            "reconcile the sealed input before remote recovery".into(),
        ));
    }
    // This is a new wire effect, unlike completing a sealed local receipt.
    store.job_work_boundary(claim)?;
    store.submission_approval(claim)?;
    let binding = peer.binding()?;
    let renewed = Cell::new(Instant::now());
    let token = peer.wait_upload(
        &transfer.remote_path,
        transfer.expected_bytes as u64,
        false,
        |observation| {
            if renewed.get().elapsed() >= Duration::from_secs(30) {
                store.renew_claim(claim, 300)?;
                renewed.set(Instant::now());
            }
            store.check_transfer_owner(claim)?;
            store.submission_approval(claim)?;
            record_queue_observation(store, claim, transfer.id, binding, observation)
        },
    )?;
    if transfer.state == "active" {
        if transfer.remote_token != Some(token) {
            return Err(Error::Conflict(
                "recovery offer changed the accepted transfer token".into(),
            ));
        }
    } else {
        store.accept_transfer(claim, transfer.id, token)?;
    }
    store.active_transfer(claim, transfer.id)?;
    store.submission_approval(claim)?;
    peer.accept_upload(token)?;
    receive_accepted(store, claim, staging, transfer.id, peer)?;
    Ok(())
}

/// Queue and receive one frozen source file on a blocking worker. The file
/// channel comes from the selected control/session binding, not caller-supplied
/// identity text. Failed effects retain their existing journal state.
pub fn acquire_transfer(
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    transfer_id: i64,
    peer: &mut PeerControl<'_>,
) -> Result<ReceivedInput> {
    staging.check_store(store)?;
    let planned = store.planned_transfer(claim, transfer_id)?;
    if staging.retained_input(&planned, &|| Ok(()))?.is_some() {
        return Err(Error::Conflict(
            "planned transfer has retained input".into(),
        ));
    }
    negotiate_transfer(store, claim, transfer_id, peer)?;
    receive_accepted(store, claim, staging, transfer_id, peer)
}

fn receive_accepted(
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    transfer_id: i64,
    peer: &mut PeerControl<'_>,
) -> Result<ReceivedInput> {
    let transfer = store.active_transfer(claim, transfer_id)?;
    let token = transfer
        .remote_token
        .ok_or_else(|| Error::Conflict("active transfer has no accepted token".into()))?;
    let binding = peer.binding()?;
    let result = peer.file_connection(token, || {
        store.active_transfer(claim, transfer_id)?;
        Ok(())
    });
    let file = match result {
        Ok(file) => file,
        Err(error) => {
            store.observe_transfer(
                claim,
                transfer_id,
                binding,
                TransferObservation::Interrupted,
            )?;
            return Err(error);
        }
    };
    let (name, bound_token, stream) = file.into_parts();
    if name != transfer.peer || bound_token != token {
        return Err(Error::Conflict(
            "file channel differs from the accepted source".into(),
        ));
    }
    match receive_transfer_file(store, claim, staging, transfer_id, &name, stream) {
        Ok(input) => Ok(input),
        Err(error) => {
            store.observe_transfer(
                claim,
                transfer_id,
                binding,
                TransferObservation::Interrupted,
            )?;
            Err(error)
        }
    }
}

/// Receive one already-negotiated Soulseek F connection on a blocking worker.
/// The caller must establish the peer identity through its peer connection
/// manager before calling this function. A file token alone is not identity.
///
/// Intent and acceptance must already be durable. This function never submits,
/// retries, resumes or selects a source. Failures retain partial staging files
/// for reconciliation and cannot become received journal entries.
pub fn receive_transfer_file(
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    transfer_id: i64,
    peer: &str,
    mut stream: TcpStream,
) -> Result<ReceivedInput> {
    let transfer = store.active_transfer(claim, transfer_id)?;
    staging.check_store(store)?;
    if peer != transfer.peer {
        return Err(Error::Conflict(
            "file connection peer differs from the selected source".into(),
        ));
    }
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(IDLE_TIMEOUT))?;
    let mut token = [0; 4];
    stream.read_exact(&mut token)?;
    if Some(u32::from_le_bytes(token)) != transfer.remote_token {
        return Err(Error::Conflict(
            "file connection token differs from the accepted transfer".into(),
        ));
    }
    store.active_transfer(claim, transfer_id)?;
    // Soulseek receivers send a little-endian u64 offset. Initial downloads
    // always start at zero; unverified partial inputs cannot authorize resume.
    stream.write_all(&0u64.to_le_bytes())?;
    let start = Instant::now();
    let renewed = Cell::new(start);
    let check = || {
        if start.elapsed() >= TRANSFER_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "file transfer exceeded its time limit",
            ));
        }
        if renewed.get().elapsed() >= Duration::from_secs(30) {
            store.renew_claim(claim, 300).map_err(io::Error::other)?;
            renewed.set(Instant::now());
        }
        store
            .active_transfer(claim, transfer_id)
            .map_err(io::Error::other)?;
        Ok(())
    };
    let mut reader = PeerReader {
        stream,
        remaining: transfer.expected_bytes as u64,
        check: &check,
    };
    let input = staging.receive_with_progress(&transfer, &mut reader, &check)?;
    drop(reader);
    store.receive_transfer(
        claim,
        transfer.id,
        input.byte_length() as i64,
        input.sha256(),
    )?;
    Ok(input)
}

struct PeerReader<F> {
    stream: TcpStream,
    remaining: u64,
    check: F,
}

impl<F: FnMut() -> io::Result<()>> Read for PeerReader<F> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        (self.check)()?;
        if self.remaining == 0 || buffer.is_empty() {
            return Ok(0);
        }
        let count = buffer.len().min(self.remaining.min(65_536) as usize);
        let read = self.stream.read(&mut buffer[..count])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "peer closed before the complete file arrived",
            ));
        }
        self.remaining -= read as u64;
        Ok(read)
    }
}
