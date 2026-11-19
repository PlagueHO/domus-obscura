use super::{ConfigurationService, FilePermissions, LibraryDirectory, valid_reference};
use crate::domain::LibraryPath;
use crate::infrastructure::filesystem::media_adapter::{PreparedAdapter, Selection};
use crate::{Error, Result};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::time::Duration;

const MAX_CONFIGURATION: u64 = 262144;
const MAX_TLS_INPUT: u64 = 65536;
const MAX_PRIVATE_KEY: u64 = 16384;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format_version: u32,
    listen: SocketAddr,
    selection: Selection,
    certificate_file: String,
    private_key_file: String,
    client_ca_file: String,
    paired_client_file: String,
}

/// A frozen, read-only media export prepared through ConfigurationService.
/// Preparation retains the selected native roots and private input handles.
/// It does not open SQLite, bind a listener, register a service or save JSON.
pub struct PreparedMediaAdapter {
    adapter: PreparedAdapter,
    listen: SocketAddr,
}

impl ConfigurationService {
    /// Load the separate operator-managed native adapter document. It must
    /// reside directly in its selected private root. TLS references are
    /// library-relative identifiers beneath that root, not native paths.
    pub fn prepare_media_adapter(path: &Path) -> Result<PreparedMediaAdapter> {
        valid_reference(path)?;
        let parent = path.parent().ok_or(Error::InvalidInput(
            "native media configuration parent is missing".into(),
        ))?;
        let directory = LibraryDirectory::open(parent)?;
        directory.require_local()?;
        FilePermissions::current()?.check(&directory.directory_file()?, true)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::InvalidInput(
                "native media configuration name is invalid".into(),
            ))?;
        let (configuration_file, bytes) = read_private(&directory, name, MAX_CONFIGURATION)?;
        let configuration: Configuration = serde_json::from_slice(&bytes).map_err(|_| {
            Error::InvalidInput("saved native media configuration JSON is invalid".into())
        })?;
        if configuration.format_version != 1
            || !configuration.listen.ip().is_loopback()
            || configuration.listen.port() == 0
        {
            return Err(Error::InvalidInput(
                "native media configuration requires version 1 and a fixed loopback port".into(),
            ));
        }
        let selection = configuration.selection;
        selection.validate()?;
        let private = selection.private.expected.open(
            &selection.provider.boundary,
            &LibraryPath::parse(&selection.private.relative)?,
        )?;
        if private.directory().native_root() != directory.native_root() {
            return Err(Error::Conflict(
                "native media configuration is outside its selected private root".into(),
            ));
        }
        let mut protected = vec![configuration_file];
        let mut read = |reference: &str, maximum| -> Result<Vec<u8>> {
            let (file, bytes) = read_private(private.directory(), reference, maximum)?;
            protected.push(file);
            Ok(bytes)
        };
        let certificate = read(&configuration.certificate_file, MAX_TLS_INPUT)?;
        let key = read(&configuration.private_key_file, MAX_PRIVATE_KEY)?;
        let client_ca = read(&configuration.client_ca_file, MAX_TLS_INPUT)?;
        let client = read(&configuration.paired_client_file, MAX_TLS_INPUT)?;
        Ok(PreparedMediaAdapter {
            adapter: PreparedAdapter::new(
                selection,
                protected,
                &certificate,
                &key,
                &client_ca,
                &client,
            )?,
            listen: configuration.listen,
        })
    }
}

fn read_private(root: &LibraryDirectory, reference: &str, maximum: u64) -> Result<(File, Vec<u8>)> {
    if reference.is_empty() || reference.len() > 1024 {
        return Err(Error::InvalidInput(
            "native media private input reference is invalid".into(),
        ));
    }
    let relative = LibraryPath::parse(reference)?;
    let (parent, name) = relative
        .as_str()
        .rsplit_once('/')
        .unwrap_or(("", relative.as_str()));
    let directory = root.open_directory(&LibraryPath::parse(parent)?)?;
    directory.require_local()?;
    let permissions = FilePermissions::current()?;
    permissions.check(&directory.directory_file()?, true)?;
    let file = directory.open_file(std::ffi::OsStr::new(name))?;
    crate::infrastructure::filesystem::require_local_file(&file)?;
    permissions.check(&file, true)?;
    let mut bytes = Vec::new();
    (&file).take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(Error::InvalidInput(
            "native media private input exceeds its size limit".into(),
        ));
    }
    Ok((file, bytes))
}

impl PreparedMediaAdapter {
    /// Explicit foreground activation. The guarded listener admits only the
    /// configured paired client. Ctrl+C (or SIGTERM on Unix) stops admission
    /// and gives active read-only requests their bounded drain interval.
    pub async fn run(self) -> Result<()> {
        self.run_with_lifecycle(crate::infrastructure::lifecycle::Lifecycle::foreground())
            .await
    }

    pub async fn run_with_lifecycle(
        self,
        lifecycle: crate::infrastructure::lifecycle::Lifecycle,
    ) -> Result<()> {
        if lifecycle.stopping() {
            return Ok(());
        }
        let listener = TcpListener::bind(self.listen)?;
        let handle = axum_server::Handle::new();
        let serving = self.adapter.serve(listener, handle.clone());
        tokio::pin!(serving);
        lifecycle.ready()?;
        eprintln!(
            "Read-only native media adapter listening on {}",
            self.listen
        );
        tokio::select! {
            result = &mut serving => result?,
            signal = lifecycle.stopped() => {
                handle.graceful_shutdown(Some(Duration::from_secs(10)));
                let result = serving.await;
                signal?;
                result?;
            }
        }
        Ok(())
    }
}
