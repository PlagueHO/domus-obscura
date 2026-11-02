//! systemd supervises the same runtimes as foreground mode. Readiness follows
//! initialization; termination remains latched while blocking work drains.
use super::lifecycle::Lifecycle;
use crate::{Error, Result};
use std::ffi::OsStr;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Notification {
    socket: UnixDatagram,
    address: SocketAddr,
    stopping: Mutex<bool>,
}

impl Notification {
    fn address(value: &OsStr) -> Result<SocketAddr> {
        let bytes = value.as_bytes();
        if bytes.len() < 2 || bytes.len() > 107 || bytes.contains(&0) {
            return Err(Error::InvalidInput(
                "invalid systemd notification socket".into(),
            ));
        }
        if bytes[0] == b'@' {
            Ok(SocketAddr::from_abstract_name(&bytes[1..])?)
        } else if bytes[0] == b'/' {
            Ok(SocketAddr::from_pathname(Path::new(value))?)
        } else {
            Err(Error::InvalidInput(
                "systemd notification socket must be absolute or abstract".into(),
            ))
        }
    }

    fn new(value: &OsStr) -> Result<Self> {
        let address = Self::address(value)?;
        let socket = UnixDatagram::unbound()?;
        socket.set_write_timeout(Some(Duration::from_secs(1)))?;
        Ok(Self {
            socket,
            address,
            stopping: Mutex::new(false),
        })
    }

    fn send(&self, value: &[u8]) -> Result<()> {
        if self.socket.send_to_addr(value, &self.address)? != value.len() {
            return Err(Error::Internal("incomplete systemd notification"));
        }
        Ok(())
    }

    fn ready(&self) -> Result<()> {
        let stopping = self
            .stopping
            .lock()
            .map_err(|_| Error::Internal("systemd status lock failed"))?;
        if !*stopping {
            self.send(b"READY=1")?;
        }
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let mut stopping = self
            .stopping
            .lock()
            .map_err(|_| Error::Internal("systemd status lock failed"))?;
        *stopping = true;
        self.send(b"STOPPING=1")
    }
}

fn validate(role: &str, uid: u32, configuration: &Path) -> Result<()> {
    if !matches!(role, "api" | "worker") {
        return Err(Error::InvalidInput(
            "systemd role must be api or worker".into(),
        ));
    }
    if uid == 0 {
        return Err(Error::InvalidInput(
            "systemd application services must not run as root".into(),
        ));
    }
    super::local_file::validate_reference(configuration)
}

/// Register signals before startup, then retain every runtime owner through drain.
/// The notification socket is required only by this explicit service entry point.
pub fn run(role: &str, configuration: &Path) -> Result<()> {
    validate(role, rustix::process::geteuid().as_raw(), configuration)?;
    super::local_file::read_local(configuration, super::server::configuration::MAXIMUM, true)?;
    let socket = std::env::var_os("NOTIFY_SOCKET")
        .ok_or_else(|| Error::InvalidInput("systemd NOTIFY_SOCKET is missing".into()))?;
    let notification = Arc::new(Notification::new(&socket)?);
    let ready = notification.clone();
    let lifecycle = Lifecycle::controlled(move || ready.ready());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()?;
    let _entered = runtime.enter();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let stopping = lifecycle.clone();
    let status = notification.clone();
    let signals = runtime.spawn(async move {
        tokio::select! {
            received = terminate.recv() => {
                received.ok_or(Error::Internal("systemd termination signal stream closed"))?;
            },
            received = interrupt.recv() => {
                received.ok_or(Error::Internal("systemd interrupt signal stream closed"))?;
            },
        }
        let result = status.stop();
        stopping.request_stop();
        result
    });
    let role = role.to_owned();
    let configuration = configuration.to_owned();
    let result = runtime.block_on(async {
        enum Prepared {
            Api(Box<super::server::PreparedServer>),
            Worker(Box<super::production_worker::PreparedProductionWorker>),
        }
        let loading = lifecycle.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            if loading.stopping() {
                return Ok(None);
            }
            match role.as_str() {
                "api" => super::server::PreparedServer::load(&configuration)
                    .map(|server| Some(Prepared::Api(Box::new(server)))),
                "worker" => {
                    super::production_worker::PreparedProductionWorker::load(&configuration)
                        .map(|worker| Some(Prepared::Worker(Box::new(worker))))
                }
                _ => Err(Error::Internal("validated systemd role changed")),
            }
        })
        .await
        .map_err(|_| Error::Internal("systemd startup task failed"))??;
        match prepared {
            Some(Prepared::Api(server)) => server.run_with_lifecycle(lifecycle).await,
            Some(Prepared::Worker(worker)) => worker.run_with_lifecycle(lifecycle).await,
            None => Ok(()),
        }
    });
    if signals.is_finished() {
        runtime
            .block_on(signals)
            .map_err(|_| Error::Internal("systemd signal task failed"))??;
    } else {
        signals.abort();
        match runtime.block_on(signals) {
            Ok(result) => result?,
            Err(error) if error.is_cancelled() => {}
            Err(_) => return Err(Error::Internal("systemd signal task failed")),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_rejects_root_roles_and_relative_configuration() {
        assert!(validate("api", 0, Path::new("/tmp/service.json")).is_err());
        assert!(validate("worker", 1000, Path::new("service.json")).is_err());
        assert!(validate("API", 1000, Path::new("/tmp/service.json")).is_err());
        assert!(validate("worker", 1000, Path::new("/tmp/service.json")).is_ok());
    }

    #[test]
    fn systemd_notification_supports_path_and_abstract_sockets() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notify");
        let receiver = UnixDatagram::bind(&path)?;
        receiver.set_read_timeout(Some(Duration::from_secs(1)))?;
        let notification = Notification::new(path.as_os_str())?;
        notification.ready()?;
        let mut buffer = [0; 32];
        let length = receiver.recv(&mut buffer)?;
        assert_eq!(&buffer[..length], b"READY=1");
        notification.stop()?;
        let length = receiver.recv(&mut buffer)?;
        assert_eq!(&buffer[..length], b"STOPPING=1");
        notification.ready()?;
        receiver.set_read_timeout(Some(Duration::from_millis(20)))?;
        assert!(receiver.recv(&mut buffer).is_err());

        let name = format!("domus-test-{}", std::process::id());
        let receiver = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes())?)?;
        receiver.set_read_timeout(Some(Duration::from_secs(1)))?;
        let notification = Notification::new(OsStr::new(&format!("@{name}")))?;
        notification.ready()?;
        let length = receiver.recv(&mut buffer)?;
        assert_eq!(&buffer[..length], b"READY=1");
        for invalid in ["", "relative", "@", "/"] {
            assert!(Notification::address(OsStr::new(invalid)).is_err());
        }
        Ok(())
    }

    #[test]
    fn systemd_readiness_failure_is_not_success() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let notification = Notification::new(directory.path().join("absent").as_os_str())?;
        assert!(notification.ready().is_err());
        assert!(notification.stop().is_err());
        notification.ready()?;
        Ok(())
    }
}
