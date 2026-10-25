use crate::domain::JobState;
use crate::infrastructure::soulseek::PeerControl;
use crate::infrastructure::staging::{ReceivedInput, StagingRoot};
use crate::infrastructure::store::{JobClaim, Store};
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
    store.job_work_boundary(claim)?;
    let files = store.acquisition_files(claim)?;
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
    store.start_transfer(claim, transfer_id)?;
    let token = peer.queue_upload(
        &transfer.remote_path,
        transfer.expected_bytes as u64,
        || {
            store.submitting_transfer(claim, transfer_id)?;
            Ok(())
        },
    )?;
    store.accept_transfer(claim, transfer_id, token)?;
    store.active_transfer(claim, transfer_id)?;
    peer.accept_upload(token)?;
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
    negotiate_transfer(store, claim, transfer_id, peer)?;
    let transfer = store.active_transfer(claim, transfer_id)?;
    let token = transfer
        .remote_token
        .ok_or_else(|| Error::Conflict("active transfer has no accepted token".into()))?;
    let file = peer.file_connection(token, || {
        store.active_transfer(claim, transfer_id)?;
        Ok(())
    })?;
    let (name, bound_token, stream) = file.into_parts();
    if name != transfer.peer || bound_token != token {
        return Err(Error::Conflict(
            "file channel differs from the accepted source".into(),
        ));
    }
    receive_transfer_file(store, claim, staging, transfer_id, &name, stream)
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
