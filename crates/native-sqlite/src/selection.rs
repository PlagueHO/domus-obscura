use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs::{File, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};

pub const SELECTION_FILE: &str = ".domus-storage-selection.lock";
const HEADER: &[u8] = b"domus-storage-selection-v1\n";

/// A pre-existing coordination file locked for one frozen logical selection.
///
/// The caller must verify its externally expected native identity, locality and
/// private permissions first. Native Context construction consumes the actual
/// locked handle, not a duplicate. There is no explicit unlock on destruction.
pub struct SelectionFence {
    pub(crate) file: File,
    pub(crate) record: Vec<u8>,
}

impl SelectionFence {
    /// Bytes for explicit fresh setup only. Missing or damaged files are never
    /// initialized by acquisition. This record grants no root or media approval.
    pub fn initial_record() -> Vec<u8> {
        record(&"0".repeat(64))
    }

    pub fn acquire(mut file: File, selection: &str) -> Result<Self> {
        if selection.len() != 64
            || !selection
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || selection.bytes().all(|byte| byte == b'0')
            || !file.metadata()?.is_file()
        {
            return Err(Error::InvalidInput);
        }
        let expected = record(selection);
        file.try_lock_shared().map_err(lock_error)?;
        let current = read_record(&mut file)?;
        if current == expected {
            return Ok(Self {
                file,
                record: expected,
            });
        }
        // Only this admission owns this handle; no SQLite dependent exists.
        // Never upgrade while retaining a shared lock owned by this admission.
        file.unlock()?;
        file.try_lock().map_err(lock_error)?;
        let current = read_record(&mut file)?;
        if current != expected {
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&expected)?;
            file.set_len(expected.len() as u64)?;
            file.sync_all()?;
        }
        file.unlock()?;
        file.try_lock_shared().map_err(lock_error)?;
        // Lock conversion can have a gap on either supported platform.
        if read_record(&mut file)? != expected {
            return Err(Error::SelectionInUse);
        }
        Ok(Self {
            file,
            record: expected,
        })
    }
}

fn record(selection: &str) -> Vec<u8> {
    let mut bytes = HEADER.to_vec();
    bytes.extend_from_slice(selection.as_bytes());
    bytes.push(b'\n');
    let checksum = format!("{:x}\n", Sha256::digest(&bytes));
    bytes.extend_from_slice(checksum.as_bytes());
    bytes
}

fn read_record(file: &mut File) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(257).read_to_end(&mut bytes)?;
    let length = HEADER.len() + 64 + 1 + 64 + 1;
    if bytes.len() != length || !bytes.starts_with(HEADER) {
        return Err(Error::SelectionRecord);
    }
    let token = &bytes[HEADER.len()..HEADER.len() + 64];
    if !token
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(Error::SelectionRecord);
    }
    let token = std::str::from_utf8(token).map_err(|_| Error::SelectionRecord)?;
    if bytes != record(token) {
        return Err(Error::SelectionRecord);
    }
    Ok(bytes)
}

fn lock_error(error: TryLockError) -> Error {
    match error {
        TryLockError::WouldBlock => Error::SelectionInUse,
        TryLockError::Error(error) => Error::Io(error),
    }
}
