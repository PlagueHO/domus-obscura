//! Process-crash probes. This module and every checkpoint are absent from normal builds.
use crate::infrastructure::artwork::{CoverJpeg, normalize_cover};
use serde_json::json;
use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::net::TcpStream;

use crate::media_fixtures::{cover_source, flac, mp3};
type TestResult = Result<(), Box<dyn std::error::Error>>;
include!("../tests/support/album_metadata.rs");
mod profiles {
    include!("../tests/support/mpeg.rs");
}

struct Probe {
    target: String,
    channel: TcpStream,
    fail_write: bool,
}

thread_local! {
    static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

/// A selected probe reports an exact boundary, then waits for its parent to kill
/// the process. A lost control connection fails the operation instead of resuming it.
pub(crate) fn checkpoint(name: &str) -> io::Result<()> {
    PROBE.with_borrow_mut(|probe| {
        let Some(probe) = probe.as_mut().filter(|probe| probe.target == name) else {
            return Ok(());
        };
        if probe.fail_write {
            #[cfg(windows)]
            let code = 112;
            #[cfg(unix)]
            let code = 28;
            return Err(io::Error::from_raw_os_error(code));
        }
        writeln!(probe.channel, "{name}")?;
        let mut command = [0];
        probe.channel.read_exact(&mut command)?;
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "crash probe must be terminated, never released",
        ))
    })
}

mod album {
    use crate::infrastructure::private_fixture as private;
    include!("../tests/media/fixture.rs");
    include!("crash_tests/cases.rs");
    include!("crash_tests/faults.rs");
}
