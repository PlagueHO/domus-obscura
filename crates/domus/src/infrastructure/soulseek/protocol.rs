//! Transport-independent wire framing and bounded field decoding.
//! Session state, request routing, network policy and file access belong outside
//! this module. Message codes remain intact for operation-specific dispatch.
use crate::domain::sources::{SourceFileMetadata, remote_identifier};
use crate::{Error, Result};
use flate2::{Decompress, FlushDecompress, Status};
use std::collections::BTreeMap;
use std::io::Write;

pub(super) const MAX_FRAME: usize = 1024 * 1024;
pub(super) const MAX_EXPANDED: usize = 4 * 1024 * 1024;

pub(super) fn expanded(input: &[u8]) -> Result<Vec<u8>> {
    if input.is_empty() || input.len() > MAX_FRAME - 4 {
        return Err(protocol());
    }
    let mut decoder = Decompress::new(true);
    let mut output = vec![0; MAX_EXPANDED + 1];
    let status = decoder
        .decompress(input, &mut output, FlushDecompress::Finish)
        .map_err(|_| protocol())?;
    if status != Status::StreamEnd
        || decoder.total_in() != input.len() as u64
        || decoder.total_out() > MAX_EXPANDED as u64
    {
        return Err(protocol());
    }
    output.truncate(decoder.total_out() as usize);
    Ok(output)
}

/// Search and folder responses share the same file/attribute wire shape.
pub(super) fn file(fields: &mut Fields<'_>) -> Result<SourceFileMetadata> {
    if fields.take(1)? != [1] {
        return Err(protocol());
    }
    let path = fields.text(4096)?;
    remote_identifier(path).map_err(|_| protocol())?;
    let expected_bytes = i64::try_from(fields.u64()?).map_err(|_| protocol())?;
    if expected_bytes <= 0 {
        return Err(protocol());
    }
    let extension = fields.text(16)?;
    if !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(protocol());
    }
    let attributes = fields.u32()?;
    if attributes > 32 {
        return Err(protocol());
    }
    let mut values = BTreeMap::new();
    for _ in 0..attributes {
        if values.insert(fields.u32()?, fields.u32()?).is_some() {
            return Err(protocol());
        }
    }
    let vbr = values
        .get(&2)
        .map(|value| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(protocol()),
        })
        .transpose()?;
    Ok(SourceFileMetadata {
        remote_path: path.into(),
        expected_bytes,
        extension: extension.into(),
        bitrate_kbps: values.get(&0).copied(),
        duration_seconds: values.get(&1).copied(),
        vbr,
        sample_rate: values.get(&4).copied(),
        bit_depth: values.get(&5).copied(),
    })
}

pub(super) fn protocol() -> Error {
    Error::Remote("Soulseek returned an invalid or unsupported protocol message")
}

pub(super) fn string(body: &mut Vec<u8>, value: &str) {
    body.extend_from_slice(&(value.len() as u32).to_le_bytes());
    body.extend_from_slice(value.as_bytes());
}

pub(super) fn send(writer: &mut impl Write, code: u32, body: &[u8]) -> Result<()> {
    if body.len() > MAX_FRAME - 4 {
        return Err(protocol());
    }
    writer.write_all(&((body.len() + 4) as u32).to_le_bytes())?;
    writer.write_all(&code.to_le_bytes())?;
    writer.write_all(body)?;
    Ok(())
}

pub(super) fn send_init(writer: &mut impl Write, code: u8, body: &[u8]) -> Result<()> {
    if body.len() > MAX_FRAME - 1 {
        return Err(protocol());
    }
    writer.write_all(&((body.len() + 1) as u32).to_le_bytes())?;
    writer.write_all(&[code])?;
    writer.write_all(body)?;
    Ok(())
}

/// The caller must fill each complete buffer or return an error. Network callers
/// retain their absolute deadline and ownership checks on every partial read.
pub(super) fn read_frame(mut read: impl FnMut(&mut [u8]) -> Result<()>) -> Result<(u32, Vec<u8>)> {
    let mut size = [0; 4];
    read(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    if !(4..=MAX_FRAME).contains(&size) {
        return Err(protocol());
    }
    let mut code = [0; 4];
    read(&mut code)?;
    let mut body = vec![0; size - 4];
    read(&mut body)?;
    Ok((u32::from_le_bytes(code), body))
}

pub(super) struct Fields<'a>(pub(super) &'a [u8]);

impl<'a> Fields<'a> {
    pub(super) fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.0.len() {
            return Err(protocol());
        }
        let (value, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(value)
    }

    pub(super) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| protocol())?,
        ))
    }

    pub(super) fn boolean(&mut self) -> Result<bool> {
        match self.take(1)? {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err(protocol()),
        }
    }

    pub(super) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| protocol())?,
        ))
    }

    pub(super) fn text(&mut self, maximum: usize) -> Result<&'a str> {
        let count = self.u32()? as usize;
        if count > maximum {
            return Err(protocol());
        }
        std::str::from_utf8(self.take(count)?).map_err(|_| protocol())
    }

    pub(super) fn end(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(protocol())
        }
    }
}

#[cfg(test)]
mod tests;
