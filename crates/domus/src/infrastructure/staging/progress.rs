use std::io::{self, Read, Seek, SeekFrom, Write};

pub(super) struct ProgressIo<'a, T> {
    inner: &'a mut T,
    heartbeat: &'a dyn Fn() -> io::Result<()>,
}

impl<'a, T> ProgressIo<'a, T> {
    pub(super) fn new(inner: &'a mut T, heartbeat: &'a dyn Fn() -> io::Result<()>) -> Self {
        Self { inner, heartbeat }
    }
}

impl<T: Read> Read for ProgressIo<'_, T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        (self.heartbeat)()?;
        self.inner.read(buffer)
    }
}
impl<T: Write> Write for ProgressIo<'_, T> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        (self.heartbeat)()?;
        self.inner.write(buffer)
    }
    fn flush(&mut self) -> io::Result<()> {
        (self.heartbeat)()?;
        self.inner.flush()
    }
}
impl<T: Seek> Seek for ProgressIo<'_, T> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        (self.heartbeat)()?;
        self.inner.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn lost_ownership_stops_every_io_operation_before_touching_the_stream() {
        let mut backing = Cursor::new(vec![1, 2, 3]);
        let lost = || {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "claim expired",
            ))
        };
        {
            let mut guarded = ProgressIo::new(&mut backing, &lost);
            assert!(guarded.read(&mut [0; 1]).is_err());
            assert!(guarded.write(&[9]).is_err());
            assert!(guarded.seek(SeekFrom::End(0)).is_err());
            assert!(guarded.flush().is_err());
        }
        assert_eq!(backing.position(), 0);
        assert_eq!(backing.into_inner(), vec![1, 2, 3]);
    }
}
