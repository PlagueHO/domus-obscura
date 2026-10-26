use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

/// A virtual empty output verifies a deterministic rewrite against existing
/// bytes. It never writes to the backing file. Reads see only verified bytes.
pub(super) struct ReplayOutput<'a> {
    file: &'a mut File,
    position: u64,
    verified: u64,
    actual: u64,
}

impl<'a> ReplayOutput<'a> {
    pub(super) fn new(file: &'a mut File) -> io::Result<Self> {
        let actual = file.metadata()?.len();
        file.seek(SeekFrom::Start(0))?;
        Ok(Self {
            file,
            position: 0,
            verified: 0,
            actual,
        })
    }
}

impl Read for ReplayOutput<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = (self.verified.saturating_sub(self.position)).min(buffer.len() as u64) as usize;
        let read = self.file.read(&mut buffer[..count])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Write for ReplayOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self
            .position
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= self.actual)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "interrupted output is incomplete",
                )
            })?;
        let mut buffer = [0u8; 65_536];
        for chunk in bytes.chunks(buffer.len()) {
            self.file.read_exact(&mut buffer[..chunk.len()])?;
            if &buffer[..chunk.len()] != chunk {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "interrupted output differs from the frozen normalization",
                ));
            }
        }
        self.position = end;
        self.verified = self.verified.max(end);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for ReplayOutput<'_> {
    fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
        let position = match seek {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => self.verified.checked_add_signed(offset),
        }
        .filter(|position| *position <= self.actual)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid replay position"))?;
        self.file.seek(SeekFrom::Start(position))?;
        self.position = position;
        Ok(position)
    }
}
